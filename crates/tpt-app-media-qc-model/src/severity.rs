//! Severity and verdict levels ([spec § 6.3]).

use serde::{Deserialize, Serialize};

/// Impact grade attached to a finding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum Severity {
    #[default]
    Info,
    Warning,
    Error,
    Critical,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Warning => "warning",
            Severity::Error => "error",
            Severity::Critical => "critical",
        }
    }
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Severity {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "info" | "information" => Ok(Severity::Info),
            "warning" | "warn" => Ok(Severity::Warning),
            "error" | "fail" => Ok(Severity::Error),
            "critical" | "fatal" => Ok(Severity::Critical),
            other => Err(format!("unknown severity: {other}")),
        }
    }
}

/// Verdict of an aggregated QC analysis.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VerdictDecision {
    Pass,
    Warn,
    Fail,
    Inconclusive,
}

impl VerdictDecision {
    pub fn as_str(self) -> &'static str {
        match self {
            VerdictDecision::Pass => "pass",
            VerdictDecision::Warn => "warn",
            VerdictDecision::Fail => "fail",
            VerdictDecision::Inconclusive => "inconclusive",
        }
    }
}

impl std::fmt::Display for VerdictDecision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_order() {
        assert!(Severity::Info < Severity::Warning);
        assert!(Severity::Warning < Severity::Error);
        assert!(Severity::Error < Severity::Critical);
    }

    #[test]
    fn severity_roundtrips() {
        for s in ["info", "warning", "error", "critical"] {
            let sev: Severity = s.parse().unwrap();
            assert_eq!(sev.as_str(), s);
        }
    }
}
