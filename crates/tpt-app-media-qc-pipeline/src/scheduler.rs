//! Batch scheduling over multiple jobs ([spec § 11]).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use tpt_app_media_qc_core::cost::{CostClass, SchedulerLimits};
use tpt_app_media_qc_core::error::Error;

use crate::engine::QcEngine;
use crate::job::Job;
use crate::QcRun;

/// Result of a batch run: one outcome per job, preserving input order.
pub type SchedulingResult = Vec<tpt_app_media_qc_core::error::Result<QcRun>>;

/// Bound the scheduler by per-cost-class concurrency limits.
pub struct Scheduler<'a> {
    engine: &'a QcEngine,
    limits: SchedulerLimits,
}

/// Run jobs against an engine using the default limits.
pub fn run_jobs(engine: &QcEngine, jobs: &[Job], limits: &SchedulerLimits) -> SchedulingResult {
    if jobs.is_empty() {
        return Vec::new();
    }

    let dominant_cost = engine
        .rules
        .iter()
        .map(|r| r.capabilities().cost)
        .max()
        .unwrap_or(CostClass::Metadata);
    let workers = limits.concurrency_for(dominant_cost).clamp(1, jobs.len());

    let next = AtomicUsize::new(0);
    thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let mut local: Vec<(usize, tpt_app_media_qc_core::error::Result<QcRun>)> = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= jobs.len() {
                            break;
                        }
                        let job = &jobs[i];
                        local.push((i, engine.check(&job.asset, job.level)));
                    }
                    local
                })
            })
            .collect();

        let mut results: Vec<tpt_app_media_qc_core::error::Result<QcRun>> =
            (0..jobs.len()).map(|_| Err(Error::Job("unreported job result".into()))).collect();
        for handle in handles {
            for (i, result) in handle.join().unwrap_or_else(|_| {
                Vec::new() // a worker panicked: results for those jobs stay "unreported"
            }) {
                results[i] = result;
            }
        }
        results
    })
}

impl<'a> Scheduler<'a> {
    pub fn new(engine: &'a QcEngine, limits: SchedulerLimits) -> Self {
        Self { engine, limits }
    }

    pub fn run(&self, jobs: &[Job]) -> SchedulingResult {
        run_jobs(self.engine, jobs, &self.limits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::inspector::NoopInspector;
    use tpt_app_media_qc_model::asset::{Asset, AssetFingerprint};

    fn asset(n: u64) -> Asset {
        Asset {
            id: Default::default(),
            path: format!("file{n}.mp4").into(),
            fingerprint: AssetFingerprint { sha256: format!("{n:064x}"), size_bytes: n },
            size_bytes: n,
            modified_time: None,
            duration: None,
            streams: vec![],
        }
    }

    #[test]
    fn scheduler_preserves_order_and_runs_all_jobs() {
        let profile = Arc::new(tpt_app_media_qc_profile::model::Profile {
            name: "test".into(),
            version: 1,
            ..Default::default()
        });
        let inspector = crate::inspector::arc(NoopInspector);
        let engine = QcEngine::new(profile.clone(), inspector);
        let jobs: Vec<Job> =
            (0..12).map(|i| Job::new(format!("job-{i}"), asset(i), profile.clone())).collect();

        let results = run_jobs(&engine, &jobs, &SchedulerLimits::default());
        assert_eq!(results.len(), 12);
        for (i, r) in results.iter().enumerate() {
            let run = r.as_ref().unwrap();
            assert_eq!(run.asset.path, format!("file{i}.mp4"));
        }
    }

    #[test]
    fn empty_job_list_is_a_noop() {
        let profile = Arc::new(tpt_app_media_qc_profile::model::Profile::default());
        let engine = QcEngine::new(profile, crate::inspector::arc(NoopInspector));
        assert!(run_jobs(&engine, &[], &SchedulerLimits::default()).is_empty());
    }
}