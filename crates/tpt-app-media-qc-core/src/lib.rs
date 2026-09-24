//! TPT Media QC core primitives.
//!
//! This crate holds shared infrastructure that every other crate (and
//! eventually the TPT foundation crates) depends on:
//!
//! * [`config`]   — application identity and versioning ([spec § 14.1])
//! * [`error`]    — the unified error type
//! * [`cost`]     — scheduler cost classification ([spec § 11])
//! * [`fingerprint`] — content-based asset fingerprinting ([spec § 6.1])

pub mod cost;
pub mod error;
pub mod fingerprint;

/// Application identity and versioning.
pub mod config {
    /// Public product name.
    pub const APP_NAME: &str = "TPT Media QC";

    /// Short invocation name used by CLI and installers.
    pub const APP_BIN: &str = "tpt-media-qc";

    /// Repository / product family.
    pub const APP_FAMILY: &str = "TPT Apps";

    /// Semantic version of the application, taken from `Cargo.toml`.
    pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

    /// Human-readable build label for diagnostics and reports.
    pub fn app_display_version() -> String {
        format!("{APP_NAME} {APP_VERSION}")
    }

    /// Version of the ruleset shipped with this build.
    ///
    /// The ruleset version participates in cache keys and report integrity
    /// fields ([spec § 14.1], [spec § 19]). It must be bumped whenever the
    /// built-in rule catalogue or its measurement semantics change.
    pub const RULESET_VERSION: &str = "0.1.0";
}

/// Lightweight semver-style triple used by tooling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Version {
    pub fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// Parse a `MAJOR.MINOR.PATCH` string, ignoring pre-release/build metadata.
    pub fn parse(s: &str) -> Option<Self> {
        let mut it = s.split('.');
        let major = it.next()?.parse().ok()?;
        let minor = it.next().unwrap_or("0").parse().ok()?;
        let patch = it
            .next()
            .unwrap_or("0")
            .split(['-', '+'])
            .next()?
            .parse()
            .ok()?;
        Some(Self::new(major, minor, patch))
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_parse_roundtrip() {
        assert_eq!(Version::parse("1.2.3"), Some(Version::new(1, 2, 3)));
        assert_eq!(Version::parse("0.1.0"), Some(Version::new(0, 1, 0)));
        assert_eq!(Version::parse("1.2.3-alpha.1"), Some(Version::new(1, 2, 3)));
        assert_eq!(Version::parse("garbage"), None);
    }
}
