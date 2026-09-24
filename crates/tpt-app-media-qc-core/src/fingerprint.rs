//! Content-based asset fingerprinting ([spec § 6.1]).
//!
//! A file path alone must never identify an asset because files can be moved
//! or replaced. Fingerprints hash file *content* so identical media shares a
//! fingerprint regardless of location.

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::Path;

/// Size of the streaming hash buffer. Bounded memory regardless of file size.
const READ_BUFFER_SIZE: usize = 64 * 1024;

/// Where to hash from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum FingerprintStrategy {
    /// Hash the entire file.
    ///
    /// Most correct for identity; costs full read time on import.
    #[default]
    FullScan,
    /// Hash a bounded prefix plus the file size.
    ///
    /// Fast enough for interactive import. Identical-prefix collisions are a
    /// theoretical risk and are acceptable for the quick path only.
    BoundedPrefix,
}

/// Configuration for fingerprint computation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FingerprintConfig {
    pub strategy: FingerprintStrategy,
    /// Number of bytes hashed when `strategy == BoundedPrefix`.
    pub max_bytes: u64,
}

impl Default for FingerprintConfig {
    fn default() -> Self {
        Self {
            strategy: FingerprintStrategy::default(),
            max_bytes: 16 * 1024 * 1024,
        }
    }
}

/// A content-based SHA-256 fingerprint of an asset.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Fingerprint {
    sha256: [u8; 32],
}

impl Fingerprint {
    pub fn new(sha256: [u8; 32]) -> Self {
        Self { sha256 }
    }

    /// Hash any byte source.
    pub fn from_reader(reader: impl Read) -> io::Result<Self> {
        let mut hasher = Sha256::new();
        let mut buf = [0u8; READ_BUFFER_SIZE];
        let mut reader = reader;
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        Ok(Self::new(hasher.finalize().into()))
    }

    /// Compute the fingerprint of a file using the given strategy.
    pub fn of_file(path: &Path, config: FingerprintConfig) -> Result<Self> {
        let file = File::open(path).map_err(Error::from)?;
        let meta = file.metadata().map_err(Error::from)?;
        let size = meta.len();

        match config.strategy {
            FingerprintStrategy::FullScan => Ok(Self::from_reader(BufReader::new(file))?),
            FingerprintStrategy::BoundedPrefix => {
                let n = size.min(config.max_bytes);
                let mut hasher = Sha256::new();
                let mut reader = BufReader::new(file).take(n);
                let mut buf = [0u8; READ_BUFFER_SIZE];
                loop {
                    let read = reader.read(&mut buf)?;
                    if read == 0 {
                        break;
                    }
                    hasher.update(&buf[..read]);
                }
                // Bind the fingerprint to size so truncated-vs-complete files
                // of identical prefixes still differ.
                hasher.update(size.to_le_bytes());
                Ok(Self::new(hasher.finalize().into()))
            }
        }
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.sha256
    }

    /// Lowercase hex string, used in reports and cache keys.
    pub fn to_hex(&self) -> String {
        hex::encode(self.sha256)
    }
}

impl From<[u8; 32]> for Fingerprint {
    fn from(value: [u8; 32]) -> Self {
        Self::new(value)
    }
}

impl From<Fingerprint> for [u8; 32] {
    fn from(value: Fingerprint) -> Self {
        value.sha256
    }
}

impl std::fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_hex())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_file(bytes: &[u8]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("tpt-qc-fp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("asset.bin");
        let mut f = File::create(&path).unwrap();
        f.write_all(bytes).unwrap();
        path
    }

    #[test]
    fn same_content_same_fingerprint() {
        let a = temp_file(b"hello world media asset");
        let b = temp_file(b"hello world media asset");
        let fa = Fingerprint::of_file(&a, FingerprintConfig::default()).unwrap();
        let fb = Fingerprint::of_file(&b, FingerprintConfig::default()).unwrap();
        assert_eq!(fa, fb);
        let _ = std::fs::remove_dir_all(a.parent().unwrap());
        let _ = std::fs::remove_dir_all(b.parent().unwrap());
    }

    #[test]
    fn different_content_different_fingerprint() {
        let a = temp_file(b"abc");
        let b = temp_file(b"abd");
        let fa = Fingerprint::of_file(&a, FingerprintConfig::default()).unwrap();
        let fb = Fingerprint::of_file(&b, FingerprintConfig::default()).unwrap();
        assert_ne!(fa, fb);
        assert_eq!(fa.to_hex().len(), 64);
        let _ = std::fs::remove_dir_all(a.parent().unwrap());
        let _ = std::fs::remove_dir_all(b.parent().unwrap());
    }

    #[test]
    fn replacement_detected_despite_same_prefix() {
        let a = temp_file(b"00001111");
        let b = temp_file(b"000011112222");
        let cfg = FingerprintConfig {
            strategy: FingerprintStrategy::BoundedPrefix,
            max_bytes: 4,
        };
        let fa = Fingerprint::of_file(&a, cfg).unwrap();
        let fb = Fingerprint::of_file(&b, cfg).unwrap();
        assert_ne!(fa, fb);
        let _ = std::fs::remove_dir_all(a.parent().unwrap());
        let _ = std::fs::remove_dir_all(b.parent().unwrap());
    }

    #[test]
    fn missing_file_errors() {
        let r = Fingerprint::of_file(
            Path::new("definitely-not-here.bin"),
            FingerprintConfig::default(),
        );
        assert!(r.is_err());
    }
}
