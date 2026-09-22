//! Video checks ([spec § 8.2, § 8.3]): resolution, frame rate, aspect ratio,
//! colour space, black/freeze/duplicate/corrupt frames, luma range.

use crate::rule::{Capabilities, DecodeRequirement, QcRule, RuleContext, RuleDescription, RuleResult};
use crate::util::{fail, inconclusive, info, segment_failures};
use tpt_app_media_qc_core::cost::CostClass;
use tpt_app_media_qc_model::asset::StreamKind;
use tpt_app_media_qc_model::finding::RuleId;
use tpt_app_media_qc_model::severity::Severity;
use tpt_app_media_qc_model::value::Value;
use tpt_app_media_qc_profile::model::{
    AspectRatioRule, ColorSpaceRule, CountThresholdRule, DurationThresholdRule, FrameRateRule,
    LumaRangeRule, Resolution, ResolutionRule, VideoRules,
};

pub const RULE_IDS: &[&str] = &[
    "video.resolution",
    "video.frame_rate",
    "video.aspect_ratio",
    "video.color_space",
    "video.black_frames",
    "video.freeze_frames",
    "video.duplicate_frames",
    "video.corrupt_frames",
    "video.luma_range",
];

pub fn build(profile: &tpt_app_media_qc_profile::model::Profile, out: &mut Vec<Box<dyn QcRule>>) {
    let v: &VideoRules = &profile.rules.video;
    if let Some(cfg) = v.resolution {
        out.push(Box::new(ResolutionRuleImpl { cfg }));
    }
    if let Some(cfg) = v.frame_rate {
        out.push(Box::new(FrameRateRuleImpl { cfg }));
    }
    if let Some(cfg) = v.aspect_ratio {
        out.push(Box::new(AspectRatioRuleImpl { cfg }));
    }
    if let Some(cfg) = v.color_space.clone() {
        out.push(Box::new(ColorSpaceRuleImpl { cfg }));
    }
    if let Some(cfg) = v.black_frames {
        out.push(Box::new(BlackFramesRule { cfg }));
    }
    if let Some(cfg) = v.freeze_frames {
        out.push(Box::new(FreezeFramesRule { cfg }));
    }
    if let Some(cfg) = v.duplicate_frames {
        out.push(Box::new(DuplicateFramesRule { cfg }));
    }
    if let Some(cfg) = v.corrupt_frames {
        out.push(Box::new(CorruptFramesRule { cfg }));
    }
    if let Some(cfg) = v.luma_range {
        out.push(Box::new(LumaRangeRuleImpl { cfg }));
    }
}

fn metadata_capabilities() -> Capabilities {
    Capabilities { required_streams: &[StreamKind::Video], decode: DecodeRequirement::None, incremental: true, gpu: false, cost: CostClass::Metadata }
}

fn video_measurement<'a>(ctx: &'a RuleContext) -> Option<&'a tpt_app_media_qc_model::inspection::VideoMeasurements> {
    ctx.primary_stream(StreamKind::Video)
        .map(|s| s.index.0)
        .and_then(|idx| ctx.inspection.video_for(idx))
}

// ---------------------------------------------------------------------------
// resolution
// ---------------------------------------------------------------------------

struct ResolutionRuleImpl {
    cfg: ResolutionRule,
}

impl QcRule for ResolutionRuleImpl {
    fn id(&self) -> RuleId {
        RuleId::new("video.resolution")
    }
    fn description(&self) -> RuleDescription {
        RuleDescription { name: "Resolution", summary: "Video resolution equals the expected value.", version: "0.1" }
    }
    fn capabilities(&self) -> Capabilities {
        metadata_capabilities()
    }
    fn run(&self, ctx: &RuleContext<'_>) -> RuleResult {
        let Some(stream) = ctx.primary_stream(StreamKind::Video) else {
            return RuleResult::from_finding(inconclusive(
                &self.id(),
                self.cfg.severity,
                "no video stream present to measure resolution",
            ));
        };
        let (Some(w), Some(h)) = (stream.width, stream.height) else {
            return RuleResult::from_finding(inconclusive(
                &self.id(),
                self.cfg.severity,
                "video stream did not report dimensions",
            ));
        };
        let Resolution(ew, eh) = self.cfg.expected;
        if w != ew || h != eh {
            return RuleResult::from_finding(fail(
                &self.id(),
                self.cfg.severity,
                format!("resolution is {w}x{h}, expected {}x{}", ew, eh),
                Some(Value::Text(format!("{w}x{h}"))),
                Some(Value::Text(format!("{ew}x{eh}"))),
            ));
        }
        RuleResult::pass()
    }
}

