//! Video checks ([spec § 8.2, § 8.3]): resolution, frame rate, aspect ratio,
//! colour space, black/freeze/duplicate/corrupt frames, luma range.

use crate::rule::{
    Capabilities, DecodeRequirement, QcRule, RuleContext, RuleDescription, RuleResult,
};
use crate::util::{fail, inconclusive, info, segment_failures};
use tpt_app_media_qc_core::cost::CostClass;
use tpt_app_media_qc_model::asset::{FieldOrder, StreamKind};
use tpt_app_media_qc_model::finding::RuleId;
use tpt_app_media_qc_model::severity::Severity;
use tpt_app_media_qc_model::value::Value;
use tpt_app_media_qc_profile::model::{
    AspectRatioRule, ColorSpaceRule, CountThresholdRule, DurationThresholdRule,
    FieldOrderExpectation, FrameRateRule, LumaRangeRule, PhotosensitivityRule, Resolution,
    ResolutionRule, ScanExpectation, ScanFormatRule, VideoRules,
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
    "video.scan_format",
    "video.photosensitivity",
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
    if let Some(cfg) = v.scan_format {
        out.push(Box::new(ScanFormatRuleImpl { cfg }));
    }
    if let Some(cfg) = v.photosensitivity {
        out.push(Box::new(PhotosensitivityRuleImpl { cfg }));
    }
}

fn metadata_capabilities() -> Capabilities {
    Capabilities {
        required_streams: &[StreamKind::Video],
        decode: DecodeRequirement::None,
        incremental: true,
        gpu: false,
        cost: CostClass::Metadata,
    }
}

