//! Typed QC profile model ([spec § 9]).
//!
//! Every rule that the engine supports has a typed configuration with a
//! default severity, so a profile can be written with a scalar severity
//! (`frame_rate: error`) or a full configuration block.

use serde::{Deserialize, Serialize};

use tpt_app_media_qc_model::severity::Severity;
use tpt_app_media_qc_model::Rational;

/// A complete QC profile: identity + rule selection + verdict policy.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    pub version: u32,
    pub rules: RuleSetConfig,
    pub policy: Policy,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            name: "default".into(),
            version: 1,
            rules: RuleSetConfig::default(),
            policy: Policy::default(),
        }
    }
}

/// Rule selection grouped by media category ([spec § 8]).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RuleSetConfig {
    pub container: ContainerRules,
    pub video: VideoRules,
    pub audio: AudioRules,
    pub subtitle: SubtitleRules,
    pub voice: VoiceRules,
}

// ---------------------------------------------------------------------------
// Container rules ([spec § 8.1])
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ContainerRules {
    /// File must be readable and a recognised media container.
    pub readable: Option<Severity>,
    /// Container structure fully valid (no corrupt atoms/boxes/trailers).
    pub container_validity: Option<Severity>,
    /// Garbage or malformed metadata entries.
    pub malformed_metadata: Option<Severity>,
    /// Container duration vs. stream durations and across streams.
    pub duration_consistency: Option<ToleranceRule>,
    /// Minimum overall/container bitrate.
    pub bitrate: Option<MinBitrateRule>,
    /// Presence of a start timecode where expected.
    pub timecode_present: Option<Severity>,
    /// Expected stream time base.
    pub timebase: Option<ExpectValueRule>,
    /// Timestamp continuity (no backward jumps or unaccounted gaps).
    pub timestamp_continuity: Option<TimestampContinuityRule>,
    /// Stream kinds not expected by the profile.
    pub unexpected_streams: Option<Severity>,
    /// Expected stream counts.
    pub stream_presence: Option<StreamPresenceRule>,
}

/// A tolerance in milliseconds around an equality check.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToleranceRule {
    pub tolerance_ms: u64,
    #[serde(default)]
    pub severity: Severity,
}

/// A lower bound on bitrate (bits/second).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct MinBitrateRule {
    pub min_bps: u64,
    #[serde(default)]
    pub severity: Severity,
}

/// Expect a scalar value (e.g. the stream time base).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExpectValueRule {
    pub value: Rational,
    #[serde(default)]
    pub severity: Severity,
}

/// Timestamp continuity: maximum tolerated gap between consecutive timestamps.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TimestampContinuityRule {
    #[serde(default = "default_ts_gap_ms")]
    pub max_gap_ms: u64,
    #[serde(default)]
    pub severity: Severity,
}

/// Expected stream counts.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct StreamPresenceRule {
    #[serde(default)]
    pub min_video: u64,
    #[serde(default)]
    pub min_audio: u64,
    #[serde(default = "default_max_streams")]
    pub max_streams: u64,
    #[serde(default)]
    pub severity: Severity,
}

pub(crate) fn default_ts_gap_ms() -> u64 {
    150
}

pub(crate) fn default_max_streams() -> u64 {
    32
}

// ---------------------------------------------------------------------------
// Video rules ([spec § 8.3])
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct VideoRules {
    pub resolution: Option<ResolutionRule>,
    pub frame_rate: Option<FrameRateRule>,
    pub aspect_ratio: Option<AspectRatioRule>,
    pub black_frames: Option<DurationThresholdRule>,
    pub freeze_frames: Option<DurationThresholdRule>,
    pub duplicate_frames: Option<CountThresholdRule>,
    pub corrupt_frames: Option<CountThresholdRule>,
    pub luma_range: Option<LumaRangeRule>,
    pub color_space: Option<ColorSpaceRule>,
}

/// Exact expected resolution.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resolution(pub u64, pub u64);

impl Resolution {
    pub fn label(&self) -> String {
        format!("{}x{}", self.0, self.1)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResolutionRule {
    pub expected: Resolution,
    #[serde(default)]
    pub severity: Severity,
}

/// Expected frame rate with tolerance (in frames per second).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct FrameRateRule {
    pub expected: Rational,
    #[serde(default = "default_frame_rate_tolerance")]
    pub tolerance: f64,
    #[serde(default)]
    pub severity: Severity,
}

pub(crate) fn default_frame_rate_tolerance() -> f64 {
    0.001
}

/// Expected aspect ratio as `width/height` (e.g. `16/9`), with a relative
/// tolerance.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct AspectRatioRule {
    pub expected: Ratio,
    #[serde(default = "default_aspect_tolerance")]
    pub tolerance: f64,
    #[serde(default)]
    pub severity: Severity,
}

pub(crate) fn default_aspect_tolerance() -> f64 {
    0.005
}

/// A maximum duration threshold (e.g. black/freeze).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct DurationThresholdRule {
    pub max_duration_ms: u64,
    #[serde(default)]
    pub severity: Severity,
}

/// A maximum count threshold (e.g. duplicate/corrupt frames, clipping hits).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CountThresholdRule {
    pub max_events: u64,
    #[serde(default)]
    pub severity: Severity,
}

