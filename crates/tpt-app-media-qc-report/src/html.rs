//! Standalone HTML report rendering (single-file, self-contained).

use std::io::Write;
use std::path::Path;

use tpt_app_media_qc_core::error::Result;
use tpt_app_media_qc_model::report::Report;

use crate::output::{commit_writer, staged_writer};
use crate::template::{rule_rollup, ReportTemplate};

#[derive(Clone, Copy, Debug, Default)]
pub struct WriteHtmlOptions {
    /// Embed report JSON in a `<pre>` block for machine re-use.
    pub embed_json: bool,
    /// Which view of the report to render.
    pub template: ReportTemplate,
}

/// Render a readable single-page HTML report.
pub fn render_html(path: &Path, report: &Report, options: WriteHtmlOptions) -> Result<()> {
    let counts = report.status_counts();
    let mut body = String::new();

    body.push_str(
        "<!DOCTYPE html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
         <title>{TITLE}</title>\
         <style>body{font-family:system-ui,sans-serif;margin:2rem}\
         table{border-collapse:collapse;width:100%}td,th{border:1px solid #ccc;padding:.4rem;text-align:left}\
         .pass{color:#166534}.fail{color:#b91c1c}.warn{color:#b45309}.inconc{color:#6b7280}\
         code{background:#f4f4f5;padding:.1em .3em;border-radius:3px}</style></head><body>",
    );

    let template = options.template;
    body = body.replace("{TITLE}", template.title());

    body.push_str(&format!(
        "<h1>{}</h1><p><strong>asset:</strong> <code>{}</code></p>         <p><strong>verdict:</strong> <span class=\"{}\">{}</span> &mdash; pass {}, warn {}, fail {}, inconclusive {}</p>         <p><strong>profile:</strong> {} v{}</p>",
        template.title(),
        escape(&report.asset_path),
        report.verdict.as_str(),
        report.verdict.as_str(),
        counts.pass,
        counts.warn,
        counts.fail,
        counts.inconclusive,
        escape(&report.profile.name),
        report.profile.version,
    ));

    if template.shows_integrity() {
        body.push_str(&format!(
            "<p><strong>analysis:</strong> <code>{}</code></p>             <p><strong>profile sha256:</strong> <code>{}</code></p>             <p><strong>app:</strong> {} {} (ruleset {}) &mdash; created {}</p>",
            report.analysis_id.0,
            if template == ReportTemplate::Audit {
                &report.integrity.profile_sha256[..]
            } else {
                &report.integrity.profile_sha256[..16.min(report.integrity.profile_sha256.len())]
            },
            escape(&report.app.name),
            escape(&report.app.version),
            escape(&report.app.ruleset_version),
            report.created_at.to_rfc3339()
        ));
    } else {
        body.push_str(&format!(
            "<p><strong>created:</strong> {}</p>",
            report.created_at.to_rfc3339()
        ));
    }

    if template == ReportTemplate::Audit {
        body.push_str(&format!(
            "<h2>Integrity</h2><table>             <tr><th>Asset SHA-256</th><td><code>{}</code></td></tr>             <tr><th>Application version</th><td>{}</td></tr>             <tr><th>Ruleset version</th><td>{}</td></tr>             <tr><th>Asset size</th><td>{} bytes</td></tr>             <tr><th>Host</th><td>{} / {} / {} CPUs</td></tr></table>",
            escape(&report.integrity.asset_sha256),
            escape(&report.integrity.application_version),
            escape(&report.integrity.ruleset_version),
            report.asset_size_bytes,
            escape(&report.host.os),
            escape(&report.host.arch),
            report.host.cpu_count,
        ));
    }

    if template.shows_rollup() {
        body.push_str("<h2>Issues by rule</h2><table><tr><th>Rule</th><th>Status</th><th>Findings</th><th>Detail</th></tr>");
        let rows = rule_rollup(report);
        if rows.is_empty() {
            body.push_str("<tr><td colspan=\"4\">No issues.</td></tr>");
        }
        for row in rows {
            body.push_str(&format!(
                "<tr><td><code>{}</code></td><td class=\"{}\">{}</td><td>{}</td><td>{}</td></tr>",
                escape(&row.rule_id),
                css_class(row.status.as_str()),
                row.status.as_str(),
                row.findings,
                escape(&row.message)
            ));
        }
        body.push_str("</table>");
    }

    if template.lists_findings() {
        body.push_str("<h2>Findings</h2><table><tr><th>Rule</th><th>Status</th><th>Severity</th><th>Stream</th><th>Time</th><th>Measured</th><th>Expected</th><th>Message</th></tr>");
        let findings = template.findings(report);
        if findings.is_empty() {
            body.push_str("<tr><td colspan=\"8\">No findings.</td></tr>");
        }
        for f in findings {
            let stream = f.stream_idx.map(|s| s.to_string()).unwrap_or_default();
            let time = f
                .time_range
                .map(|r| format!("{}–{} ms", r.start_ms, r.end_ms))
                .unwrap_or_default();
            let measured = f
                .measured
                .as_ref()
                .map(|v| v.to_string())
                .unwrap_or_default();
            let expected = f
                .expected
                .as_ref()
                .map(|v| v.to_string())
                .unwrap_or_default();
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
    }

    if options.embed_json {
        let json = serde_json::to_string_pretty(report)?;
        body.push_str("<h2>Machine-readable JSON</h2><pre><code>");
        body.push_str(&escape(&json));
        body.push_str("</code></pre>");
    }

    body.push_str("</body></html>");
    let mut out = staged_writer(path)?;
    out.write_all(body.as_bytes())?;
    commit_writer(out, path)
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
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
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
            fingerprint: AssetFingerprint {
                sha256: "ab".repeat(32),
                size_bytes: 1,
            },
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

        render_html(
            &path,
            &report,
            WriteHtmlOptions {
                embed_json: true,
                ..Default::default()
            },
        )
        .unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("<!DOCTYPE html>"));
        assert!(text.contains("QC Report"));
        assert!(text.contains("Machine-readable JSON"));
    }

    fn render_with(template: ReportTemplate) -> String {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.html");
        let report = crate::template::testutil::sample_report();
        render_html(
            &path,
            &report,
            WriteHtmlOptions {
                embed_json: false,
                template,
            },
        )
        .unwrap();
        fs::read_to_string(&path).unwrap()
    }

    #[test]
    fn templates_change_the_content() {
        let detailed = render_with(ReportTemplate::Detailed);
        assert!(detailed.contains("<title>QC Report</title>"));
        assert!(detailed.contains("fine"), "detailed lists passing findings");
        assert!(!detailed.contains("Issues by rule"));

        let summary = render_with(ReportTemplate::Summary);
        assert!(summary.contains("<title>QC Summary</title>"));
        assert!(!summary.contains("<td>fine</td>"), "summary hides passes");
        assert!(summary.contains("black at 9s"));

        let exec = render_with(ReportTemplate::Executive);
        assert!(exec.contains("Issues by rule"));
        assert!(!exec.contains("<h2>Findings</h2>"));
        assert!(
            !exec.contains("black at 9s"),
            "roll-up shows the first message only"
        );

        let audit = render_with(ReportTemplate::Audit);
        assert!(audit.contains("Issues by rule"));
        assert!(audit.contains("<h2>Integrity</h2>"));
        assert!(audit.contains(&"ab".repeat(32)));
    }
}
