//! TPT Media QC desktop application.
//!
//! This crate is the native shell around the same profile, inspection, engine
//! and report code used by the CLI. Import and presentation stay local; media
//! scanning runs on Tauri's blocking worker pool so the UI remains responsive.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;
use tpt_app_media_qc_cli::app::{is_media_file, run_qc};
use tpt_app_media_qc_model::asset::Asset;
use tpt_app_media_qc_model::inspection::Inspection;
use tpt_app_media_qc_model::report::{AnalysisId, Report, StatusCounts};
use tpt_app_media_qc_model::severity::VerdictDecision;
use tpt_app_media_qc_model::time::DurationSeconds;
use tpt_app_media_qc_pipeline::QcRun;
use tpt_app_media_qc_profile::model::Profile;
use tpt_app_media_qc_profile::parse_str;
use tpt_app_media_qc_report::{
    build_report, render_html, render_pdf, write_csv, write_json_report, WriteCsvOptions,
    WriteHtmlOptions, WritePdfOptions,
};

const BUNDLED_PROFILES: &[(&str, &str)] = &[
    (
        "generic",
        include_str!("../../../profiles/generic/generic.yaml"),
    ),
    (
        "broadcast-ebu-r128",
        include_str!("../../../profiles/broadcast/ebu-r128.yaml"),
    ),
    (
        "broadcast-atsc-a85",
        include_str!("../../../profiles/broadcast/atsc-a85.yaml"),
    ),
    (
        "streaming-vod",
        include_str!("../../../profiles/streaming/vod.yaml"),
    ),
];

const MAX_IMPORT_FILES: usize = 10_000;

#[derive(Clone, Debug, Serialize)]
pub struct ProfileInfo {
    pub id: String,
    pub name: String,
    pub version: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DesktopRun {
    pub asset: Asset,
    pub profile_name: String,
    pub profile_version: u32,
    pub inspection: Inspection,
    pub report: Report,
    pub counts: StatusCounts,
    pub verdict: VerdictDecision,
}

fn resolve_profile(selector: &str) -> Result<Profile, String> {
    let selector = selector.trim();
    if selector.is_empty() || selector == "generic" {
        return parse_str(BUNDLED_PROFILES[0].1).map_err(|error| error.to_string());
    }
    if let Some((_, source)) = BUNDLED_PROFILES.iter().find(|(id, _)| *id == selector) {
        return parse_str(source).map_err(|error| error.to_string());
    }

    let source = std::fs::read_to_string(selector)
        .map_err(|error| format!("could not read profile '{selector}': {error}"))?;
    parse_str(&source).map_err(|error| format!("invalid profile '{selector}': {error}"))
}

fn desktop_run(mut run: QcRun, profile: &Profile) -> DesktopRun {
    if run.asset.duration.is_none() {
        run.asset.duration = run
            .inspection
            .container
            .duration
            .map(|duration| DurationSeconds::from_millis(duration.0));
    }
    let report = build_report(&run, AnalysisId::new(), profile);
    DesktopRun {
        asset: run.asset,
        profile_name: run.profile_name,
        profile_version: run.profile_version,
        inspection: run.inspection,
        report,
        counts: run.counts,
        verdict: run.verdict,
    }
}

fn add_media_file(path: &Path, seen: &mut BTreeSet<PathBuf>, files: &mut Vec<String>) -> bool {
    if !path.is_file() || !is_media_file(path) {
        return false;
    }
    let key = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if seen.insert(key) {
        files.push(path.to_string_lossy().into_owned());
    }
    files.len() < MAX_IMPORT_FILES
}

fn collect_media(inputs: &[String], recursive: bool) -> Result<Vec<String>, String> {
    let mut pending: Vec<PathBuf> = inputs.iter().map(PathBuf::from).collect();
    let mut visited_directories = BTreeSet::new();
    let mut seen_files = BTreeSet::new();
    let mut files = Vec::new();

    while let Some(path) = pending.pop() {
        let metadata = std::fs::metadata(&path)
            .map_err(|error| format!("could not inspect '{}': {error}", path.display()))?;
        if metadata.is_dir() {
            let canonical = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            if !visited_directories.insert(canonical) {
                continue;
            }
            let entries = std::fs::read_dir(&path)
                .map_err(|error| format!("could not read folder '{}': {error}", path.display()))?;
            for entry in entries {
                let entry = entry.map_err(|error| {
                    format!("could not read an entry in '{}': {error}", path.display())
                })?;
                if recursive {
                    pending.push(entry.path());
                } else if !add_media_file(&entry.path(), &mut seen_files, &mut files)
                    && files.len() >= MAX_IMPORT_FILES
                {
                    break;
                }
            }
        } else if !add_media_file(&path, &mut seen_files, &mut files)
            && files.len() >= MAX_IMPORT_FILES
        {
            break;
        }
        if files.len() >= MAX_IMPORT_FILES {
            break;
        }
    }

    files.sort_by_key(|path| path.to_ascii_lowercase());
    Ok(files)
}

#[tauri::command]
fn list_profiles() -> Vec<ProfileInfo> {
    BUNDLED_PROFILES
        .iter()
        .filter_map(|(id, source)| {
            parse_str(source).ok().map(|profile| ProfileInfo {
                id: (*id).to_string(),
                name: profile.name,
                version: profile.version,
            })
        })
        .collect()
}

#[tauri::command]
async fn scan_asset(path: String, profile: String, quick: bool) -> Result<DesktopRun, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let profile = resolve_profile(&profile)?;
        let run = run_qc(Path::new(&path), profile.clone(), quick)?;
        Ok(desktop_run(run, &profile))
    })
    .await
    .map_err(|error| format!("scan worker failed: {error}"))?
}