/// Luma (legal-range) rule.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LumaRangeRule {
    /// Maximum permitted fraction of samples outside legal [16, 235].
    #[serde(default)]
    pub max_out_of_legal: f64,
    #[serde(default)]
    pub severity: Severity,
}

/// Expected colour-space metadata tag (e.g. `bt709`, `bt2020nc`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColorSpaceRule {
    pub expected: String,
    #[serde(default)]
    pub severity: Severity,
}

/// A simple ratio (`n/d`), e.g. `16/9`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ratio {
    pub n: f64,
    pub d: f64,
}

impl Ratio {
    pub fn value(&self) -> f64 {
        self.n / self.d
    }

    pub fn from_parts(n: f64, d: f64) -> Self {
        Self { n, d }
    }

    pub fn label(&self) -> String {
        format!("{}:{}", self.n as i64, self.d as i64)
    }
}

// ---------------------------------------------------------------------------
// Audio rules ([spec § 8.5, § 8.6])
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AudioRules {
    pub sample_rate: Option<SampleRateRule>,
    pub bit_depth: Option<BitDepthRule>,
    pub channel_layout: Option<ChannelLayoutRule>,
    pub silence: Option<DurationThresholdRule>,
    pub clipping: Option<CountThresholdRule>,
    pub peak: Option<DbThresholdRule>,
    pub true_peak: Option<DbThresholdRule>,
    pub loudness: Option<LoudnessRule>,
    pub phase: Option<PhaseRule>,
    pub dc_offset: Option<DcOffsetRule>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SampleRateRule {
    pub expected: u64,
    #[serde(default)]
    pub severity: Severity,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BitDepthRule {
    pub expected: u64,
    #[serde(default)]
    pub severity: Severity,
}

/// Expected channel layout (count + optional label).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChannelLayoutRule {
    pub channels: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<String>,
    #[serde(default)]
    pub severity: Severity,
}

/// A dB threshold (peak in dBFS, true-peak in dBTP).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct DbThresholdRule {
    /// Maximum permitted level; the unit depends on the rule
    /// (`peak` dBFS, `true_peak` dBTP).
    pub max_db: f64,
    #[serde(default)]
    pub severity: Severity,
}

/// Loudness standard/profile selection ([spec § 8.6]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[derive(Default)]
pub enum LoudnessStandard {
    #[default]
    EbuR128,
    AtscA85,
    // BS.1770-4 measurement used with a custom target.
    Bs1770,
}

impl LoudnessStandard {
    /// The standard's nominal integrated target.
    pub fn default_target_lufs(&self) -> f64 {
        match self {
            LoudnessStandard::EbuR128 => -23.0,
            LoudnessStandard::AtscA85 => -24.0,
            LoudnessStandard::Bs1770 => -18.0,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            LoudnessStandard::EbuR128 => "EBU R128",
            LoudnessStandard::AtscA85 => "ATSC A/85",
            LoudnessStandard::Bs1770 => "ITU-R BS.1770",
        }
    }
}

/// Loudness profile selection ([spec § 8.6]).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LoudnessRule {
    #[serde(default)]
    pub standard: LoudnessStandard,
    pub target_lufs: f64,
    pub tolerance_lu: f64,
    #[serde(default = "loudness_default_severity")]
    pub severity: Severity,
}

fn loudness_default_severity() -> Severity {
    Severity::Error
}

impl Default for LoudnessRule {
    fn default() -> Self {
        Self {
            standard: LoudnessStandard::EbuR128,
            target_lufs: -23.0,
            tolerance_lu: 1.0,
            severity: Severity::Error,
        }
    }
}

/// Minimum phase correlation allowed (negative extremes = out of phase).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PhaseRule {
    #[serde(default = "default_min_phase")]
    pub min_correlation: f64,
    #[serde(default)]
    pub severity: Severity,
}

pub(crate) fn default_min_phase() -> f64 {
    -0.5
}

/// Maximum permitted DC offset as a percentage of full scale.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct DcOffsetRule {
    #[serde(default)]
    pub max_offset_percent: f64,
    #[serde(default)]
    pub severity: Severity,
}

// ---------------------------------------------------------------------------
// Subtitle / voice rules (post-MVP; structure reserved now)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SubtitleRules {
    /// Demanded subtitle track language (e.g. `eng`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Severity used when no subtitle track at all is present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub missing_subtitles: Option<Severity>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct VoiceRules {}

// ---------------------------------------------------------------------------
// Verdict policy
// ---------------------------------------------------------------------------

/// How findings combine into the final verdict ([spec § 6.3], § 10).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Policy {
    /// Findings at or above this severity can force a FAIL verdict
    /// (when their status is Fail).
    #[serde(default = "default_fail_on")]
    pub fail_on: Severity,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            fail_on: Severity::Error,
        }
    }
}

fn default_fail_on() -> Severity {
    Severity::Error
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_defaults_to_error() {
        assert_eq!(Policy::default().fail_on, Severity::Error);
    }

    #[test]
    fn loudness_standard_defaults() {
        assert_eq!(LoudnessStandard::EbuR128.default_target_lufs(), -23.0);
        assert_eq!(LoudnessStandard::AtscA85.default_target_lufs(), -24.0);
    }
}
