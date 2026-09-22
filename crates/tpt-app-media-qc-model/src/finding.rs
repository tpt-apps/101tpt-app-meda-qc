//! QC finding, evidence and ranges ([spec § 6.3, § 6.4]).

use crate::asset::StreamId;
use crate::evidence::Evidence;
use crate::severity::{Severity, VerdictDecision};
use crate::time::DurationSeconds;
use crate::value::Value;
use serde::{Deserialize, Serialize};

/// Stable identifier for a QC rule, e.g. `container.readable`.
///
/// Rule IDs are part of the stable public contract: reports reference rules by
/// these strings and cache invalidation keys on them ([spec § 19]).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RuleId(pub String);

impl RuleId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for RuleId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for RuleId {
    fn from(v: &str) -> Self {
        Self::new(v)
    }
}

impl From<String> for RuleId {
    fn from(v: String) -> Self {
        Self::new(v)
    }
}

/// A time range, from `start` to `end` (both inclusive milliseconds > µs).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeRange {
    pub start_ms: u64,
    pub end_ms: u64,
}

impl TimeRange {
    pub fn new(start_ms: u64, end_ms: u64) -> Self {
        Self { start_ms, end_ms }
    }

    pub fn duration_ms(&self) -> u64 {
        self.end_ms.saturating_sub(self.start_ms)
    }

    pub fn from_durations(start: DurationSeconds, end: DurationSeconds) -> Self {
        Self::new(start.as_secs() * 1000, end.as_secs() * 1000)
    }

    pub fn to_finding_time(&self) -> (DurationSeconds, DurationSeconds) {
        (DurationSeconds::from_millis(self.start_ms), DurationSeconds::from_millis(self.end_ms))
    }
}

/// A frame range within a single stream's frame numbering.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameRange {
    pub start: u64,
    pub end: u64,
}

impl FrameRange {
    pub fn new(start: u64, end: u64) -> Self {
        Self { start, end }
    }

    pub fn length(&self) -> u64 {
        self.end.saturating_sub(self.start)
    }
}

/// One QC finding — an individual rule outcome with full provenance
/// ([spec § 6.3], [spec § 3.3] "explain every failure").
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QcFinding {
    pub rule_id: RuleId,
    pub status: VerdictDecision,
    pub severity: Severity,
    pub message: String,
    pub measured: Option<Value>,
    pub expected: Option<Value>,
    pub stream_idx: Option<StreamId>,
    pub time_range: Option<TimeRange>,
    pub frame_range: Option<FrameRange>,
    pub evidence: Vec<Evidence>,
    /// Optional confidence for heuristic rules (0.0..=1.0). Deterministic
    /// technical rules leave this unset.
    pub confidence: Option<f32>,
}

impl QcFinding {
    pub fn new(rule_id: impl Into<RuleId>) -> Self {
        Self {
            rule_id: rule_id.into(),
            status: VerdictDecision::Pass,
            severity: Severity::Info,
            message: String::new(),
            measured: None,
            expected: None,
            stream_idx: None,
            time_range: None,
            frame_range: None,
            evidence: Vec::new(),
            confidence: None,
        }
    }

    pub fn pass(mut self) -> Self {
        self.status = VerdictDecision::Pass;
        self
    }

    pub fn fail(mut self) -> Self {
        self.status = VerdictDecision::Fail;
        self
    }

    /// Set an arbitrary verdict (e.g. `Inconclusive`).
    pub fn status(mut self, status: VerdictDecision) -> Self {
        self.status = status;
        self
    }

    pub fn on(mut self, rule_id: impl Into<RuleId>) -> Self {
        self.rule_id = rule_id.into();
        self
    }

    pub fn some(self, message: impl Into<String>) -> Self {
        self.set_message(message)
    }

    pub fn set_message(mut self, message: impl Into<String>) -> Self {
        self.message = message.into();
        self
    }

    pub fn severity(mut self, severity: Severity) -> Self {
        self.severity = severity;
        self
    }

    pub fn measured(mut self, value: impl Into<Value>) -> Self {
        self.measured = Some(value.into());
        self
    }

    pub fn expected(mut self, value: impl Into<Value>) -> Self {
        self.expected = Some(value.into());
        self
    }

    pub fn stream(mut self, stream: StreamId) -> Self {
        self.stream_idx = Some(stream);
        self
    }

    pub fn time(mut self, range: TimeRange) -> Self {
        self.time_range = Some(range);
        self
    }

    pub fn frames(mut self, range: FrameRange) -> Self {
        self.frame_range = Some(range);
        self
    }

    pub fn evidence(mut self, evidence: Evidence) -> Self {
        self.evidence.push(evidence);
        self
    }
}