//! Profile persistence (spec § 9, § 18).

use chrono::Utc;
use tpt_app_media_qc_profile::hash::profile_sha256;
use tpt_app_media_qc_profile::model::Profile;

use crate::error::Result;
use crate::Store;

/// A stored profile row (canonical document + identity).
#[derive(Clone, Debug)]
pub struct StoredProfile {
    pub sha256: String,
    pub name: String,
    pub version: u32,
    pub document: String,
}

impl Store {
    /// Store a profile keyed on its canonical SHA-256. The document column
    /// holds the canonical JSON serialisation: `Profile` round-trips through
    /// serde, whereas the canonical YAML body would emit `null` option fields
    /// that the strict profile parser rejects on re-read.
    pub fn put_profile(&self, profile: &Profile) -> Result<String> {
        let sha = profile_sha256(profile);
        let document = serde_json::to_string(profile)?;
        self.conn().execute(
            r#"
            INSERT INTO profiles (sha256, name, version, document, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(sha256) DO UPDATE SET
                name = excluded.name,
                version = excluded.version,
                document = excluded.document
            "#,
            rusqlite::params![
                sha,
                profile.name,
                profile.version as i64,
                document,
                Utc::now().timestamp_millis()
            ],
        )?;
        Ok(sha)
    }

    /// Look up a stored profile by SHA-256.
    pub fn profile_by_sha256(&self, sha256: &str) -> Result<Option<Profile>> {
        use rusqlite::OptionalExtension;
        let row: Option<(String, i64, String)> = self
            .conn()
            .query_row(
                "SELECT name, version, document FROM profiles WHERE sha256 = ?1",
                [sha256],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        match row {
            Some((name, version, doc)) => {
                let mut profile = decode_document(&doc)?;
                profile.name = name;
                if version > 0 {
                    profile.version = version as u32;
                }
                Ok(Some(profile))
            }
            None => Ok(None),
        }
    }

    /// Look up a stored profile by name (latest version wins).
    pub fn profile_by_name(&self, name: &str) -> Result<Option<Profile>> {
        use rusqlite::OptionalExtension;
        let row: Option<(String, u32, String)> = self
            .conn()
            .query_row(
                "SELECT sha256, version, document FROM profiles WHERE name = ?1 ORDER BY version DESC LIMIT 1",
                [name],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        match row {
            Some((_sha, _version, doc)) => Ok(Some(decode_document(&doc)?)),
            None => Ok(None),
        }
    }

    /// List every stored profile.
    pub fn profiles(&self) -> Result<Vec<(String, String, u32)>> {
        let mut stmt = self
            .conn()
            .prepare("SELECT sha256, name, version FROM profiles ORDER BY name, version")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }
}

/// Decode the stored JSON document back into a validated-shaped `Profile`.
fn decode_document(doc: &str) -> Result<Profile> {
    serde_json::from_str(doc).map_err(crate::error::Error::Json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpt_app_media_qc_profile::parse_str;

    #[test]
    fn profile_roundtrip_by_sha() {
        let store = Store::in_memory().unwrap();
        let p =
            parse_str("name: x\nversion: 2\nrules:\n  container:\n    readable: error\n").unwrap();
        let sha = store.put_profile(&p).unwrap();
        let back = store.profile_by_sha256(&sha).unwrap().unwrap();
        assert_eq!(back.name, "x");
        assert_eq!(back.version, 2);
        assert_eq!(
            back.rules.container.readable,
            Some(tpt_app_media_qc_model::severity::Severity::Error)
        );
    }

    #[test]
    fn profile_by_name_latest_version() {
        let store = Store::in_memory().unwrap();
        store
            .put_profile(&parse_str("name: x\nversion: 1\n").unwrap())
            .unwrap();
        store
            .put_profile(&parse_str("name: x\nversion: 2\n").unwrap())
            .unwrap();
        let found = store.profile_by_name("x").unwrap().unwrap();
        assert_eq!(found.version, 2);
        let list = store.profiles().unwrap();
        assert_eq!(list.len(), 2);
    }
}
