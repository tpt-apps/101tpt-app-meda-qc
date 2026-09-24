//! Per-rule result cache (spec § 19).
//!
//! Each rule's cached results are keyed on the asset fingerprint + application
//! version + ruleset version + rule id + *that rule's* configuration hash.
//! The per-rule configuration hash plays the role of the profile hash and
//! analysis configuration hash at the granularity that matters: changing one
//! rule's threshold changes only that rule's hash, so only that rule's cached
//! results are invalidated (spec § 19 example).

use chrono::Utc;
use tpt_app_media_qc_core::config::{APP_VERSION, RULESET_VERSION};
use tpt_app_media_qc_model::finding::QcFinding;
use tpt_app_media_qc_profile::hash::per_rule_config_hash;
use tpt_app_media_qc_profile::model::Profile;

use crate::error::Result;
use crate::Store;

/// Identity components of a cache entry — the full key per spec § 19.
#[derive(Clone, Debug)]
pub struct CacheKey<'a> {
    pub asset_sha256: &'a str,
    pub profile: &'a Profile,
    pub rule_id: &'a str,
}

impl Store {
    /// Look up cached findings for a rule. Returns `None` when the rule is not
    /// configured, is not cached, or the cache key changed.
    pub fn cache_get(&self, key: &CacheKey<'_>) -> Result<Option<Vec<QcFinding>>> {
        let Some(config_hash) = per_rule_config_hash(key.profile, key.rule_id) else {
            return Ok(None);
        };

        let row: Option<String> = {
            use rusqlite::OptionalExtension;
            self.conn()
                .query_row(
                    r#"
                    SELECT findings_json FROM rule_cache
                    WHERE asset_sha256 = ?1
                      AND app_version = ?2 AND ruleset_version = ?3
                      AND rule_id = ?4 AND config_hash = ?5
                    "#,
                    rusqlite::params![
                        key.asset_sha256,
                        APP_VERSION,
                        RULESET_VERSION,
                        key.rule_id,
                        config_hash
                    ],
                    |row| row.get(0),
                )
                .optional()?
        };

        match row {
            Some(json) => Ok(Some(serde_json::from_str(&json)?)),
            None => Ok(None),
        }
    }

    /// Store a rule's findings for later reuse. Returns `false` (and stores
    /// nothing) if the rule is not configured for the profile.
    pub fn cache_put(&self, key: &CacheKey<'_>, findings: &[QcFinding]) -> Result<bool> {
        let Some(config_hash) = per_rule_config_hash(key.profile, key.rule_id) else {
            return Ok(false);
        };
        let json = serde_json::to_string(findings)?;

        self.conn().execute(
            r#"
            INSERT INTO rule_cache
                (asset_sha256, app_version, ruleset_version, rule_id, config_hash, findings_json, cached_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(asset_sha256, app_version, ruleset_version, rule_id, config_hash)
            DO UPDATE SET findings_json = excluded.findings_json, cached_at = excluded.cached_at
            "#,
            rusqlite::params![
                key.asset_sha256,
                APP_VERSION,
                RULESET_VERSION,
                key.rule_id,
                config_hash,
                json,
                Utc::now().timestamp_millis()
            ],
        )?;
        Ok(true)
    }

    /// Drop all cached results for a profile/rule combination (used when a
    /// ruleset version changes, for example).
    pub fn cache_clear_rule(&self, rule_id: &str) -> Result<()> {
        self.conn().execute(
            "DELETE FROM rule_cache WHERE rule_id = ?1 OR ruleset_version <> ?2",
            rusqlite::params![rule_id, RULESET_VERSION],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpt_app_media_qc_model::finding::QcFinding;
    use tpt_app_media_qc_profile::parse_str;

    fn profile(doc: &str) -> Profile {
        parse_str(doc).unwrap()
    }

    #[test]
    fn cache_roundtrip_full_key() {
        let store = Store::in_memory().unwrap();
        let p =
            profile("name: x\nrules:\n  video:\n    black_frames:\n      max_duration_ms: 500\n");
        let sha = "ab".repeat(32);
        let key = CacheKey {
            asset_sha256: &sha,
            profile: &p,
            rule_id: "video.black_frames",
        };

        assert!(store.cache_get(&key).unwrap().is_none());

        let finding = QcFinding::new("video.black_frames")
            .fail()
            .severity(tpt_app_media_qc_model::severity::Severity::Error)
            .some("black for 900ms");
        store
            .cache_put(&key, std::slice::from_ref(&finding))
            .unwrap();

        let got = store.cache_get(&key).unwrap().unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].rule_id, finding.rule_id);
        assert_eq!(got[0].status, finding.status);
    }

    #[test]
    fn threshold_change_invalidates_only_that_rule() {
        let store = Store::in_memory().unwrap();
        let sha = "cd".repeat(32);
        let p500 = profile("name: x\nrules:\n  video:\n    black_frames:\n      max_duration_ms: 500\n    freeze_frames:\n      max_duration_ms: 900\n");
        let p600 = profile("name: x\nrules:\n  video:\n    black_frames:\n      max_duration_ms: 600\n    freeze_frames:\n      max_duration_ms: 900\n");

        let key_bf = CacheKey {
            asset_sha256: &sha,
            profile: &p500,
            rule_id: "video.black_frames",
        };
        let key_ff = CacheKey {
            asset_sha256: &sha,
            profile: &p500,
            rule_id: "video.freeze_frames",
        };
        store
            .cache_put(&key_bf, &[QcFinding::new("video.black_frames").fail()])
            .unwrap();
        store
            .cache_put(&key_ff, &[QcFinding::new("video.freeze_frames").pass()])
            .unwrap();

        // Same profile: both cache hits.
        assert!(store.cache_get(&key_bf).unwrap().is_some());
        assert!(store.cache_get(&key_ff).unwrap().is_some());

        // Black-frame threshold bumped to 600: its entry misses...
        let key_bf_new = CacheKey {
            asset_sha256: &sha,
            profile: &p600,
            rule_id: "video.black_frames",
        };
        assert!(store.cache_get(&key_bf_new).unwrap().is_none());
        // ...but the untouched freeze-frames rule still hits (spec §19).
        let key_ff_same = CacheKey {
            asset_sha256: &sha,
            profile: &p600,
            rule_id: "video.freeze_frames",
        };
        assert!(store.cache_get(&key_ff_same).unwrap().is_some());
    }

    #[test]
    fn unconfigured_rule_is_never_cached() {
        let store = Store::in_memory().unwrap();
        let p = profile("name: x\nrules:\n  container:\n    readable: error\n");
        let sha = "ee".repeat(32);
        let key = CacheKey {
            asset_sha256: &sha,
            profile: &p,
            rule_id: "video.black_frames",
        };
        assert!(store.cache_get(&key).unwrap().is_none());
    }
}
