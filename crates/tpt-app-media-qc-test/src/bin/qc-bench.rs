//! `qc-bench` — std-only micro-benchmarks for the QC hot paths ([spec § 20],
//! todo item 24).
//!
//! Run: `cargo run -p tpt-app-media-qc-test --bin qc-bench [-- <filter>]`
//!
//! These are cycle-rough throughput probes, not statistical benchmarks: they
//! detect regressions and publish the committed baseline in
//! `tests/performance/baseline.md`. Results depend on the host machine and
//! load; compare runs on the same hardware.

use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;

use tpt_app_media_qc_core::fingerprint::Fingerprint;
use tpt_app_media_qc_model::report::AnalysisId;
use tpt_app_media_qc_pipeline::{arc, QcEngine};
use tpt_app_media_qc_profile::Profile;
use tpt_app_media_qc_report::{
    build_report, render_html, render_pdf, write_json_report, WriteHtmlOptions, WritePdfOptions,
};
use tpt_app_media_qc_rules::build_rules;
use tpt_app_media_qc_test::{fixtures_dir, load_profile, run_fixture, MediaFixture};

const GENERIC_YAML: &str = include_str!("../../../../profiles/generic/generic.yaml");

struct BenchResult {
    name: &'static str,
    samples: u32,
    iters: u64,
    median_ns: f64,
    min_ns: f64,
    bytes_per_op: u64,
}

fn bench<F: FnMut()>(
    name: &'static str,
    iters: u64,
    samples: u32,
    bytes: u64,
    mut f: F,
) -> BenchResult {
    for _ in 0..3 {
        f(); // warmup
    }
    let mut durations = Vec::with_capacity(samples as usize);
    for _ in 0..samples {
        let start = Instant::now();
        for _ in 0..iters {
            f();
        }
        durations.push(start.elapsed().as_nanos() as f64);
    }
    durations.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let per_op = |ns: f64| ns / iters as f64;
    BenchResult {
        name,
        samples,
        iters,
        median_ns: per_op(durations[durations.len() / 2]),
        min_ns: per_op(durations[0]),
        bytes_per_op: bytes,
    }
}

fn main() {
    let filter = std::env::args().nth(1).unwrap_or_default();

    // -- shared inputs ------------------------------------------------------
    let profile = Arc::new(load_profile("generic").expect("generic profile parses"));
    let fixture = MediaFixture::load_named("clean-master").expect("clean-master fixture");
    let fixture_json =
        std::fs::read(fixtures_dir().join("clean-master.json")).expect("fixture bytes");
    let rules = build_rules(&profile);
    let engine = QcEngine::new(
        Arc::clone(&profile),
        arc(tpt_app_media_qc_pipeline::NoopInspector),
    );

    // Deterministic 8 MiB input buffer for hashing (xorshift fill).
    let hash_input: Vec<u8> = {
        let mut v = vec![0u8; 8 * 1024 * 1024];
        let mut state = 0x9E3779B97F4A7C15u64;
        for chunk in v.chunks_mut(8) {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            chunk.copy_from_slice(&state.to_le_bytes()[..chunk.len()]);
        }
        v
    };

    let mut results = vec![
        bench(
            "fingerprint_full_scan_8mib",
            8,
            9,
            hash_input.len() as u64,
            || {
                black_box(Fingerprint::from_reader(black_box(&hash_input[..])).unwrap());
            },
        ),
        bench(
            "profile_parse_generic_yaml",
            200,
            9,
            GENERIC_YAML.len() as u64,
            || {
                black_box(Profile::from_yaml(GENERIC_YAML).unwrap());
            },
        ),
        bench("build_rules_generic", 200, 9, 0, || {
            black_box(build_rules(black_box(&profile)));
        }),
        bench("run_rules_clean_master", 200, 9, 0, || {
            black_box(tpt_app_media_qc_pipeline::run_rules(
                black_box(&rules),
                black_box(&fixture.asset),
                black_box(&fixture.inspection),
            ));
        }),
        bench("engine_check_noop_inspection", 50, 9, 0, || {
            black_box(
                engine
                    .check_metadata_only(black_box(&fixture.asset))
                    .unwrap(),
            );
        }),
        bench(
            "fixture_deserialize",
            200,
            9,
            fixture_json.len() as u64,
            || {
                black_box(
                    serde_json::from_slice::<MediaFixture>(black_box(&fixture_json)).unwrap(),
                );
            },
        ),
    ];

    // Report pipeline: build + render the text and print formats.
    let run = run_fixture(&fixture, &profile);
    let report_for = || build_report(&run, AnalysisId::default(), &profile);
    let tmp = std::env::temp_dir().join("tpt-qc-bench");
    let _ = std::fs::create_dir_all(&tmp);
    let json_path = tmp.join("bench.json");
    let html_path = tmp.join("bench.html");
    let pdf_path = tmp.join("bench.pdf");

    results.push(bench("report_build_and_json", 100, 9, 0, || {
        let report = report_for();
        black_box(serde_json::to_string(&report).unwrap());
    }));
    results.push(bench("report_render_html", 50, 9, 0, || {
        render_html(
            &html_path,
            &report_for(),
            WriteHtmlOptions { embed_json: true },
        )
        .unwrap();
    }));
    results.push(bench("report_render_pdf", 20, 7, 0, || {
        render_pdf(&pdf_path, &report_for(), WritePdfOptions {}).unwrap();
    }));
    let _ = write_json_report(&json_path, &report_for());

    // -- output -------------------------------------------------------------
    println!(
        "{:<30} {:>12} {:>12} {:>16}",
        "benchmark", "median/op", "min/op", "throughput"
    );
    println!("{}", "-".repeat(74));
    for r in results
        .iter()
        .filter(|r| filter.is_empty() || r.name.contains(filter.as_str()))
    {
        let throughput = if r.bytes_per_op > 0 {
            format!(
                "{:.1} MiB/s",
                (r.bytes_per_op as f64 / 1_048_576.0) / (r.median_ns / 1_000_000_000.0)
            )
        } else {
            format!("{:.0} ops/s", 1_000_000_000.0 / r.median_ns)
        };
        println!(
            "{:<30} {:>12} {:>12} {:>16}",
            r.name,
            format_ns(r.median_ns),
            format_ns(r.min_ns),
            throughput
        );
    }
    let (samples, iters) = results
        .first()
        .map(|r| (r.samples, r.iters))
        .unwrap_or((0, 0));
    println!("\n{samples} samples per bench, {iters} iterations per sample; lower is better.");

    let _ = std::fs::remove_dir_all(&tmp);
}

fn format_ns(ns: f64) -> String {
    if ns >= 1_000_000.0 {
        format!("{:.2} ms", ns / 1_000_000.0)
    } else if ns >= 1_000.0 {
        format!("{:.1} us", ns / 1_000.0)
    } else {
        format!("{ns:.0} ns")
    }
}
