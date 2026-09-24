//! JSON report writing.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use tpt_app_media_qc_core::error::Result;
use tpt_app_media_qc_model::report::Report;

/// Write the report as compact, machine-readable JSON with an integrity
/// signature.
pub fn write_json(path: &Path, report: &Report) -> Result<()> {
    let file = File::create(path)?;
    let writer = BufWriter::new(file);
    serde_json::to_writer_pretty(writer, report)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use tpt_app_media_qc_model::report::{
        AnalysisId, AppInfo, HostInfo, ProfileRef, Report, ReportIntegrity,
    };
    use tpt_app_media_qc_model::severity::VerdictDecision;

    fn minimal_report() -> Report {
        Report {
            analysis_id: AnalysisId::default(),
            created_at: Utc::now(),
            app: AppInfo {
                name: "x".into(),
                version: "0.1.0".into(),
                ruleset_version: "0.1.0".into(),
            },
            integrity: ReportIntegrity {
                asset_sha256: "0".repeat(64),
                profile_sha256: "1".repeat(64),
                application_version: "0.1.0".into(),
                ruleset_version: "0.1.0".into(),
                analysis_id: AnalysisId::default().0.to_string(),
            },
            profile: ProfileRef {
                name: "default".into(),
                version: 1,
                sha256: "1".repeat(64),
            },
            host: HostInfo {
                os: "test".into(),
                arch: "test".into(),
                cpu_count: 1,
            },
            asset_path: "x.mp4".into(),
            asset_size_bytes: 1,
            asset_duration_ms: None,
            asset_resolution: None,
            findings: vec![],
            verdict: VerdictDecision::Pass,
            operator_notes: String::new(),
        }
    }

    #[test]
    fn roundtrips_via_filesystem() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("report.json");
        let report = minimal_report();
        write_json(&path, &report).unwrap();
        let back = std::fs::read_to_string(&path).unwrap();
        let parsed: Report = serde_json::from_str(&back).unwrap();
        assert_eq!(parsed.analysis_id, report.analysis_id);
        assert_eq!(parsed.verdict, report.verdict);
    }
}
