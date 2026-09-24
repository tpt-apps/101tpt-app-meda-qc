//! Subcommand implementations and exit-code mapping.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tpt_app_media_qc_core::config::APP_VERSION;
use tpt_app_media_qc_core::fingerprint::{Fingerprint, FingerprintConfig};
use tpt_app_media_qc_model::asset::{Asset, AssetFingerprint};
use tpt_app_media_qc_model::report::AnalysisId;
use tpt_app_media_qc_model::severity::VerdictDecision;
use tpt_app_media_qc_pipeline::{arc, InspectionLevel, Inspector, NoopInspector, QcEngine};
use tpt_app_media_qc_profile::model::Profile;
use tpt_app_media_qc_profile::{parse_str, profile_sha256};
use tpt_app_media_qc_report::{
    build_report, render_html, render_pdf, write_csv, write_json_report, WriteCsvOptions,
    WriteHtmlOptions, WritePdfOptions,
};
use tpt_app_media_qc_rules::known_rule_ids;

use crate::cli::{Cli, Command};
use crate::probe::{FfprobeInspector, HybridInspector};

use crate::exit::{
    exit_code_for_verdict, EXIT_ERROR, EXIT_FAIL, EXIT_NO_INSPECTOR, EXIT_PATH, EXIT_PROFILE,
};

/// Run the parsed CLI and return the process exit code.
pub fn run(cli: Cli) -> i32 {
    match cli.command {
        Command::Check {
            file,
            profile,
            quick,
            json,
            html,
            pdf,
            csv,
            quiet,
        } => run_check(file, profile, quick, json, html, pdf, csv, quiet),
        Command::Batch {
            files,
            profile,
            quick,
            out,
            fail_fast,
        } => run_batch(files, profile, quick, out, fail_fast),
        Command::Info {
            file,
            profile,
            quick,
        } => run_info(file, profile, quick),
        Command::ListRules { profile } => run_list_rules(profile),
        Command::Watch {
            input,
            profile,
            pass,
            warn,
            fail,
            report,
            quick,
        } => {
            let profile = match load_profile(profile.as_deref()) {
                Ok(p) => p,
                Err(code) => return code,
            };
            crate::watch::run_watch(
                crate::watch::WatchConfig {
                    input,
                    pass,
                    warn,
                    fail,
                    report,
                },
                profile,
                quick,
            )
        }
    }
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn err_exit(msg: impl AsRef<str>, code: i32) -> i32 {
    eprintln!("error: {}", msg.as_ref());
    code
}

fn load_profile(path: Option<&Path>) -> Result<Profile, i32> {
    let default_doc = "name: generic\nversion: 1\nrules:\n  container:\n    readable: error\n    container_validity: error\n    malformed_metadata: warning\n    bitrate:\n      min_bps: 1000000\n    timecode_present: warning\n    timestamp_continuity:\n      max_gap_ms: 150\n    stream_presence:\n      min_video: 1\n  video:\n    resolution:\n      expected: \"1920x1080\"\n      severity: warning\n    frame_rate:\n      expected: \"25/1\"\n      tolerance: 0.001\n      severity: warning\n    black_frames:\n      max_duration_ms: 500\n    freeze_frames:\n      max_duration_ms: 500\n    duplicate_frames:\n      max_events: 10\n      severity: warning\n    corrupt_frames:\n      max_events: 5\n      severity: warning\n  audio:\n    silence:\n      max_duration_ms: 4000\n      severity: warning\n    clipping:\n      max_events: 10\n      severity: warning\n    loudness:\n      standard: ebu-r128\n      target_lufs: -23\n      tolerance_lu: 1\n    true_peak:\n      max_db: -1\n      severity: warning\n    phase:\n      min_correlation: -0.5\n      severity: warning\npolicy:\n  fail_on: error\n";

    match path {
        Some(p) => {
            let raw = match std::fs::read_to_string(p) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("error: cannot read profile '{}': {e}", p.display());
                    return Err(EXIT_PROFILE);
                }
            };
            match parse_str(&raw) {
                Ok(profile) => Ok(profile),
                Err(e) => {
                    eprintln!("error: invalid profile '{}': {e}", p.display());
                    Err(EXIT_PROFILE)
                }
            }
        }
        None => match parse_str(default_doc) {
            Ok(profile) => Ok(profile),
            Err(_) => Err(EXIT_PROFILE),
        },
    }
}