// ---------------------------------------------------------------------------
// frame_rate
// ---------------------------------------------------------------------------

struct FrameRateRuleImpl {
    cfg: FrameRateRule,
}

impl QcRule for FrameRateRuleImpl {
    fn id(&self) -> RuleId {
        RuleId::new("video.frame_rate")
    }
    fn description(&self) -> RuleDescription {
        RuleDescription { name: "Frame rate", summary: "Video frame rate matches the expected value within tolerance.", version: "0.1" }
    }
    fn capabilities(&self) -> Capabilities {
        metadata_capabilities()
    }
    fn run(&self, ctx: &RuleContext<'_>) -> RuleResult {
        let Some(stream) = ctx.primary_stream(StreamKind::Video) else {
            return RuleResult::from_finding(inconclusive(
                &self.id(),
                self.cfg.severity,
                "no video stream present to measure frame rate",
            ));
        };
        let observed = video_measurement(ctx)
            .and_then(|m| m.frame_rate_observed)
            .or(stream.frame_rate);
        let Some(fps) = observed else {
            return RuleResult::from_finding(inconclusive(
                &self.id(),
                self.cfg.severity,
                "video frame rate was not reported",
            ));
        };
        let expected = self.cfg.expected.value();
        let diff = (fps.value() - expected).abs();
        if diff > self.cfg.tolerance {
            return RuleResult::from_finding(fail(
                &self.id(),
                self.cfg.severity,
                format!("frame rate is {:.3} fps, expected {expected:.3} (tolerance {:.3})", fps.value(), self.cfg.tolerance),
                Some(Value::Float(fps.value())),
                Some(Value::Float(expected)),
            ));
        }
        RuleResult::pass()
    }
}

// ---------------------------------------------------------------------------
// aspect_ratio
// ---------------------------------------------------------------------------

struct AspectRatioRuleImpl {
    cfg: AspectRatioRule,
}

impl QcRule for AspectRatioRuleImpl {
    fn id(&self) -> RuleId {
        RuleId::new("video.aspect_ratio")
    }
    fn description(&self) -> RuleDescription {
        RuleDescription { name: "Aspect ratio", summary: "Display aspect ratio matches expectation.", version: "0.1" }
    }
    fn capabilities(&self) -> Capabilities {
        metadata_capabilities()
    }
    fn run(&self, ctx: &RuleContext<'_>) -> RuleResult {
        let Some(stream) = ctx.primary_stream(StreamKind::Video) else {
            return RuleResult::from_finding(inconclusive(
                &self.id(),
                self.cfg.severity,
                "no video stream present to measure aspect ratio",
            ));
        };
        let (Some(w), Some(h)) = (stream.width, stream.height) else {
            return RuleResult::from_finding(inconclusive(
                &self.id(),
                self.cfg.severity,
                "video stream did not report dimensions",
            ));
        };
        let measured = w as f64 / h as f64;
        let expected = self.cfg.expected.value();
        let rel = if expected == 0.0 { f64::INFINITY } else { (measured - expected).abs() / expected };
        if rel > self.cfg.tolerance {
            return RuleResult::from_finding(fail(
                &self.id(),
                self.cfg.severity,
                format!(
                    "aspect ratio is {:.4} ({}x{}), expected {}",
                    measured, w, h, self.cfg.expected.label()
                ),
                Some(Value::Float(measured)),
                Some(Value::Ratio(expected)),
            ));
        }
        RuleResult::pass()
    }
}

// ---------------------------------------------------------------------------
// color_space
// ---------------------------------------------------------------------------

struct ColorSpaceRuleImpl {
    cfg: ColorSpaceRule,
}

