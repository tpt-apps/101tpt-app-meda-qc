//! QC pipeline: inspection boundary, rule execution, aggregation and verdict.
//!
//! The pipeline owns the seam between probe front-ends ([`Inspector`]) and the
//! QC engine. It runs a profile's rules over an asset's inspection, aggregates
//! findings into a per-asset [`QcRun`] and resolves a verdict from the profile
//! policy.

mod engine;
mod inspector;
mod scheduler;

pub mod job;
pub mod verdict;

pub use engine::QcEngine;
pub use inspector::{arc, InspectionLevel, Inspector, NoopInspector};
pub use job::Job;
pub use scheduler::{run_jobs, Scheduler, SchedulingResult};
pub use verdict::{VerdictPolicy, VerdictResolution};

use tpt_app_media_qc_model::asset::Asset;
use tpt_app_media_qc_model::finding::QcFinding;
use tpt_app_media_qc_model::inspection::Inspection;
use tpt_app_media_qc_model::report::StatusCounts;
use tpt_app_media_qc_model::severity::VerdictDecision;
use tpt_app_media_qc_rules::QcRule;

/// Outcome of QC-ing a single asset: inspection, findings and verdict.
#[derive(Clone, Debug)]
pub struct QcRun {
    pub asset: Asset,
    pub profile_name: String,
    pub profile_version: u32,
    pub inspection: Inspection,
    pub findings: Vec<QcFinding>,
    /// Earliest (worst) status per rule id; rules that produced nothing are
    /// recorded as `Pass`.
    pub per_rule: Vec<RuleStatus>,
    pub counts: StatusCounts,
    pub verdict: VerdictDecision,
    pub policy: VerdictPolicy,
}

/// Per-rule aggregation entry ([spec § 6.5] "status per rule").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleStatus {
    pub rule_id: String,
    pub best: VerdictDecision,
    pub findings: usize,
}

impl RuleStatus {
    pub fn new(rule_id: impl Into<String>, best: VerdictDecision, findings: usize) -> Self {
        Self {
            rule_id: rule_id.into(),
            best,
            findings,
        }
    }
}

/// Run a pre-built set of rules over an inspection (shared by the engine and
/// by tests).
pub fn run_rules(
    rules: &[Box<dyn QcRule>],
    asset: &Asset,
    inspection: &Inspection,
) -> Vec<QcFinding> {
    let ctx = tpt_app_media_qc_rules::RuleContext { asset, inspection };
    let mut findings = Vec::new();
    for rule in rules {
        let outcome = rule.execute(&ctx);
        findings.extend(outcome.findings);
    }
    findings
}

/// Rank statuses from best (pass) to worst (fail) for per-rule aggregation.
fn status_rank(s: VerdictDecision) -> u8 {
    match s {
        VerdictDecision::Pass => 0,
        VerdictDecision::Warn => 1,
        VerdictDecision::Inconclusive => 2,
        VerdictDecision::Fail => 3,
    }
}

/// Aggregate findings into per-rule statuses and counts.
pub fn aggregate(
    rules: &[Box<dyn QcRule>],
    findings: &[QcFinding],
) -> (Vec<RuleStatus>, StatusCounts) {
    let mut per_rule: Vec<RuleStatus> = Vec::new();
    let mut counts = StatusCounts::default();

    let mut by_rule: std::collections::BTreeMap<&str, (VerdictDecision, usize)> =
        std::collections::BTreeMap::new();
    for f in findings {
        let entry = by_rule
            .entry(f.rule_id.as_str())
            .or_insert((VerdictDecision::Pass, 0));
        if status_rank(f.status) > status_rank(entry.0) {
            entry.0 = f.status;
        }
        entry.1 += 1;
        match f.status {
            VerdictDecision::Fail => counts.fail += 1,
            VerdictDecision::Warn => counts.warn += 1,
            VerdictDecision::Inconclusive => counts.inconclusive += 1,
            VerdictDecision::Pass => counts.pass += 1,
        }
    }

    for rule in rules {
        let (best, n) = by_rule
            .get(rule.id().as_str())
            .copied()
            .unwrap_or((VerdictDecision::Pass, 0));
        if n == 0 {
            counts.pass += 1;
        }
        per_rule.push(RuleStatus::new(rule.id().as_str(), best, n));
    }

    (per_rule, counts)
}

/// Small helper so callers can construct an inspection quickly (used by
/// higher-level builders and the CLI for offline re-runs).
pub fn empty_inspection() -> Inspection {
    Inspection::default()
}

/// Convenience alias for the pipeline's result type.
pub type PipelineResult<T> = tpt_app_media_qc_core::error::Result<T>;
