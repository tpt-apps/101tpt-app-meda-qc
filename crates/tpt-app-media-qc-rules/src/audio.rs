//! Audio checks ([spec § 8.4–§ 8.8]): sample rate, bit depth, channel
//! layout, silence, clipping, peak/true-peak, loudness, phase, DC offset.

use crate::rule::{
    Capabilities, DecodeRequirement, QcRule, RuleContext, RuleDescription, RuleResult,
};
use crate::util::{fail, inconclusive, info, segment_failures};
use tpt_app_media_qc_core::cost::CostClass;
use tpt_app_media_qc_model::asset::StreamKind;
use tpt_app_media_qc_model::finding::RuleId;
use tpt_app_media_qc_model::severity::Severity;
use tpt_app_media_qc_model::value::Value;
use tpt_app_media_qc_profile::model::{
    AudioRules, BitDepthRule, ChannelLayoutRule, CountThresholdRule, DbThresholdRule, DcOffsetRule,
    DurationThresholdRule, LoudnessRule, PhaseRule, SampleRateRule,
};

pub const RULE_IDS: &[&str] = &[
    "audio.sample_rate",
    "audio.bit_depth",
    "audio.channel_layout",
    "audio.silence",
    "audio.clipping",
    "audio.peak",
    "audio.true_peak",
    "audio.loudness",
    "audio.phase",
    "audio.dc_offset",
];

pub fn build(profile: &tpt_app_media_qc_profile::model::Profile, out: &mut Vec<Box<dyn QcRule>>) {
    let a: &AudioRules = &profile.rules.audio;
    if let Some(cfg) = a.sample_rate {
        out.push(Box::new(SampleRateRuleImpl { cfg }));
    }
    if let Some(cfg) = a.bit_depth {
        out.push(Box::new(BitDepthRuleImpl { cfg }));
    }
    if let Some(cfg) = a.channel_layout.clone() {
        out.push(Box::new(ChannelLayoutRuleImpl { cfg }));
    }
    if let Some(cfg) = a.silence {
        out.push(Box::new(SilenceRule { cfg }));
    }
    if let Some(cfg) = a.clipping {
        out.push(Box::new(ClippingRule { cfg }));
    }
    if let Some(cfg) = a.peak {
        out.push(Box::new(PeakRule { cfg }));
    }
    if let Some(cfg) = a.true_peak {
        out.push(Box::new(TruePeakRule { cfg }));
    }
    if let Some(cfg) = a.loudness {
        out.push(Box::new(LoudnessRuleImpl { cfg }));
    }
    if let Some(cfg) = a.phase {
        out.push(Box::new(PhaseRuleImpl { cfg }));
    }
    if let Some(cfg) = a.dc_offset {
        out.push(Box::new(DcOffsetRuleImpl { cfg }));
    }
}

fn audio_measurement<'a>(
    ctx: &'a RuleContext,
) -> Option<&'a tpt_app_media_qc_model::inspection::AudioMeasurements> {
    ctx.primary_stream(StreamKind::Audio)
        .map(|s| s.index.0)
        .and_then(|idx| ctx.inspection.audio_for(idx))
        .filter(|measurement| measurement.decoded_frame_count.unwrap_or_default() > 0)
}

// ---------------------------------------------------------------------------
// sample_rate
// ---------------------------------------------------------------------------

struct SampleRateRuleImpl {
    cfg: SampleRateRule,
}

