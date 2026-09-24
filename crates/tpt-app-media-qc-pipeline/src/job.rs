//! A single scheduled QC job.

use std::sync::Arc;
use tpt_app_media_qc_model::asset::Asset;
use tpt_app_media_qc_profile::model::Profile;

use crate::inspector::InspectionLevel;

/// One unit of QC work: check an asset against a profile.
#[derive(Clone, Debug)]
pub struct Job {
    /// Caller-assigned job identifier (e.g. batch row id).
    pub id: String,
    pub asset: Asset,
    pub profile: Arc<Profile>,
    /// Inspection depth for this job.
    pub level: InspectionLevel,
}

impl Job {
    pub fn new(id: impl Into<String>, asset: Asset, profile: Arc<Profile>) -> Self {
        Self {
            id: id.into(),
            asset,
            profile,
            level: InspectionLevel::default(),
        }
    }

    pub fn with_level(mut self, level: InspectionLevel) -> Self {
        self.level = level;
        self
    }
}
