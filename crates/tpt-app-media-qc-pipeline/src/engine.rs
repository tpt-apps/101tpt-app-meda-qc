//! Rule execution engine for a single asset ([spec § 11]).

use std::sync::Arc;

use tpt_app_media_qc_core::error::Result;
use tpt_app_media_qc_model::asset::Asset;
use tpt_app_media_qc_profile::model::Profile;
use tpt_app_media_qc_rules::{build_rules, QcRule};

use crate::inspector::{InspectionLevel, Inspector};
use crate::verdict::{resolve, VerdictPolicy};
use crate::{aggregate, run_rules, QcRun};

/// The per-profile QC engine. Cheap to share (all members are `Arc`).
pub struct QcEngine {
    pub profile: Arc<Profile>,
    pub rules: Vec<Box<dyn QcRule>>,
    pub inspector: Arc<dyn Inspector>,
}

impl QcEngine {
    pub fn new(profile: Arc<Profile>, inspector: Arc<dyn Inspector>) -> Self {
        let rules = build_rules(&profile);
        Self { profile, rules, inspector }
    }

    /// Shortcut for metadata-only checks.
    pub fn check_metadata_only(&self, asset: &Asset) -> Result<QcRun> {
        self.check(asset, InspectionLevel::MetadataOnly)
    }

    /// Inspect and QC an asset at the given depth.
    pub fn check(&self, asset: &Asset, level: InspectionLevel) -> Result<QcRun> {
        let metadata = self.inspector.inspect_metadata(asset)?;
        let inspection = match level {
            InspectionLevel::MetadataOnly => metadata,
            InspectionLevel::Full => {
                let decoded = self.inspector.inspect_decode(asset, &metadata, level)?;
                // `Inspection::merge_into` semantics: prefer the decoded values
                // but keep container/stream fields from metadata.
                let mut merged = metadata;
                merged.video.extend(decoded.video);
                merged.audio.extend(decoded.audio);
                merged.container = decoded.container;
                merged.diagnostics = decoded.diagnostics;
                merged
            }
        };

        let findings = run_rules(&self.rules, asset, &inspection);
        let (per_rule, counts) = aggregate(&self.rules, &findings);
        let policy = VerdictPolicy::from_profile(&self.profile.policy);
        let resolution = resolve(policy, &findings);

        Ok(QcRun {
            asset: asset.clone(),
            profile_name: self.profile.name.clone(),
            profile_version: self.profile.version,
            inspection,
            findings,
            per_rule,
            counts,
            verdict: resolution.verdict,
            policy,
        })
    }
}