//! Shared test utilities and integration/golden support ([spec § 24]).
//!
//! This crate is the harness side of the product test suites stored in the
//! workspace-root `tests/` directory:
//!
//! - `tests/fixtures/` — deterministic *measurement* fixtures
//!   ([`MediaFixture`] JSON documents: an [`Asset`] plus the [`Inspection`]
//!   a probe would have produced for it). Rules consume measurements, not raw
//!   media bytes ([spec § 3.5]), so pinning the measurement set directly
//!   exercises the full rule engine without requiring encoded media. When the
//!   TPT decode stack lands (todo §2/§8/§10), encoded-media fixtures produced
//!   by real probes will be added alongside; the golden runner is agnostic —
//!   any [`Inspector`] can feed it.
//! - `tests/golden/` — expected-result manifests ([spec § 24.2],
//!   [`GoldenManifest`]) verified by the golden runner.
//! - `tests/performance/` — benchmark baselines produced by the `qc-bench`
//!   binary in this crate (spec §20).
//!
//! Integration tests live in this crate's `tests/` directory so `cargo test
//! --workspace` runs them together with the unit suites.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tpt_app_media_qc_core::error::Result as CoreResult;
use tpt_app_media_qc_model::asset::Asset;
use tpt_app_media_qc_model::inspection::Inspection;
use tpt_app_media_qc_pipeline::{arc, InspectionLevel, Inspector, QcEngine, QcRun};
use tpt_app_media_qc_profile::Profile;

/// Environment variable that switches the golden runner into regeneration
/// mode: set to `1` to rewrite every manifest from the current engine output
/// (review the diff before committing).
pub const UPDATE_GOLDEN_ENV: &str = "MEDIA_QC_UPDATE_GOLDEN";

/// The workspace repository root, derived from this crate's location.
pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

/// The workspace-root `tests/` directory (fixture + golden data).
pub fn repo_tests_dir() -> PathBuf {
    repo_root().join("tests")
}

/// The measurement-fixture directory (`tests/fixtures/`).
pub fn fixtures_dir() -> PathBuf {
    repo_tests_dir().join("fixtures")
}