#[tauri::command]
fn expand_media_paths(paths: Vec<String>, recursive: bool) -> Result<Vec<String>, String> {
    collect_media(&paths, recursive)
}

#[tauri::command]
fn export_report(run: DesktopRun, path: String, format: String) -> Result<String, String> {
    let output = PathBuf::from(path);
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("could not create report directory: {error}"))?;
    }

    match format.to_ascii_lowercase().as_str() {
        "json" => write_json_report(&output, &run.report)
            .map_err(|error| format!("could not write JSON report: {error}"))?,
        "html" => render_html(&output, &run.report, WriteHtmlOptions { embed_json: true })
            .map_err(|error| format!("could not write HTML report: {error}"))?,
        "pdf" => render_pdf(&output, &run.report, WritePdfOptions {})
            .map_err(|error| format!("could not write PDF report: {error}"))?,
        "csv" => write_csv(&output, &run.report, WriteCsvOptions { header: true })
            .map_err(|error| format!("could not write CSV report: {error}"))?,
        other => return Err(format!("unsupported report format '{other}'")),
    }
    Ok(output.to_string_lossy().into_owned())
}

#[tauri::command]
fn open_path(app: AppHandle, path: String) -> Result<(), String> {
    let target = PathBuf::from(&path);
    if !target.exists() {
        return Err(format!("path does not exist: {path}"));
    }
    app.opener()
        .open_path(&path, None::<&str>)
        .map_err(|error| format!("could not open '{path}': {error}"))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            list_profiles,
            scan_asset,
            expand_media_paths,
            export_report,
            open_path
        ])
        .run(tauri::generate_context!())
        .expect("error while running TPT Media QC desktop application");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_profiles_are_valid() {
        let profiles = list_profiles();
        assert!(profiles.iter().any(|profile| profile.id == "generic"));
        assert!(profiles.len() >= 4);
    }

    #[test]
    fn folder_import_filters_and_deduplicates_media() {
        let directory = tempfile::tempdir().unwrap();
        let media = directory.path().join("sample.wav");
        let ignored = directory.path().join("notes.txt");
        std::fs::write(&media, b"RIFF").unwrap();
        std::fs::write(&ignored, b"text").unwrap();

        let files =
            collect_media(&[directory.path().to_string_lossy().into_owned()], false).unwrap();
        assert_eq!(files, vec![media.to_string_lossy().into_owned()]);
    }

    #[test]
    fn non_recursive_folder_import_skips_nested_media() {
        let directory = tempfile::tempdir().unwrap();
        let nested = directory.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        let direct = directory.path().join("sample.wav");
        let child = nested.join("nested.mp4");
        std::fs::write(&direct, b"RIFF").unwrap();
        std::fs::write(&child, b"ftyp").unwrap();

        let files =
            collect_media(&[directory.path().to_string_lossy().into_owned()], false).unwrap();
        assert_eq!(files, vec![direct.to_string_lossy().into_owned()]);
    }

    #[test]
    fn nested_folder_import_is_recursive() {
        let directory = tempfile::tempdir().unwrap();
        let nested = directory.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        let media = nested.join("sample.mp4");
        std::fs::write(&media, b"ftyp").unwrap();

        let files =
            collect_media(&[directory.path().to_string_lossy().into_owned()], true).unwrap();
        assert_eq!(files, vec![media.to_string_lossy().into_owned()]);
    }
}
