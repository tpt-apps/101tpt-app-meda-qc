//! User preferences (spec § 18). JSON values keyed by a string.

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::Store;
use crate::error::Result;

impl Store {
    /// Set a preference value (any JSON-serializable T).
    pub fn set_pref<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        let json = serde_json::to_string(value)?;
        self.conn().execute(
            r#"
            INSERT INTO preferences (key, value_json) VALUES (?1, ?2)
            ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json
            "#,
            rusqlite::params![key, json],
        )?;
        Ok(())
    }

    /// Get a preference value.
    pub fn get_pref<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        use rusqlite::OptionalExtension;
        let row: Option<String> = self
            .conn()
            .query_row(
                "SELECT value_json FROM preferences WHERE key = ?1",
                [key],
                |row| row.get(0),
            )
            .optional()?;
        match row {
            Some(json) => Ok(Some(serde_json::from_str(&json)?)),
            None => Ok(None),
        }
    }

    /// Remove a preference.
    pub fn delete_pref(&self, key: &str) -> Result<()> {
        self.conn().execute("DELETE FROM preferences WHERE key = ?1", [key])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pref_roundtrip() {
        let store = Store::in_memory().unwrap();
        store.set_pref("theme", &"dark").unwrap();
        assert_eq!(store.get_pref::<String>("theme").unwrap().as_deref(), Some("dark"));
        assert!(store.get_pref::<String>("nope").unwrap().is_none());
        store.delete_pref("theme").unwrap();
        assert!(store.get_pref::<String>("theme").unwrap().is_none());
    }

    #[test]
    fn pref_overwrite() {
        let store = Store::in_memory().unwrap();
        store.set_pref("workers", &8u64).unwrap();
        store.set_pref("workers", &4u64).unwrap();
        assert_eq!(store.get_pref::<u64>("workers").unwrap(), Some(4));
    }
}