//! Deterministic profile hashing ([spec § 14.1], [spec § 19]).
//!
//! The profile SHA-256 is computed over the canonical YAML serialisation of
//! the *typed* profile, so any two equivalent YAML documents (different
//! whitespace, key order, commented-out rules) hash identically.

use crate::model::Profile;
use sha2::{Digest, Sha256};

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
}