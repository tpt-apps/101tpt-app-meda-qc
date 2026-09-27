//! Validation for filesystem paths accepted at the application boundary.
//!
//! This module deliberately validates the paths that cross a user-controlled
//! boundary; it does not claim to be an operating-system sandbox. Callers must
//! use the returned canonical path when opening media or writing reports.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Extensions recognised by the batch/import workflows. An explicitly selected
/// file is still probed by callers; this list is used only for directory scans
/// and desktop import filtering.
const MEDIA_EXTENSIONS: &[&str] = &[
    "aac", "ac3", "avi", "eac3", "flac", "m2ts", "m4v", "mkv", "mov", "mp3", "mp4", "mxf", "mts",
    "opus", "ts", "w64", "wav", "webm",
];

/// Validate an existing regular file and return its canonical path.
///
/// This rejects empty/traversal-only names and directories. A symlink is
/// resolved before the path is returned, so downstream file operations do not
/// repeatedly follow a user-controlled link.
pub fn validate_existing_file(path: &Path) -> Result<PathBuf> {
    if path.as_os_str().is_empty()
        || path.to_string_lossy().contains('\0')
        || matches!(path.file_name(), Some(name) if name == OsStr::new(".") || name == OsStr::new(".."))
    {
        return Err(Error::PathNotAllowed(path.to_path_buf()));
    }

    let canonical = std::fs::canonicalize(path)?;
    if !canonical.is_file() {
        return Err(Error::PathNotAllowed(canonical));
    }
    Ok(canonical)
}

/// Validate an existing directory and return its canonical path.
pub fn validate_existing_directory(path: &Path) -> Result<PathBuf> {
    if path.as_os_str().is_empty() || path.to_string_lossy().contains('\0') {
        return Err(Error::PathNotAllowed(path.to_path_buf()));
    }
    let canonical = std::fs::canonicalize(path)?;
    if !canonical.is_dir() {
        return Err(Error::PathNotAllowed(canonical));
    }
    Ok(canonical)
}

/// Validate an existing file with a recognised media extension.
///
/// This is intended for import/folder traversal. A caller that deliberately
/// accepts an extensionless media file should use [`validate_existing_file`].
pub fn validate_media_file(path: &Path) -> Result<PathBuf> {
    let canonical = validate_existing_file(path)?;
    let extension = canonical
        .extension()
        .and_then(OsStr::to_str)
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| Error::PathNotAllowed(canonical.clone()))?;
    if MEDIA_EXTENSIONS.iter().any(|allowed| *allowed == extension) {
        Ok(canonical)
    } else {
        Err(Error::PathNotAllowed(canonical))
    }
}

/// Validate a new report path without creating its parent directory.
///
/// The parent must already exist and is canonicalised. Requiring the expected
/// extension keeps the desktop export command from turning an arbitrary path
/// into a report overwrite.
pub fn validate_output_path(path: &Path, extension: &str) -> Result<PathBuf> {
    if path.as_os_str().is_empty() || path.to_string_lossy().contains('\0') {
        return Err(Error::PathNotAllowed(path.to_path_buf()));
    }
    let expected = extension.trim_start_matches('.').to_ascii_lowercase();
    let actual = path
        .extension()
        .and_then(OsStr::to_str)
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| Error::PathNotAllowed(path.to_path_buf()))?;
    if actual != expected {
        return Err(Error::PathNotAllowed(path.to_path_buf()));
    }
    let file_name = path
        .file_name()
        .filter(|name| *name != OsStr::new(".") && *name != OsStr::new(".."))
        .ok_or_else(|| Error::PathNotAllowed(path.to_path_buf()))?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent = std::fs::canonicalize(parent)?;
    if !parent.is_dir() {
        return Err(Error::PathNotAllowed(parent));
    }
    let destination = parent.join(file_name);
    if let Ok(metadata) = std::fs::symlink_metadata(&destination) {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(Error::PathNotAllowed(destination));
        }
    }
    Ok(destination)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_validation_canonicalises_supported_files() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("clip.WAV");
        std::fs::write(&path, b"RIFF").unwrap();
        assert_eq!(
            validate_media_file(&path).unwrap(),
            path.canonicalize().unwrap()
        );
    }

    #[test]
    fn media_validation_rejects_directories_and_unknown_extensions() {
        let directory = tempfile::tempdir().unwrap();
        assert!(validate_media_file(directory.path()).is_err());
        assert_eq!(
            validate_existing_directory(directory.path()).unwrap(),
            directory.path().canonicalize().unwrap()
        );
        let path = directory.path().join("notes.txt");
        std::fs::write(&path, b"text").unwrap();
        assert!(validate_media_file(&path).is_err());
    }

    #[test]
    fn output_validation_requires_existing_parent_and_extension() {
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(
            validate_output_path(&directory.path().join("report.json"), "json").unwrap(),
            directory.path().canonicalize().unwrap().join("report.json")
        );
        assert!(validate_output_path(&directory.path().join("report.txt"), "json").is_err());
        assert!(
            validate_output_path(&directory.path().join("missing/report.json"), "json").is_err()
        );
    }
}
