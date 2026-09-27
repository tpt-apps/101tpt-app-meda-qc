//! Shared staging for report exporters.
//!
//! Every exporter writes into a randomly named file beside its destination and
//! only then replaces the requested path. This prevents a failed render from
//! leaving a partially written audit report and avoids predictable temporary
//! filenames in shared directories.

use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use tempfile::NamedTempFile;
use tpt_app_media_qc_core::error::{Error, Result};

pub(crate) fn destination(path: &Path) -> Result<PathBuf> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent = std::fs::canonicalize(parent)?;
    if !parent.is_dir() {
        return Err(Error::PathNotAllowed(parent));
    }
    let file_name = path
        .file_name()
        .ok_or_else(|| Error::PathNotAllowed(path.to_path_buf()))?;
    let destination = parent.join(file_name);
    if let Ok(metadata) = std::fs::symlink_metadata(&destination) {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(Error::PathNotAllowed(destination));
        }
    }
    Ok(destination)
}

pub(crate) fn staged_file(path: &Path) -> Result<NamedTempFile> {
    let parent = destination(path)?
        .parent()
        .ok_or_else(|| Error::PathNotAllowed(path.to_path_buf()))?
        .to_path_buf();
    NamedTempFile::new_in(parent).map_err(|_| Error::TempFile(path.to_path_buf()))
}

pub(crate) fn staged_writer(path: &Path) -> Result<BufWriter<NamedTempFile>> {
    Ok(BufWriter::new(staged_file(path)?))
}

pub(crate) fn commit_file(file: NamedTempFile, path: &Path) -> Result<()> {
    file.as_file().sync_all()?;
    file.persist(destination(path)?)
        .map_err(|error| Error::TempFile(error.file.path().to_path_buf()))?;
    Ok(())
}

pub(crate) fn commit_writer(mut writer: BufWriter<NamedTempFile>, path: &Path) -> Result<()> {
    writer.flush()?;
    let file = writer
        .into_inner()
        .map_err(|_| Error::TempFile(path.to_path_buf()))?;
    commit_file(file, path)
}
