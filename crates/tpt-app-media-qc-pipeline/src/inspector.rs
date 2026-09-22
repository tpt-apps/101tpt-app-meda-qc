//! The inspection boundary ([spec § 11]).
//!
//! A probe front-end (ffprobe today; the TPT foundation crates later)
//! implements [`Inspector`]. The engine calls it for a metadata-only pass and
//! optionally a decode pass, and rules only ever see measurements.

use std::sync::Arc;
use tpt_app_media_qc_core::error::Result;
use tpt_app_media_qc_model::asset::Asset;
use tpt_app_media_qc_model::inspection::Inspection;

/// How thorough an inspection run should be.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum InspectionLevel {
    /// Container + stream metadata only (quick scan / streaming path).
    #[default]
    MetadataOnly,
    /// Also decode streams for the measurement rules.
    Full,
}

/// Inspection provider boundary.
pub trait Inspector: Send + Sync {
    /// Human-readable probe name, e.g. `ffprobe`.
    fn name(&self) -> &str;

    /// The metadata-only pass: container validity, format, durations,
    /// bitrates, stream table, start timecodes.
    fn inspect_metadata(&self, asset: &Asset) -> Result<Inspection>;

    /// The decode pass. Implementations refine the metadata inspection with
    /// decoded measurements. The default just reuses metadata unchanged, which
    /// is valid for metadata-only inspectors.
    fn inspect_decode(
        &self,
        asset: &Asset,
        metadata: &Inspection,
        level: InspectionLevel,
    ) -> Result<Inspection> {
        let _ = (asset, level);
        Ok(metadata.clone())
    }
}

/// A no-op inspector: no measurements at all, so every decode-dependent rule
/// reports `Inconclusive` instead of guessing ([spec § 3.4]).
pub struct NoopInspector;

impl Inspector for NoopInspector {
    fn name(&self) -> &str {
        "noop"
    }

    fn inspect_metadata(&self, _asset: &Asset) -> Result<Inspection> {
        Ok(Inspection::default())
    }
}

/// Convenience: an [`Inspector`] object usable wherever an `Arc<dyn Inspector>`
/// is accepted.
pub fn arc<I: Inspector + 'static>(inspector: I) -> Arc<dyn Inspector> {
    Arc::new(inspector) as Arc<dyn Inspector>
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpt_app_media_qc_model::inspection::ContainerValidity;

    #[test]
    fn noop_inspector_is_untouched_and_scanned_as_not_scanned() {
        let asset = Asset {
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
        };
        let inspection = NoopInspector.inspect_metadata(&asset).unwrap();
        assert_eq!(inspection.container.validity, ContainerValidity::NotScanned);
    }
}