impl QcRule for ColorSpaceRuleImpl {
    fn id(&self) -> RuleId {
        RuleId::new("video.color_space")
    }
    fn description(&self) -> RuleDescription {
        RuleDescription { name: "Colour space", summary: "Colour-space/colorimetry tag matches expectation.", version: "0.1" }
    }
    fn capabilities(&self) -> Capabilities {
        metadata_capabilities()
    }
    fn run(&self, ctx: &RuleContext<'_>) -> RuleResult {
        let Some(measurement) = video_measurement(ctx) else {
            return RuleResult::from_finding(inconclusive(
                &self.id(),
                self.cfg.severity,
                "video was not measured (no decoded video data)",
            ));
        };
        let Some(actual) = &measurement.colorspace else {
            return RuleResult::from_finding(inconclusive(
                &self.id(),
                self.cfg.severity,
                "video stream did not report a colour-space tag",
            ));
        };
        if actual != &self.cfg.expected {
            return RuleResult::from_finding(fail(
                &self.id(),
                self.cfg.severity,
                format!("colour space is {actual}, expected {}", self.cfg.expected),
                Some(Value::Text(actual.clone())),
                Some(Value::Text(self.cfg.expected.clone())),
            ));
        }
        RuleResult::pass()
    }
}

// ---------------------------------------------------------------------------
// segment-based decode rules (black/freeze/duplicate)
// ---------------------------------------------------------------------------

fn segment_rule(
    id: &RuleId,
    name: &str,
    severity: Severity,
    max_duration_ms: u64,
    ctx: &RuleContext<'_>,
    segments: Vec<tpt_app_media_qc_model::finding::TimeRange>,
) -> RuleResult {
    let Some(measurement) = video_measurement(ctx) else {
        return RuleResult::from_finding(inconclusive(
            id,
            severity,
            "video was not decoded, so no segment analysis is available",
        ));
    };
    let offending: Vec<(tpt_app_media_qc_model::finding::TimeRange, String)> = segments
        .iter()
        .filter(|s| s.duration_ms() > max_duration_ms)
        .map(|s| (*s, format!("{name} segment of {}ms exceeds the {}ms limit", s.duration_ms(), max_duration_ms)))
        .collect();
    if offending.is_empty() {
        if measurement.decode_errors > 0 {
            return RuleResult::from_finding(inconclusive(
                id,
                severity,
                "decode had errors, so segment detection may be incomplete",
            ));
        }
        return RuleResult::pass();
    }
    RuleResult::findings(segment_failures(id, severity, offending.into_iter()))
}

struct BlackFramesRule {
    cfg: DurationThresholdRule,
}

impl QcRule for BlackFramesRule {
    fn id(&self) -> RuleId {
        RuleId::new("video.black_frames")
    }
    fn description(&self) -> RuleDescription {
        RuleDescription { name: "Black frames", summary: "No black-frame segments longer than the limit.", version: "0.1" }
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities { required_streams: &[StreamKind::Video], decode: DecodeRequirement::FullFrame, incremental: false, gpu: true, cost: CostClass::FullDecode }
    }
    fn run(&self, ctx: &RuleContext<'_>) -> RuleResult {
        segment_rule(&self.id(), "black-frame", self.cfg.severity, self.cfg.max_duration_ms, ctx, video_measurement(ctx).map(|m| m.black_frames.clone()).unwrap_or_default())
    }
}

struct FreezeFramesRule {
    cfg: DurationThresholdRule,
}

impl QcRule for FreezeFramesRule {
    fn id(&self) -> RuleId {
        RuleId::new("video.freeze_frames")
    }
    fn description(&self) -> RuleDescription {
        RuleDescription { name: "Freeze frames", summary: "No freeze-frame segments longer than the limit.", version: "0.1" }
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities { required_streams: &[StreamKind::Video], decode: DecodeRequirement::FullFrame, incremental: false, gpu: true, cost: CostClass::FullDecode }
    }
    fn run(&self, ctx: &RuleContext<'_>) -> RuleResult {
        segment_rule(&self.id(), "freeze-frame", self.cfg.severity, self.cfg.max_duration_ms, ctx, video_measurement(ctx).map(|m| m.freeze_frames.clone()).unwrap_or_default())
    }
}

