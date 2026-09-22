//! Fuzz the machine-readable result/report parser ([spec § 24.4]; model
//! crate). Arbitrary bytes are deserialised into the report and finding
//! structures; parsing must either succeed or return an error — never panic —
//! and a successful parse must round-trip through serialisation.

#![no_main]

use libfuzzer_sys::fuzz_target;
use tpt_app_media_qc_model::finding::QcFinding;
use tpt_app_media_qc_model::report::Report;

fuzz_target!(|data: &[u8]| {
    // The report structure is the machine-readable exchange format (spec
    // §14): a total, panic-free parser is part of the stable contract.
    if let Ok(report) = serde_json::from_slice::<Report>(data) {
        let re = serde_json::to_vec(&report).expect("report is serializable");
        let reparsed: Report =
            serde_json::from_slice(&re).expect("re-serialised report must re-parse");
        assert_eq!(reparsed.asset_path, report.asset_path);
        assert_eq!(reparsed.findings.len(), report.findings.len());
        assert_eq!(reparsed.verdict, report.verdict);
    }

    // Individual findings are also consumed standalone (per-rule cache,
    // spec §19).
    if let Ok(finding) = serde_json::from_slice::<QcFinding>(data) {
        let re = serde_json::to_vec(&finding).expect("finding is serializable");
        let _: QcFinding = serde_json::from_slice(&re).expect("re-serialised finding must re-parse");
    }
});