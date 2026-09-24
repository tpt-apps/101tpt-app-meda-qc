//! The SQLite-backed [`Store`].

use std::path::Path;

use crate::error::Result;
use crate::schema;
use rusqlite::Connection;

/// Local application state backed by a single SQLite database (spec § 18).
///
/// A `Store` is cheap to clone? No — hold it directly or wrap in `Arc`.
/// Connection setup runs the schema migrations on open.
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Open (creating if needed) the database at `path`, running migrations.
    pub fn open(path: &Path) -> Result<Self> {
        if path.to_string_lossy() != ":memory:" {
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent)?;
                }
            }
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", true)?;
        schema::migrate(&conn)?;
        Ok(Self { conn })
    }

    /// An in-memory database for tests and ephemeral tooling.
    pub fn in_memory() -> Result<Self> {
        Self::open(Path::new(":memory:"))
    }

    /// Access to the raw connection for custom queries.
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Current on-disk schema version.
    pub fn schema_version(&self) -> i64 {
        self.conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_memory_opens_and_migrates() {
        let store = Store::in_memory().unwrap();
        assert_eq!(store.schema_version(), crate::schema::SCHEMA_VERSION);
    }

    #[test]
    fn opens_on_disk_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("qc.db");
        let store = Store::open(&path).unwrap();
        assert!(path.exists());
        assert_eq!(store.schema_version(), crate::schema::SCHEMA_VERSION);
        drop(store);
    }
}
