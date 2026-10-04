//! Validate every bundled profile parses and hashes deterministically.
//!
//! This is the repo's self-check for the `profiles/` tree (spec §9): a profile
//! that ships with the product must parse under strict validation and must
//! hash to a stable value (integrity fields + cache keys depend on it).

use std::path::{Path, PathBuf};

use tpt_app_media_qc_profile::model::Profile;
use tpt_app_media_qc_profile::{parse_str, profile_sha256};

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
    assert!(
        !files.is_empty(),
        "no profile YAML found under {}",
        root.display()
    );

    for path in &files {
        let source = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let profile: Profile =
            parse_str(&source).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));

        assert!(
            !profile.name.is_empty(),
            "{} has empty name",
            path.display()
        );
        // Required name/version round-trip sanity.
        assert_eq!(
            profile_sha256(&profile).len(),
            64,
            "{} sha must be 64 hex",
            path.display()
        );

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

#[test]
fn hdr10_profile_configures_the_hdr_rule() {
    let text = std::fs::read_to_string(profiles_root().join("streaming/hdr10.yaml")).unwrap();
    let profile = parse_str(&text).expect("hdr10 profile parses");
    let hdr = profile.rules.video.hdr.expect("hdr rule configured");
    assert_eq!(hdr.mode, tpt_app_media_qc_profile::model::HdrMode::Hdr10);
    assert_eq!(hdr.max_cll_nits, Some(4000));
    assert!(hdr.require_static_metadata);
}

#[test]
fn unknown_hdr_mode_is_rejected() {
    let text = "name: x\nversion: 1\nrules:\n  video:\n    hdr: dolby\n";
    assert!(parse_str(text).is_err());
}

#[test]
fn subtitle_rules_parse_with_documented_defaults() {
    use tpt_app_media_qc_model::severity::Severity;
    let text = "\
name: subs
version: 1
rules:
  subtitle:
    presence:
      min_subtitle: 2
      max_subtitle: 4
      severity: error
    language:
      required: [\"eng\", \"fre\"]
      min_tracks: 2
    timing:
      max_gap_ms: 5000
      max_cue_duration_ms: 7000
      severity: warning
    content:
      max_chars_per_line: 42
      max_lines_per_cue: 2
    duration_match:
      tolerance_ms: 250
      allow_longer: false
      severity: warning
";
    let profile = parse_str(text).expect("subtitle profile parses");
    let s = &profile.rules.subtitle;

    let presence = s.presence.expect("presence configured");
    assert_eq!(presence.min_subtitle, 2);
    assert_eq!(presence.max_subtitle, Some(4));
    assert_eq!(presence.severity, Severity::Error);

    let language = s.language.as_ref().expect("language configured");
    assert_eq!(language.required, ["eng".to_string(), "fre".to_string()]);
    assert_eq!(language.min_tracks, 2);

    let timing = s.timing.expect("timing configured");
    // Unset limits fall back to "no defect tolerated".
    assert_eq!(timing.max_overlaps, 0);
    assert_eq!(timing.max_invalid_durations, 0);
    assert_eq!(timing.max_gap_ms, Some(5_000));
    assert_eq!(timing.max_cue_duration_ms, Some(7_000));

    let content = s.content.expect("content configured");
    assert_eq!(content.max_malformed, 0);
    assert_eq!(content.max_empty, 0);
    assert_eq!(content.max_chars_per_line, Some(42));

    let duration = s.duration_match.expect("duration_match configured");
    assert_eq!(duration.tolerance_ms, 250);
    assert!(!duration.allow_longer);
}

#[test]
fn subtitle_scalar_shorthands_use_sane_defaults() {
    let text = "name: subs\nversion: 1\nrules:\n  subtitle:\n    presence: error\n    timing: warning\n    content: error\n    duration_match: warning\n";
    let s = parse_str(text).expect("shorthand parses").rules.subtitle;
    assert_eq!(s.presence.expect("presence").min_subtitle, 1);
    assert!(s.presence.expect("presence").max_subtitle.is_none());
    let timing = s.timing.expect("timing");
    assert_eq!(timing.max_overlaps, 0);
    assert!(timing.max_gap_ms.is_none());
    let duration = s.duration_match.expect("duration_match");
    assert_eq!(duration.tolerance_ms, 1000);
    assert!(duration.allow_longer);
    assert!(s.language.is_none());
}

#[test]
fn malformed_subtitle_blocks_are_rejected() {
    // Unknown key at the group level.
    assert!(parse_str("name: x\nrules:\n  subtitle:\n    missing_subtitles: error\n").is_err());
    // Unknown key inside a rule mapping.
    assert!(
        parse_str("name: x\nrules:\n  subtitle:\n    timing:\n      max_overlaps_typo: 1\n")
            .is_err()
    );
    // `language` needs an explicit required list; a bare severity is ambiguous.
    assert!(parse_str("name: x\nrules:\n  subtitle:\n    language: error\n").is_err());
    // Empty required list is meaningless.
    assert!(
        parse_str("name: x\nrules:\n  subtitle:\n    language:\n      required: []\n").is_err()
    );
    // `allow_longer` must be a boolean.
    assert!(parse_str(
        "name: x\nrules:\n  subtitle:\n    duration_match:\n      allow_longer: yes please\n"
    )
    .is_err());
}