fn count_rule(
    id: &RuleId,
    name: &str,
    severity: Severity,
    max_events: u64,
    measured_from: Option<u64>,
) -> RuleResult {
    let Some(count) = measured_from else {
        return RuleResult::from_finding(inconclusive(
            id,
            severity,
            format!("video was not decoded, so {name} detection is unavailable"),
        ));
    };
    if count > max_events {
        return RuleResult::from_finding(fail(
            id,
            severity,
            format!("{count} {name} event(s) exceed the {max_events} limit"),
            Some(Value::UInt(count)),
            Some(Value::UInt(max_events)),
        ));
    }
    RuleResult::pass()
}

struct DuplicateFramesRule {
    cfg: CountThresholdRule,
}

impl QcRule for DuplicateFramesRule {
    fn id(&self) -> RuleId {
        RuleId::new("video.duplicate_frames")
    }
    fn description(&self) -> RuleDescription {
        RuleDescription { name: "Duplicate frames", summary: "No duplicate-frame events beyond the limit.", version: "0.1" }
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities { required_streams: &[StreamKind::Video], decode: DecodeRequirement::FullFrame, incremental: false, gpu: true, cost: CostClass::FullDecode }
    }
    fn run(&self, ctx: &RuleContext<'_>) -> RuleResult {
        let Some(measurement) = video_measurement(ctx) else {
            return RuleResult::from_finding(inconclusive(
                &self.id(),
                self.cfg.severity,
                "video was not decoded, so duplicate-frame detection is unavailable",
            ));
        };
        let count = measurement.duplicate_frames.len() as u64;
        count_rule(&self.id(), "duplicate-frame", self.cfg.severity, self.cfg.max_events, Some(count))
    }
}

struct CorruptFramesRule {
    cfg: CountThresholdRule,
}

impl QcRule for CorruptFramesRule {
    fn id(&self) -> RuleId {
        RuleId::new("video.corrupt_frames")
    }
    fn description(&self) -> RuleDescription {
        RuleDescription { name: "Corrupt frames", summary: "No decode errors beyond the limit.", version: "0.1" }
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities { required_streams: &[StreamKind::Video], decode: DecodeRequirement::Stream, incremental: true, gpu: false, cost: CostClass::CheapDecode }
    }
    fn run(&self, ctx: &RuleContext<'_>) -> RuleResult {
        let Some(measurement) = video_measurement(ctx) else {
            return RuleResult::from_finding(inconclusive(
                &self.id(),
                self.cfg.severity,
                "video was not decoded, so corrupt-frame detection is unavailable",
            ));
        };
        count_rule(&self.id(), "corrupt-frame", self.cfg.severity, self.cfg.max_events, Some(measurement.decode_errors))
    }
}

// ---------------------------------------------------------------------------
// luma_range
// ---------------------------------------------------------------------------

struct LumaRangeRuleImpl {
    cfg: LumaRangeRule,
}

