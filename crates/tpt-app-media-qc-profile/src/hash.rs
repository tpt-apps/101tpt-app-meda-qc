//! Deterministic profile hashing ([spec § 14.1], [spec § 19]).
//!
//! The profile SHA-256 is computed over the canonical YAML serialisation of
//! the *typed* profile, so any two equivalent YAML documents (different
//! whitespace, key order, commented-out rules) hash identically.

use crate::model::Profile;
use sha2::{Digest, Sha256};
use serde::Serialize;

/// Result of canonical serialisation + hashing.
pub fn profile_sha256(profile: &Profile) -> String {
    let canonical = serde_yaml::to_string(profile).expect("profile is serializable");
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    hex::encode(hasher.finalize())
}

/// Canonical YAML body, exposed for debugging and signature verification.
pub fn canonical_yaml(profile: &Profile) -> String {
    serde_yaml::to_string(profile).expect("profile is serializable")
}

/// Per-rule configuration hash (spec § 19 "analysis configuration hash").
///
/// The cache layer keys each rule's cached results on the hash of *that
/// rule's* configuration object, so changing one rule's threshold invalidates
/// only that rule's cached results, not the whole analysis. Returns `None`
/// for rule ids that are not configured (no cache entry is expected then).
///
/// The hash is over the canonical serde serialization of the rule's
/// configuration struct; equivalent YAML documents produce identical hashes.
pub fn per_rule_config_hash(profile: &Profile, rule_id: &str) -> Option<String> {
    fn hash<T: Serialize>(config: &Option<T>) -> Option<String> {
        let body = config.as_ref()?;
        let canonical = serde_json::to_string(body).ok()?;
        let mut hasher = Sha256::new();
        hasher.update(canonical.as_bytes());
        Some(hex::encode(hasher.finalize()))
    }

    let c = &profile.rules.container;
    let v = &profile.rules.video;
    let a = &profile.rules.audio;
    let s = &profile.rules.subtitle;
    match rule_id {
        "container.readable" => hash(&c.readable),
        "container.container_validity" => hash(&c.container_validity),
        "container.malformed_metadata" => hash(&c.malformed_metadata),
        "container.duration_consistency" => hash(&c.duration_consistency),
        "container.bitrate" => hash(&c.bitrate),
        "container.timecode_present" => hash(&c.timecode_present),
        "container.timebase" => hash(&c.timebase),
        "container.timestamp_continuity" => hash(&c.timestamp_continuity),
        "container.unexpected_streams" => hash(&c.unexpected_streams),
        "container.stream_presence" => hash(&c.stream_presence),
        "video.resolution" => hash(&v.resolution),
        "video.frame_rate" => hash(&v.frame_rate),
        "video.aspect_ratio" => hash(&v.aspect_ratio),
        "video.black_frames" => hash(&v.black_frames),
        "video.freeze_frames" => hash(&v.freeze_frames),
        "video.duplicate_frames" => hash(&v.duplicate_frames),
        "video.corrupt_frames" => hash(&v.corrupt_frames),
        "video.luma_range" => hash(&v.luma_range),
        "video.color_space" => hash(&v.color_space),
        "audio.sample_rate" => hash(&a.sample_rate),
        "audio.bit_depth" => hash(&a.bit_depth),
        "audio.channel_layout" => hash(&a.channel_layout),
        "audio.silence" => hash(&a.silence),
        "audio.clipping" => hash(&a.clipping),
        "audio.peak" => hash(&a.peak),
        "audio.true_peak" => hash(&a.true_peak),
        "audio.loudness" => hash(&a.loudness),
        "audio.phase" => hash(&a.phase),
        "audio.dc_offset" => hash(&a.dc_offset),
        "subtitle.language" => hash(&s.language),
        "subtitle.missing_subtitles" => hash(&s.missing_subtitles),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_str;

    #[test]
    fn equivalent_documents_hash_identically() {
        let a = parse_str(
            "name: x\nversion: 1\nrules:\n  container:\n    readable: error\n",
        )
        .unwrap();
        // Different ordering + comments, same semantics.
        let b = parse_str(
            "# a comment\nname: x\nrules:\n  container:\n    readable: error\nversion: 1\n",
        )
        .unwrap();

        let ha = profile_sha256(&a);
        let hb = profile_sha256(&b);
        assert_eq!(ha, hb, "equivalent profiles must hash identically");
        assert_eq!(ha.len(), 64);
    }

    #[test]
    fn different_rules_hash_differently() {
        let a = parse_str("name: x\nrules:\n  container:\n    readable: error\n").unwrap();
        let b = parse_str("name: x\nrules:\n  container:\n    readable: warning\n").unwrap();
        assert_ne!(profile_sha256(&a), profile_sha256(&b));
    }

    #[test]
    fn threshold_change_invalidates_only_that_rule_hash_component() {
        // A hash is global by construction; the fine-grained per-rule
        // invalidation is handled by the pipeline cache layer keying on the
        // per-rule configuration object (see pipeline crate).
        let a = parse_str(
            "name: x\nrules:\n  video:\n    black_frames:\n      max_duration_ms: 500\n",
        )
        .unwrap();
        let b = parse_str(
            "name: x\nrules:\n  video:\n    black_frames:\n      max_duration_ms: 600\n",
        )
        .unwrap();
        assert_ne!(profile_sha256(&a), profile_sha256(&b));
    }

    #[test]
    fn per_rule_hash_changes_only_for_touched_rule() {
        let a = parse_str(
            "name: x\nrules:\n  video:\n    black_frames:\n      max_duration_ms: 500\n      severity: warning\n    freeze_frames:\n      max_duration_ms: 900\n",
        )
        .unwrap();
        let b = parse_str(
            "name: x\nrules:\n  video:\n    black_frames:\n      max_duration_ms: 600\n      severity: warning\n    freeze_frames:\n      max_duration_ms: 900\n",
        )
        .unwrap();

        let bf_a = per_rule_config_hash(&a, "video.black_frames").expect("configured");
        let bf_b = per_rule_config_hash(&b, "video.black_frames").expect("configured");
        assert_ne!(bf_a, bf_b, "touched rule's hash must change");

        assert_eq!(
            per_rule_config_hash(&a, "video.freeze_frames"),
            per_rule_config_hash(&b, "video.freeze_frames"),
            "untouched rule's hash must NOT change (spec §19)"
        );
    }

    #[test]
    fn per_rule_hash_unconfigured_is_none() {
        let profile = parse_str("name: x\nrules:\n  container:\n    readable: error\n").unwrap();
        assert!(per_rule_config_hash(&profile, "container.readable").is_some());
        assert!(per_rule_config_hash(&profile, "video.black_frames").is_none());
        assert!(per_rule_config_hash(&profile, "video.nope").is_none());
    }
}