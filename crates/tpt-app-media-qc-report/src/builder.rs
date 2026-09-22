//! Assemble a [`Report`] from a pipeline [`QcRun`].

use std::path::Path;

use chrono::Utc;
use tpt_app_media_qc_core::config::{APP_NAME, APP_VERSION, RULESET_VERSION};
use tpt_app_media_qc_core::error::Result;
use tpt_app_media_qc_model::asset::StreamKind;
use tpt_app_media_qc_model::report::{
    AnalysisId, AppInfo, HostInfo, ProfileRef, Report, ReportIntegrity,
};
use tpt_app_media_qc_pipeline::QcRun;
use tpt_app_media_qc_profile::hash::profile_sha256;
use tpt_app_media_qc_profile::model::Profile;

use crate::json;

/// Build the immutable report for a completed QC run.
///
/// `analysis_id` defaults to a fresh UUID; pass an existing one to reproduce
/// byte-for-byte identical reports. The profile document is hashed so the
/// integrity field matches the profile actually used.
pub fn build_report(run: &QcRun, analysis_id: AnalysisId, profile: &Profile) -> Report {
    let asset_duration_ms = run.asset.duration.map(|d| d.as_secs() * 1000);
    let asset_resolution = run
        .asset
        .streams
        .iter()
        .find(|s| s.kind == StreamKind::Video && s.width.is_some() && s.height.is_some())
        .and_then(|s| s.dimension_label());

    let profile_sha = profile_sha256(profile);

    Report {
        analysis_id,
        created_at: Utc::now(),
        app: AppInfo {
            name: APP_NAME.to_string(),
            version: APP_VERSION.to_string(),
            ruleset_version: RULESET_VERSION.to_string(),
        },
        integrity: ReportIntegrity {
            asset_sha256: run.asset.fingerprint.sha256.clone(),
            profile_sha256: profile_sha.clone(),
            application_version: APP_VERSION.to_string(),
            ruleset_version: RULESET_VERSION.to_string(),
            analysis_id: analysis_id.0.to_string(),
        },
        profile: ProfileRef {
            name: run.profile_name.clone(),
            version: run.profile_version,
            sha256: profile_sha.clone(),
        },
        host: report_host_info(),
        asset_path: run.asset.path.display().to_string(),
        asset_size_bytes: run.asset.size_bytes,
        asset_duration_ms,
        asset_resolution,
        findings: run.findings.clone(),
        verdict: run.verdict,
        operator_notes: String::new(),
    }
}

/// Host information for the report body.
pub fn report_host_info() -> HostInfo {
    HostInfo {
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        cpu_count: std::thread::available_parallelism().map(|n| n.get() as u64).unwrap_or(1),
    }
}

/// Persist the report as compact JSON at `path`.
pub fn write_json_report(path: &Path, report: &Report) -> Result<()> {
    json::write_json(path, report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpt_app_media_qc_model::asset::{Asset, AssetFingerprint};

    fn sample_asset() -> Asset {
        Asset {
            id: Default::default(),
            path: "x.mp4".into(),
            fingerprint: AssetFingerprint { sha256: "ab".repeat(32), size_bytes: 1 },
            size_bytes: 1,
            modified_time: None,
            duration: None,
            streams: vec![],
        }
    }

    fn sample_run() -> QcRun {
        let profile = tpt_app_media_qc_profile::model::Profile::default();
        let inspector = tpt_app_media_qc_pipeline::arc(tpt_app_media_qc_pipeline::NoopInspector);
        let engine = tpt_app_media_qc_pipeline::QcEngine::new(std::sync::Arc::new(profile), inspector);
        engine.check_metadata_only(&sample_asset()).unwrap()
    }

    #[test]
    fn report_has_all_integrity_fields() {
        let run = sample_run();
        let profile = tpt_app_media_qc_profile::model::Profile::default();
        let report = build_report(&run, AnalysisId::default(), &profile);
        assert_eq!(report.integrity.asset_sha256.len(), 64);
        assert_eq!(report.integrity.profile_sha256.len(), 64);
        assert_eq!(report.integrity.application_version, report.app.version);
        assert_eq!(report.profile.name, "default");
    }
}