//! Flat CSV export of findings (spreadsheet-friendly).

use std::io::Write;
use std::path::Path;

use tpt_app_media_qc_core::error::Result;
use tpt_app_media_qc_model::report::Report;

#[derive(Clone, Copy, Debug, Default)]
pub struct WriteCsvOptions {
    /// Emit a header row (default true).
    pub header: bool,
}

/// Write one CSV row per finding.
pub fn write_csv(path: &Path, report: &Report, options: WriteCsvOptions) -> Result<()> {
    let mut out = csv_writer(path)?;
    if options.header {
        writeln!(
            out,
            "asset,rule,status,severity,stream,start_ms,end_ms,measured,expected,message"
        )?;
    }

    for f in &report.findings {
        write_csv_row(
            &mut out,
            CsvRow {
                asset: &report.asset_path,
                rule: f.rule_id.as_str(),
                status: f.status.as_str(),
                severity: f.severity.as_str(),
                stream_idx: f.stream_idx,
                time_range: f.time_range,
                measured: f.measured.as_ref().map(|v| v.to_string()),
                expected: f.expected.as_ref().map(|v| v.to_string()),
                message: &f.message,
            },
        )?;
    }

    out.flush()?;
    Ok(())
}

fn csv_writer(path: &Path) -> Result<std::io::BufWriter<std::fs::File>> {
    let file = std::fs::File::create(path)?;
    Ok(std::io::BufWriter::new(file))
}

fn write_csv_row(out: &mut impl Write, row: CsvRow<'_>) -> std::io::Result<()> {
    let stream = row.stream_idx.map(|s| s.to_string()).unwrap_or_default();
    let (start, end) = match row.time_range {
        Some(r) => (r.start_ms.to_string(), r.end_ms.to_string()),
        None => (String::new(), String::new()),
    };
    let m = row.measured.unwrap_or_default();
    let e = row.expected.unwrap_or_default();
    let msg = row.message.replace('\n', " ");
    writeln!(
        out,
        "{},{},{},{},{},{},{},{},{},{}",
        csv_cell(row.asset),
        csv_cell(row.rule),
        csv_cell(row.status),
        csv_cell(row.severity),
        csv_cell(&stream),
        csv_cell(&start),
        csv_cell(&end),
        csv_cell(&m),
        csv_cell(&e),
        csv_cell(&msg),
    )
}

fn csv_cell(s: &str) -> String {
    s.replace(',', " ")
}

struct CsvRow<'a> {
    asset: &'a str,
    rule: &'a str,
    status: &'a str,
    severity: &'a str,
    stream_idx: Option<tpt_app_media_qc_model::asset::StreamId>,
    time_range: Option<tpt_app_media_qc_model::finding::TimeRange>,
    measured: Option<String>,
    expected: Option<String>,
    message: &'a str,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use tpt_app_media_qc_model::report::Report;

    use crate::builder::build_report;
    use tpt_app_media_qc_model::asset::{Asset, AssetFingerprint};

    fn run() -> (Report, String) {
        let asset = Asset {
            id: Default::default(),
            path: "x.mp4".into(),
            fingerprint: AssetFingerprint {
                sha256: "ab".repeat(32),
                size_bytes: 1,
            },
            size_bytes: 1,
            modified_time: None,
            duration: None,
            streams: vec![],
        };
        let profile = tpt_app_media_qc_profile::model::Profile {
            name: "port".into(),
            rules: tpt_app_media_qc_profile::model::RuleSetConfig {
                container: tpt_app_media_qc_profile::model::ContainerRules {
                    readable: Some(tpt_app_media_qc_model::severity::Severity::Error),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        let engine = tpt_app_media_qc_pipeline::QcEngine::new(
            std::sync::Arc::new(profile.clone()),
            tpt_app_media_qc_pipeline::arc(tpt_app_media_qc_pipeline::NoopInspector),
        );
        let qc_run = engine.check_metadata_only(&asset).unwrap();
        let report = build_report(
            &qc_run,
            tpt_app_media_qc_model::report::AnalysisId::default(),
            &profile,
        );
        (report, "x.mp4".into())
    }

    #[test]
    fn csv_writes_header_and_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path: PathBuf = dir.path().join("out.csv");
        let (report, _) = run();
        write_csv(&path, &report, WriteCsvOptions { header: true }).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("asset,rule,status"));
        assert!(text.contains("inconclusive"));
    }
}
