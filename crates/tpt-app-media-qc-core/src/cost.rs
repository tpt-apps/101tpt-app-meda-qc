//! Scheduler cost classification ([spec § 11]).
//!
//! Every rule declares the computational class of its execution so the
//! scheduler can bound concurrency and memory per class.

use serde::{Deserialize, Serialize};

/// Broad computational cost classes used by the job scheduler.
///
/// Ordering from cheapest to most expensive; the scheduler processes lower
/// classes first and never demoates a class once started.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostClass {
    /// Metadata/container-only inspection. Never decodes media.
    Metadata,
    /// Requires a full decode pass but no full-frame retention.
    CheapDecode,
    /// Requires a full decode pass with some frame retention.
    FullDecode,
    /// Requires GPU-capable execution.
    Gpu,
    /// Long-running expensive analysis (perceptual, ML, PSE, ...).
    ExpensiveAnalysis,
}

impl CostClass {
    /// Human-readable label.
    pub fn as_str(self) -> &'static str {
        match self {
            CostClass::Metadata => "metadata",
            CostClass::CheapDecode => "cheap-decode",
            CostClass::FullDecode => "full-decode",
            CostClass::Gpu => "gpu",
            CostClass::ExpensiveAnalysis => "expensive-analysis",
        }
    }

    /// Default concurrency allowance for this class on a worker machine.
    ///
    /// These are conservative defaults; the user may override them. GPU and
    /// expensive classes are intentionally low to avoid starving the UI.
    pub fn default_concurrency(self) -> usize {
        match self {
            CostClass::Metadata => 8,
            CostClass::CheapDecode => 2,
            CostClass::FullDecode => 2,
            CostClass::Gpu => 1,
            CostClass::ExpensiveAnalysis => 1,
        }
    }
}

impl std::fmt::Display for CostClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Bounded concurrency configuration derived from
/// [`CostClass::default_concurrency`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SchedulerLimits {
    /// Maximum concurrent jobs per cost class.
    pub per_class: std::collections::BTreeMap<CostClass, usize>,
}

impl Default for SchedulerLimits {
    fn default() -> Self {
        let mut map = std::collections::BTreeMap::new();
        for class in [
            CostClass::Metadata,
            CostClass::CheapDecode,
            CostClass::FullDecode,
            CostClass::Gpu,
            CostClass::ExpensiveAnalysis,
        ] {
            map.insert(class, class.default_concurrency());
        }
        Self { per_class: map }
    }
}

impl SchedulerLimits {
    pub fn concurrency_for(&self, class: CostClass) -> usize {
        self.per_class.get(&class).copied().unwrap_or_else(|| class.default_concurrency())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cost_class_ordering_is_cheapest_first() {
        assert!(CostClass::Metadata < CostClass::CheapDecode);
        assert!(CostClass::CheapDecode < CostClass::ExpensiveAnalysis);
    }

    #[test]
    fn defaults_are_sane() {
        let limits = SchedulerLimits::default();
        assert!(limits.concurrency_for(CostClass::Metadata) >= limits.concurrency_for(CostClass::Gpu));
    }
}