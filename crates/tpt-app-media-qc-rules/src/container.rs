//! Container checks ([spec § 8.1]): readability, validity, malformed
//! metadata, stream counts, duration consistency, bitrate, timecode,
//! timebase, timestamp continuity, unexpected streams.

use crate::rule::{Capabilities, QcRule, RuleContext, RuleDescription, RuleResult};
use crate::util::{container_scanned, fail, inconclusive, info, segment_failures};
use tpt_app_media_qc_model::asset::StreamKind;
use tpt_app_media_qc_model::finding::{QcFinding, RuleId, TimeRange};
use tpt_app_media_qc_model::inspection::ContainerValidity;
use tpt_app_media_qc_model::severity::Severity;
use tpt_app_media_qc_model::value::Value;
use tpt_app_media_qc_profile::model::{
    ContainerRules, MinBitrateRule, StreamPresenceRule, TimestampContinuityRule, ToleranceRule,
};

pub const RULE_IDS: &[&str] = &[
    "container.readable",
    "container.container_validity",
    "container.malformed_metadata",
    "container.duration_consistency",
    "container.bitrate",
    "container.timecode_present",
    "container.timebase",
    "container.timestamp_continuity",
    "container.unexpected_streams",
    "container.stream_presence",
];

pub fn build(profile: &tpt_app_media_qc_profile::model::Profile, out: &mut Vec<Box<dyn QcRule>>) {
    let c: &ContainerRules = &profile.rules.container;
    if let Some(severity) = c.readable {
        out.push(Box::new(ReadableRule { severity }));
    }
    if let Some(severity) = c.container_validity {
        out.push(Box::new(ValidityRule { severity }));
    }
    if let Some(severity) = c.malformed_metadata {
        out.push(Box::new(MalformedMetadataRule { severity }));
    }
    if let Some(cfg) = c.duration_consistency {
        out.push(Box::new(DurationConsistencyRule { cfg }));
    }
    if let Some(cfg) = c.bitrate {
        out.push(Box::new(BitrateRule { cfg }));
    }
    if let Some(severity) = c.timecode_present {
        out.push(Box::new(TimecodePresentRule { severity }));
    }
    if let Some(cfg) = c.timebase {
        out.push(Box::new(TimebaseRule { cfg }));
    }
    if let Some(cfg) = c.timestamp_continuity {
        out.push(Box::new(TimestampContinuityRuleImpl { cfg }));
    }
    if let Some(severity) = c.unexpected_streams {
        out.push(Box::new(UnexpectedStreamsRule { severity }));
    }
    if let Some(cfg) = c.stream_presence {
        out.push(Box::new(StreamPresenceRuleImpl { cfg }));
    }
}

