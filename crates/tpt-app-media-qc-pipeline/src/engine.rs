//! Rule execution engine for a single asset ([spec § 11]).

use std::sync::Arc;

use tpt_app_media_qc_core::error::Result;
use tpt_app_media_qc_model::asset::Asset;
use tpt_app_media_qc_profile::model::Profile;
use tpt_app_media_qc_rules::{build_rules, QcRule};

use crate::inspector::{InspectionLevel, Inspector};
use crate::verdict::{resolve, VerdictPolicy};
use crate::{aggregate, run_rules, QcRun};
use tpt_app_media_qc_model::inspection::Inspection;

/// Merge a full-decode inspection into its metadata baseline.
///
/// A decoder refines an existing stream rather than creating a second entry
/// for it. Otherwise `video_for(0)` would return metadata first and hide the
/// decoded measurements from every rule.
fn merge_inspections(metadata: Inspection, decoded: Inspection) -> Inspection {
    fn merge_by_stream<T, F>(mut metadata: Vec<T>, decoded: Vec<T>, key: F) -> Vec<T>
    where
        F: Fn(&T) -> u64,
    {
        for replacement in decoded {
            if let Some(existing) = metadata
                .iter_mut()
                .find(|item| key(item) == key(&replacement))
            {
                *existing = replacement;
            } else {
                metadata.push(replacement);
            }
        }
        metadata.sort_by_key(|item| key(item));
        metadata
    }

    let mut merged = metadata;
    merged.container = decoded.container;
    merged.video = merge_by_stream(merged.video, decoded.video, |video| video.stream_idx);
    merged.audio = merge_by_stream(merged.audio, decoded.audio, |audio| audio.stream_idx);
    merged.diagnostics.extend(decoded.diagnostics);
    merged
}

/// The per-profile QC engine. Cheap to share (all members are `Arc`).
pub struct QcEngine {
    pub profile: Arc<Profile>,
    pub rules: Vec<Box<dyn QcRule>>,
    pub inspector: Arc<dyn Inspector>,
}

impl QcEngine {
    pub fn new(profile: Arc<Profile>, inspector: Arc<dyn Inspector>) -> Self {
        let rules = build_rules(&profile);
        Self {
            profile,
            rules,
            inspector,
        }
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
                merge_inspections(metadata, decoded)
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

#[cfg(test)]
mod tests {
    use super::*;
    use tpt_app_media_qc_model::inspection::{
        AudioMeasurements, ContainerInspection, ContainerValidity, VideoMeasurements,
    };

    #[test]
    fn decoded_measurements_replace_metadata_for_the_same_stream() {
        let metadata = Inspection {
            container: ContainerInspection {
                validity: ContainerValidity::Ok,
                ..Default::default()
            },
            video: vec![VideoMeasurements {
                stream_idx: 7,
                decoded_frame_count: None,
                ..Default::default()
            }],
            ..Default::default()
        };
        let decoded = Inspection {
            container: metadata.container.clone(),
            video: vec![VideoMeasurements {
                stream_idx: 7,
                decoded_frame_count: Some(3),
                ..Default::default()
            }],
            audio: vec![AudioMeasurements {
                stream_idx: 2,
                peak_db: Some(-3.0),
                ..Default::default()
            }],
            ..Default::default()
        };

        let merged = merge_inspections(metadata, decoded);
        assert_eq!(merged.video.len(), 1);
        assert_eq!(merged.video[0].decoded_frame_count, Some(3));
        assert_eq!(merged.audio[0].peak_db, Some(-3.0));
    }
}
