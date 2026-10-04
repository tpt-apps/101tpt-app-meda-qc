//! Inspection results — the measurement boundary between probe front-ends
//! (ffprobe, and later the TPT foundation crates) and the QC rules.
//!
//! Rules consume *measurements*, not raw media bytes ([spec § 3.5]). The
//! inspection model is the stable exchange format: a probe fills it in, rules
//! read it back. Missing measurements (`None`/empty vectors) are expected on
//! the metadata-only path and rules must report `Inconclusive` or pass
//! accordingly ([spec § 3.4]).

use crate::asset::Stream;
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
    /// Scanning order (interlacement) observed for this stream. `None` when
    /// the probe front-end did not report a field order.
    #[serde(default)]
    pub field_order: Option<crate::asset::FieldOrder>,
    /// General-flash transitions for photosensitive-epilepsy (PSE) analysis.
    /// `None` when the decode pass did not run flash analysis.
    #[serde(default)]
    pub flash: Option<FlashMeasurements>,
    /// Colour/HDR signalling reported by the container or bitstream.
    #[serde(default)]
    pub hdr: Option<HdrMetadata>,
    /// Stuck/dead/flickering pixel analysis ([spec § 8.4]). `None` when the
    /// decode pass did not run dead-pixel analysis.
    #[serde(default)]
    pub dead_pixels: Option<DeadPixelStats>,
}

/// A run of pixels that stayed at an extreme value while the rest of the
/// picture varied (spec § 8.4 dead pixel detection).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeadPixelCluster {
    /// Left edge in source pixels.
    pub x: u32,
    /// Top edge in source pixels.
    pub y: u32,
    /// Cluster width in source pixels.
    pub width: u32,
    /// Cluster height in source pixels.
    pub height: u32,
    /// Number of flagged cells in the cluster.
    pub cells: u32,
    pub kind: DeadPixelKind,
}

/// How a stuck pixel cluster manifests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeadPixelKind {
    /// Always dark even in bright shots (dead pixel).
    #[default]
    Dead,
    /// Always bright even in dark shots (stuck pixel).
    Stuck,
    /// Alternates between the dark and bright extremes (flicker pixel).
    Flicker,
}

impl DeadPixelKind {
    pub fn as_str(self) -> &'static str {
        match self {
            DeadPixelKind::Dead => "dead",
            DeadPixelKind::Stuck => "stuck",
            DeadPixelKind::Flicker => "flicker",
        }
    }
}

/// Stuck-pixel analysis over the decoded frames.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DeadPixelStats {
    /// Frames that contributed to the analysis.
    pub frames_sampled: u64,
    /// Cells compared (frame area, or the sampled area when reduced).
    pub cells_examined: u64,
    /// True when the picture exceeded the per-frame cell budget and was
    /// stride-sampled, so coverage is coarser than one cell per source pixel.
    pub resolution_limited: bool,
    pub dark_cells: u64,
    pub bright_cells: u64,
    pub flicker_cells: u64,
    /// Flagged clusters, capped by the analyzer.
    pub clusters: Vec<DeadPixelCluster>,
}

impl DeadPixelStats {
    /// Total flagged cells across all three kinds.
    pub fn total_cells(&self) -> u64 {
        self.dark_cells + self.bright_cells + self.flicker_cells
    }

    /// Fraction of examined cells that were flagged.
    pub fn flagged_fraction(&self) -> f64 {
        if self.cells_examined == 0 {
            0.0
        } else {
            self.total_cells() as f64 / self.cells_examined as f64
        }
    }
}

/// Static HDR and colorimetry signalling for a video stream. Every field is
/// `None` when the probe front-end did not report it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HdrMetadata {
    /// Transfer characteristic, e.g. `bt709`, `smpte2084` (PQ), `arib-std-b67` (HLG).
    pub transfer: Option<String>,
    /// Colour primaries, e.g. `bt709`, `bt2020`.
    pub primaries: Option<String>,
    /// Matrix coefficients, e.g. `bt709`, `bt2020nc`.
    pub matrix: Option<String>,
    /// Sample range: `tv` (limited) or `pc` (full).
    pub range: Option<String>,
    /// SMPTE ST 2086 mastering display colour volume.
    pub mastering_display: Option<MasteringDisplay>,
    /// Maximum content light level (MaxCLL) in nits.
    pub max_cll: Option<u32>,
    /// Maximum frame-average light level (MaxFALL) in nits.
    pub max_fall: Option<u32>,
    /// Dolby Vision configuration record present.
    #[serde(default)]
    pub dolby_vision: bool,
}

