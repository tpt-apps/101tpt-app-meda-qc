//! SQLite persistence for local application state (spec § 18).
//!
//! The store persists: projects, assets, fingerprints, QC jobs, profiles,
//! results, findings, report metadata and user preferences. Original media is
//! **never** stored in the database — only paths and content fingerprints.
//!
//! The store also implements the per-rule analysis cache (spec § 19): cached
//! results are keyed on asset fingerprint + application version + ruleset
//! version + profile hash + the hash of the individual rule's configuration,
//! so changing one rule's threshold invalidates only that rule's cached
//! results.

pub mod assets;
pub mod cache;
pub mod jobs;
pub mod prefs;
pub mod profiles;
pub mod projects;
pub mod schema;
pub mod store;

/// The store's error/material types follow the workspace core error.
pub mod error {
    pub use tpt_app_media_qc_core::error::{Error, Result};
}

pub use store::Store;
