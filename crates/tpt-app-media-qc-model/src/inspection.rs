//! Inspection results — the measurement boundary between probe front-ends
//! (ffprobe, and later the TPT foundation crates) and the QC rules.
//!
//! Rules consume *measurements*, not raw media bytes ([spec § 3.5]). The
//! inspection model is the stable exchange format: a probe fills it in, rules
//! read it back. Missing measurements (`None`/empty vectors) are expected on
//! the metadata-only path and rules must report `Inconclusive` or pass
//! accordingly ([spec § 3.4]).

use crate::finding::TimeRange;
use crate::time::FrameRate;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;

/// Whether the container could be opened and scanned.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ContainerValidity {
    Ok,
    /// Container atoms/structure could not be fully parsed.
    Corrupt(String),
    /// File unreadable (missing, permission, non-media).
    Unreadable(String),
    /// No inspection was attempted (e.g. metadata-only path without a probe).
    #[default]
    NotScanned,
}

/// Container-level measurements.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ContainerInspection {
    pub validity: ContainerValidity,
    pub format: Option<String>,
    /// Overall bitrate in bits/second as reported by the container.
    pub bitrate_bps: Option<u64>,
    pub duration: Option<DurationMillis>,
    /// Whether timestamps were contiguous (no gaps > tolerance).
    pub timestamps_contiguous: Option<bool>,
    /// Whether a start timecode (e.g. QuickTime `tmcd` / MXF `StartTimecode`)
    /// is present in the container.
    pub timecode_present: Option<bool>,
    /// Timestamp jumps discovered beyond the configured tolerance.
    pub timestamp_gaps: Vec<TimeRange>,
    /// Malformed/garbled metadata entries (key: raw value).
    pub malformed_metadata: Vec<String>,
    /// Number of decode errors encountered while scanning.
    pub decode_errors: u64,
}

/// Millisecond-precision duration for measurements.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DurationMillis(pub u64);

impl DurationMillis {
    pub fn as_secs(&self) -> u64 {
        self.0 / 1000
    }
}

impl From<Duration> for DurationMillis {
    fn from(d: Duration) -> Self {
        DurationMillis(d.as_millis() as u64)
    }
}

impl From<DurationMillis> for Duration {
    fn from(v: DurationMillis) -> Self {
        Duration::from_millis(v.0)
    }
}

/// A video stream's measurement set.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct VideoMeasurements {
    /// Stream index this measurement belongs to.
    pub stream_idx: u64,
    /// Observed (measured over decoded frames) frame rate.
    pub frame_rate_observed: Option<FrameRate>,
    /// Number of successfully decoded frames represented by this measurement.
    ///
    /// `None` means no decode pass supplied coverage. `Some(0)` means a decode
    /// pass ran but could not decode any frames. Decode-dependent rules must
    /// distinguish these states from a complete scan with no detected defects.
    #[serde(default)]
    pub decoded_frame_count: Option<u64>,
    /// Number of decode errors attributed to this stream.
    pub decode_errors: u64,
    /// Black-frame segments (subseconds resolution).
    pub black_frames: Vec<TimeRange>,
    /// Freeze-frame segments.
    pub freeze_frames: Vec<TimeRange>,
    /// Duplicate-frame segments.
    pub duplicate_frames: Vec<TimeRange>,
    /// Luma statistics over sampled decoded frames.
    pub luma: Option<LumaStats>,
    /// Colour-space/colorimetry tag observed (e.g. `bt709`, `bt2020nc`).
    pub colorspace: Option<String>,
}

/// Luma statistics over sampled frames ([spec § 8.3] brightness/luma range).
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct LumaStats {
    pub min: u8,
    pub max: u8,
    pub mean: f64,
    /// Fraction of samples below legal `16` (BT.709 studio swing) — `0..1`.
    pub below_legal: f64,
    /// Fraction of samples above legal `235`.
    pub above_legal: f64,
    /// Fraction of samples at maximum white (`255`) — clipping indicator.
    pub clipped_white: f64,
}

/// An audio stream's measurement set.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AudioMeasurements {
    pub stream_idx: u64,
    /// Number of successfully decoded audio sample frames represented by this
    /// measurement (one frame contains one sample per channel).
    ///
    /// `None` means no decode pass supplied coverage. `Some(0)` means a decode
    /// pass ran but could not decode any frames. Decode-dependent rules must
    /// distinguish these states from a complete scan with no detected defects.
    #[serde(default)]
    pub decoded_frame_count: Option<u64>,
    /// Decode errors for this stream.
    pub decode_errors: u64,
    /// Detected silence segments (below configured threshold).
    pub silence: Vec<TimeRange>,
    /// Whether the analyzer stopped retaining silence ranges after reaching
    /// its bounded range limit. The silence rule must then report
    /// `Inconclusive`, because absence of a retained range is not a pass.
    #[serde(default)]
    pub silence_truncated: bool,
    /// Number of clipping events (samples at/beyond saturation).
    pub clipping_events: u64,
    /// Peak level in dBFS (-inf..0).
    pub peak_db: Option<f64>,
    /// True-peak in dBTP.
    pub true_peak_db: Option<f64>,
    /// Integrated loudness in LUFS using the profile's standard.
    pub loudness_lufs: Option<f64>,
    /// Loudness range (LRA) in LU when the standard defines it.
    pub loudness_range_lu: Option<f64>,
    /// Stereo phase correlation across the stream (negative = out of phase).
    pub phase_correlation: Option<f64>,
    /// DC offset estimate in % (-100..100).
    pub dc_offset_percent: Option<f64>,
}

/// The complete inspection of a single asset under one run.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Inspection {
    pub container: ContainerInspection,
    pub video: Vec<VideoMeasurements>,
    pub audio: Vec<AudioMeasurements>,
    /// Raw probe/diagnostic payload preserved for evidence and report body.
    pub diagnostics: BTreeMap<String, serde_json::Value>,
}

impl Inspection {
    pub fn video_for(&self, idx: u64) -> Option<&VideoMeasurements> {
        self.video.iter().find(|v| v.stream_idx == idx)
    }

    pub fn audio_for(&self, idx: u64) -> Option<&AudioMeasurements> {
        self.audio.iter().find(|a| a.stream_idx == idx)
    }
}