impl QcRule for SampleRateRuleImpl {
    fn id(&self) -> RuleId {
        RuleId::new("audio.sample_rate")
    }
    fn description(&self) -> RuleDescription {
        RuleDescription {
            name: "Sample rate",
            summary: "Audio sample rate matches the expected value.",
            version: "0.1",
        }
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            required_streams: &[StreamKind::Audio],
            decode: DecodeRequirement::None,
            incremental: true,
            gpu: false,
            cost: CostClass::Metadata,
        }
    }
    fn run(&self, ctx: &RuleContext<'_>) -> RuleResult {
        let Some(stream) = ctx.primary_stream(StreamKind::Audio) else {
            return RuleResult::from_finding(inconclusive(
                &self.id(),
                self.cfg.severity,
                "no audio stream present to measure sample rate",
            ));
        };
        let Some(measured) = stream.sample_rate else {
            return RuleResult::from_finding(inconclusive(
                &self.id(),
                self.cfg.severity,
                "audio stream did not report a sample rate",
            ));
        };
        if measured != self.cfg.expected {
            return RuleResult::from_finding(fail(
                &self.id(),
                self.cfg.severity,
                format!(
                    "sample rate is {measured} Hz, expected {} Hz",
                    self.cfg.expected
                ),
                Some(Value::UInt(measured)),
                Some(Value::UInt(self.cfg.expected)),
            ));
        }
        RuleResult::pass()
    }
}

// ---------------------------------------------------------------------------
// bit_depth
// ---------------------------------------------------------------------------

struct BitDepthRuleImpl {
    cfg: BitDepthRule,
}

impl QcRule for BitDepthRuleImpl {
    fn id(&self) -> RuleId {
        RuleId::new("audio.bit_depth")
    }
    fn description(&self) -> RuleDescription {
        RuleDescription {
            name: "Bit depth",
            summary: "Audio bit depth matches the expected value.",
            version: "0.1",
        }
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            required_streams: &[StreamKind::Audio],
            decode: DecodeRequirement::None,
            incremental: true,
            gpu: false,
            cost: CostClass::Metadata,
        }
    }
    fn run(&self, ctx: &RuleContext<'_>) -> RuleResult {
        let Some(stream) = ctx.primary_stream(StreamKind::Audio) else {
            return RuleResult::from_finding(inconclusive(
                &self.id(),
                self.cfg.severity,
                "no audio stream present to measure bit depth",
            ));
        };
        let Some(measured) = stream.bit_depth else {
            return RuleResult::from_finding(inconclusive(
                &self.id(),
                self.cfg.severity,
                "audio stream did not report a bit depth",
            ));
        };
        if measured != self.cfg.expected {
            return RuleResult::from_finding(fail(
                &self.id(),
                self.cfg.severity,
                format!("bit depth is {measured}, expected {}", self.cfg.expected),
                Some(Value::UInt(measured)),
                Some(Value::UInt(self.cfg.expected)),
            ));
        }
        RuleResult::pass()
    }
}

// ---------------------------------------------------------------------------
// channel_layout
// ---------------------------------------------------------------------------

struct ChannelLayoutRuleImpl {
    cfg: ChannelLayoutRule,
}

impl QcRule for ChannelLayoutRuleImpl {
    fn id(&self) -> RuleId {
        RuleId::new("audio.channel_layout")
    }
    fn description(&self) -> RuleDescription {
        RuleDescription {
            name: "Channel layout",
            summary: "Channel count (and optionally layout) matches the expected value.",
            version: "0.1",
        }
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            required_streams: &[StreamKind::Audio],
            decode: DecodeRequirement::None,
            incremental: true,
            gpu: false,
            cost: CostClass::Metadata,
        }
    }
    fn run(&self, ctx: &RuleContext<'_>) -> RuleResult {
        let Some(stream) = ctx.primary_stream(StreamKind::Audio) else {
            return RuleResult::from_finding(inconclusive(
                &self.id(),
                self.cfg.severity,
                "no audio stream present to measure channel layout",
            ));
        };
        let Some(channels) = stream.channels else {
            return RuleResult::from_finding(inconclusive(
                &self.id(),
                self.cfg.severity,
                "audio stream did not report a channel count",
            ));
        };
        if channels != self.cfg.channels {
            return RuleResult::from_finding(fail(
                &self.id(),
                self.cfg.severity,
                format!(
                    "channel count is {channels}, expected {}",
                    self.cfg.channels
                ),
                Some(Value::UInt(channels)),
                Some(Value::UInt(self.cfg.channels)),
            ));
        }
        if let (Some(expected_layout), Some(actual)) = (&self.cfg.layout, &stream.channel_layout) {
            if actual != expected_layout {
                return RuleResult::from_finding(fail(
                    &self.id(),
                    self.cfg.severity,
                    format!("channel layout is '{actual}', expected '{expected_layout}'"),
                    Some(Value::Text(actual.clone())),
                    Some(Value::Text(expected_layout.clone())),
                ));
            }
        }
        RuleResult::pass()
    }
}

