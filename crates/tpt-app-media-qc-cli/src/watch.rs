//! Watch-folder processing (spec § 13).
//!
//! Watches an input directory recursively, runs each new media file against a
//! profile and routes it to a pass/warn/fail destination folder, writing an
//! optional per-asset report. All processing is fully local.

use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use notify::{Event, EventKind, RecursiveMode, Watcher};
use tpt_app_media_qc_model::severity::VerdictDecision;
use tpt_app_media_qc_pipeline::{InspectionLevel, QcEngine};
use tpt_app_media_qc_profile::model::Profile;

use crate::app::{build_asset, is_media_file, make_inspector, write_reports};
use crate::exit::{EXIT_ERROR, EXIT_NO_INSPECTOR, EXIT_PATH};

/// Configuration for a watch-folder session (spec § 13 example).
pub struct WatchConfig {
    pub input: PathBuf,
    pub pass: PathBuf,
    pub warn: PathBuf,
    pub fail: PathBuf,
    pub report: Option<PathBuf>,
}

/// Run a watch-folder session until interrupted.
pub fn run_watch(config: WatchConfig, profile: Profile, quick: bool) -> i32 {
    if !config.input.is_dir() {
        eprintln!(
            "error: input directory '{}' not found",
            config.input.display()
        );
        return EXIT_PATH;
    }
    for dir in [&config.pass, &config.warn, &config.fail] {
        if let Err(e) = std::fs::create_dir_all(dir) {
            eprintln!("error: cannot create destination '{}': {e}", dir.display());
            return EXIT_ERROR;
        }
    }
    if let Some(rd) = &config.report {
        if let Err(e) = std::fs::create_dir_all(rd) {
            eprintln!(
                "error: cannot create report directory '{}': {e}",
                rd.display()
            );
            return EXIT_ERROR;
        }
    }

    let inspector = match make_inspector(quick) {
        Ok(i) => i,
        Err(EXIT_NO_INSPECTOR) => {
            eprintln!("error: no probe backend available; cannot watch folders");
            return EXIT_NO_INSPECTOR;
        }
        Err(code) => return code,
    };
    let level = if quick {
        InspectionLevel::MetadataOnly
    } else {
        InspectionLevel::Full
    };
    let engine = QcEngine::new(Arc::new(profile.clone()), inspector);

    process_existing(&config, &engine, &profile, level);

    let (tx, rx) = mpsc::channel::<PathBuf>();
    let mut watcher = match notify::recommended_watcher(move |res: notify::Result<Event>| {
        if let Ok(event) = res {
            for p in event.paths {
                if is_new_file_event(&event.kind) && is_media_file(&p) {
                    let _ = tx.send(p);
                }
            }
        }
    }) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("error: cannot start filesystem watcher: {e}");
            return EXIT_ERROR;
        }
    };
    if let Err(e) = watcher.watch(&config.input, RecursiveMode::Recursive) {
        eprintln!("error: cannot watch '{}': {e}", config.input.display());
        return EXIT_ERROR;
    }

    println!(
        "watch: monitoring {} (Ctrl+C to stop)",
        config.input.display()
    );
    while let Ok(path) = rx.recv() {
        if under_any_dest(&path, &config) {
            continue;
        }
        process_one(&path, &config, &engine, &profile, level);
    }
    0
}

pub fn is_new_file_event(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_)
            | EventKind::Modify(notify::event::ModifyKind::Name(
                notify::event::RenameMode::To
            ))
    )
}

fn under_any_dest(path: &Path, config: &WatchConfig) -> bool {
    let dests = [
        Some(&config.pass),
        Some(&config.warn),
        Some(&config.fail),
        config.report.as_ref(),
    ];
    dests.into_iter().flatten().any(|d| path.starts_with(d))
}

/// Process files already present in the input directory.
fn process_existing(
    config: &WatchConfig,
    engine: &QcEngine,
    profile: &Profile,
    level: InspectionLevel,
) {
    let mut files = Vec::new();
    collect_media(&config.input, &mut files);
    files.sort();
    let mut routed = 0usize;
    for path in files {
        if under_any_dest(&path, config) {
            continue;
        }
        if wait_until_stable(&path) {
            process_one(&path, config, engine, profile, level);
            routed += 1;
        }
    }
    eprintln!("watch: scanned {} existing media file(s)", routed);
}

