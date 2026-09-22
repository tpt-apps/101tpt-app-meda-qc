//! Standalone HTML report rendering (single-file, self-contained).

use std::path::Path;

use tpt_app_media_qc_core::error::Result;
use tpt_app_media_qc_model::report::Report;

#[derive(Clone, Copy, Debug, Default)]
pub struct WriteHtmlOptions {
    /// Embed report JSON in a `<pre>` block for machine re-use.
    pub embed_json: bool,
}

/// Render a readable single-page HTML report.
pub fn render_html(path: &Path, report: &Report, options: WriteHtmlOptions) -> Result<()> {
    let counts = report.status_counts();
    let mut body = String::new();

    body.push_str(
        "<!DOCTYPE html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
         <title>QC Report</title>\
         <style>body{font-family:system-ui,sans-serif;margin:2rem}\
         table{border-collapse:collapse;width:100%}td,th{border:1px solid #ccc;padding:.4rem;text-align:left}\
         .pass{color:#166534}.fail{color:#b91c1c}.warn{color:#b45309}.inconc{color:#6b7280}\
         code{background:#f4f4f5;padding:.1em .3em;border-radius:3px}</style></head><body>",
    );

    body.push_str(&format!(
        "<h1>QC Report</h1><p><strong>asset:</strong> <code>{}</code></p>\
         <p><strong>verdict:</strong> <span class=\"{}\">{}</span> &mdash; pass {}, warn {}, fail {}, inconclusive {}</p>\
         <p><strong>analysis:</strong> <code>{}</code></p>\
         <p><strong>profile:</strong> {} v{} &mdash; sha256 <code>{}</code></p>\
         <p><strong>app:</strong> {} {} (ruleset {}) &mdash; created {}</p>",
        escape(&report.asset_path),
        report.verdict.as_str(),
        report.verdict.as_str(),
        counts.pass,
        counts.warn,
        counts.fail,
        counts.inconclusive,
        report.analysis_id.0,
        escape(&report.profile.name),
        report.profile.version,
        &report.integrity.profile_sha256[..16.min(report.integrity.profile_sha256.len())],
        escape(&report.app.name),
        escape(&report.app.version),
        escape(&report.app.ruleset_version),
        report.created_at.to_rfc3339()
    ));

    body.push_str("<h2>Findings</h2><table><tr><th>Rule</th><th>Status</th><th>Severity</th><th>Stream</th><th>Time</th><th>Measured</th><th>Expected</th><th>Message</th></tr>");
    if report.findings.is_empty() {
        body.push_str("<tr><td colspan=\"8\">No findings.</td></tr>");
    }
    for f in &report.findings {
        let stream = f.stream_idx.map(|s| s.to_string()).unwrap_or_default();
        let time = f
            .time_range
            .map(|r| format!("{}–{} ms", r.start_ms, r.end_ms))
            .unwrap_or_default();
        let measured = f.measured.as_ref().map(|v| v.to_string()).unwrap_or_default();
        let expected = f.expected.as_ref().map(|v| v.to_string()).unwrap_or_default();
        body.push_str(&format!(
            "<tr><td><code>{}</code></td><td class=\"{}\">{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            escape(f.rule_id.as_str()),
            css_class(f.status.as_str()),
            f.status.as_str(),
            f.severity.as_str(),
            escape(&stream),
            escape(&time),
            escape(&measured),
            escape(&expected),
            escape(&f.message)
        ));
    }
    body.push_str("</table>");

    if options.embed_json {
        let json = serde_json::to_string_pretty(report)?;
        body.push_str("<h2>Machine-readable JSON</h2><pre><code>");
        body.push_str(&escape(&json));
        body.push_str("</code></pre>");
    }

    body.push_str("</body></html>");
    std::fs::write(path, body)?;
    Ok(())
}

fn css_class(status: &str) -> &str {
    match status {
        "pass" => "pass",
        "fail" => "fail",
        "warn" => "warn",
        _ => "inconc",
    }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use tpt_app_media_qc_model::asset::{Asset, AssetFingerprint};
    use tpt_app_media_qc_model::report::AnalysisId;

    #[test]
    fn html_renders_standalone() {
        let dir = tempfile::tempdir().unwrap();
        let path: PathBuf = dir.path().join("report.html");
        let asset = Asset {
            id: Default::default(),
            path: "x.mp4".into(),
            fingerprint: AssetFingerprint { sha256: "ab".repeat(32), size_bytes: 1 },
            size_bytes: 1,
            modified_time: None,
            duration: None,
            streams: vec![],
        };
        let profile = tpt_app_media_qc_profile::model::Profile::default();
        let engine = tpt_app_media_qc_pipeline::QcEngine::new(
            std::sync::Arc::new(profile.clone()),
            tpt_app_media_qc_pipeline::arc(tpt_app_media_qc_pipeline::NoopInspector),
        );
        let run = engine.check_metadata_only(&asset).unwrap();
        let report = crate::build_report(&run, AnalysisId::default(), &profile);

        render_html(&path, &report, WriteHtmlOptions { embed_json: true }).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("<!DOCTYPE html>"));
        assert!(text.contains("QC Report"));
        assert!(text.contains("Machine-readable JSON"));
    }
}