// ---------------------------------------------------------------------------
// silence (segment rule)
// ---------------------------------------------------------------------------

struct SilenceRule {
    cfg: DurationThresholdRule,
}

impl QcRule for SilenceRule {
    fn id(&self) -> RuleId {
        RuleId::new("audio.silence")
    }
    fn description(&self) -> RuleDescription {
        RuleDescription {
            name: "Silence",
            summary: "No silence segments longer than the limit.",
            version: "0.1",
        }
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            required_streams: &[StreamKind::Audio],
            decode: DecodeRequirement::Stream,
            incremental: true,
            gpu: false,
            cost: CostClass::FullDecode,
        }
    }
    fn run(&self, ctx: &RuleContext<'_>) -> RuleResult {
        let Some(measurement) = audio_measurement(ctx) else {
            return RuleResult::from_finding(inconclusive(
                &self.id(),
                self.cfg.severity,
                "audio was not decoded, so silence detection is unavailable",
            ));
        };
        let offending: Vec<(tpt_app_media_qc_model::finding::TimeRange, String)> = measurement
            .silence
            .iter()
            .filter(|s| s.duration_ms() > self.cfg.max_duration_ms)
            .map(|s| {
                (
                    *s,
                    format!(
                        "silence of {}ms exceeds the {}ms limit",
                        s.duration_ms(),
                        self.cfg.max_duration_ms
                    ),
                )
            })
            .collect();
        if !offending.is_empty() {
            return RuleResult::findings(segment_failures(
                &self.id(),
                self.cfg.severity,
                offending.into_iter(),
            ));
        }
        if measurement.silence_truncated {
            return RuleResult::from_finding(inconclusive(
                &self.id(),
                self.cfg.severity,
                "silence range limit reached, so silence detection is incomplete",
            ));
        }
        if measurement.decode_errors > 0 {
            return RuleResult::from_finding(inconclusive(
                &self.id(),
                self.cfg.severity,
                "audio decode had errors, so silence detection may be incomplete",
            ));
        }
        RuleResult::pass()
    }
}

// ---------------------------------------------------------------------------
// clipping (count rule)
// ---------------------------------------------------------------------------

struct ClippingRule {
    cfg: CountThresholdRule,
}

impl QcRule for ClippingRule {
    fn id(&self) -> RuleId {
        RuleId::new("audio.clipping")
    }
    fn description(&self) -> RuleDescription {
        RuleDescription {
            name: "Clipping",
            summary: "No clipping events beyond the limit.",
            version: "0.1",
        }
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            required_streams: &[StreamKind::Audio],
            decode: DecodeRequirement::Stream,
            incremental: true,
            gpu: false,
            cost: CostClass::FullDecode,
        }
    }
    fn run(&self, ctx: &RuleContext<'_>) -> RuleResult {
        let Some(measurement) = audio_measurement(ctx) else {
            return RuleResult::from_finding(inconclusive(
                &self.id(),
                self.cfg.severity,
                "audio was not decoded, so clipping detection is unavailable",
            ));
        };
        if measurement.clipping_events > self.cfg.max_events {
            return RuleResult::from_finding(fail(
                &self.id(),
                self.cfg.severity,
                format!(
                    "{} clipping event(s) exceed the {} limit",
                    measurement.clipping_events, self.cfg.max_events
                ),
                Some(Value::UInt(measurement.clipping_events)),
                Some(Value::UInt(self.cfg.max_events)),
            ));
        }
        if measurement.decode_errors > 0 {
            return RuleResult::from_finding(inconclusive(
                &self.id(),
                self.cfg.severity,
                "audio decode had errors, so clipping detection may be incomplete",
            ));
        }
        RuleResult::pass()
    }
}