pub(crate) fn build_asset(path: &Path) -> Result<Asset, i32> {
    if !path.is_file() {
        return Err(EXIT_PATH);
    }
    let fingerprint = match Fingerprint::of_file(path, FingerprintConfig::default()) {
        Ok(fp) => AssetFingerprint {
            sha256: fp.to_hex(),
            size_bytes: fp.as_bytes().len() as u64,
        },
        Err(e) => {
            eprintln!("error: could not fingerprint '{}': {e}", path.display());
            return Err(EXIT_ERROR);
        }
    };
    let size = match std::fs::metadata(path) {
        Ok(m) => m.len(),
        Err(e) => {
            eprintln!("error: could not stat '{}': {e}", path.display());
            return Err(EXIT_ERROR);
        }
    };
    Ok(Asset {
        id: Default::default(),
        path: path.to_path_buf(),
        fingerprint,
        size_bytes: size,
        modified_time: std::fs::metadata(path)
            .ok()
            .and_then(|m| m.modified().ok())
            .map(|t| {
                t.duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0)
            }),
        duration: None,
        streams: vec![],
    })
}

pub(crate) fn make_inspector(quick: bool) -> Result<Arc<dyn Inspector>, i32> {
    if quick {
        // Metadata-only path works without a probe binary but yields an empty
        // inspection; prefer the probe when available.
        if FfprobeInspector::available() {
            Ok(arc(FfprobeInspector))
        } else {
            Ok(arc(NoopInspector))
        }
    } else {
        if FfprobeInspector::available() {
            Ok(arc(HybridInspector::default()))
        } else {
            Err(EXIT_NO_INSPECTOR)
        }
    }
}

fn level_for(profile: &Profile) -> InspectionLevel {
    if profile_has_decode_rules(profile) {
        InspectionLevel::Full
    } else {
        InspectionLevel::MetadataOnly
    }
}

fn print_summary(asset: &Path, run: &tpt_app_media_qc_pipeline::QcRun) {
    println!(
        "asset    {}\nprofile  {} v{}\nverdict  {}  (pass {} · warn {} · fail {} · inconclusive {})",
        asset.display(),
        run.profile_name,
        run.profile_version,
        run.verdict.as_str(),
        run.counts.pass,
        run.counts.warn,
        run.counts.fail,
        run.counts.inconclusive
    );
}

fn print_findings(run: &tpt_app_media_qc_pipeline::QcRun) {
    for f in &run.findings {
        let stream = f
            .stream_idx
            .map(|s| format!(" s{}", s.0))
            .unwrap_or_default();
        let time = f
            .time_range
            .map(|r| format!(" [{}-{}ms]", r.start_ms, r.end_ms))
            .unwrap_or_default();
        println!(
            "[{}] {}{}{}: {}",
            f.status.as_str().to_uppercase(),
            f.rule_id.as_str(),
            stream,
            time,
            f.message
        );
    }
}

