//! Unified error type for TPT Media QC.

use std::path::PathBuf;

/// The unified application error type.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("YAML error: {0}")]
    Yaml(#[from] serde_yaml::Error),

    #[error("profile error: {0}")]
    Profile(String),

    #[error("rule error: {0}")]
    Rule(String),

    #[error("media probe error: {0}")]
    Probe(String),

    #[error("invalid timecode: {0}")]
    Timecode(String),

    #[error("invalid value: {0}")]
    Value(String),

    #[error("path not allowed: {0}")]
    PathNotAllowed(PathBuf),

    #[error("temporary file error: {0}")]
    TempFile(PathBuf),

    #[error("job error: {0}")]
    Job(String),

    #[error("sqlite error: {0}")]
    Sqlite(String),

    #[error("unsupported: {0}")]
    Unsupported(String),

    #[error("internal error: {0}")]
    Internal(String),
}

#[cfg(feature = "sqlite")]
impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Error::Sqlite(e.to_string())
    }
}

/// Convenience alias used across the workspace.
pub type Result<T> = std::result::Result<T, Error>;

/// Internal error helper used by crates that must not leak dependencies.
pub fn internal(message: impl Into<String>) -> Error {
    Error::Internal(message.into())
}