// ---------------------------------------------------------------------------
// dB thresholds (peak / true_peak)
// ---------------------------------------------------------------------------

fn db_rule(
    id: &RuleId,
    name: &str,
    unit: &str,
    severity: Severity,
    max_db: f64,
    measured: Option<f64>,
    decode_errors: u64,
) -> RuleResult {
    let Some(measured) = measured else {
        return RuleResult::from_finding(inconclusive(
            id,
            severity,
            format!("{name} was not measured"),
        ));
    };
    if measured > max_db {
        return RuleResult::from_finding(fail(
            id,
            severity,
            format!("{name} is {measured:.2} {unit}, exceeding the {max_db:.2} {unit} limit"),
            Some(Value::Ratio(measured)),
            Some(Value::Ratio(max_db)),
        ));
    }
    if decode_errors > 0 {
        return RuleResult::from_finding(inconclusive(
            id,
            severity,
            format!("{name} may be inaccurate because audio decode had errors"),
        ));
    }
    RuleResult::from_finding(info(
        id,
        Severity::Info,
        format!("{name} {measured:.2} {unit}"),
        Some(Value::Ratio(measured)),
    ))
}

struct PeakRule {
    cfg: DbThresholdRule,
}

impl QcRule for PeakRule {
    fn id(&self) -> RuleId {
        RuleId::new("audio.peak")
    }
    fn description(&self) -> RuleDescription {
        RuleDescription {
            name: "Peak level",
            summary: "Sample peak stays below the configured dBFS limit.",
            version: "0.1",
        }
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            required_streams: &[StreamKind::Audio],
            decode: DecodeRequirement::Stream,
            incremental: true,
            gpu: false,
            cost: CostClass::CheapDecode,
        }
    }
    fn run(&self, ctx: &RuleContext<'_>) -> RuleResult {
        let (measured, decode_errors) =
            audio_measurement(ctx).map_or((None, 0_u64), |m| (m.peak_db, m.decode_errors));
        db_rule(
            &self.id(),
            "peak level",
            "dBFS",
            self.cfg.severity,
            self.cfg.max_db,
            measured,
            decode_errors,
        )
    }
}

struct TruePeakRule {
    cfg: DbThresholdRule,
}

impl QcRule for TruePeakRule {
    fn id(&self) -> RuleId {
        RuleId::new("audio.true_peak")
    }
    fn description(&self) -> RuleDescription {
        RuleDescription {
            name: "True peak",
            summary: "True-peak stays below the configured dBTP limit.",
            version: "0.1",
        }
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            required_streams: &[StreamKind::Audio],
            decode: DecodeRequirement::Stream,
            incremental: false,
            gpu: false,
            cost: CostClass::ExpensiveAnalysis,
        }
    }
    fn run(&self, ctx: &RuleContext<'_>) -> RuleResult {
        let (measured, decode_errors) =
            audio_measurement(ctx).map_or((None, 0_u64), |m| (m.true_peak_db, m.decode_errors));
        db_rule(
            &self.id(),
            "true-peak",
            "dBTP",
            self.cfg.severity,
            self.cfg.max_db,
            measured,
            decode_errors,
        )
    }
}

// ---------------------------------------------------------------------------
// loudness
// ---------------------------------------------------------------------------

struct LoudnessRuleImpl {
    cfg: LoudnessRule,
}