fn collect_media(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_media(&path, out);
        } else if is_media_file(&path) {
            out.push(path);
        }
    }
}

/// Wait for a newly-created file to stop growing (partial copy protection).
fn wait_until_stable(path: &Path) -> bool {
    let mut prev = std::fs::metadata(path).ok().map(|m| m.len()).unwrap_or(0);
    for _ in 0..25 {
        std::thread::sleep(Duration::from_millis(200));
        let now = std::fs::metadata(path).ok().map(|m| m.len()).unwrap_or(0);
        if now == prev && now > 0 {
            return true;
        }
        prev = now;
    }
    false
}

fn process_one(
    path: &Path,
    config: &WatchConfig,
    engine: &QcEngine,
    profile: &Profile,
    level: InspectionLevel,
) {
    let asset = match build_asset(path) {
        Ok(a) => a,
        Err(_) => {
            eprintln!(
                "watch: could not fingerprint '{}'; leaving in place",
                path.display()
            );
            return;
        }
    };
    let run = match engine.check(&asset, level) {
        Ok(r) => r,
        Err(e) => {
            eprintln!(
                "watch: probe failed for '{}': {e}; leaving in place",
                path.display()
            );
            return;
        }
    };

    if let Some(rd) = &config.report {
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("asset");
        let name = format!("{stem}-{}.json", &run.asset.fingerprint.sha256[..16]);
        if let Err(e) = write_reports(&run, profile, Some(&rd.join(name)), None, None, None) {
            eprintln!(
                "watch: could not write report for '{}': {e}",
                path.display()
            );
        }
    }

    let dest = match run.verdict {
        VerdictDecision::Pass => &config.pass,
        VerdictDecision::Warn | VerdictDecision::Inconclusive => &config.warn,
        VerdictDecision::Fail => &config.fail,
    };
    match route_file(path, dest) {
        Ok(()) => println!(
            "{} {} -> {}",
            run.verdict.as_str().to_uppercase(),
            path.display(),
            dest.display()
        ),
        Err(e) => eprintln!("watch: could not route '{}': {e}", path.display()),
    }
}

/// Move a file into a destination directory, copying as a fallback when a
/// cross-device rename is not possible.
fn route_file(src: &Path, dest_dir: &Path) -> Result<(), std::io::Error> {
    std::fs::create_dir_all(dest_dir)?;
    let mut target = dest_dir.join(src.file_name().unwrap_or_default());
    if target.exists() {
        let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("asset");
        let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("bin");
        for i in 1.. {
            target = dest_dir.join(format!("{stem}-{i}.{ext}"));
            if !target.exists() {
                break;
            }
        }
    }
    match std::fs::rename(src, &target) {
        Ok(()) => Ok(()),
        Err(_) => {
            std::fs::copy(src, &target)?;
            std::fs::remove_file(src)?;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_with_unique_names_when_colliding() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out");
        std::fs::create_dir_all(&dest).unwrap();

        let a = dir.path().join("clip.mov");
        std::fs::write(&a, b"a").unwrap();
        std::fs::write(dest.join("clip.mov"), b"existing").unwrap();

        route_file(&a, &dest).unwrap();
        assert!(!a.exists());
        assert!(dest.join("clip-1.mov").exists());
    }

    #[test]
    fn new_file_event_matches_create() {
        use notify::event::{CreateKind, ModifyKind, RenameMode};
        assert!(is_new_file_event(&EventKind::Create(CreateKind::File)));
        assert!(is_new_file_event(&EventKind::Modify(ModifyKind::Name(
            RenameMode::To
        ))));
        assert!(!is_new_file_event(&EventKind::Modify(ModifyKind::Data(
            notify::event::DataChange::Size
        ))));
    }

    #[test]
    fn dest_dirs_are_excluded() {
        let cfg = WatchConfig {
            input: "in".into(),
            pass: "in/approved".into(),
            warn: "review".into(),
            fail: "rejected".into(),
            report: None,
        };
        assert!(under_any_dest(Path::new("in/approved/x.mp4"), &cfg));
        assert!(under_any_dest(Path::new("review/y.mp4"), &cfg));
        assert!(!under_any_dest(Path::new("in/pending/z.mp4"), &cfg));
    }
}
