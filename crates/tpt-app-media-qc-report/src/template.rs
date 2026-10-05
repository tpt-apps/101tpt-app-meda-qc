//! Report templates (spec § 27 "richer report templates").
//!
//! A template chooses *which parts* of an immutable [`Report`] a human-facing
//! rendering shows. The underlying report, its verdict and its integrity
//! fields never change; JSON export is always complete.

use tpt_app_media_qc_model::finding::QcFinding;
use tpt_app_media_qc_model::report::Report;
use tpt_app_media_qc_model::severity::VerdictDecision;

/// Which view of the report a renderer produces.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReportTemplate {
    /// Header plus every finding. The historical layout.
    #[default]
    Detailed,
    /// Header plus only the findings that need attention (not `pass`).
    Summary,
    /// Verdict and a per-rule roll-up for non-technical readers; no
    /// per-finding table.
    Executive,
    /// Everything in `Detailed`, plus a per-rule roll-up and the full
    /// integrity / host block, for archiving and dispute resolution.
    Audit,
}

impl ReportTemplate {
    pub const ALL: [ReportTemplate; 4] = [
        ReportTemplate::Detailed,
        ReportTemplate::Summary,
        ReportTemplate::Executive,
        ReportTemplate::Audit,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ReportTemplate::Detailed => "detailed",
            ReportTemplate::Summary => "summary",
            ReportTemplate::Executive => "executive",
            ReportTemplate::Audit => "audit",
        }
    }

    /// Parse a template name (case-insensitive).
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|t| t.as_str().eq_ignore_ascii_case(s.trim()))
    }

    /// Heading shown at the top of the report.
    pub fn title(self) -> &'static str {
        match self {
            ReportTemplate::Detailed => "QC Report",
            ReportTemplate::Summary => "QC Summary",
            ReportTemplate::Executive => "QC Executive Summary",
            ReportTemplate::Audit => "QC Audit Report",
        }
    }

    /// Findings the template lists individually, in report order.
    pub fn findings(self, report: &Report) -> Vec<&QcFinding> {
        match self {
            ReportTemplate::Detailed | ReportTemplate::Audit => report.findings.iter().collect(),
            ReportTemplate::Summary => report
                .findings
                .iter()
                .filter(|f| f.status != VerdictDecision::Pass)
                .collect(),
            ReportTemplate::Executive => Vec::new(),
        }
    }

    /// Whether the per-finding table is shown.
    pub fn lists_findings(self) -> bool {
        self != ReportTemplate::Executive
    }

    /// Whether the per-rule roll-up is shown.
    pub fn shows_rollup(self) -> bool {
        matches!(self, ReportTemplate::Executive | ReportTemplate::Audit)
    }

    /// Whether the full integrity and host block is shown.
    pub fn shows_integrity(self) -> bool {
        matches!(self, ReportTemplate::Detailed | ReportTemplate::Audit)
    }

    /// Whether the host block (OS, architecture, CPUs) is shown.
    pub fn shows_host(self) -> bool {
        self == ReportTemplate::Audit
    }
}

impl std::fmt::Display for ReportTemplate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One line of the per-rule roll-up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleRollup {
    pub rule_id: String,
    /// The worst status among the rule's findings.
    pub status: VerdictDecision,
    pub findings: usize,
    /// Message of the worst finding.
    pub message: String,
}

fn rank(status: VerdictDecision) -> u8 {
    match status {
        VerdictDecision::Fail => 3,
        VerdictDecision::Inconclusive => 2,
        VerdictDecision::Warn => 1,
        VerdictDecision::Pass => 0,
    }
}

/// Group non-pass findings by rule, worst first (ties keep report order).
pub fn rule_rollup(report: &Report) -> Vec<RuleRollup> {
    let mut rows: Vec<RuleRollup> = Vec::new();
    for f in &report.findings {
        if f.status == VerdictDecision::Pass {
            continue;
        }
        match rows.iter_mut().find(|r| r.rule_id == f.rule_id.as_str()) {
            Some(row) => {
                row.findings += 1;
                if rank(f.status) > rank(row.status) {
                    row.status = f.status;
                    row.message = f.message.clone();
                }
            }
            None => rows.push(RuleRollup {
                rule_id: f.rule_id.as_str().to_string(),
                status: f.status,
                findings: 1,
                message: f.message.clone(),
            }),
        }
    }
    rows.sort_by_key(|r| std::cmp::Reverse(rank(r.status)));
    rows
}