fn video_measurement<'a>(
    ctx: &'a RuleContext,
) -> Option<&'a tpt_app_media_qc_model::inspection::VideoMeasurements> {
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
        RuleDescription {
            name: "Resolution",
            summary: "Video resolution equals the expected value.",
            version: "0.1",
        }
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
        RuleDescription {
            name: "Frame rate",
            summary: "Video frame rate matches the expected value within tolerance.",
            version: "0.1",
        }
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
                format!(
                    "frame rate is {:.3} fps, expected {expected:.3} (tolerance {:.3})",
                    fps.value(),
                    self.cfg.tolerance
                ),
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
        RuleDescription {
            name: "Aspect ratio",
            summary: "Display aspect ratio matches expectation.",
            version: "0.1",
        }
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
        let rel = if expected == 0.0 {
            f64::INFINITY
        } else {
            (measured - expected).abs() / expected
        };
        if rel > self.cfg.tolerance {
            return RuleResult::from_finding(fail(
                &self.id(),
                self.cfg.severity,
                format!(
                    "aspect ratio is {:.4} ({}x{}), expected {}",
                    measured,
                    w,
                    h,
                    self.cfg.expected.label()
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
        RuleDescription {
            name: "Colour space",
            summary: "Colour-space/colorimetry tag matches expectation.",
            version: "0.1",
        }
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
// scan_format (interlacement / field order)
// ---------------------------------------------------------------------------

struct ScanFormatRuleImpl {
    cfg: ScanFormatRule,
}

impl QcRule for ScanFormatRuleImpl {
    fn id(&self) -> RuleId {
        RuleId::new("video.scan_format")
    }
    fn description(&self) -> RuleDescription {
        RuleDescription {
            name: "Scan format",
            summary: "Scanning order (progressive/interlaced) and field order match the delivery expectation.",
            version: "0.1",
        }
    }
    fn capabilities(&self) -> Capabilities {
        metadata_capabilities()
    }
    fn run(&self, ctx: &RuleContext<'_>) -> RuleResult {
        let Some(stream) = ctx.primary_stream(StreamKind::Video) else {
            return RuleResult::from_finding(inconclusive(
                &self.id(),
                self.cfg.severity,
                "no video stream present to measure scanning format",
            ));
        };
        let observed = video_measurement(ctx)
            .and_then(|m| m.field_order)
            .or(stream.field_order);
        let Some(measured) = observed else {
            return RuleResult::from_finding(inconclusive(
                &self.id(),
                self.cfg.severity,
                "video stream did not report a scanning order",
            ));
        };

        // Scan-structure expectation first. An unsignalled field order is
        // common on progressive web deliverables and carries no evidence of
        // interlacement, so it cannot disprove a `progressive` expectation —
        // it is reported Inconclusive instead (spec §3.4). For an `interlaced`
        // delivery the signal must be present, so `unknown` is a defect.
        match self.cfg.scan {
            ScanExpectation::Progressive => {
                if measured.is_interlaced() {
                    return RuleResult::from_finding(fail(
                        &self.id(),
                        self.cfg.severity,
                        format!(
                            "scanning order is {}, expected progressive",
                            measured.as_str()
                        ),
                        Some(Value::Text(measured.as_str().into())),
                        Some(Value::Text("progressive".into())),
                    ));
                }
                if measured == FieldOrder::Unknown {
                    return RuleResult::from_finding(inconclusive(
                        &self.id(),
                        self.cfg.severity,
                        "field order not signalled; cannot confirm progressive scan",
                    ));
                }
            }
            ScanExpectation::Interlaced => {
                if measured == FieldOrder::Progressive {
                    return RuleResult::from_finding(fail(
                        &self.id(),
                        self.cfg.severity,
                        "video is progressive, expected interlaced",
                        Some(Value::Text("progressive".into())),
                        Some(Value::Text("interlaced".into())),
                    ));
                }
                if measured == FieldOrder::Unknown {
                    return RuleResult::from_finding(fail(
                        &self.id(),
                        self.cfg.severity,
                        "field order not signalled; interlaced delivery requires a signalled field order",
                        Some(Value::Text("unknown".into())),
                        Some(Value::Text("interlaced".into())),
                    ));
                }
            }
            ScanExpectation::Any => {}
        }

        // Field-order expectation. Only interlaced material has fields, so a
        // pinned order on progressive video is a configuration conflict, and
        // an unsignalled order cannot satisfy the pin.
        if let FieldOrderExpectation::TopFieldFirst | FieldOrderExpectation::BottomFieldFirst =
            self.cfg.field_order
        {
            let expected = match self.cfg.field_order {
                FieldOrderExpectation::TopFieldFirst => FieldOrder::TopFieldFirst,
                _ => FieldOrder::BottomFieldFirst,
            };
            if measured == FieldOrder::Progressive {
                return RuleResult::from_finding(fail(
                    &self.id(),
                    self.cfg.severity,
                    format!(
                        "video is progressive; field-order expectation {} requires interlaced video",
                        expected.as_str()
                    ),
                    Some(Value::Text("progressive".into())),
                    Some(Value::Text(expected.as_str().into())),
                ));
            }
            if measured != expected {
                return RuleResult::from_finding(fail(
                    &self.id(),
                    self.cfg.severity,
                    format!(
                        "field display order is {}, expected {}",
                        measured.as_str(),
                        expected.as_str()
                    ),
                    Some(Value::Text(measured.as_str().into())),
                    Some(Value::Text(expected.as_str().into())),
                ));
            }
        }

        RuleResult::from_finding(info(
            &self.id(),
            Severity::Info,
            format!("scanning order is {}", measured.as_str()),
            Some(Value::Text(measured.as_str().into())),
        ))
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
    if measurement.decoded_frame_count.unwrap_or(0) == 0 {
        return RuleResult::from_finding(inconclusive(
            id,
            severity,
            "video decode produced no frames, so segment analysis is unavailable",
        ));
    }
    let offending: Vec<(tpt_app_media_qc_model::finding::TimeRange, String)> = segments
        .iter()
        .filter(|s| s.duration_ms() > max_duration_ms)
        .map(|s| {
            (
                *s,
                format!(
                    "{name} segment of {}ms exceeds the {}ms limit",
                    s.duration_ms(),
                    max_duration_ms
                ),
            )
        })
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
        RuleDescription {
            name: "Black frames",
            summary: "No black-frame segments longer than the limit.",
            version: "0.1",
        }
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            required_streams: &[StreamKind::Video],
            decode: DecodeRequirement::FullFrame,
            incremental: false,
            gpu: true,
            cost: CostClass::FullDecode,
        }
    }
    fn run(&self, ctx: &RuleContext<'_>) -> RuleResult {
        segment_rule(
            &self.id(),
            "black-frame",
            self.cfg.severity,
            self.cfg.max_duration_ms,
            ctx,
            video_measurement(ctx)
                .map(|m| m.black_frames.clone())
                .unwrap_or_default(),
        )
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
        RuleDescription {
            name: "Freeze frames",
            summary: "No freeze-frame segments longer than the limit.",
            version: "0.1",
        }
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            required_streams: &[StreamKind::Video],
            decode: DecodeRequirement::FullFrame,
            incremental: false,
            gpu: true,
            cost: CostClass::FullDecode,
        }
    }
    fn run(&self, ctx: &RuleContext<'_>) -> RuleResult {
        segment_rule(
            &self.id(),
            "freeze-frame",
            self.cfg.severity,
            self.cfg.max_duration_ms,
            ctx,
            video_measurement(ctx)
                .map(|m| m.freeze_frames.clone())
                .unwrap_or_default(),
        )
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
        RuleDescription {
            name: "Duplicate frames",
            summary: "No duplicate-frame events beyond the limit.",
            version: "0.1",
        }
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            required_streams: &[StreamKind::Video],
            decode: DecodeRequirement::FullFrame,
            incremental: false,
            gpu: true,
            cost: CostClass::FullDecode,
        }
    }
    fn run(&self, ctx: &RuleContext<'_>) -> RuleResult {
        let Some(measurement) = video_measurement(ctx) else {
            return RuleResult::from_finding(inconclusive(
                &self.id(),
                self.cfg.severity,
                "video was not decoded, so duplicate-frame detection is unavailable",
            ));
        };
        if measurement.decoded_frame_count.unwrap_or(0) == 0 {
            return RuleResult::from_finding(inconclusive(
                &self.id(),
                self.cfg.severity,
                "video decode produced no frames, so duplicate-frame detection is unavailable",
            ));
        }
        let count = measurement.duplicate_frames.len() as u64;
        count_rule(
            &self.id(),
            "duplicate-frame",
            self.cfg.severity,
            self.cfg.max_events,
            Some(count),
        )
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
        RuleDescription {
            name: "Corrupt frames",
            summary: "No decode errors beyond the limit.",
            version: "0.1",
        }
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            required_streams: &[StreamKind::Video],
            decode: DecodeRequirement::Stream,
            incremental: true,
            gpu: false,
            cost: CostClass::CheapDecode,
        }
    }
    fn run(&self, ctx: &RuleContext<'_>) -> RuleResult {
        let Some(measurement) = video_measurement(ctx) else {
            return RuleResult::from_finding(inconclusive(
                &self.id(),
                self.cfg.severity,
                "video was not decoded, so corrupt-frame detection is unavailable",
            ));
        };
        if measurement.decoded_frame_count.unwrap_or(0) == 0 {
            // Decode errors recorded without a single decoded frame describe the
            // decode attempt, not corrupt frames in the asset (spec § 8.2). A
            // codec the adapter does not decode (H.264, HEVC, ProRes…) must never
            // be reported as corrupt media, so this stays inconclusive.
            return RuleResult::from_finding(inconclusive(
                &self.id(),
                self.cfg.severity,
                "video decode produced no frames, so corrupt-frame detection is unavailable",
            ));
        }
        count_rule(
            &self.id(),
            "corrupt-frame",
            self.cfg.severity,
            self.cfg.max_events,
            Some(measurement.decode_errors),
        )
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
        RuleDescription {
            name: "Luma range",
            summary: "Luma stays within the legal range [16, 235].",
            version: "0.1",
        }
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            required_streams: &[StreamKind::Video],
            decode: DecodeRequirement::FullFrame,
            incremental: false,
            gpu: false,
            cost: CostClass::FullDecode,
        }
    }
    fn run(&self, ctx: &RuleContext<'_>) -> RuleResult {
        let Some(measurement) = video_measurement(ctx) else {
            return RuleResult::from_finding(inconclusive(
                &self.id(),
                self.cfg.severity,
                "video was not decoded, so luma could not be measured",
            ));
        };
        if measurement.decoded_frame_count.unwrap_or(0) == 0 {
            return RuleResult::from_finding(inconclusive(
                &self.id(),
                self.cfg.severity,
                "video decode produced no frames, so luma could not be measured",
            ));
        }
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
            format!(
                "luma within legal range (min {}, max {})",
                luma.min, luma.max
            ),
            Some(Value::Ratio(out)),
        ))
    }
}

