//! Job persistence and findings storage (spec § 18).

use chrono::Utc;
use tpt_app_media_qc_model::finding::QcFinding;
use tpt_app_media_qc_model::report::Report;
use tpt_app_media_qc_pipeline::QcRun;

use crate::error::Result;
use crate::Store;

/// Lifecycle status of a persisted QC job.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl JobStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            JobStatus::Queued => "queued",
            JobStatus::Running => "running",
            JobStatus::Succeeded => "succeeded",
            JobStatus::Failed => "failed",
            JobStatus::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "running" => JobStatus::Running,
            "succeeded" => JobStatus::Succeeded,
            "failed" => JobStatus::Failed,
            "cancelled" => JobStatus::Cancelled,
            _ => JobStatus::Queued,
        }
    }
}

/// A stored job row.
#[derive(Clone, Debug)]
pub struct JobRecord {
    pub id: String,
    pub project_id: Option<String>,
    pub asset_sha256: String,
    pub profile_sha256: String,
    pub level: String,
    pub status: JobStatus,
    pub submitted_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub verdict: Option<String>,
    pub error: Option<String>,
}
impl JobRecord {
    fn map_status(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(JobRecord {
            id: row.get(0)?,
            project_id: row.get(1)?,
            asset_sha256: row.get(2)?,
            profile_sha256: row.get(3)?,
            level: row.get(4)?,
            status: JobStatus::parse(&row.get::<_, String>(5)?),
            submitted_at: row.get(6)?,
            started_at: row.get(7)?,
            finished_at: row.get(8)?,
            verdict: row.get(9)?,
            error: row.get(10)?,
        })
    }
}

impl Store {
    /// Insert a new queued job.
    #[allow(clippy::too_many_arguments)]
    pub fn create_job(
        &self,
        id: &str,
        project_id: Option<&str>,
        asset_sha256: &str,
        profile_sha256: &str,
        level: &str,
    ) -> Result<()> {
        self.conn().execute(
            r#"
            INSERT INTO jobs (id, project_id, asset_sha256, profile_sha256, level, status, submitted_at)
            VALUES (?1, ?2, ?3, ?4, ?5, 'queued', ?6)
            "#,
            rusqlite::params![id, project_id, asset_sha256, profile_sha256, level, Utc::now().timestamp_millis()],
        )?;
        Ok(())
    }

