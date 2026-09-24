//! QC domain model ([spec § 6]).
//!
//! Pure data types shared by every layer: assets, streams, findings,
//! evidence, timecode, severity and the immutable report structure.

pub mod asset;
pub mod evidence;
pub mod finding;
pub mod inspection;
pub mod report;
pub mod severity;
pub mod time;
pub mod value;

pub use asset::{Asset, AssetFingerprint, AssetId, Stream, StreamId, StreamKind};
pub use evidence::{Evidence, EvidenceKind, EvidencePayload};
pub use finding::{FrameRange, QcFinding, RuleId, TimeRange};
pub use inspection::{
    AudioMeasurements, ContainerInspection, ContainerValidity, Inspection, LumaStats,
    VideoMeasurements,
};
pub use report::{AnalysisId, AppInfo, HostInfo, ProfileRef, Report, ReportIntegrity, Verdict};
pub use severity::{Severity, VerdictDecision};
pub use time::{DurationSeconds, FrameRate, Rational, TimeBase, Timecode};
pub use value::Value;