// ---------------------------------------------------------------------------
// photosensitivity (general flash)
// ---------------------------------------------------------------------------

struct PhotosensitivityRuleImpl {
    cfg: PhotosensitivityRule,
}

/// Merged windows (ms) in which more than `limit` flashes occur within any one
/// second, for one region's transition timestamps. A flash is a pair of
/// opposing transitions, so the transition budget is `2 × limit`.
fn flash_hazard_windows(transitions: &[u64], limit: f64) -> Vec<(u64, u64, f64)> {
    let mut windows: Vec<(u64, u64, f64)> = Vec::new();
    let mut start = 0usize;
    for end in 0..transitions.len() {
        while transitions[end] - transitions[start] >= 1000 {
            start += 1;
        }
        let flashes = (end - start + 1) as f64 / 2.0;
        if flashes > limit {
            let (from, to) = (transitions[start], transitions[end]);
            match windows.last_mut() {
                Some(last) if from <= last.1 => {
                    last.1 = to;
                    last.2 = last.2.max(flashes);
                }
                _ => windows.push((from, to, flashes)),
            }
        }
    }
    windows
}

impl QcRule for PhotosensitivityRuleImpl {
    fn id(&self) -> RuleId {
        RuleId::new("video.photosensitivity")
    }
    fn description(&self) -> RuleDescription {
        RuleDescription {
            name: "Photosensitivity (general flash)",
            summary: "No more than the permitted flashes per second (Harding/BT.1702-style screen; not a compliance claim).",
            version: "0.1",
        }
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            required_streams: &[StreamKind::Video],
            decode: DecodeRequirement::FullFrame,
            incremental: false,
            gpu: false,
            cost: CostClass::FullDecode,
        }
    }
    fn run(&self, ctx: &RuleContext<'_>) -> RuleResult {
        let id = self.id();
        let severity = self.cfg.severity;
        let Some(measurement) = video_measurement(ctx) else {
            return RuleResult::from_finding(inconclusive(
                &id,
                severity,
                "video was not decoded, so flashing could not be measured",
            ));
        };
        if measurement.decoded_frame_count.unwrap_or(0) == 0 {
            return RuleResult::from_finding(inconclusive(
                &id,
                severity,
                "video decode produced no frames, so flashing could not be measured",
            ));
        }
        let Some(flash) = &measurement.flash else {
            return RuleResult::from_finding(inconclusive(
                &id,
                severity,
                "flash analysis was not performed",
            ));
        };
        let limit = self.cfg.max_flashes_per_second;
        // Each flash class (general, red) is judged on its own; windows within a
        // class are already merged.
        let merged: Vec<(&str, u64, u64, f64)> = flash
            .transitions_ms
            .iter()
            .zip(tpt_app_media_qc_model::inspection::FLASH_CHANNEL_NAMES)
            .flat_map(|(transitions, name)| {
                flash_hazard_windows(transitions, limit)
                    .into_iter()
                    .map(move |(from, to, flashes)| (name, from, to, flashes))
            })
            .collect();
        if merged.is_empty() {
            if measurement.decode_errors > 0 {
                return RuleResult::from_finding(inconclusive(
                    &id,
                    severity,
                    "decode had errors, so flash detection may be incomplete",
                ));
            }
            return RuleResult::pass();
        }
        RuleResult::findings(segment_failures(
            &id,
            severity,
            merged.into_iter().map(|(name, from, to, flashes)| {
                (
                    tpt_app_media_qc_model::finding::TimeRange::new(from, to.max(from + 1)),
                    format!(
                        "{name}: up to {flashes:.1} flashes in one second exceeds the limit of {limit:.1}"
                    ),
                )
            }),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::bundled_asset;
    use tpt_app_media_qc_model::finding::TimeRange;
    use tpt_app_media_qc_model::inspection::{Inspection, LumaStats, VideoMeasurements};
    use tpt_app_media_qc_model::severity::VerdictDecision;
    use tpt_app_media_qc_profile::model::Ratio;

    fn ctx_with(video: Vec<VideoMeasurements>) -> RuleContext<'static> {
        let asset = bundled_asset();
        let inspection: &'static Inspection = Box::leak(Box::new(Inspection {
            video,
            ..Default::default()
        }));
        RuleContext { asset, inspection }
    }

    fn measurement() -> VideoMeasurements {
        VideoMeasurements {
            stream_idx: 0,
            ..Default::default()
        }
    }

    #[test]
    fn resolution_passes_and_fails() {
        let cfg = ResolutionRule {
            expected: Resolution(1920, 1080),
            severity: Severity::Error,
        };
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
        assert_eq!(
            r.findings[0].status,
            tpt_app_media_qc_model::severity::VerdictDecision::Fail
        );
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
        assert_eq!(
            r.findings[0].status,
            tpt_app_media_qc_model::severity::VerdictDecision::Fail
        );
    }

    fn default_tol() -> f64 {
        0.005
    }

    #[test]
    fn black_frames_over_limit_fail_and_attach_range() {
        let cfg = DurationThresholdRule {
            max_duration_ms: 500,
            severity: Severity::Warning,
        };
        let rule = BlackFramesRule { cfg };
        let mut m = measurement();
        m.decoded_frame_count = Some(1);
        m.black_frames = vec![TimeRange::new(1_000, 2_500)];
        let r = rule.execute(&ctx_with(vec![m]));
        assert_eq!(
            r.findings[0].status,
            tpt_app_media_qc_model::severity::VerdictDecision::Fail
        );
        assert_eq!(r.findings[0].time_range, Some(TimeRange::new(1_000, 2_500)));
    }

    #[test]
    fn luma_out_of_legal_fails() {
        let cfg = LumaRangeRule {
            max_out_of_legal: 0.02,
            severity: Severity::Warning,
        };
        let rule = LumaRangeRuleImpl { cfg };
        let mut m = measurement();
        m.decoded_frame_count = Some(1);
        m.luma = Some(LumaStats {
            min: 16,
            max: 235,
            mean: 100.0,
            below_legal: 0.05,
            above_legal: 0.0,
            clipped_white: 0.0,
        });
        let r = rule.execute(&ctx_with(vec![m]));
        assert_eq!(
            r.findings[0].status,
            tpt_app_media_qc_model::severity::VerdictDecision::Fail
        );
    }

    #[test]
    fn colorspace_mismatch_fails() {
        let cfg = ColorSpaceRule {
            expected: "bt2020nc".into(),
            severity: Severity::Error,
        };
        let rule = ColorSpaceRuleImpl { cfg };
        let mut m = measurement();
        m.colorspace = Some("bt709".into());
        let r = rule.execute(&ctx_with(vec![m]));
        assert_eq!(
            r.findings[0].status,
            tpt_app_media_qc_model::severity::VerdictDecision::Fail
        );
    }

    // -- scan_format --------------------------------------------------------

    use tpt_app_media_qc_profile::model::FieldOrderExpectation;

    fn scan_rule(scan: ScanExpectation, field_order: FieldOrderExpectation) -> ScanFormatRuleImpl {
        ScanFormatRuleImpl {
            cfg: ScanFormatRule {
                scan,
                field_order,
                severity: Severity::Error,
            },
        }
    }

    fn ctx_with_order(order: Option<FieldOrder>) -> RuleContext<'static> {
        let mut asset = bundled_asset().clone();
        if let Some(stream) = asset
            .streams
            .iter_mut()
            .find(|s| s.kind == StreamKind::Video)
        {
            stream.field_order = order;
        }
        let asset: &'static _ = Box::leak(Box::new(asset));
        let inspection: &'static Inspection = Box::leak(Box::new(Inspection::default()));
        RuleContext { asset, inspection }
    }

    #[test]
    fn scan_format_progressive_passes_on_progressive_stream() {
        let rule = scan_rule(ScanExpectation::Progressive, FieldOrderExpectation::Any);
        assert!(rule
            .execute(&ctx_with_order(Some(FieldOrder::Progressive)))
            .is_pass());
    }

    #[test]
    fn scan_format_progressive_fails_on_signalled_interlaced() {
        let rule = scan_rule(ScanExpectation::Progressive, FieldOrderExpectation::Any);
        let mut m = measurement();
        m.field_order = Some(FieldOrder::TopFieldFirst);
        let r = rule.execute(&ctx_with(vec![m]));
        assert_eq!(
            r.findings[0].status,
            tpt_app_media_qc_model::severity::VerdictDecision::Fail
        );
    }

    #[test]
    fn scan_format_unsignalled_is_inconclusive_for_progressive_expectation() {
        let rule = scan_rule(ScanExpectation::Progressive, FieldOrderExpectation::Any);
        let r = rule.execute(&ctx_with_order(Some(FieldOrder::Unknown)));
        assert!(r.findings.iter().any(|f| {
            f.status == tpt_app_media_qc_model::severity::VerdictDecision::Inconclusive
        }));
    }

    #[test]
    fn scan_format_interlaced_fails_on_unsignalled_and_passes_on_signalled() {
        let rule = scan_rule(ScanExpectation::Interlaced, FieldOrderExpectation::Any);
        let r = rule.execute(&ctx_with_order(Some(FieldOrder::Unknown)));
        assert_eq!(
            r.findings[0].status,
            tpt_app_media_qc_model::severity::VerdictDecision::Fail
        );

        let mut m = measurement();
        m.field_order = Some(FieldOrder::BottomFieldFirst);
        let r = rule.execute(&ctx_with(vec![m]));
        assert!(r.is_pass());
    }

    #[test]
    fn scan_format_field_order_pin() {
        let rule = scan_rule(ScanExpectation::Any, FieldOrderExpectation::TopFieldFirst);
        let mut m = measurement();
        m.field_order = Some(FieldOrder::TopFieldFirst);
        assert!(rule.execute(&ctx_with(vec![m])).is_pass());

        let mut m = measurement();
        m.field_order = Some(FieldOrder::BottomFieldFirst);
        let r = rule.execute(&ctx_with(vec![m]));
        assert_eq!(
            r.findings[0].status,
            tpt_app_media_qc_model::severity::VerdictDecision::Fail
        );

        // A pinned field order cannot be satisfied by progressive video.
        let mut m = measurement();
        m.field_order = Some(FieldOrder::Progressive);
        let r = rule.execute(&ctx_with(vec![m]));
        assert_eq!(
            r.findings[0].status,
            tpt_app_media_qc_model::severity::VerdictDecision::Fail
        );
    }

    #[test]
    fn scan_format_missing_report_is_inconclusive() {
        let rule = scan_rule(ScanExpectation::Progressive, FieldOrderExpectation::Any);
        let r = rule.execute(&ctx_with_order(None));
        assert!(r.findings.iter().any(|f| {
            f.status == tpt_app_media_qc_model::severity::VerdictDecision::Inconclusive
        }));
    }

    // -- corrupt_frames -----------------------------------------------------

    fn corrupt_rule(max_events: u64) -> CorruptFramesRule {
        CorruptFramesRule {
            cfg: CountThresholdRule {
                max_events,
                severity: Severity::Error,
            },
        }
    }

    #[test]
    fn corrupt_frames_without_decoded_frames_is_inconclusive_not_fail() {
        // An asset whose codec the adapter does not decode (H.264, HEVC, …)
        // yields zero decoded frames. Recorded decode errors must not be
        // reported as corrupt media against a zero-event profile limit.
        let rule = corrupt_rule(0);
        let mut m = measurement();
        m.decoded_frame_count = Some(0);
        m.decode_errors = 1;
        let r = rule.execute(&ctx_with(vec![m]));
        assert_eq!(
            r.findings[0].status,
            tpt_app_media_qc_model::severity::VerdictDecision::Inconclusive
        );
    }

    #[test]
    fn corrupt_frames_fails_when_decoded_frames_carry_decode_errors() {
        let rule = corrupt_rule(0);
        let mut m = measurement();
        m.decoded_frame_count = Some(120);
        m.decode_errors = 2;
        let r = rule.execute(&ctx_with(vec![m]));
        assert_eq!(
            r.findings[0].status,
            tpt_app_media_qc_model::severity::VerdictDecision::Fail
        );
    }

    #[test]
    fn corrupt_frames_passes_when_decoded_frames_are_clean() {
        let rule = corrupt_rule(0);
        let mut m = measurement();
        m.decoded_frame_count = Some(120);
        assert!(rule.execute(&ctx_with(vec![m])).is_pass());
    }

    fn flash_ctx(transitions: Vec<u64>) -> RuleContext<'static> {
        let mut m = measurement();
        m.decoded_frame_count = Some(100);
        m.flash = Some(tpt_app_media_qc_model::inspection::FlashMeasurements {
            transitions_ms: vec![transitions, vec![]],
        });
        ctx_with(vec![m])
    }

    fn photosensitivity() -> PhotosensitivityRuleImpl {
        PhotosensitivityRuleImpl {
            cfg: PhotosensitivityRule {
                max_flashes_per_second: 3.0,
                severity: Severity::Error,
            },
        }
    }

    #[test]
    fn photosensitivity_passes_within_three_flashes_per_second() {
        // 6 transitions in under a second = 3 flashes.
        let ctx = flash_ctx(vec![0, 100, 200, 300, 400, 500, 1500, 1600]);
        assert!(photosensitivity()
            .run(&ctx)
            .findings
            .iter()
            .all(|f| f.status != VerdictDecision::Fail));
    }

    #[test]
    fn photosensitivity_fails_above_the_limit_and_attaches_range() {
        let ctx = flash_ctx(vec![0, 100, 200, 300, 400, 500, 600, 700]);
        let result = photosensitivity().run(&ctx);
        assert_eq!(result.findings.len(), 1);
        assert_eq!(result.findings[0].status, VerdictDecision::Fail);
    }

    #[test]
    fn photosensitivity_without_flash_analysis_is_inconclusive() {
        let mut m = measurement();
        m.decoded_frame_count = Some(10);
        let result = photosensitivity().run(&ctx_with(vec![m]));
        assert_eq!(result.findings[0].status, VerdictDecision::Inconclusive);
    }
}
