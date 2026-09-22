//! End-to-end pipeline integration test ([spec § 11, § 14]).
//!
//! Fixture inspector → QC engine → immutable report → JSON/HTML/CSV/PDF
//! exports, all offline (no media files, no `ffprobe` requirement).

use std::sync::Arc;

use tpt_app_media_qc_model::report::Report;
use tpt_app_media_qc_model::severity::VerdictDecision;
use tpt_app_media_qc_report::{
    build_report, render_html, render_pdf, write_csv, write_json_report, WriteCsvOptions,
    WriteHtmlOptions, WritePdfOptions,
};
use tpt_app_media_qc_test::{load_profile, run_fixture, MediaFixture};

/// All fixtures parse and the defect fixtures produce the expected verdicts.
#[test]
fn fixtures_produce_expected_verdicts() {
    let profile = Arc::new(load_profile("golden-suite").expect("golden-suite profile parses"));

    let clean = MediaFixture::load_named("clean-master").expect("clean fixture");
    let run = run_fixture(&clean, &profile);
    assert_eq!(run.verdict, VerdictDecision::Pass, "clean-master must pass");

    let boundary = MediaFixture::load_named("boundary-thresholds").expect("boundary fixture");
    let run = run_fixture(&boundary, &profile);
    assert_eq!(
        run.verdict,
        VerdictDecision::Pass,
        "boundary values must pass (strict >)"
    );

    for name in ["container-problems", "video-defects", "audio-defects"] {
        let fixture = MediaFixture::load_named(name).unwrap_or_else(|e| panic!("{name}: {e}"));
        let run = run_fixture(&fixture, &profile);
        assert_eq!(run.verdict, VerdictDecision::Fail, "{name} must fail");
        assert!(
            !run.findings.is_empty(),
            "{name} must explain its failure with findings (spec §3.3)"
        );
        // Every failure must be auditable: measured vs expected (spec §6.3).
        for finding in run
            .findings
            .iter()
            .filter(|f| f.status == VerdictDecision::Fail)
        {
            assert!(
                !finding.message.is_empty(),
                "{name}: failing finding needs a message"
            );
        }
    }
}

/// The unscanned fixture reports Inconclusive (never guessed) per spec §3.4.
#[test]
fn unscanned_measurements_report_inconclusive() {
    let profile = Arc::new(load_profile("golden-suite").expect("golden-suite profile parses"));
    let fixture = MediaFixture::load_named("unscanned").expect("unscanned fixture");
    let run = run_fixture(&fixture, &profile);

    assert!(
        run.counts.inconclusive > 0,
        "an unscanned asset must produce inconclusive findings"
    );
    assert_eq!(
        run.verdict,
        VerdictDecision::Warn,
        "inconclusive at error severity warns"
    );
}

/// A full offline report pipeline: engine → report → all four renderers.
#[test]
fn clean_fixture_renders_every_report_format() {
    let profile = Arc::new(load_profile("generic").expect("generic profile parses"));
    let fixture = MediaFixture::load_named("clean-master").expect("clean fixture");
    let run = run_fixture(&fixture, &profile);

    let report = build_report(&run, Default::default(), &profile);
    assert_eq!(report.verdict, run.verdict);
    assert_eq!(report.integrity.asset_sha256, run.asset.fingerprint.sha256);
    assert_eq!(report.integrity.profile_sha256.len(), 64);

    let dir = std::env::temp_dir().join(format!(
        "tpt-qc-integration-{}-{}",
        std::process::id(),
        run.findings.len()
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");

    let json_path = dir.join("report.json");
    let html_path = dir.join("report.html");
    let csv_path = dir.join("report.csv");
    let pdf_path = dir.join("report.pdf");

    write_json_report(&json_path, &report).expect("json export");
    render_html(&html_path, &report, WriteHtmlOptions { embed_json: true }).expect("html export");
    write_csv(&csv_path, &report, WriteCsvOptions { header: true }).expect("csv export");
    render_pdf(&pdf_path, &report, WritePdfOptions {}).expect("pdf export");

    // Every renderer must have produced non-empty output.
    for path in [&json_path, &html_path, &csv_path, &pdf_path] {
        let meta = std::fs::metadata(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert!(meta.len() > 0, "{} must not be empty", path.display());
    }

    // The machine-readable report round-trips with its verdict intact.
    let round_trip: Report = serde_json::from_str(
        &std::fs::read_to_string(&json_path).expect("json report is readable"),
    )
    .expect("json report re-parses");
    assert_eq!(round_trip.verdict, report.verdict);
    assert_eq!(round_trip.findings.len(), report.findings.len());
    assert_eq!(
        round_trip.integrity.profile_sha256,
        report.integrity.profile_sha256
    );

    let _ = std::fs::remove_dir_all(&dir);
}