    /// Mark a job as running.
    pub fn start_job(&self, id: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE jobs SET status = 'running', started_at = ?1 WHERE id = ?2",
            rusqlite::params![Utc::now().timestamp_millis(), id],
        )?;
        Ok(())
    }

    /// Record a successful run: verdict, flattened findings, report metadata
    /// and the immutable report body.
    pub fn complete_job(&self, id: &str, run: &QcRun, report: &Report) -> Result<()> {
        let finished = Utc::now().timestamp_millis();
        let report_json = serde_json::to_string(report)?;

        self.conn().execute(
            r#"
            UPDATE jobs SET
                status = 'succeeded',
                finished_at = ?1,
                verdict = ?2,
                report_json = ?3,
                error = NULL
            WHERE id = ?4
            "#,
            rusqlite::params![finished, run.verdict.as_str(), report_json, id],
        )?;

        let mut insert = self.conn().prepare(
            r#"
                INSERT INTO findings
                    (job_id, rule_id, status, severity, message, measured_json, expected_json,
                     stream_index, time_start_ms, time_end_ms, frame_start, frame_end,
                     evidence_json, confidence)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                "#,
        )?;
        for f in &run.findings {
            let time = f.time_range;
            let frames = f.frame_range;
            insert.execute(rusqlite::params![
                id,
                f.rule_id.as_str(),
                f.status.as_str(),
                f.severity.as_str(),
                f.message,
                f.measured.as_ref().map(serde_json::to_string).transpose()?,
                f.expected.as_ref().map(serde_json::to_string).transpose()?,
                f.stream_idx.map(|s| s.0),
                time.map(|t| t.start_ms),
                time.map(|t| t.end_ms),
                frames.map(|f| f.start),
                frames.map(|f| f.end),
                serde_json::to_string(&f.evidence)?,
                f.confidence,
            ])?;
        }

        self.conn().execute(
            r#"
            INSERT INTO report_metadata
                (job_id, analysis_id, created_at, asset_sha256, profile_sha256, application_version, ruleset_version)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(job_id) DO UPDATE SET
                analysis_id = excluded.analysis_id,
                created_at = excluded.created_at,
                asset_sha256 = excluded.asset_sha256,
                profile_sha256 = excluded.profile_sha256,
                application_version = excluded.application_version,
                ruleset_version = excluded.ruleset_version
            "#,
            rusqlite::params![
                id,
                report.analysis_id.0.to_string(),
                report.created_at.timestamp_millis(),
                report.integrity.asset_sha256,
                report.integrity.profile_sha256,
                report.integrity.application_version,
                report.integrity.ruleset_version
            ],
        )?;
        Ok(())
    }

    /// Mark a job as failed with an error message. The job's findings are not
    /// touched; failure state is isolated per job (spec § 21).
    pub fn fail_job(&self, id: &str, error: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE jobs SET status = 'failed', finished_at = ?1, error = ?2 WHERE id = ?3",
            rusqlite::params![Utc::now().timestamp_millis(), error, id],
        )?;
        Ok(())
    }

    /// Cancel a queued/running job.
    pub fn cancel_job(&self, id: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE jobs SET status = 'cancelled', finished_at = ?1 WHERE id = ?2 AND status IN ('queued', 'running')",
            rusqlite::params![Utc::now().timestamp_millis(), id],
        )?;
        Ok(())
    }

    /// Fetch a single job record.
    pub fn job(&self, id: &str) -> Result<Option<JobRecord>> {
        use rusqlite::OptionalExtension;
        self.conn()
            .query_row(
                r#"
                SELECT id, project_id, asset_sha256, profile_sha256, level, status,
                       submitted_at, started_at, finished_at, verdict, error
                FROM jobs WHERE id = ?1
                "#,
                [id],
                JobRecord::map_status,
            )
            .optional()
            .map_err(Into::into)
    }

    /// List jobs, newest first, optionally filtered by status.
    pub fn jobs(&self, status: Option<JobStatus>, limit: u64) -> Result<Vec<JobRecord>> {
        let sql = match status {
            Some(_) => {
                r#"
                SELECT id, project_id, asset_sha256, profile_sha256, level, status,
                       submitted_at, started_at, finished_at, verdict, error
                FROM jobs WHERE status = ?1 ORDER BY submitted_at DESC LIMIT ?2
                "#
            }
            None => {
                r#"
                SELECT id, project_id, asset_sha256, profile_sha256, level, status,
                       submitted_at, started_at, finished_at, verdict, error
                FROM jobs ORDER BY submitted_at DESC LIMIT ?1
                "#
            }
        };

        let mut stmt = self.conn().prepare(sql)?;
        let rows = match status {
            Some(s) => stmt.query_map(
                rusqlite::params![s.as_str(), limit as i64],
                JobRecord::map_status,
            )?,
            None => stmt.query_map([limit as i64], JobRecord::map_status)?,
        };
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Load the flattened findings for a completed job.
    pub fn findings_for_job(&self, id: &str) -> Result<Vec<QcFinding>> {
        let mut stmt = self.conn().prepare(
            r#"
            SELECT rule_id, status, severity, message, measured_json, expected_json,
                   stream_index, time_start_ms, time_end_ms, frame_start, frame_end,
                   evidence_json, confidence
            FROM findings WHERE job_id = ?1 ORDER BY id
            "#,
        )?;
        let rows = stmt.query_map([id], |row| {
            Ok(QcFinding {
                rule_id: tpt_app_media_qc_model::finding::RuleId::new(row.get::<_, String>(0)?),
                status: parse_verdict(&row.get::<_, String>(1)?),
                severity: parse_severity(&row.get::<_, String>(2)?),
                message: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                measured: row
                    .get::<_, Option<String>>(4)?
                    .as_deref()
                    .map(serde_json::from_str)
                    .transpose()
                    .unwrap_or(None),
                expected: row
                    .get::<_, Option<String>>(5)?
                    .as_deref()
                    .map(serde_json::from_str)
                    .transpose()
                    .unwrap_or(None),
                stream_idx: row
                    .get::<_, Option<u64>>(6)?
                    .map(tpt_app_media_qc_model::asset::StreamId::new),
                time_range: match (row.get::<_, Option<u64>>(7)?, row.get::<_, Option<u64>>(8)?) {
                    (Some(start_ms), Some(end_ms)) => Some(
                        tpt_app_media_qc_model::finding::TimeRange::new(start_ms, end_ms),
                    ),
                    _ => None,
                },
                frame_range: match (
                    row.get::<_, Option<u64>>(9)?,
                    row.get::<_, Option<u64>>(10)?,
                ) {
                    (Some(start), Some(end)) => {
                        Some(tpt_app_media_qc_model::finding::FrameRange::new(start, end))
                    }
                    _ => None,
                },
                evidence: row
                    .get::<_, Option<String>>(11)?
                    .as_deref()
                    .map(serde_json::from_str)
                    .transpose()
                    .unwrap_or_default()
                    .unwrap_or_default(),
                confidence: row.get(12)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r);
        }
        out.into_iter()
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }
}

fn parse_verdict(s: &str) -> tpt_app_media_qc_model::severity::VerdictDecision {
    match s {
        "pass" => tpt_app_media_qc_model::severity::VerdictDecision::Pass,
        "warn" => tpt_app_media_qc_model::severity::VerdictDecision::Warn,
        "fail" => tpt_app_media_qc_model::severity::VerdictDecision::Fail,
        _ => tpt_app_media_qc_model::severity::VerdictDecision::Inconclusive,
    }
}

fn parse_severity(s: &str) -> tpt_app_media_qc_model::severity::Severity {
    s.parse()
        .unwrap_or(tpt_app_media_qc_model::severity::Severity::Info)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpt_app_media_qc_model::asset::{Asset, AssetFingerprint};

    fn sample_run() -> QcRun {
        let profile = tpt_app_media_qc_profile::model::Profile::default();
        let inspector = tpt_app_media_qc_pipeline::arc(tpt_app_media_qc_pipeline::NoopInspector);
        let engine =
            tpt_app_media_qc_pipeline::QcEngine::new(std::sync::Arc::new(profile), inspector);
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
        engine.check_metadata_only(&asset).unwrap()
    }

    #[test]
    fn job_lifecycle() {
        let store = Store::in_memory().unwrap();
        store
            .create_job(
                "j1",
                None,
                "ab".repeat(32).as_str(),
                "cd".repeat(32).as_str(),
                "metadata",
            )
            .unwrap();
        assert_eq!(store.job("j1").unwrap().unwrap().status, JobStatus::Queued);

        store.start_job("j1").unwrap();
        assert_eq!(store.job("j1").unwrap().unwrap().status, JobStatus::Running);

        let run = sample_run();
        let report = tpt_app_media_qc_report_fixture(&run);
        store.complete_job("j1", &run, &report).unwrap();
        let rec = store.job("j1").unwrap().unwrap();
        assert_eq!(rec.status, JobStatus::Succeeded);
        assert_eq!(rec.verdict.as_deref(), Some("pass"));

        let findings = store.findings_for_job("j1").unwrap();
        assert_eq!(findings.len(), run.findings.len());
    }

    #[test]
    fn failed_job_keeps_isolation() {
        let store = Store::in_memory().unwrap();
        store
            .create_job(
                "j1",
                None,
                "aa".repeat(32).as_str(),
                "bb".repeat(32).as_str(),
                "metadata",
            )
            .unwrap();
        store.fail_job("j1", "probe failed").unwrap();
        let rec = store.job("j1").unwrap().unwrap();
        assert_eq!(rec.status, JobStatus::Failed);
        assert_eq!(rec.error.as_deref(), Some("probe failed"));
        assert!(store.jobs(Some(JobStatus::Failed), 10).unwrap().len() == 1);
    }

    fn tpt_app_media_qc_report_fixture(run: &QcRun) -> Report {
        // Minimal report reusing the integrity fields from a run.
        use tpt_app_media_qc_core::config::{APP_VERSION, RULESET_VERSION};
        use tpt_app_media_qc_model::report::{AnalysisId, AppInfo};
        let profile_sha = "ee".repeat(32);
        Report {
            analysis_id: AnalysisId::default(),
            created_at: chrono::Utc::now(),
            app: AppInfo {
                name: "TPT Media QC".into(),
                version: APP_VERSION.into(),
                ruleset_version: RULESET_VERSION.into(),
            },
            integrity: tpt_app_media_qc_model::report::ReportIntegrity {
                asset_sha256: run.asset.fingerprint.sha256.clone(),
                profile_sha256: profile_sha.clone(),
                application_version: APP_VERSION.into(),
                ruleset_version: RULESET_VERSION.into(),
                analysis_id: AnalysisId::default().0.to_string(),
            },
            profile: tpt_app_media_qc_model::report::ProfileRef {
                name: run.profile_name.clone(),
                version: run.profile_version,
                sha256: profile_sha,
            },
            host: tpt_app_media_qc_model::report::HostInfo {
                os: "test".into(),
                arch: "test".into(),
                cpu_count: 1,
            },
            asset_path: run.asset.path.display().to_string(),
            asset_size_bytes: run.asset.size_bytes,
            asset_duration_ms: None,
            asset_resolution: None,
            findings: run.findings.clone(),
            verdict: run.verdict,
            operator_notes: String::new(),
        }
    }
}
