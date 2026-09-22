//! Golden-media test suite ([spec § 24.2]).
//!
//! Every manifest in `tests/golden/` pairs a measurement fixture from
//! `tests/fixtures/` with a profile and pins the expected verdict plus the
//! expected per-rule statuses. Set `MEDIA_QC_UPDATE_GOLDEN=1` to regenerate
//! the manifests from current engine output (review before committing).

use tpt_app_media_qc_rules::known_rule_ids;
use tpt_app_media_qc_test::{
    golden_dir, golden_manifest_paths, load_profile, run_fixture, update_golden_requested,
    GoldenManifest, MediaFixture,
};

/// The canonical fixture × profile pairings of the golden suite. Update mode
/// regenerates exactly these; verify mode accepts any manifest present in
/// `tests/golden/`.
const PAIRINGS: &[(&str, &str)] = &[
    ("clean-master", "generic"),
    ("clean-master", "golden-suite"),
    ("container-problems", "golden-suite"),
    ("video-defects", "golden-suite"),
    ("audio-defects", "golden-suite"),
    ("boundary-thresholds", "golden-suite"),
    ("unscanned", "golden-suite"),
];

#[test]
fn golden_manifests_match_fixtures() {
    let update = update_golden_requested();
    let mut checked = 0usize;

    if update {
        for (fixture_name, profile_name) in PAIRINGS {
            let fixture = MediaFixture::load_named(fixture_name)
                .unwrap_or_else(|e| panic!("fixture '{fixture_name}': {e}"));
            let profile = load_profile(profile_name).unwrap_or_else(|e| panic!("{e}"));
            let run = run_fixture(&fixture, &profile);
            let manifest =
                GoldenManifest::from_run(fixture_name, profile.name.as_str(), &run, false);
            let path = GoldenManifest::path_for(&manifest.fixture, &manifest.profile);
            let json = serde_json::to_string_pretty(&manifest).unwrap();
            std::fs::write(&path, json + "\n")
                .unwrap_or_else(|e| panic!("could not write {}: {e}", path.display()));
            checked += 1;
        }
    } else {
        for path in golden_manifest_paths().expect("golden directory is readable") {
            let manifest = GoldenManifest::load(&path)
                .unwrap_or_else(|e| panic!("invalid manifest {}: {e}", path.display()));

            let fixture = MediaFixture::load_named(&manifest.fixture)
                .unwrap_or_else(|e| panic!("fixture '{}': {e}", manifest.fixture));
            let profile = load_profile(&manifest.profile)
                .unwrap_or_else(|e| panic!("manifest {}: {e}", path.display()));

            let run = run_fixture(&fixture, &profile);
            manifest
                .verify(&run)
                .unwrap_or_else(|e| panic!("golden mismatch in {}:\n{e}", path.display()));
            checked += 1;
        }
    }

    assert!(
        checked >= 7,
        "expected the MVP golden manifests, checked {checked}"
    );
}

#[test]
fn golden_suite_covers_every_production_rule() {
    // Union of the statuses pinned across all manifests must cover the whole
    // built-in rule catalogue (spec §24.2 / §30 "every production rule").
    let mut covered = std::collections::BTreeSet::new();
    for path in golden_manifest_paths().expect("golden directory is readable") {
        let manifest = GoldenManifest::load(&path).expect("valid manifest JSON");
        for rule in manifest.expected_statuses.keys() {
            covered.insert(rule.clone());
        }
    }

    let missing: Vec<&str> = known_rule_ids()
        .into_iter()
        .filter(|id| !covered.contains(*id))
        .collect();
    assert!(
        missing.is_empty(),
        "golden suite does not cover these production rules: {missing:?}\n(golden dir: {:?})",
        golden_dir()
    );
}