impl QcRule for LoudnessRuleImpl {
    fn id(&self) -> RuleId {
        RuleId::new("audio.loudness")
    }
    fn description(&self) -> RuleDescription {
        RuleDescription {
            name: "Loudness",
            summary: "Integrated loudness matches the target within tolerance.",
            version: "0.1",
        }
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            required_streams: &[StreamKind::Audio],
            decode: DecodeRequirement::Stream,
            incremental: false,
            gpu: false,
            cost: CostClass::ExpensiveAnalysis,
        }
    }
    fn run(&self, ctx: &RuleContext<'_>) -> RuleResult {
        let Some(measurement) = audio_measurement(ctx) else {
            return RuleResult::from_finding(inconclusive(
                &self.id(),
                self.cfg.severity,
                "audio was not decoded, so loudness could not be measured",
            ));
        };
        let Some(lufs) = measurement.loudness_lufs else {
            return RuleResult::from_finding(inconclusive(
                &self.id(),
                self.cfg.severity,
                "loudness was not measured for this stream",
            ));
        };
        let std = format!("{:?}", self.cfg.standard).to_lowercase();
        let delta = (lufs - self.cfg.target_lufs).abs();
        if delta > self.cfg.tolerance_lu {
            return RuleResult::from_finding(fail(
                &self.id(),
                self.cfg.severity,
                format!(
                    "integrated loudness is {lufs:.1} LUFS ({std}), target {} ±{} LU",
                    self.cfg.target_lufs, self.cfg.tolerance_lu
                ),
                Some(Value::Ratio(lufs)),
                Some(Value::Ratio(self.cfg.target_lufs)),
            ));
        }
        if measurement.decode_errors > 0 {
            return RuleResult::from_finding(inconclusive(
                &self.id(),
                self.cfg.severity,
                "audio decode had errors, so loudness may be inaccurate",
            ));
        }
        RuleResult::from_finding(info(
            &self.id(),
            Severity::Info,
            format!("loudness {lufs:.1} LUFS within target"),
            Some(Value::Ratio(lufs)),
        ))
    }
}

// ---------------------------------------------------------------------------
// phase
// ---------------------------------------------------------------------------

struct PhaseRuleImpl {
    cfg: PhaseRule,
}

impl QcRule for PhaseRuleImpl {
    fn id(&self) -> RuleId {
        RuleId::new("audio.phase")
    }
    fn description(&self) -> RuleDescription {
        RuleDescription { name: "Phase correlation", summary: "Phase correlation stays above the minimum (no polarity reversals/out-of-phase content).", version: "0.1" }
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            required_streams: &[StreamKind::Audio],
            decode: DecodeRequirement::Stream,
            incremental: false,
            gpu: false,
            cost: CostClass::ExpensiveAnalysis,
        }
    }
    fn run(&self, ctx: &RuleContext<'_>) -> RuleResult {
        let Some(measurement) = audio_measurement(ctx) else {
            return RuleResult::from_finding(inconclusive(
                &self.id(),
                self.cfg.severity,
                "audio was not decoded, so phase could not be measured",
            ));
        };
        let Some(corr) = measurement.phase_correlation else {
            return RuleResult::from_finding(inconclusive(
                &self.id(),
                self.cfg.severity,
                "phase correlation was not measured",
            ));
        };
        if corr < self.cfg.min_correlation {
            return RuleResult::from_finding(fail(
                &self.id(),
                self.cfg.severity,
                format!(
                    "phase correlation {corr:.3} is below the {:.3} minimum",
                    self.cfg.min_correlation
                ),
                Some(Value::Ratio(corr)),
                Some(Value::Ratio(self.cfg.min_correlation)),
            ));
        }
        if measurement.decode_errors > 0 {
            return RuleResult::from_finding(inconclusive(
                &self.id(),
                self.cfg.severity,
                "audio decode had errors, so phase may be inaccurate",
            ));
        }
        RuleResult::pass()
    }
}

// ---------------------------------------------------------------------------
// dc_offset
// ---------------------------------------------------------------------------

struct DcOffsetRuleImpl {
    cfg: DcOffsetRule,
}

