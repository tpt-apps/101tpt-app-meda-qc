//! Database schema and migrations.
//!
//! Versioning uses SQLite's `PRAGMA user_version`. Migration 1 creates the
//! full initial schema; later versions add columns/tables incrementally.

use crate::error::Result;

pub const SCHEMA_VERSION: i64 = 1;

/// Run the migration for the current schema version if needed.
pub fn migrate(conn: &rusqlite::Connection) -> Result<()> {
    let current: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if current < 1 {
        create_v1(conn)?;
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    }
    // Future migrations: `if current < 2 { ... }` etc.
    Ok(())
}

fn create_v1(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS meta (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        -- Projects group assets and jobs (spec §18).
        CREATE TABLE IF NOT EXISTS projects (
            id          TEXT PRIMARY KEY,
            name        TEXT NOT NULL,
            description TEXT,
            created_at  INTEGER NOT NULL
        );

        -- Assets: identity is the content fingerprint, never the path.
        CREATE TABLE IF NOT EXISTS assets (
            sha256        TEXT PRIMARY KEY,
            path          TEXT NOT NULL,
            size_bytes    INTEGER NOT NULL,
            modified_time INTEGER,
            duration_ms   INTEGER,
            first_seen_at INTEGER NOT NULL,
            last_seen_at  INTEGER NOT NULL
        );

        -- Fingerprint bookkeeping (strategy + size bound the identity).
        CREATE TABLE IF NOT EXISTS fingerprints (
            sha256      TEXT PRIMARY KEY,
            strategy    TEXT NOT NULL,
            size_bytes  INTEGER NOT NULL,
            computed_at INTEGER NOT NULL
        );

        -- Profiles as stored canonical documents.
        CREATE TABLE IF NOT EXISTS profiles (
            sha256     TEXT PRIMARY KEY,
            name       TEXT NOT NULL,
            version    INTEGER NOT NULL,
            document   TEXT NOT NULL,
            created_at INTEGER NOT NULL
        );

        -- QC jobs: status lifecycle queued -> running -> finished kinds.
        CREATE TABLE IF NOT EXISTS jobs (
            id            TEXT PRIMARY KEY,
            project_id    TEXT,
            asset_sha256  TEXT NOT NULL,
            profile_sha256 TEXT NOT NULL,
            level         TEXT NOT NULL,
            status        TEXT NOT NULL DEFAULT 'queued',
            submitted_at  INTEGER NOT NULL,
            started_at    INTEGER,
            finished_at   INTEGER,
            verdict       TEXT,
            error         TEXT,
            report_json   TEXT
        );

        -- Findings flattened into queryable rows.
        CREATE TABLE IF NOT EXISTS findings (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            job_id        TEXT NOT NULL,
            rule_id       TEXT NOT NULL,
            status        TEXT NOT NULL,
            severity      TEXT NOT NULL,
            message       TEXT,
            measured_json TEXT,
            expected_json TEXT,
            stream_index  INTEGER,
            time_start_ms INTEGER,
            time_end_ms   INTEGER,
            frame_start   INTEGER,
            frame_end     INTEGER,
            evidence_json TEXT,
            confidence    REAL
        );
        CREATE INDEX IF NOT EXISTS idx_findings_job ON findings(job_id);

        -- Report integrity metadata (spec §14.1), mirrored from the report.
        CREATE TABLE IF NOT EXISTS report_metadata (
            job_id             TEXT PRIMARY KEY,
            analysis_id        TEXT NOT NULL,
            created_at         INTEGER NOT NULL,
            asset_sha256       TEXT NOT NULL,
            profile_sha256     TEXT NOT NULL,
            application_version TEXT NOT NULL,
            ruleset_version    TEXT NOT NULL
        );

        -- Per-rule analysis cache (spec §19).
        -- Key excludes the whole-profile hash: the per-rule config hash is the
        -- fine-grained "profile hash + analysis configuration hash" component,
        -- so changing one rule's threshold invalidates only that rule's rows.
        CREATE TABLE IF NOT EXISTS rule_cache (
            asset_sha256    TEXT NOT NULL,
            app_version     TEXT NOT NULL,
            ruleset_version TEXT NOT NULL,
            rule_id         TEXT NOT NULL,
            config_hash     TEXT NOT NULL,
            findings_json   TEXT NOT NULL,
            cached_at       INTEGER NOT NULL,
            PRIMARY KEY (asset_sha256, app_version, ruleset_version, rule_id, config_hash)
        );
        CREATE INDEX IF NOT EXISTS idx_rule_cache_asset ON rule_cache(asset_sha256);

        -- User preferences: JSON values.
        CREATE TABLE IF NOT EXISTS preferences (
            key        TEXT PRIMARY KEY,
            value_json TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS watch_folders (
            id          TEXT PRIMARY KEY,
            name        TEXT NOT NULL,
            input_path  TEXT NOT NULL,
            profile_sha TEXT,
            pass_path   TEXT,
            warn_path   TEXT,
            fail_path   TEXT,
            report_path TEXT
        );
        "#,
    )?;
    Ok(())
}
