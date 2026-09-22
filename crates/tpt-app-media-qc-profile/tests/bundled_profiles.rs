//! Validate every bundled profile parses and hashes deterministically.
//!
//! This is the repo's self-check for the `profiles/` tree (spec §9): a profile
//! that ships with the product must parse under strict validation and must
//! hash to a stable value (integrity fields + cache keys depend on it).

use std::path::{Path, PathBuf};

use tpt_app_media_qc_profile::{parse_str, profile_sha256};
use tpt_app_media_qc_profile::model::Profile;

/// Workspace `profiles/` directory relative to this crate's manifest dir.
fn profiles_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate under workspace")
        .parent()
        .expect("workspace root")
        .join("profiles")
}

fn collect_yaml(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read profiles dir") {
        let entry = entry.expect("entry");
        let path = entry.path();
        if path.is_dir() {
            collect_yaml(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("yaml") {
            out.push(path);
        }
    }
}

#[test]
fn every_bundled_profile_parses_and_hashes() {
    let root = profiles_root();
    assert!(root.is_dir(), "expected profiles dir at {}", root.display());

    let mut files = Vec::new();
    collect_yaml(&root, &mut files);
    files.sort();
    assert!(!files.is_empty(), "no profile YAML found under {}", root.display());

    for path in &files {
        let source = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let profile: Profile = parse_str(&source)
            .unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));

        assert!(!profile.name.is_empty(), "{} has empty name", path.display());
        // Required name/version round-trip sanity.
        assert_eq!(profile_sha256(&profile).len(), 64, "{} sha must be 64 hex", path.display());

        // Determinism: hashing twice yields the same digest.
        assert_eq!(
            profile_sha256(&profile),
            profile_sha256(&profile),
            "{} must hash deterministically",
            path.display()
        );
    }
}

#[test]
fn generic_profile_matches_cli_default_shape() {
    // The CLI embeds a generic default with a known rule set; the bundled
    // profile must stay a superset-compatible, valid profile. This guards the
    // documented "generic is the default" contract without duplicating the
    // embedded constant.
    let generic = profiles_root().join("generic/generic.yaml");
    let source = std::fs::read_to_string(&generic)
        .unwrap_or_else(|e| panic!("read {}: {e}", generic.display()));
    let profile = parse_str(&source).expect("generic profile parses");
    assert_eq!(profile.name, "generic");
    assert!(profile.rules.video.black_frames.is_some());
    assert!(profile.rules.audio.loudness.is_some());
}