impl QcRule for DcOffsetRuleImpl {
    fn id(&self) -> RuleId {
        RuleId::new("audio.dc_offset")
    }
    fn description(&self) -> RuleDescription {
        RuleDescription {
            name: "DC offset",
            summary: "DC offset stays within the permitted percentage of full scale.",
            version: "0.1",
        }
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            required_streams: &[StreamKind::Audio],
            decode: DecodeRequirement::Stream,
            incremental: true,
            gpu: false,
            cost: CostClass::CheapDecode,
        }
    }
    fn run(&self, ctx: &RuleContext<'_>) -> RuleResult {
        let Some(measurement) = audio_measurement(ctx) else {
            return RuleResult::from_finding(inconclusive(
                &self.id(),
                self.cfg.severity,
                "audio was not decoded, so DC offset could not be measured",
            ));
        };
        let Some(offset) = measurement.dc_offset_percent else {
            return RuleResult::from_finding(inconclusive(
                &self.id(),
                self.cfg.severity,
                "DC offset was not measured",
            ));
        };
        if offset.abs() > self.cfg.max_offset_percent {
            return RuleResult::from_finding(fail(
                &self.id(),
                self.cfg.severity,
                format!(
                    "DC offset {offset:+.2}% exceeds the {:.2}% limit",
                    self.cfg.max_offset_percent
                ),
                Some(Value::Ratio(offset)),
                Some(Value::Ratio(self.cfg.max_offset_percent)),
            ));
        }
        if measurement.decode_errors > 0 {
            return RuleResult::from_finding(inconclusive(
                &self.id(),
                self.cfg.severity,
                "audio decode had errors, so DC offset may be inaccurate",
            ));
        }
        RuleResult::pass()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::bundled_asset;
    use tpt_app_media_qc_model::inspection::{AudioMeasurements, Inspection};
    use tpt_app_media_qc_model::severity::VerdictDecision as Status;

    fn ctx_with(audio: Vec<AudioMeasurements>) -> RuleContext<'static> {
        let asset = bundled_asset();
        let inspection: &'static Inspection = Box::leak(Box::new(Inspection {
            audio,
            ..Default::default()
        }));
        RuleContext { asset, inspection }
    }

    fn measurement() -> AudioMeasurements {
        AudioMeasurements {
            stream_idx: 1,
            decoded_frame_count: Some(480),
            ..Default::default()
        }
    }

    #[test]
    fn sample_rate_matches_bundled() {
        let rule = SampleRateRuleImpl {
            cfg: SampleRateRule {
                expected: 48_000,
                severity: Severity::Error,
            },
        };
        assert!(rule.execute(&ctx_with(vec![])).is_pass());
    }

    #[test]
    fn sample_rate_mismatch_fails() {
        let rule = SampleRateRuleImpl {
            cfg: SampleRateRule {
                expected: 44_100,
                severity: Severity::Error,
            },
        };
        let r = rule.execute(&ctx_with(vec![]));
        assert_eq!(r.findings[0].status, Status::Fail);
    }

    #[test]
    fn channel_layout_bundled_is_stereo() {
        let rule = ChannelLayoutRuleImpl {
            cfg: ChannelLayoutRule {
                channels: 2,
                layout: Some("stereo".into()),
                severity: Severity::Error,
            },
        };
        assert!(rule.execute(&ctx_with(vec![])).is_pass());
    }

    #[test]
    fn silence_over_limit_fails() {
        use tpt_app_media_qc_model::finding::TimeRange;
        let rule = SilenceRule {
            cfg: DurationThresholdRule {
                max_duration_ms: 1000,
                severity: Severity::Warning,
            },
        };
        let mut m = measurement();
        m.silence = vec![TimeRange::new(5_000, 9_000)];
        let r = rule.execute(&ctx_with(vec![m]));
        assert_eq!(r.findings[0].status, Status::Fail);
        assert_eq!(r.findings[0].time_range, Some(TimeRange::new(5_000, 9_000)));
    }

    #[test]
    fn silence_rule_is_inconclusive_when_range_collection_was_truncated() {
        let rule = SilenceRule {
            cfg: DurationThresholdRule {
                max_duration_ms: 1_000,
                severity: Severity::Warning,
            },
        };
        let mut m = measurement();
        m.silence_truncated = true;
        let r = rule.execute(&ctx_with(vec![m]));
        assert_eq!(r.findings[0].status, Status::Inconclusive);
    }

    #[test]
    fn peak_above_limit_fails() {
        let rule = PeakRule {
            cfg: DbThresholdRule {
                max_db: -1.0,
                severity: Severity::Error,
            },
        };
        let mut m = measurement();
        m.peak_db = Some(-0.3);
        let r = rule.execute(&ctx_with(vec![m]));
        assert_eq!(r.findings[0].status, Status::Fail);
    }

    #[test]
    fn loudness_off_target_fails() {
        let rule = LoudnessRuleImpl {
            cfg: LoudnessRule {
                standard: tpt_app_media_qc_profile::model::LoudnessStandard::EbuR128,
                target_lufs: -23.0,
                tolerance_lu: 1.0,
                severity: Severity::Error,
            },
        };
        let mut m = measurement();
        m.loudness_lufs = Some(-18.0);
        let r = rule.execute(&ctx_with(vec![m]));
        assert_eq!(r.findings[0].status, Status::Fail);
    }

    #[test]
    fn dc_offset_over_limit_fails() {
        let rule = DcOffsetRuleImpl {
            cfg: DcOffsetRule {
                max_offset_percent: 1.0,
                severity: Severity::Warning,
            },
        };
        let mut m = measurement();
        m.dc_offset_percent = Some(3.2);
        let r = rule.execute(&ctx_with(vec![m]));
        assert_eq!(r.findings[0].status, Status::Fail);
    }

    #[test]
    fn decode_errors_make_clean_audio_results_inconclusive() {
        let mut m = measurement();
        m.decode_errors = 1;
        m.peak_db = Some(-3.0);
        m.loudness_lufs = Some(-23.0);
        m.phase_correlation = Some(0.9);
        m.dc_offset_percent = Some(0.0);

        let peak = PeakRule {
            cfg: DbThresholdRule {
                max_db: 0.0,
                severity: Severity::Error,
            },
        };
        assert_eq!(
            peak.execute(&ctx_with(vec![m.clone()])).findings[0].status,
            Status::Inconclusive
        );

        let clipping = ClippingRule {
            cfg: CountThresholdRule {
                max_events: 0,
                severity: Severity::Error,
            },
        };
        assert_eq!(
            clipping.execute(&ctx_with(vec![m.clone()])).findings[0].status,
            Status::Inconclusive
        );

        let loudness = LoudnessRuleImpl {
            cfg: LoudnessRule {
                standard: tpt_app_media_qc_profile::model::LoudnessStandard::EbuR128,
                target_lufs: -23.0,
                tolerance_lu: 1.0,
                severity: Severity::Error,
            },
        };
        assert_eq!(
            loudness.execute(&ctx_with(vec![m.clone()])).findings[0].status,
            Status::Inconclusive
        );

        let phase = PhaseRuleImpl {
            cfg: PhaseRule {
                min_correlation: -0.5,
                severity: Severity::Error,
            },
        };
        assert_eq!(
            phase.execute(&ctx_with(vec![m.clone()])).findings[0].status,
            Status::Inconclusive
        );

        let dc_offset = DcOffsetRuleImpl {
            cfg: DcOffsetRule {
                max_offset_percent: 1.0,
                severity: Severity::Warning,
            },
        };
        assert_eq!(
            dc_offset.execute(&ctx_with(vec![m])).findings[0].status,
            Status::Inconclusive
        );
    }

    #[test]
    fn known_silence_failure_survives_range_truncation() {
        use tpt_app_media_qc_model::finding::TimeRange;
        let rule = SilenceRule {
            cfg: DurationThresholdRule {
                max_duration_ms: 1_000,
                severity: Severity::Warning,
            },
        };
        let mut m = measurement();
        m.silence = vec![TimeRange::new(0, 2_000)];
        m.silence_truncated = true;
        let r = rule.execute(&ctx_with(vec![m]));
        assert_eq!(r.findings[0].status, Status::Fail);
    }
}
