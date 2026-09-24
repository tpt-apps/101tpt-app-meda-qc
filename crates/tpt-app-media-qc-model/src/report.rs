//! Immutable QC report produced from a result set ([spec § 14], § 14.1).
//!
//! Reports must be generated from an immutable QC result set. This structure
//! carries every audit field required by the spec so reports are reproducible
//! from asset fingerprint + profile + application/ruleset version.

use crate::finding::QcFinding;
use crate::severity::VerdictDecision;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Unique analysis identifier (report integrity field, [spec § 14.1]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AnalysisId(pub uuid::Uuid);

impl AnalysisId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

impl Default for AnalysisId {
    fn default() -> Self {
        Self::new()
    }
}

/// Application identity embedded in every report ([spec § 14.1]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppInfo {
    pub name: String,
    pub version: String,
    pub ruleset_version: String,
}

/// Host information embedded in reports.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostInfo {
    pub os: String,
    pub arch: String,
    pub cpu_count: u64,
}

/// Profile reference embedded in reports ([spec § 14.1] profile SHA-256).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileRef {
    pub name: String,
    pub version: u32,
    /// SHA-256 of the canonical profile representation.
    pub sha256: String,
}

/// The five report-integrity fields required by [spec § 14.1].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportIntegrity {
    /// SHA-256 of the asset content.
    pub asset_sha256: String,
    /// SHA-256 of the profile.
    pub profile_sha256: String,
    pub application_version: String,
    pub ruleset_version: String,
    pub analysis_id: String,
}

/// Capture state of the verdict: wins highest-status finding.
pub use crate::severity::VerdictDecision as Verdict;

/// The immutable report.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Report {
    pub analysis_id: AnalysisId,
    pub created_at: DateTime<Utc>,
    pub app: AppInfo,
    pub integrity: ReportIntegrity,
    pub profile: ProfileRef,
    pub host: HostInfo,
    pub asset_path: String,
    pub asset_size_bytes: u64,
    pub asset_duration_ms: Option<u64>,
    /// Width x height of the primary video stream, when present.
    pub asset_resolution: Option<String>,
    pub findings: Vec<QcFinding>,
    pub verdict: Verdict,
    pub operator_notes: String,
}

impl Report {
    /// Number of findings per status.
    pub fn status_counts(&self) -> StatusCounts {
        let mut counts = StatusCounts::default();
        for f in &self.findings {
            match f.status {
                VerdictDecision::Pass => counts.pass += 1,
                VerdictDecision::Warn => counts.warn += 1,
                VerdictDecision::Fail => counts.fail += 1,
                VerdictDecision::Inconclusive => counts.inconclusive += 1,
            }
        }
        counts
    }
}

/// Per-status finding counts, used by all report renderers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusCounts {
    pub pass: usize,
    pub warn: usize,
    pub fail: usize,
    pub inconclusive: usize,
}

impl StatusCounts {
    pub fn total(&self) -> usize {
        self.pass + self.warn + self.fail + self.inconclusive
    }
}