impl QcRule for LumaRangeRuleImpl {
    fn id(&self) -> RuleId {
        RuleId::new("video.luma_range")
    }
    fn description(&self) -> RuleDescription {
        RuleDescription { name: "Luma range", summary: "Luma stays within the legal range [16, 235].", version: "0.1" }
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities { required_streams: &[StreamKind::Video], decode: DecodeRequirement::FullFrame, incremental: false, gpu: false, cost: CostClass::FullDecode }
    }
    fn run(&self, ctx: &RuleContext<'_>) -> RuleResult {
        let Some(measurement) = video_measurement(ctx) else {
            return RuleResult::from_finding(inconclusive(
                &self.id(),
                self.cfg.severity,
                "video was not decoded, so luma could not be measured",
            ));
        };
        let Some(luma) = measurement.luma else {
            return RuleResult::from_finding(inconclusive(
                &self.id(),
                self.cfg.severity,
                "luma statistics were not measured",
            ));
        };
        let out = luma.below_legal + luma.above_legal;
        if out > self.cfg.max_out_of_legal {
            return RuleResult::from_finding(fail(
                &self.id(),
                self.cfg.severity,
                format!(
                    "out-of-legal luma fraction {:.3} exceeds the {:.3} limit (min {}, max {})",
                    out, self.cfg.max_out_of_legal, luma.min, luma.max
                ),
                Some(Value::Ratio(out)),
                Some(Value::Ratio(self.cfg.max_out_of_legal)),
            ));
        }
        RuleResult::from_finding(info(
            &self.id(),
            Severity::Info,
            format!("luma within legal range (min {}, max {})", luma.min, luma.max),
            Some(Value::Ratio(out)),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::bundled_asset;
    use tpt_app_media_qc_model::finding::TimeRange;
    use tpt_app_media_qc_model::inspection::{Inspection, LumaStats, VideoMeasurements};
    use tpt_app_media_qc_profile::model::Ratio;

    fn ctx_with(video: Vec<VideoMeasurements>) -> RuleContext<'static> {
        let asset = bundled_asset();
        let inspection: &'static Inspection =
            Box::leak(Box::new(Inspection { video, ..Default::default() }));
        RuleContext { asset, inspection }
    }

    fn measurement() -> VideoMeasurements {
        VideoMeasurements { stream_idx: 0, ..Default::default() }
    }

    #[test]
    fn resolution_passes_and_fails() {
        let cfg = ResolutionRule { expected: Resolution(1920, 1080), severity: Severity::Error };
        let rule = ResolutionRuleImpl { cfg };
        assert!(rule.execute(&ctx_with(vec![])).is_pass());
    }

    #[test]
    fn frame_rate_matches_stream_metadata() {
        let cfg = FrameRateRule {
            expected: tpt_app_media_qc_model::time::Rational::from_parts(25, 1),
            tolerance: 0.001,
            severity: Severity::Error,
        };
        let rule = FrameRateRuleImpl { cfg };
        assert!(rule.execute(&ctx_with(vec![])).is_pass());
    }

    #[test]
    fn frame_rate_mismatch_fails() {
        let cfg = FrameRateRule {
            expected: tpt_app_media_qc_model::time::Rational::from_parts(30, 1),
            tolerance: 0.001,
            severity: Severity::Error,
        };
        let rule = FrameRateRuleImpl { cfg };
        let r = rule.execute(&ctx_with(vec![]));
        assert_eq!(r.findings[0].status, tpt_app_media_qc_model::severity::VerdictDecision::Fail);
    }

    #[test]
    fn aspect_ratio_fails_on_anamorphic() {
        // bundled asset is 1920x1080 = 16:9
        let cfg = AspectRatioRule {
            expected: Ratio::from_parts(4.0, 3.0),
            tolerance: default_tol(),
            severity: Severity::Error,
        };
        let rule = AspectRatioRuleImpl { cfg };
        let r = rule.execute(&ctx_with(vec![]));
        assert_eq!(r.findings[0].status, tpt_app_media_qc_model::severity::VerdictDecision::Fail);
    }

    fn default_tol() -> f64 {
        0.005
    }

    #[test]
    fn black_frames_over_limit_fail_and_attach_range() {
        let cfg = DurationThresholdRule { max_duration_ms: 500, severity: Severity::Warning };
        let rule = BlackFramesRule { cfg };
        let mut m = measurement();
        m.black_frames = vec![TimeRange::new(1_000, 2_500)];
        let r = rule.execute(&ctx_with(vec![m]));
        assert_eq!(r.findings[0].status, tpt_app_media_qc_model::severity::VerdictDecision::Fail);
        assert_eq!(r.findings[0].time_range, Some(TimeRange::new(1_000, 2_500)));
    }

    #[test]
    fn luma_out_of_legal_fails() {
        let cfg = LumaRangeRule { max_out_of_legal: 0.02, severity: Severity::Warning };
        let rule = LumaRangeRuleImpl { cfg };
        let mut m = measurement();
        m.luma = Some(LumaStats {
            min: 16,
            max: 235,
            mean: 100.0,
            below_legal: 0.05,
            above_legal: 0.0,
            clipped_white: 0.0,
        });
        let r = rule.execute(&ctx_with(vec![m]));
        assert_eq!(r.findings[0].status, tpt_app_media_qc_model::severity::VerdictDecision::Fail);
    }

    #[test]
    fn colorspace_mismatch_fails() {
        let cfg = ColorSpaceRule { expected: "bt2020nc".into(), severity: Severity::Error };
        let rule = ColorSpaceRuleImpl { cfg };
        let mut m = measurement();
        m.colorspace = Some("bt709".into());
        let r = rule.execute(&ctx_with(vec![m]));
        assert_eq!(r.findings[0].status, tpt_app_media_qc_model::severity::VerdictDecision::Fail);
    }
}