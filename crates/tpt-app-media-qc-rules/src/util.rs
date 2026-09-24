//! Helpers shared by rule implementations.

use tpt_app_media_qc_model::finding::{QcFinding, RuleId, TimeRange};
use tpt_app_media_qc_model::inspection::Inspection;
use tpt_app_media_qc_model::severity::{Severity, VerdictDecision as Status};
use tpt_app_media_qc_model::value::Value;

/// Build a single FAIL finding with measured/expected values.
pub fn fail(
    rule: &RuleId,
    severity: Severity,
    message: impl Into<String>,
    measured: impl Into<Option<Value>>,
    expected: impl Into<Option<Value>>,
) -> QcFinding {
    QcFinding::new(rule.clone())
        .status(Status::Fail)
        .severity(severity)
        .set_message(message)
        .measured_or(measured)
        .expected_or(expected)
}

/// Build a single INCONCLUSIVE finding (measurement missing).
pub fn inconclusive(rule: &RuleId, severity: Severity, message: impl Into<String>) -> QcFinding {
    QcFinding::new(rule.clone())
        .status(Status::Inconclusive)
        .severity(severity)
        .set_message(message)
}

/// Build a single PASS-with-info finding.
pub fn info(
    rule: &RuleId,
    severity: Severity,
    message: impl Into<String>,
    measured: impl Into<Option<Value>>,
) -> QcFinding {
    QcFinding::new(rule.clone())
        .severity(severity)
        .set_message(message)
        .measured_or(measured)
}

/// One finding per offending segment, attaching its `TimeRange`.
pub fn segment_failures(
    rule: &RuleId,
    severity: Severity,
    segments: impl Iterator<Item = (TimeRange, String)>,
) -> Vec<QcFinding> {
    segments
        .map(|(range, message)| {
            QcFinding::new(rule.clone())
                .status(Status::Fail)
                .severity(severity)
                .set_message(message)
                .time(range)
        })
        .collect()
}

/// Convenient setter that only sets when `Some`.
trait MeasuredSetter {
    fn measured_or(self, value: impl Into<Option<Value>>) -> Self;
    fn expected_or(self, value: impl Into<Option<Value>>) -> Self;
}

impl MeasuredSetter for QcFinding {
    fn measured_or(self, value: impl Into<Option<Value>>) -> Self {
        match value.into() {
            Some(v) => self.measured(v),
            None => self,
        }
    }

    fn expected_or(self, value: impl Into<Option<Value>>) -> Self {
        match value.into() {
            Some(v) => self.expected(v),
            None => self,
        }
    }
}

/// Check that the container was actually scanned; if not, rule outcomes are
/// inconclusive rather than guessed.
pub fn container_scanned(inspection: &Inspection) -> bool {
    !matches!(
        inspection.container.validity,
        tpt_app_media_qc_model::inspection::ContainerValidity::NotScanned
    )
}