/// The golden-manifest directory (`tests/golden/`).
pub fn golden_dir() -> PathBuf {
    repo_tests_dir().join("golden")
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// A deterministic measurement fixture: the asset under QC paired with the
/// inspection a probe front-end would have produced ([spec § 3.5]).
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MediaFixture {
    pub asset: Asset,
    pub inspection: Inspection,
}

impl MediaFixture {
    /// Load a fixture from a JSON document.
    pub fn load(path: &Path) -> std::io::Result<Self> {
        let source = fs::read_to_string(path)?;
        serde_json::from_str(&source)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    /// Load a fixture from `tests/fixtures/<name>.json`.
    pub fn load_named(name: &str) -> std::io::Result<Self> {
        Self::load(&fixtures_dir().join(format!("{name}.json")))
    }
}

/// An [`Inspector`] that replays a fixture's canned [`Inspection`] instead of
/// probing real media. This seam makes the whole rule engine testable without
/// encoded media files or an `ffprobe` installation.
pub struct FixtureInspector {
    inspection: Inspection,
}

impl FixtureInspector {
    pub fn new(fixture: &MediaFixture) -> Self {
        Self {
            inspection: fixture.inspection.clone(),
        }
    }
}

impl Inspector for FixtureInspector {
    fn name(&self) -> &str {
        "fixture"
    }

    fn inspect_metadata(&self, _asset: &Asset) -> CoreResult<Inspection> {
        Ok(self.inspection.clone())
    }
}

// ---------------------------------------------------------------------------
// Profiles
// ---------------------------------------------------------------------------

/// Load a bundled profile by name. Searches the repository `profiles/` tree
/// (`generic`, `broadcast`, `streaming`, `examples`) and then
/// `tests/fixtures/` for `<name>.yaml`.
pub fn load_profile(name: &str) -> std::result::Result<Profile, String> {
    let root = repo_root();
    let mut candidates = Vec::new();
    for group in ["generic", "broadcast", "streaming", "examples"] {
        candidates.push(
            root.join("profiles")
                .join(group)
                .join(format!("{name}.yaml")),
        );
    }
    candidates.push(fixtures_dir().join(format!("{name}.yaml")));

    for path in &candidates {
        if path.is_file() {
            let source = fs::read_to_string(path)
                .map_err(|e| format!("could not read profile {}: {e}", path.display()))?;
            return Profile::from_yaml(&source)
                .map_err(|e| format!("invalid profile {}: {e}", path.display()));
        }
    }
    Err(format!(
        "profile '{name}' not found; searched:\n{}",
        candidates
            .iter()
            .map(|p| format!("  {}", p.display()))
            .collect::<Vec<_>>()
            .join("\n")
    ))
}

// ---------------------------------------------------------------------------
// Running fixtures
// ---------------------------------------------------------------------------

/// Run a fixture through the QC engine at metadata-only depth with the given
/// profile and return the full [`QcRun`].
pub fn run_fixture(fixture: &MediaFixture, profile: &Profile) -> QcRun {
    let engine = QcEngine::new(
        Arc::new(profile.clone()),
        arc(FixtureInspector::new(fixture)),
    );
    engine
        .check(&fixture.asset, InspectionLevel::MetadataOnly)
        .expect("fixture QC run must succeed: fixtures never touch the filesystem")
}

// ---------------------------------------------------------------------------
// Golden manifests (spec §24.2)
// ---------------------------------------------------------------------------

/// Expected result for one fixture × profile pairing ([spec § 24.2]).
///
/// The format matches `tests/golden/README.md`: `expected_statuses` is a
/// **minimum** set — every listed rule id must have exactly the listed best
/// status; rules not listed are unconstrained. Findings are deliberately not
/// compared by message text so thresholds can evolve without golden churn.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GoldenManifest {
    /// Fixture stem (file name in `tests/fixtures/` without `.json`).
    pub fixture: String,
    /// Profile name resolved by [`load_profile`].
    pub profile: String,
    /// Expected final verdict: `pass` | `warn` | `fail` | `inconclusive`.
    pub expected_verdict: String,
    /// Minimum per-rule best-status map: rule id → expected status.
    #[serde(default)]
    pub expected_statuses: BTreeMap<String, String>,
    /// `true` marks a placeholder manifest whose constraints are advisory
    /// (used for fixtures that do not yet pin behaviour).
    #[serde(default)]
    pub allow_critical_empty: bool,
}

impl GoldenManifest {
    /// The canonical golden path for a fixture × profile pairing:
    /// `tests/golden/<fixture>.<profile>.json`.
    pub fn path_for(fixture: &str, profile: &str) -> PathBuf {
        golden_dir().join(format!("{fixture}.{profile}.json"))
    }

    /// Load a manifest from a JSON document.
    pub fn load(path: &Path) -> std::io::Result<Self> {
        let source = fs::read_to_string(path)?;
        serde_json::from_str(&source)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    /// Build a manifest that records the *full* per-rule status map of a run.
    pub fn from_run(fixture: &str, profile: &str, run: &QcRun, allow_critical_empty: bool) -> Self {
        Self {
            fixture: fixture.to_string(),
            profile: profile.to_string(),
            expected_verdict: run.verdict.as_str().to_string(),
            expected_statuses: statuses_of(run),
            allow_critical_empty,
        }
    }

    /// Verify a run against this manifest's expectations.
    pub fn verify(&self, run: &QcRun) -> std::result::Result<(), String> {
        if self.expected_statuses.is_empty() && self.allow_critical_empty {
            return Ok(());
        }

        let actual: BTreeMap<&str, &str> = run
            .per_rule
            .iter()
            .map(|r| (r.rule_id.as_str(), r.best.as_str()))
            .collect();

        if run.verdict.as_str() != self.expected_verdict {
            return Err(format!(
                "verdict mismatch: expected '{}', got '{}' ({} fail, {} warn, {} inconclusive)",
                self.expected_verdict,
                run.verdict.as_str(),
                run.counts.fail,
                run.counts.warn,
                run.counts.inconclusive,
            ));
        }

        let mut mismatches = Vec::new();
        for (rule_id, expected) in &self.expected_statuses {
            match actual.get(rule_id.as_str()) {
                Some(actual) if actual == expected => {}
                Some(actual) => mismatches.push(format!(
                    "rule '{rule_id}': expected status '{expected}', got '{actual}'"
                )),
                None => mismatches.push(format!(
                    "rule '{rule_id}' expects status '{expected}' but the rule is not enabled by profile '{}'",
                    self.profile
                )),
            }
        }
        if mismatches.is_empty() {
            Ok(())
        } else {
            Err(mismatches.join("\n"))
        }
    }
}

/// Best status per rule id of a run, as plain strings (rule id → status).
pub fn statuses_of(run: &QcRun) -> BTreeMap<String, String> {
    run.per_rule
        .iter()
        .map(|r| (r.rule_id.clone(), r.best.as_str().to_string()))
        .collect()
}

/// Whether the golden runner should regenerate manifests instead of verifying.
pub fn update_golden_requested() -> bool {
    std::env::var(UPDATE_GOLDEN_ENV).ok().as_deref() == Some("1")
}

/// All `*.json` golden manifests in `tests/golden/`, sorted by path.
pub fn golden_manifest_paths() -> std::io::Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(golden_dir())? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) == Some("json") {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_profiles_resolve() {
        assert_eq!(load_profile("generic").unwrap().name, "generic");
        assert!(load_profile("does-not-exist").is_err());
    }

    #[test]
    fn fixture_directory_contains_fixtures() {
        let found = fs::read_dir(fixtures_dir())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
            .count();
        assert!(
            found > 0,
            "expected measurement fixtures in {:?}",
            fixtures_dir()
        );
    }
}
