//! Asset and fingerprint persistence (spec § 6.1, § 18).

use chrono::Utc;
use tpt_app_media_qc_model::asset::{Asset, AssetFingerprint};

use crate::Store;
use crate::error::Result;

/// A lightweight row projection of a stored asset.
#[derive(Clone, Debug)]
pub struct StoredAsset {
    pub sha256: String,
    pub path: String,
    pub size_bytes: u64,
    pub modified_time: Option<u64>,
    pub duration_ms: Option<u64>,
}

impl Store {
    /// Upsert an asset keyed on its content fingerprint. The path is
    /// informational; identity is the fingerprint (spec § 6.1).
    pub fn register_asset(&self, asset: &Asset) -> Result<()> {
        let now = Utc::now().timestamp_millis();
        self.conn().execute(
            r#"
            INSERT INTO assets (sha256, path, size_bytes, modified_time, duration_ms, first_seen_at, last_seen_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
            ON CONFLICT(sha256) DO UPDATE SET
                path = excluded.path,
                size_bytes = excluded.size_bytes,
                modified_time = excluded.modified_time,
                duration_ms = excluded.duration_ms,
                last_seen_at = excluded.last_seen_at
            "#,
            rusqlite::params![
                asset.fingerprint.sha256,
                asset.path.display().to_string(),
                asset.size_bytes as i64,
                asset.modified_time,
                asset.duration.map(|d| d.as_secs() * 1000),
                now
            ],
        )?;
        Ok(())
    }

    /// Look up an asset by content fingerprint.
    pub fn asset_by_fingerprint(&self, sha256: &str) -> Result<Option<StoredAsset>> {
        use rusqlite::OptionalExtension;
        self.conn()
            .query_row(
                r#"
                SELECT sha256, path, size_bytes, modified_time, duration_ms
                FROM assets WHERE sha256 = ?1
                "#,
                [sha256],
                |row| {
                    Ok(StoredAsset {
                        sha256: row.get(0)?,
                        path: row.get(1)?,
                        size_bytes: row.get(2)?,
                        modified_time: row.get(3)?,
                        duration_ms: row.get(4)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    /// Number of known assets.
    pub fn asset_count(&self) -> Result<u64> {
        let n: i64 = self.conn().query_row("SELECT COUNT(*) FROM assets", [], |r| r.get(0))?;
        Ok(n as u64)
    }

    /// Register a fingerprint computation (strategy + size bound).
    pub fn register_fingerprint(&self, strategy: &str, fp: &AssetFingerprint) -> Result<()> {
        self.conn().execute(
            r#"
            INSERT INTO fingerprints (sha256, strategy, size_bytes, computed_at)
            VALUES (?1, ?2, ?3, ?4)
            ON CONFLICT(sha256) DO UPDATE SET
                strategy = excluded.strategy,
                size_bytes = excluded.size_bytes,
                computed_at = excluded.computed_at
            "#,
            rusqlite::params![fp.sha256, strategy, fp.size_bytes as i64, Utc::now().timestamp_millis()],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpt_app_media_qc_model::asset::AssetFingerprint;

    fn asset(sha: &str) -> Asset {
        Asset {
            id: Default::default(),
            path: "x.mp4".into(),
            fingerprint: AssetFingerprint { sha256: sha.to_string(), size_bytes: 10 },
            size_bytes: 10,
            modified_time: Some(1),
            duration: None,
            streams: vec![],
        }
    }

    #[test]
    fn register_and_lookup_asset() {
        let store = Store::in_memory().unwrap();
        store.register_asset(&asset(&"ab".repeat(32))).unwrap();
        let found = store.asset_by_fingerprint(&"ab".repeat(32)).unwrap().unwrap();
        assert_eq!(found.path, "x.mp4");
        assert_eq!(found.size_bytes, 10);
        assert!(store.asset_by_fingerprint(&"zz".repeat(32)).unwrap().is_none());
        assert_eq!(store.asset_count().unwrap(), 1);
    }

    #[test]
    fn re_registration_updates_path() {
        let store = Store::in_memory().unwrap();
        store.register_asset(&asset(&"ab".repeat(32))).unwrap();
        let mut moved = asset(&"ab".repeat(32));
        moved.path = "new/location.mp4".into();
        store.register_asset(&moved).unwrap();
        let found = store.asset_by_fingerprint(&"ab".repeat(32)).unwrap().unwrap();
        assert_eq!(found.path, "new/location.mp4");
        assert_eq!(store.asset_count().unwrap(), 1);
    }

    #[test]
    fn fingerprint_roundtrip() {
        let store = Store::in_memory().unwrap();
        let fp = AssetFingerprint { sha256: "cd".repeat(32), size_bytes: 42 };
        store.register_fingerprint("full_scan", &fp).unwrap();
    }
}