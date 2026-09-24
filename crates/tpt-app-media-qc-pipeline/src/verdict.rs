//! Verdict resolution from findings and the profile policy ([spec § 16]).

use tpt_app_media_qc_model::finding::QcFinding;
use tpt_app_media_qc_model::severity::{Severity, VerdictDecision};
use tpt_app_media_qc_profile::model::Policy;

/// The policy thresholds derived from a profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VerdictPolicy {
    /// Findings with this `Severity` or higher that report `Fail` force a FAIL
    /// verdict.
    pub fail_on: Severity,
}

impl VerdictPolicy {
    pub fn from_profile(policy: &Policy) -> Self {
        Self {
            fail_on: policy.fail_on,
        }
    }
}

impl Default for VerdictPolicy {
    fn default() -> Self {
        Self {
            fail_on: Severity::Error,
        }
    }
}

/// Detailed verdict resolution, kept for reporting and CLI exit codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VerdictResolution {
    pub verdict: VerdictDecision,
    pub fail_on: Severity,
    /// Findings that forced a FAIL verdict (status Fail, severity ≥ fail_on).
    pub failed: usize,
    /// Fail findings below the fail_on threshold (downgraded to warnings).
    pub below_threshold: usize,
    /// Inconclusive findings at or above the fail_on severity.
    pub inconclusive: usize,
}

impl VerdictResolution {
    /// Map to a process exit code section per spec §16 (0 ok, 1 warn, 2 fail).
    pub fn exit_code(&self) -> u8 {
        match self.verdict {
            VerdictDecision::Fail => 2,
            VerdictDecision::Warn => 1,
            _ => 0,
        }
    }
}

/// Resolve the verdict for an asset.
pub fn resolve(policy: VerdictPolicy, findings: &[QcFinding]) -> VerdictResolution {
    let mut failed = 0usize;
    let mut below_threshold = 0usize;
    let mut inconclusive = 0usize;

    for f in findings {
        match f.status {
            VerdictDecision::Fail if f.severity >= policy.fail_on => failed += 1,
            VerdictDecision::Fail => below_threshold += 1,
            VerdictDecision::Inconclusive if f.severity >= policy.fail_on => inconclusive += 1,
            _ => {}
        }
    }

    let verdict = if failed > 0 {
        VerdictDecision::Fail
    } else if below_threshold > 0 || inconclusive > 0 {
        VerdictDecision::Warn
    } else {
        VerdictDecision::Pass
    };

    VerdictResolution {
        verdict,
        fail_on: policy.fail_on,
        failed,
        below_threshold,
        inconclusive,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpt_app_media_qc_model::finding::RuleId;

    fn failing(sev: Severity) -> QcFinding {
        QcFinding::new(RuleId::new("t"))
            .status(VerdictDecision::Fail)
            .severity(sev)
    }

    fn inconclusive(sev: Severity) -> QcFinding {
        QcFinding::new(RuleId::new("t"))
            .status(VerdictDecision::Inconclusive)
            .severity(sev)
    }

    #[test]
    fn below_threshold_fail_becomes_warn() {
        let policy = VerdictPolicy {
            fail_on: Severity::Error,
        };
        let r = resolve(policy, &[failing(Severity::Warning)]);
        assert_eq!(r.verdict, VerdictDecision::Warn);
        assert_eq!(r.below_threshold, 1);
    }

    #[test]
    fn at_or_above_threshold_forces_fail() {
        let policy = VerdictPolicy {
            fail_on: Severity::Error,
        };
        assert_eq!(
            resolve(policy, &[failing(Severity::Error)]).verdict,
            VerdictDecision::Fail
        );
        assert_eq!(
            resolve(policy, &[failing(Severity::Critical)]).verdict,
            VerdictDecision::Fail
        );
        assert_eq!(
            resolve(policy, &[failing(Severity::Warning)]).verdict,
            VerdictDecision::Warn
        );
    }

    #[test]
    fn inconclusive_at_threshold_warns() {
        let policy = VerdictPolicy {
            fail_on: Severity::Error,
        };
        assert_eq!(
            resolve(policy, &[inconclusive(Severity::Error)]).verdict,
            VerdictDecision::Warn
        );
        assert_eq!(
            resolve(policy, &[inconclusive(Severity::Info)]).verdict,
            VerdictDecision::Pass
        );
    }

    #[test]
    fn empty_findings_pass() {
        assert_eq!(
            resolve(VerdictPolicy::default(), &[]).verdict,
            VerdictDecision::Pass
        );
    }

    #[test]
    fn exit_codes_match_spec() {
        assert_eq!(resolve(VerdictPolicy::default(), &[]).exit_code(), 0);
        assert_eq!(
            resolve(
                VerdictPolicy {
                    fail_on: Severity::Error
                },
                &[failing(Severity::Warning)]
            )
            .exit_code(),
            1
        );
        assert_eq!(
            resolve(
                VerdictPolicy {
                    fail_on: Severity::Error
                },
                &[failing(Severity::Error)]
            )
            .exit_code(),
            2
        );
    }
}