#[cfg(test)]
pub(crate) mod testutil {
    use super::*;
    use tpt_app_media_qc_model::asset::{Asset, AssetFingerprint};
    use tpt_app_media_qc_model::report::AnalysisId;
    use tpt_app_media_qc_model::severity::Severity;

    /// A report with a pass, two failures of one rule, and a warning.
    pub fn sample_report() -> Report {
        let asset = Asset {
            id: Default::default(),
            path: "x.mp4".into(),
            fingerprint: AssetFingerprint {
                sha256: "ab".repeat(32),
                size_bytes: 1,
            },
            size_bytes: 1,
            modified_time: None,
            duration: None,
            streams: vec![],
        };
        let profile = tpt_app_media_qc_profile::model::Profile::default();
        let engine = tpt_app_media_qc_pipeline::QcEngine::new(
            std::sync::Arc::new(profile.clone()),
            tpt_app_media_qc_pipeline::arc(tpt_app_media_qc_pipeline::NoopInspector),
        );
        let run = engine.check_metadata_only(&asset).unwrap();
        let mut report = crate::build_report(&run, AnalysisId::default(), &profile);
        let finding = |rule: &str, status, message: &str| {
            QcFinding::new(rule)
                .status(status)
                .severity(Severity::Error)
                .set_message(message)
        };
        report.findings = vec![
            finding("a.ok", VerdictDecision::Pass, "fine"),
            finding("b.black", VerdictDecision::Fail, "black at 1s"),
            finding("b.black", VerdictDecision::Fail, "black at 9s"),
            finding("c.loud", VerdictDecision::Warn, "slightly loud"),
        ];
        report
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::sample_report;
    use super::*;

    #[test]
    fn names_round_trip() {
        for t in ReportTemplate::ALL {
            assert_eq!(ReportTemplate::parse(t.as_str()), Some(t));
        }
        assert_eq!(
            ReportTemplate::parse(" Audit "),
            Some(ReportTemplate::Audit)
        );
        assert_eq!(ReportTemplate::parse("fancy"), None);
    }

    #[test]
    fn templates_select_findings() {
        let r = sample_report();
        assert_eq!(ReportTemplate::Detailed.findings(&r).len(), 4);
        assert_eq!(ReportTemplate::Audit.findings(&r).len(), 4);
        assert_eq!(ReportTemplate::Summary.findings(&r).len(), 3);
        assert!(ReportTemplate::Executive.findings(&r).is_empty());
    }

    #[test]
    fn csv_summary_drops_passing_rows() {
        let report = sample_report();
        let dir = tempfile::tempdir().unwrap();
        let rows = |template| {
            let path = dir.path().join(format!("{template}.csv"));
            crate::write_csv(
                &path,
                &report,
                crate::WriteCsvOptions {
                    header: false,
                    template,
                },
            )
            .unwrap();
            std::fs::read_to_string(path).unwrap().lines().count()
        };
        assert_eq!(rows(ReportTemplate::Detailed), 4);
        assert_eq!(rows(ReportTemplate::Audit), 4);
        assert_eq!(rows(ReportTemplate::Summary), 3);
        assert_eq!(rows(ReportTemplate::Executive), 3);
    }

    #[test]
    fn every_template_renders_a_pdf() {
        let report = sample_report();
        let dir = tempfile::tempdir().unwrap();
        for template in ReportTemplate::ALL {
            let path = dir.path().join(format!("{template}.pdf"));
            crate::render_pdf(&path, &report, crate::WritePdfOptions { template }).unwrap();
            let bytes = std::fs::read(&path).unwrap();
            assert!(bytes.starts_with(b"%PDF"), "{template}");
        }
    }

    #[test]
    fn rollup_groups_by_rule_worst_first() {
        let rows = rule_rollup(&sample_report());
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].rule_id, "b.black");
        assert_eq!(rows[0].findings, 2);
        assert_eq!(rows[0].status, VerdictDecision::Fail);
        assert_eq!(rows[1].rule_id, "c.loud");
    }
}