fn no_container(ctx: &RuleContext, severity: Severity, id: &RuleId) -> Option<RuleResult> {
    if !container_scanned(ctx.inspection) {
        Some(RuleResult::from_finding(inconclusive(
            id,
            severity,
            "container was not inspected (metadata scan returned no container data)",
        )))
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// readable / container_validity
// ---------------------------------------------------------------------------

struct ReadableRule {
    severity: Severity,
}

impl QcRule for ReadableRule {
    fn id(&self) -> RuleId {
        RuleId::new("container.readable")
    }
    fn description(&self) -> RuleDescription {
        RuleDescription {
            name: "Readable",
            summary: "The file exists, is readable and appears to be a media container.",
            version: "0.1",
        }
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities::metadata_only()
    }
    fn run(&self, ctx: &RuleContext<'_>) -> RuleResult {
        if let Some(r) = no_container(ctx, self.severity, &self.id()) {
            return r;
        }
        match &ctx.inspection.container.validity {
            ContainerValidity::Ok => RuleResult::pass(),
            ContainerValidity::Unreadable(detail) => RuleResult::from_finding(fail(
                &self.id(),
                self.severity,
                format!("file is not readable as a media container: {detail}"),
                None,
                Some(Value::Text("readable media container".into())),
            )),
            ContainerValidity::Corrupt(detail) => RuleResult::from_finding(fail(
                &self.id(),
                self.severity,
                format!("container could not be fully parsed: {detail}"),
                None,
                Some(Value::Text("valid container structure".into())),
            )),
            ContainerValidity::NotScanned => unreachable!(),
        }
    }
}

struct ValidityRule {
    severity: Severity,
}

impl QcRule for ValidityRule {
    fn id(&self) -> RuleId {
        RuleId::new("container.container_validity")
    }
    fn description(&self) -> RuleDescription {
        RuleDescription {
            name: "Container validity",
            summary: "Container structure parses without corruption.",
            version: "0.1",
        }
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities::metadata_only()
    }
    fn run(&self, ctx: &RuleContext<'_>) -> RuleResult {
        if let Some(r) = no_container(ctx, self.severity, &self.id()) {
            return r;
        }
        match &ctx.inspection.container.validity {
            ContainerValidity::Ok => RuleResult::pass(),
            ContainerValidity::Corrupt(detail) => RuleResult::from_finding(fail(
                &self.id(),
                self.severity,
                format!("container structure is corrupt: {detail}"),
                None,
                Some(Value::Text("valid container".into())),
            )),
            ContainerValidity::Unreadable(_) => RuleResult::pass(),
            ContainerValidity::NotScanned => unreachable!(),
        }
    }
}

// ---------------------------------------------------------------------------
// malformed_metadata
// ---------------------------------------------------------------------------

struct MalformedMetadataRule {
    severity: Severity,
}

impl QcRule for MalformedMetadataRule {
    fn id(&self) -> RuleId {
        RuleId::new("container.malformed_metadata")
    }
    fn description(&self) -> RuleDescription {
        RuleDescription {
            name: "Malformed metadata",
            summary: "No garbage or unparsable metadata entries in the container.",
            version: "0.1",
        }
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities::metadata_only()
    }
    fn run(&self, ctx: &RuleContext<'_>) -> RuleResult {
        let entries = &ctx.inspection.container.malformed_metadata;
        if entries.is_empty() {
            return RuleResult::pass();
        }
        let mut findings = Vec::new();
        for entry in entries {
            findings.push(fail(
                &self.id(),
                self.severity,
                format!("malformed metadata entry: {entry}"),
                Some(Value::Text(entry.clone())),
                Some(Value::Text("well-formed metadata values".into())),
            ));
        }
        RuleResult::findings(findings)
    }
}

// ---------------------------------------------------------------------------
// duration_consistency
// ---------------------------------------------------------------------------

struct DurationConsistencyRule {
    cfg: ToleranceRule,
}

impl QcRule for DurationConsistencyRule {
    fn id(&self) -> RuleId {
        RuleId::new("container.duration_consistency")
    }
    fn description(&self) -> RuleDescription {
        RuleDescription {
            name: "Duration consistency",
            summary: "Container and stream durations agree within tolerance.",
            version: "0.1",
        }
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities::metadata_only()
    }
    fn run(&self, ctx: &RuleContext<'_>) -> RuleResult {
        let mut durations: Vec<(String, u64)> = Vec::new();
        if let Some(d) = ctx.inspection.container.duration {
            durations.push(("container".into(), d.0));
        }
        for s in &ctx.asset.streams {
            if let Some(d) = s.duration {
                durations.push((format!("stream {}", s.index), d.as_secs() * 1000));
            }
        }
        if durations.is_empty() {
            return RuleResult::from_finding(inconclusive(
                &self.id(),
                self.cfg.severity,
                "no durations were reported by the container or streams",
            ));
        }
        let mut findings = Vec::new();
        for (i, (label, dur)) in durations.iter().enumerate() {
            for (jlabel, jdur) in durations.iter().skip(i + 1) {
                let delta = (*dur).abs_diff(*jdur);
                if delta > self.cfg.tolerance_ms {
                    findings.push(fail(
                        &self.id(),
                        self.cfg.severity,
                        format!(
                            "duration mismatch: {label} {}ms vs {jlabel} {}ms (tolerance {}ms)",
                            dur / 1000, jdur / 1000, self.cfg.tolerance_ms
                        ),
                        Some(Value::DurationMs(*dur)),
                        Some(Value::DurationMs(*jdur)),
                    ));
                }
            }
        }
        if findings.is_empty() {
            // Report the measured duration as informational pass.
            let (label, dur) = &durations[0];
            RuleResult::from_finding(info(
                &self.id(),
                Severity::Info,
                format!("all durations agree within tolerance ({label} = {}s)", dur / 1000),
                Some(Value::DurationMs(*dur)),
            ))
        } else {
            RuleResult::findings(findings)
        }
    }
}

// ---------------------------------------------------------------------------
// bitrate
// ---------------------------------------------------------------------------

struct BitrateRule {
    cfg: MinBitrateRule,
}

impl QcRule for BitrateRule {
    fn id(&self) -> RuleId {
        RuleId::new("container.bitrate")
    }
    fn description(&self) -> RuleDescription {
        RuleDescription {
            name: "Container bitrate",
            summary: "Overall/container bitrate meets the profile minimum.",
            version: "0.1",
        }
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities::metadata_only()
    }
    fn run(&self, ctx: &RuleContext<'_>) -> RuleResult {
        if let Some(r) = no_container(ctx, self.cfg.severity, &self.id()) {
            return r;
        }
        let Some(bitrate) = ctx.inspection.container.bitrate_bps else {
            return RuleResult::from_finding(inconclusive(
                &self.id(),
                self.cfg.severity,
                "container did not report an overall bitrate",
            ));
        };
        if bitrate < self.cfg.min_bps {
            RuleResult::from_finding(fail(
                &self.id(),
                self.cfg.severity,
                format!("container bitrate {:.1} Mbps is below the {:.1} Mbps minimum", bitrate as f64 / 1_000_000.0, self.cfg.min_bps as f64 / 1_000_000.0),
                Some(Value::bytes(bitrate)),
                Some(Value::bytes(self.cfg.min_bps)),
            ))
        } else {
            RuleResult::from_finding(info(
                &self.id(),
                Severity::Info,
                format!("container bitrate {:.1} Mbps", bitrate as f64 / 1_000_000.0),
                Some(Value::bytes(bitrate)),
            ))
        }
    }
}

// ---------------------------------------------------------------------------
// timecode_present
// ---------------------------------------------------------------------------

struct TimecodePresentRule {
    severity: Severity,
}

impl QcRule for TimecodePresentRule {
    fn id(&self) -> RuleId {
        RuleId::new("container.timecode_present")
    }
    fn description(&self) -> RuleDescription {
        RuleDescription {
            name: "Timecode present",
            summary: "A start timecode is present in the container where expected.",
            version: "0.1",
        }
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities::metadata_only()
    }
    fn run(&self, ctx: &RuleContext<'_>) -> RuleResult {
        match ctx.inspection.container.timecode_present {
            None => RuleResult::from_finding(inconclusive(
                &self.id(),
                self.severity,
                "container did not report whether a start timecode exists",
            )),
            Some(true) => RuleResult::pass(),
            Some(false) => RuleResult::from_finding(fail(
                &self.id(),
                self.severity,
                "no start timecode present in the container",
                Some(Value::Bool(false)),
                Some(Value::Bool(true)),
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// timebase
// ---------------------------------------------------------------------------

struct TimebaseRule {
    cfg: tpt_app_media_qc_profile::model::ExpectValueRule,
}

impl QcRule for TimebaseRule {
    fn id(&self) -> RuleId {
        RuleId::new("container.timebase")
    }
    fn description(&self) -> RuleDescription {
        RuleDescription {
            name: "Stream time base",
            summary: "Video stream time base equals the expected value.",
            version: "0.1",
        }
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities::metadata_only()
    }
    fn run(&self, ctx: &RuleContext<'_>) -> RuleResult {
        let mut findings = Vec::new();
        for stream in ctx.streams_of(StreamKind::Video) {
            let Some(tb) = stream.time_base else {
                findings.push(inconclusive(
                    &self.id(),
                    self.cfg.severity,
                    format!("stream {} did not report a time base", stream.index),
                ));
                continue;
            };
            if tb != self.cfg.value {
                findings.push(fail(
                    &self.id(),
                    self.cfg.severity,
                    format!("stream {} time base {} != expected {}", stream.index, tb, self.cfg.value),
                    Some(Value::Text(tb.to_string())),
                    Some(Value::Text(self.cfg.value.to_string())),
                ));
            }
        }
        if findings.is_empty() && ctx.streams_of(StreamKind::Video).next().is_some() {
            RuleResult::from_finding(info(
                &self.id(),
                Severity::Info,
                format!("video time base matches expected {}", self.cfg.value),
                Some(Value::Text(self.cfg.value.to_string())),
            ))
        } else {
            RuleResult::findings(findings)
        }
    }
}

// ---------------------------------------------------------------------------
// timestamp_continuity
// ---------------------------------------------------------------------------

struct TimestampContinuityRuleImpl {
    cfg: TimestampContinuityRule,
}

impl QcRule for TimestampContinuityRuleImpl {
    fn id(&self) -> RuleId {
        RuleId::new("container.timestamp_continuity")
    }
    fn description(&self) -> RuleDescription {
        RuleDescription {
            name: "Timestamp continuity",
            summary: "Timestamps are contiguous with no gaps or backward jumps beyond tolerance.",
            version: "0.1",
        }
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities::metadata_only()
    }
    fn run(&self, ctx: &RuleContext<'_>) -> RuleResult {
        if let Some(r) = no_container(ctx, self.cfg.severity, &self.id()) {
            return r;
        }
        let gaps = &ctx.inspection.container.timestamp_gaps;
        if gaps.is_empty() {
            return RuleResult::from_finding(info(
                &self.id(),
                Severity::Info,
                "no timestamp gaps beyond tolerance",
                Some(Value::UInt(0)),
            ));
        }
        let offending: Vec<(TimeRange, String)> = gaps
            .iter()
            .filter(|g| g.duration_ms() > self.cfg.max_gap_ms)
            .map(|g| {
                (
                    *g,
                    format!(
                        "timestamp gap of {}ms (max tolerated {}ms)",
                        g.duration_ms(),
                        self.cfg.max_gap_ms
                    ),
                )
            })
            .collect();
        if offending.is_empty() {
            return RuleResult::pass();
        }
        RuleResult::findings(segment_failures(&self.id(), self.cfg.severity, offending.into_iter()))
    }
}

// ---------------------------------------------------------------------------
// unexpected_streams
// ---------------------------------------------------------------------------

struct UnexpectedStreamsRule {
    severity: Severity,
}

impl QcRule for UnexpectedStreamsRule {
    fn id(&self) -> RuleId {
        RuleId::new("container.unexpected_streams")
    }
    fn description(&self) -> RuleDescription {
        RuleDescription {
            name: "Unexpected streams",
            summary: "No unexpected stream kinds are present.",
            version: "0.1",
        }
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities::metadata_only()
    }
    fn run(&self, ctx: &RuleContext<'_>) -> RuleResult {
        let mut findings = Vec::new();
        for stream in &ctx.asset.streams {
            if matches!(stream.kind, StreamKind::Data | StreamKind::Attachment | StreamKind::Unknown) {
                findings.push(QcFinding::new(self.id())
                    .status(tpt_app_media_qc_model::severity::VerdictDecision::Fail)
                    .severity(self.severity)
                    .stream(stream.index)
                    .set_message(format!("unexpected stream kind '{}'", stream.kind.as_str())));
            }
        }
        if findings.is_empty() && !ctx.asset.streams.is_empty() {
            RuleResult::from_finding(info(
                &self.id(),
                Severity::Info,
                "no unexpected stream kinds",
                Some(Value::UInt(ctx.asset.streams.len() as u64)),
            ))
        } else {
            RuleResult::findings(findings)
        }
    }
}

// ---------------------------------------------------------------------------
// stream_presence
// ---------------------------------------------------------------------------

struct StreamPresenceRuleImpl {
    cfg: StreamPresenceRule,
}

impl QcRule for StreamPresenceRuleImpl {
    fn id(&self) -> RuleId {
        RuleId::new("container.stream_presence")
    }
    fn description(&self) -> RuleDescription {
        RuleDescription {
            name: "Stream presence",
            summary: "Required stream counts are present.",
            version: "0.1",
        }
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities::metadata_only()
    }
    fn run(&self, ctx: &RuleContext<'_>) -> RuleResult {
        if !container_scanned(ctx.inspection) {
            return RuleResult::from_finding(inconclusive(
                &self.id(),
                self.cfg.severity,
                "container was not inspected (metadata scan returned no container data)",
            ));
        }
        let video = ctx.streams_of(StreamKind::Video).count() as u64;
        let audio = ctx.streams_of(StreamKind::Audio).count() as u64;
        let total = ctx.asset.streams.len() as u64;

        let mut findings = Vec::new();
        if video < self.cfg.min_video {
            findings.push(fail(
                &self.id(),
                self.cfg.severity,
                format!("expected at least {} video stream(s), found {video}", self.cfg.min_video),
                Some(Value::UInt(video)),
                Some(Value::UInt(self.cfg.min_video)),
            ));
        }
        if audio < self.cfg.min_audio {
            findings.push(fail(
                &self.id(),
                self.cfg.severity,
                format!("expected at least {} audio stream(s), found {audio}", self.cfg.min_audio),
                Some(Value::UInt(audio)),
                Some(Value::UInt(self.cfg.min_audio)),
            ));
        }
        if total > self.cfg.max_streams {
            findings.push(fail(
                &self.id(),
                self.cfg.severity,
                format!("expected at most {} streams, found {total}", self.cfg.max_streams),
                Some(Value::UInt(total)),
                Some(Value::UInt(self.cfg.max_streams)),
            ));
        }
        RuleResult::findings(findings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::bundled_asset;
    use tpt_app_media_qc_model::inspection::{ContainerInspection, Inspection};
    use tpt_app_media_qc_model::severity::VerdictDecision as Status;

fn ctx_with(container: ContainerInspection) -> RuleContext<'static> {
    let asset = bundled_asset();
    let inspection: &'static Inspection =
        Box::leak(Box::new(Inspection { container, ..Default::default() }));
    RuleContext { asset, inspection }
}

    #[test]
    fn readable_ok_passes() {
        let rule = ReadableRule { severity: Severity::Error };
        let ctx = ctx_with(ContainerInspection { validity: ContainerValidity::Ok, ..Default::default() });
        let r = rule.execute(&ctx);
        assert!(r.is_pass());
    }

    #[test]
    fn readable_unreadable_fails() {
        let rule = ReadableRule { severity: Severity::Error };
        let ctx = ctx_with(ContainerInspection {
            validity: ContainerValidity::Unreadable("permission denied".into()),
            ..Default::default()
        });
        let r = rule.execute(&ctx);
        assert_eq!(r.findings[0].status, Status::Fail);
        assert_eq!(r.findings[0].rule_id.as_str(), "container.readable");
    }

    #[test]
    fn not_scanned_is_inconclusive() {
        let rule = ReadableRule { severity: Severity::Error };
        let ctx = ctx_with(ContainerInspection::default());
        let r = rule.execute(&ctx);
        assert_eq!(r.findings[0].status, Status::Inconclusive);
    }

    #[test]
    fn duration_consistency_catches_mismatch() {
        let rule = DurationConsistencyRule {
            cfg: ToleranceRule { tolerance_ms: 100, severity: Severity::Error },
        };
        let mut container = ContainerInspection {
            validity: ContainerValidity::Ok,
            ..Default::default()
        };
        container.duration = Some(tpt_app_media_qc_model::inspection::DurationMillis(121_500));
        let ctx = ctx_with(container);
        let r = rule.execute(&ctx);
        assert!(r.findings.iter().any(|f| f.status == Status::Fail));
    }

    #[test]
    fn bitrate_below_minimum_fails() {
        let rule = BitrateRule {
            cfg: MinBitrateRule { min_bps: 10_000_000, severity: Severity::Error },
        };
        let mut container = ContainerInspection {
            validity: ContainerValidity::Ok,
            ..Default::default()
        };
        container.bitrate_bps = Some(8_000_000);
        let ctx = ctx_with(container);
        let r = rule.execute(&ctx);
        assert_eq!(r.findings[0].status, Status::Fail);
    }

    #[test]
    fn stream_presence_missing_video_fails() {
        let rule = StreamPresenceRuleImpl {
            cfg: StreamPresenceRule {
                min_video: 1,
                min_audio: 0,
                max_streams: 32,
                severity: Severity::Error,
            },
        };
        // bundled asset has a video stream → pass.
        let ctx = ctx_with(ContainerInspection { validity: ContainerValidity::Ok, ..Default::default() });
        assert!(rule.execute(&ctx).is_pass());

        // Flip expectation to catch the audio stream instead.
        let rule2 = StreamPresenceRuleImpl {
            cfg: StreamPresenceRule {
                min_video: 0,
                min_audio: 2,
                max_streams: 32,
                severity: Severity::Error,
            },
        };
        let r = rule2.execute(&ctx);
        assert!(r.findings.iter().any(|f| f.status == Status::Fail));
    }
}