pub(crate) fn write_reports(
    run: &tpt_app_media_qc_pipeline::QcRun,
    profile: &Profile,
    json: Option<&Path>,
    html: Option<&Path>,
    pdf: Option<&Path>,
    csv: Option<&Path>,
) -> Result<(), i32> {
    let report = build_report(run, AnalysisId::default(), profile);
    if let Some(p) = json {
        if let Err(e) = write_json_report(p, &report) {
            return Err(err_exit(
                format!("could not write JSON report: {e}"),
                EXIT_ERROR,
            ));
        }
    }
    if let Some(p) = html {
        if let Err(e) = render_html(p, &report, WriteHtmlOptions { embed_json: true }) {
            return Err(err_exit(
                format!("could not write HTML report: {e}"),
                EXIT_ERROR,
            ));
        }
    }
    if let Some(p) = pdf {
        if let Err(e) = render_pdf(p, &report, WritePdfOptions {}) {
            return Err(err_exit(
                format!("could not write PDF report: {e}"),
                EXIT_ERROR,
            ));
        }
    }
    if let Some(p) = csv {
        if let Err(e) = write_csv(p, &report, WriteCsvOptions { header: true }) {
            return Err(err_exit(
                format!("could not write CSV report: {e}"),
                EXIT_ERROR,
            ));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// check
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn run_check(
    file: PathBuf,
    profile_path: Option<PathBuf>,
    quick: bool,
    json: Option<PathBuf>,
    html: Option<PathBuf>,
    pdf: Option<PathBuf>,
    csv: Option<PathBuf>,
    quiet: bool,
) -> i32 {
    if !file.is_file() {
        return err_exit(format!("asset '{}' not found", file.display()), EXIT_PATH);
    }
    let profile = match load_profile(profile_path.as_deref()) {
        Ok(p) => p,
        Err(code) => return code,
    };

    let level = if quick {
        InspectionLevel::MetadataOnly
    } else {
        InspectionLevel::Full
    };

    let inspector = match make_inspector(level == InspectionLevel::MetadataOnly) {
        Ok(i) => i,
        Err(code) => return code,
    };

    let asset = match build_asset(&file) {
        Ok(a) => a,
        Err(code) => return code,
    };

    let engine = QcEngine::new(Arc::new(profile.clone()), inspector);
    let run = match engine.check(&asset, level) {
        Ok(r) => r,
        Err(e) => {
            return err_exit(
                format!("probe failed for '{}': {e}", file.display()),
                EXIT_ERROR,
            )
        }
    };

    if !quiet {
        print_summary(&file, &run);
        print_findings(&run);
    } else {
        println!("{} {}", file.display(), run.verdict.as_str());
    }

    if let Err(code) = write_reports(
        &run,
        &profile,
        json.as_deref(),
        html.as_deref(),
        pdf.as_deref(),
        csv.as_deref(),
    ) {
        return code;
    }

    exit_code_for_verdict(run.verdict)
}

// ---------------------------------------------------------------------------
// batch
// ---------------------------------------------------------------------------

fn collect_assets(paths: Vec<PathBuf>) -> Result<Vec<PathBuf>, i32> {
    let mut out = Vec::new();
    for p in paths {
        if p.is_dir() {
            let mut found: Vec<PathBuf> = Vec::new();
            collect_dir(&p, &mut found)?;
            out.extend(found);
        } else if p.is_file() {
            out.push(p);
        } else {
            return Err(err_exit(
                format!("path '{}' does not exist", p.display()),
                EXIT_PATH,
            ));
        }
    }
    if out.is_empty() {
        Err(err_exit("no media files found", EXIT_ERROR))
    } else {
        Ok(out)
    }
}

fn collect_dir(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), i32> {
    for entry in std::fs::read_dir(dir).map_err(|e| {
        err_exit(
            format!("cannot read directory '{}': {e}", dir.display()),
            EXIT_ERROR,
        )
    })? {
        let entry = entry.map_err(|_| EXIT_ERROR)?;
        let path = entry.path();
        if path.is_dir() {
            collect_dir(&path, out)?;
        } else if is_media_file(&path) {
            out.push(path);
        }
    }
    Ok(())
}

pub fn is_media_file(path: &Path) -> bool {
    const EXTS: &[&str] = &[
        "mov", "mp4", "mxf", "m4v", "mkv", "ts", "mts", "m2ts", "wav", "aac", "w64", "ac3", "eac3",
        "mp3", "flac", "opus", "webm", "avi",
    ];
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

fn run_batch(
    files: Vec<PathBuf>,
    profile_path: Option<PathBuf>,
    quick: bool,
    out: Option<PathBuf>,
    fail_fast: bool,
) -> i32 {
    if files.is_empty() {
        return err_exit("batch requires at least one file or directory", EXIT_ERROR);
    }
    let profile = match load_profile(profile_path.as_deref()) {
        Ok(p) => p,
        Err(code) => return code,
    };
    let paths = match collect_assets(files) {
        Ok(p) => p,
        Err(code) => return code,
    };
    if let Some(dir) = &out {
        if let Err(e) = std::fs::create_dir_all(dir) {
            return err_exit(
                format!("cannot create output directory '{}': {e}", dir.display()),
                EXIT_ERROR,
            );
        }
    }

    let inspector = match make_inspector(quick) {
        Ok(i) => i,
        Err(code) => return code,
    };
    let level = if quick {
        InspectionLevel::MetadataOnly
    } else {
        InspectionLevel::Full
    };
    let engine = QcEngine::new(Arc::new(profile.clone()), inspector);

    let mut exit = exit_code_for_verdict(VerdictDecision::Pass);
    for path in &paths {
        let asset = match build_asset(path) {
            Ok(a) => a,
            Err(code) => return code,
        };
        match engine.check(&asset, level) {
            Ok(run) => {
                print_summary(path, &run);
                print_findings(&run);
                exit = exit.max(exit_code_for_verdict(run.verdict));
                if let Some(dir) = &out {
                    let name = format!(
                        "{}.json",
                        path.file_stem().and_then(|s| s.to_str()).unwrap_or("asset")
                    );
                    write_reports(&run, &profile, Some(&dir.join(name)), None, None, None)
                        .map_err(|_| ())
                        .ok();
                }
                if fail_fast && run.verdict == VerdictDecision::Fail {
                    eprintln!("batch: failing fast after '{}'", path.display());
                    return EXIT_FAIL;
                }
            }
            Err(e) => {
                eprintln!("error: {}: {e}", path.display());
                exit = EXIT_ERROR;
            }
        }
    }
    exit
}

// ---------------------------------------------------------------------------
// info
// ---------------------------------------------------------------------------

fn run_info(file: PathBuf, profile_path: Option<PathBuf>, quick: bool) -> i32 {
    if !file.is_file() {
        return err_exit(format!("asset '{}' not found", file.display()), EXIT_PATH);
    }
    let profile = match load_profile(profile_path.as_deref()) {
        Ok(p) => p,
        Err(code) => return code,
    };
    let inspector = match make_inspector(quick) {
        Ok(i) => i,
        Err(code) => return code,
    };
    let level = if quick {
        InspectionLevel::MetadataOnly
    } else {
        level_for(&profile)
    };
    let asset = match build_asset(&file) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let engine = QcEngine::new(Arc::new(profile), inspector);
    let run = match engine.check(&asset, level) {
        Ok(r) => r,
        Err(e) => {
            return err_exit(
                format!("probe failed for '{}': {e}", file.display()),
                EXIT_ERROR,
            )
        }
    };

    println!("TPT Media QC {} · asset inspection", APP_VERSION);
    println!("path          {}", asset.path.display());
    println!("size          {} bytes", asset.size_bytes);
    println!("sha256        {}", asset.fingerprint.sha256);
    let c = &run.inspection.container;
    println!(
        "format        {}",
        c.format.clone().unwrap_or_else(|| "unknown".into())
    );
    if let Some(d) = c.duration {
        println!("duration      {} ms", d.0);
    }
    if let Some(b) = c.bitrate_bps {
        println!("bitrate       {:.1} Mbps", b as f64 / 1_000_000.0);
    }
    println!(
        "timecode      {}",
        c.timecode_present
            .map(|b| if b { "present" } else { "absent" })
            .unwrap_or("unknown")
    );
    for v in &run.inspection.video {
        println!(
            "video s{}   fps {}{}{}",
            v.stream_idx,
            v.frame_rate_observed
                .map(|f| f.to_string())
                .unwrap_or_else(|| "-".into()),
            v.colorspace
                .as_deref()
                .map(|cs| format!(" · colorspace {cs}"))
                .unwrap_or_default(),
            if v.decode_errors > 0 {
                format!(" · {} decode errors", v.decode_errors)
            } else {
                String::new()
            },
        );
    }
    for a in &run.inspection.audio {
        let peak = a
            .peak_db
            .map(|p| format!("{p:.1}"))
            .unwrap_or_else(|| "-".into());
        let tp = a
            .true_peak_db
            .map(|p| format!("{p:.1}"))
            .unwrap_or_else(|| "-".into());
        let lufs = a
            .loudness_lufs
            .map(|l| format!("{l:.1}"))
            .unwrap_or_else(|| "-".into());
        let extra = [
            (!a.silence.is_empty()).then(|| format!("{} silence segments", a.silence.len())),
            (a.clipping_events > 0).then(|| format!("{} clips", a.clipping_events)),
            (a.decode_errors > 0).then(|| format!("{} decode errors", a.decode_errors)),
        ]
        .iter()
        .flatten()
        .map(|s| format!(" · {s}"))
        .collect::<String>();
        println!(
            "audio s{}   peak {} dBFS · TP {} dBTP · {lufs} LUFS{extra}",
            a.stream_idx, peak, tp
        );
    }
    print_summary(&file, &run);
    0
}

fn profile_has_decode_rules(profile: &Profile) -> bool {
    // Any configured rule that needs decoded measurements.
    profile.rules.video.black_frames.is_some()
        || profile.rules.video.freeze_frames.is_some()
        || profile.rules.video.duplicate_frames.is_some()
        || profile.rules.video.corrupt_frames.is_some()
        || profile.rules.video.luma_range.is_some()
        || profile.rules.audio.silence.is_some()
        || profile.rules.audio.clipping.is_some()
        || profile.rules.audio.peak.is_some()
        || profile.rules.audio.true_peak.is_some()
        || profile.rules.audio.loudness.is_some()
        || profile.rules.audio.phase.is_some()
        || profile.rules.audio.dc_offset.is_some()
}

// ---------------------------------------------------------------------------
// list-rules
// ---------------------------------------------------------------------------

fn run_list_rules(profile_path: Option<PathBuf>) -> i32 {
    println!("rule catalogue ({}):", APP_VERSION);
    match load_profile(profile_path.as_deref()) {
        Ok(profile) => {
            for id in known_rule_ids() {
                println!("{id}");
            }
            println!();
            println!(
                "profile '{}' v{} sha256 {}",
                profile.name,
                profile.version,
                profile_sha256(&profile)
            );
        }
        Err(code) => return code,
    }
    0
}
