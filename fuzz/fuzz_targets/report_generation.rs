//! Fuzz report generation across every renderer ([spec § 24.4]; report crate).
//!
//! Arbitrary bytes are woven into finding messages, rule IDs, the asset path
//! and operator notes, then the report is rendered as PDF, HTML, CSV and JSON.
//! This exercises the export writers and the PDF text sanitizer.

#![no_main]

use libfuzzer_sys::fuzz_target;
use tpt_app_media_qc_model::finding::QcFinding;
use tpt_app_media_qc_model::report::{
    AnalysisId, AppInfo, HostInfo, ProfileRef, Report, ReportIntegrity,
};
use tpt_app_media_qc_model::severity::{Severity, VerdictDecision};
use tpt_app_media_qc_report::{
    render_html, render_pdf, write_csv, write_json_report, WriteCsvOptions, WriteHtmlOptions,
    WritePdfOptions,
};

fn verdict(i: u8) -> VerdictDecision {
    match i % 4 {
        0 => VerdictDecision::Pass,
        1 => VerdictDecision::Warn,
        2 => VerdictDecision::Fail,
        _ => VerdictDecision::Inconclusive,
    }
}

fn severity(i: u8) -> Severity {
    match i % 4 {
        0 => Severity::Info,
        1 => Severity::Warning,
        2 => Severity::Error,
        _ => Severity::Critical,
    }
}

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);

    let mut findings = Vec::new();
    let mut remaining: &[u8] = data;
    let mut i: u8 = 0;
    while !remaining.is_empty() && findings.len() < 16 {
        let take = (remaining[0] as usize % 48).saturating_add(1).min(remaining.len());
        let chunk = String::from_utf8_lossy(&remaining[..take]).into_owned();
        findings.push(
            QcFinding::new(format!("rule.fuzz.{i}"))
                .status(verdict(i))
                .severity(severity(i))
                .set_message(chunk),
        );
        remaining = &remaining[take..];
        i = i.wrapping_add(1);
    }

    let report = Report {
        analysis_id: AnalysisId::default(),
        created_at: chrono::Utc::now(),
        app: AppInfo {
            name: text.clone().into_owned(),
            version: text.clone().into_owned(),
            ruleset_version: "1".into(),
        },
        integrity: ReportIntegrity {
            asset_sha256: text.clone().into_owned(),
            profile_sha256: "a".repeat(64),
            application_version: "0.1.0".into(),
            ruleset_version: "1".into(),
            analysis_id: text.clone().into_owned(),
        },
        profile: ProfileRef {
            name: text.clone().into_owned(),
            version: data.len() as u32,
            sha256: "c".repeat(64),
        },
        host: HostInfo { os: "fuzz".into(), arch: "fuzz".into(), cpu_count: 1 },
        asset_path: text.clone().into_owned(),
        asset_size_bytes: data.len() as u64,
        asset_duration_ms: Some(data.len() as u64),
        asset_resolution: Some(text.clone().into_owned()),
        findings,
        verdict: verdict(data.first().copied().unwrap_or(0)),
        operator_notes: text.into_owned(),
    };

    let prefix = std::env::temp_dir().join(format!("media-qc-fuzz-{}", std::process::id()));
    let _ = render_pdf(&prefix.with_extension("pdf"), &report, WritePdfOptions {});
    let _ = render_html(&prefix.with_extension("html"), &report, WriteHtmlOptions { embed_json: true });
    let _ = write_csv(&prefix.with_extension("csv"), &report, WriteCsvOptions { header: true });
    let _ = write_json_report(&prefix.with_extension("json"), &report);
});