/// SMPTE ST 2086 mastering display colour volume.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MasteringDisplay {
    pub min_luminance_nits: Option<f64>,
    pub max_luminance_nits: Option<f64>,
    /// Red, green, blue CIE 1931 xy chromaticities.
    pub primaries_xy: Option<[(f64, f64); 3]>,
    pub white_point_xy: Option<(f64, f64)>,
}

/// The kind of transfer function an [`HdrMetadata`] signals.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DynamicRange {
    Sdr,
    /// Perceptual quantiser (SMPTE ST 2084).
    Pq,
    /// Hybrid log-gamma (ARIB STD-B67 / BT.2100).
    Hlg,
}

impl HdrMetadata {
    /// Classifies the transfer characteristic; `None` when unreported.
    pub fn dynamic_range(&self) -> Option<DynamicRange> {
        match self.transfer.as_deref()?.to_ascii_lowercase().as_str() {
            "smpte2084" | "st2084" | "pq" => Some(DynamicRange::Pq),
            "arib-std-b67" | "hlg" => Some(DynamicRange::Hlg),
            _ => Some(DynamicRange::Sdr),
        }
    }
}

/// Flash classes analysed: index 0 is the general flash, index 1 the
/// saturated-red flash.
pub const FLASH_CHANNELS: usize = 2;

/// Display names for the flash classes, indexed like [`FlashMeasurements`].
pub const FLASH_CHANNEL_NAMES: [&str; FLASH_CHANNELS] = ["general flash", "red flash"];

/// Screen-level flash transitions (Harding / ITU-R BT.1702-style): moments
/// where a significant screen area changes in the opposite direction to the
/// previous change.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FlashMeasurements {
    /// Transition timestamps in milliseconds, one list per flash class (see
    /// [`FLASH_CHANNEL_NAMES`]).
    pub transitions_ms: Vec<Vec<u64>>,
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

/// A subtitle stream's measurement set (spec § 8.7).
///
/// Cue data comes from packet headers (timing) and, for plain-text subtitle
/// codecs, the decoded cue payload. Fields that could not be measured stay
/// `None` so rules report `Inconclusive` rather than guessing ([spec § 3.4).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SubtitleMeasurements {
    /// Stream index this measurement belongs to.
    pub stream_idx: u64,
    /// Number of cues read. `None` means no cue pass ran for this stream;
    /// `Some(0)` means the pass ran and the stream carries no cues.
    #[serde(default)]
    pub cue_count: Option<u64>,
    /// Whether cue payloads were decoded as text. When `false`, content rules
    /// (character limits, empty cues) cannot be evaluated.
    #[serde(default)]
    pub text_decoded: bool,
    /// Cues whose end is not after their start.
    #[serde(default)]
    pub invalid_durations: Vec<TimeRange>,
    /// Cues whose display time overlaps the following cue.
    #[serde(default)]
    pub overlaps: Vec<TimeRange>,
    /// Cues whose payload could not be decoded as text (malformed data).
    #[serde(default)]
    pub malformed: Vec<TimeRange>,
    /// Cues that decoded cleanly but carry no visible text.
    #[serde(default)]
    pub empty_cues: Vec<TimeRange>,
    /// Largest silence between the end of one cue and the start of the next,
    /// measured within the span covered by cues.
    #[serde(default)]
    pub max_gap_ms: Option<u64>,
    /// Longest single line, in characters (requires `text_decoded`).
    #[serde(default)]
    pub max_line_chars: Option<u32>,
    /// Most lines in any single cue (requires `text_decoded`).
    #[serde(default)]
    pub max_lines_per_cue: Option<u32>,
    /// Longest cue, in milliseconds.
    #[serde(default)]
    pub max_cue_duration_ms: Option<u64>,
    /// End of the last cue, in milliseconds — the point the subtitles actually
    /// cover, which is what duration checks compare against the video.
    #[serde(default)]
    pub covered_until_ms: Option<u64>,
}

impl SubtitleMeasurements {
    /// Timing measurements were collected for this stream.
    pub fn has_cues(&self) -> bool {
        self.cue_count.is_some()
    }
}

/// The complete inspection of a single asset under one run.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Inspection {
    /// Stream metadata discovered by the metadata front-end. This is kept in
    /// the inspection so the engine can expose it to rules even when the
    /// caller supplied an otherwise metadata-only `Asset`.
    #[serde(default)]
    pub streams: Vec<Stream>,
    pub container: ContainerInspection,
    pub video: Vec<VideoMeasurements>,
    pub audio: Vec<AudioMeasurements>,
    #[serde(default)]
    pub subtitle: Vec<SubtitleMeasurements>,
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

    pub fn subtitle_for(&self, idx: u64) -> Option<&SubtitleMeasurements> {
        self.subtitle.iter().find(|s| s.stream_idx == idx)
    }
}
