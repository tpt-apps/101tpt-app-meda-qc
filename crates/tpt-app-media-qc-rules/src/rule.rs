//! The `QcRule` trait and execution context ([spec § 7]).

use tpt_app_media_qc_core::cost::CostClass;
use tpt_app_media_qc_model::asset::{Asset, Stream, StreamKind};
use tpt_app_media_qc_model::finding::{QcFinding, RuleId};
use tpt_app_media_qc_model::inspection::Inspection;
use tpt_app_media_qc_model::severity::VerdictDecision as Status;

/// How much decode access a rule needs ([spec § 7] "required decode
/// capabilities" / "full-frame access").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeRequirement {
    /// Pure metadata/container check — runs in pass 1 (quick scan).
    None,
    /// Needs decoded packets for a given stream.
    Stream,
    /// Needs full frames retained (black/freeze/duplicate detection).
    FullFrame,
}

/// Declared capabilities used by the scheduler ([spec § 7], § 11).
#[derive(Clone, Copy, Debug)]
pub struct Capabilities {
    /// Stream kinds the rule must see to do anything useful.
    pub required_streams: &'static [StreamKind],
    /// Decode requirements; `None` → metadata-only rule.
    pub decode: DecodeRequirement,
    /// Whether the rule can operate incrementally (streaming) or needs the
    /// full media before reporting.
    pub incremental: bool,
    /// GPU acceleration support (optional; CPU execution is always correct).
    pub gpu: bool,
    /// Computational cost class for the scheduler.
    pub cost: CostClass,
}

impl Capabilities {
    pub const fn metadata_only() -> Self {
        Self {
            required_streams: &[],
            decode: DecodeRequirement::None,
            incremental: true,
            gpu: false,
            cost: CostClass::Metadata,
        }
    }
}

/// Static human-readable description of a rule.
#[derive(Clone, Copy, Debug)]
pub struct RuleDescription {
    pub name: &'static str,
    pub summary: &'static str,
    pub version: &'static str,
}

/// Context handed to every rule.
#[derive(Clone, Copy)]
pub struct RuleContext<'a> {
    pub asset: &'a Asset,
    pub inspection: &'a Inspection,
}

impl<'a> RuleContext<'a> {
    /// The primary (first) stream of the given kind.
    pub fn primary_stream(&self, kind: StreamKind) -> Option<&'a Stream> {
        self.asset.streams.iter().find(|s| s.kind == kind)
    }

    pub fn streams_of(&self, kind: StreamKind) -> impl Iterator<Item = &'a Stream> {
        self.asset.streams.iter().filter(move |s| s.kind == kind)
    }
}

/// Outcome of one rule execution: zero or more findings.
///
/// Zero findings means **Pass**. Findings with [`Status::Pass`] are allowed
/// when a rule explicitly wants to report a measured value.
#[derive(Clone, Debug, Default)]
pub struct RuleResult {
    pub findings: Vec<QcFinding>,
}

impl RuleResult {
    pub fn pass() -> Self {
        Self::default()
    }

    pub fn from_finding(finding: QcFinding) -> Self {
        Self {
            findings: vec![finding],
        }
    }

    pub fn findings(findings: Vec<QcFinding>) -> Self {
        Self { findings }
    }

    pub fn is_pass(&self) -> bool {
        !self.findings.iter().any(|f| f.status != Status::Pass)
    }
}

/// The rule trait ([spec § 7]).
pub trait QcRule: Send + Sync {
    fn id(&self) -> RuleId;
    fn description(&self) -> RuleDescription;
    fn capabilities(&self) -> Capabilities;
    /// Execute the rule. The default catches unexpected panics to guarantee
    /// batch isolation ([spec § 21]).
    fn execute(&self, ctx: &RuleContext<'_>) -> RuleResult {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.run(ctx))) {
            Ok(result) => result,
            Err(payload) => RuleResult::from_finding(
                self.id()
                    .to_finding(tpt_app_media_qc_model::severity::Severity::Error)
                    .status(tpt_app_media_qc_model::severity::VerdictDecision::Inconclusive)
                    .set_message(format!(
                        "{} unexpectedly panicked while running: {:?}",
                        self.id(),
                        payload
                    )),
            ),
        }
    }

    /// Internal execution hook.
    fn run(&self, ctx: &RuleContext<'_>) -> RuleResult;
}

/// Convenience helpers usable by rule implementations.
pub trait RuleExt {
    /// A preliminary finding for this rule id.
    fn to_finding(&self, severity: tpt_app_media_qc_model::severity::Severity) -> QcFinding;
}

impl RuleExt for RuleId {
    fn to_finding(&self, severity: tpt_app_media_qc_model::severity::Severity) -> QcFinding {
        QcFinding::new(self.clone())
            .severity(severity)
            .on(self.clone())
    }
}

impl RuleExt for &str {
    fn to_finding(&self, severity: tpt_app_media_qc_model::severity::Severity) -> QcFinding {
        QcFinding::new(RuleId::new(*self)).severity(severity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpt_app_media_qc_model::severity::Severity;

    struct PanickyRule;

    impl QcRule for PanickyRule {
        fn id(&self) -> RuleId {
            RuleId::new("test.panicky")
        }
        fn description(&self) -> RuleDescription {
            RuleDescription {
                name: "panicky",
                summary: "",
                version: "0",
            }
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities::metadata_only()
        }
        fn run(&self, _ctx: &RuleContext<'_>) -> RuleResult {
            panic!("boom");
        }
    }

    #[test]
    fn panics_become_inconclusive_findings() {
        let rule = PanickyRule;
        let ctx = RuleContext {
            asset: &Asset {
                id: Default::default(),
                path: "x".into(),
                fingerprint: tpt_app_media_qc_model::asset::AssetFingerprint {
                    sha256: "0".repeat(64),
                    size_bytes: 1,
                },
                size_bytes: 1,
                modified_time: None,
                duration: None,
                streams: vec![],
            },
            inspection: &Inspection::default(),
        };
        let result = rule.execute(&ctx);
        assert_eq!(result.findings.len(), 1);
        let f = &result.findings[0];
        assert_eq!(f.status, Status::Inconclusive);
        assert_eq!(f.severity, Severity::Error);
        assert!(f.message.contains("panicked"));
    }
}
