//! Subtitle and caption checks ([spec Ãƒâ€šÃ‚Â§ 8.7]): presence, language, cue timing,
//! cue content and coverage against the video duration.
//!
//! Every rule here is metadata-driven: the probe reads cue timing from packet
//! headers and, for plain-text subtitle codecs, the cue payload. Nothing in
//! this module decodes a picture or a sample, so the rules stay in the cheap
//! metadata pass.

use crate::rule::{
    Capabilities, DecodeRequirement, QcRule, RuleContext, RuleDescription, RuleResult,
};
use crate::util::{container_scanned, fail, inconclusive, segment_failures};
use tpt_app_media_qc_core::cost::CostClass;
use tpt_app_media_qc_model::asset::{Stream, StreamKind};
use tpt_app_media_qc_model::finding::RuleId;
use tpt_app_media_qc_model::inspection::SubtitleMeasurements;
use tpt_app_media_qc_model::value::Value;
use tpt_app_media_qc_profile::model::{
    SubtitleContentRule, SubtitleDurationRule, SubtitleLanguageRule, SubtitlePresenceRule,
    SubtitleTimingRule,
};

pub const RULE_IDS: &[&str] = &[
    "subtitle.presence",
    "subtitle.language",
    "subtitle.timing",
    "subtitle.content",
    "subtitle.duration_match",
];

pub fn build(profile: &tpt_app_media_qc_profile::model::Profile, out: &mut Vec<Box<dyn QcRule>>) {
    let s = &profile.rules.subtitle;
    if let Some(cfg) = s.presence {
        out.push(Box::new(SubtitlePresenceRuleImpl { cfg }));
    }
    if let Some(cfg) = s.language.clone() {
        out.push(Box::new(SubtitleLanguageRuleImpl { cfg }));
    }
    if let Some(cfg) = s.timing {
        out.push(Box::new(SubtitleTimingRuleImpl { cfg }));
    }
    if let Some(cfg) = s.content {
        out.push(Box::new(SubtitleContentRuleImpl { cfg }));
    }
    if let Some(cfg) = s.duration_match {
        out.push(Box::new(SubtitleDurationRuleImpl { cfg }));
    }
}

fn subtitle_capabilities() -> Capabilities {
    Capabilities {
        required_streams: &[StreamKind::Subtitle],
        decode: DecodeRequirement::None,
        incremental: true,
        gpu: false,
        cost: CostClass::Metadata,
    }
}

/// Every subtitle stream, paired with the measurement captured for it (if any).
fn subtitle_streams<'a>(
    ctx: &'a RuleContext<'_>,
) -> Vec<(&'a Stream, Option<&'a SubtitleMeasurements>)> {
    ctx.streams_of(StreamKind::Subtitle)
        .map(|s| (s, ctx.inspection.subtitle_for(s.index.as_u64())))
        .collect()
}

/// The measured video duration, preferring the video stream over the container.
fn video_duration_ms(ctx: &RuleContext<'_>) -> Option<u64> {
    ctx.primary_stream(StreamKind::Video)
        .and_then(|s| s.duration)
        .map(|d| d.as_secs() * 1000)
        .or_else(|| ctx.inspection.container.duration.map(|d| d.0))
}

/// ISO 639 codes are compared loosely: case-insensitively, ignoring region
/// subtags, and treating the legacy two-letter forms as their three-letter
/// equivalents so `en` and `eng` both satisfy an `eng` delivery requirement.
fn language_matches(stream_tag: &str, wanted: &str) -> bool {
    let norm = |s: &str| {
        s.trim()
            .to_ascii_lowercase()
            .split(['-', '_'])
            .next()
            .unwrap_or_default()
            .to_string()
    };
    let expand = |s: String| -> String {
        match s.as_str() {
            "en" => "eng",
            "fr" => "fre",
            "de" => "deu",
            "es" => "spa",
            "it" => "ita",
            "nl" => "nld",
            "pt" => "por",
            "sv" => "swe",
            "da" => "dan",
            "no" => "nor",
            "fi" => "fin",
            "pl" => "pol",
            "cs" => "ces",
            "ru" => "rus",
            "ja" => "jpn",
            "ko" => "kor",
            "zh" => "zho",
            other => other,
        }
        .to_string()
    };
    expand(norm(stream_tag)) == expand(norm(wanted))
}

// ---------------------------------------------------------------------------
// presence
// ---------------------------------------------------------------------------

struct SubtitlePresenceRuleImpl {
    cfg: SubtitlePresenceRule,
}

impl QcRule for SubtitlePresenceRuleImpl {
    fn id(&self) -> RuleId {
        RuleId::new("subtitle.presence")
    }
    fn description(&self) -> RuleDescription {
        RuleDescription {
            name: "Subtitle presence",
            summary: "The asset carries the number of subtitle tracks the delivery requires.",
            version: "0.1",
        }
    }
    fn capabilities(&self) -> Capabilities {
        subtitle_capabilities()
    }
    fn run(&self, ctx: &RuleContext<'_>) -> RuleResult {
        if !container_scanned(ctx.inspection) {
            return RuleResult::from_finding(inconclusive(
                &self.id(),
                self.cfg.severity,
                "the container was not scanned, so subtitle streams are unknown",
            ));
        }
        let count = subtitle_streams(ctx).len() as u32;
        if count < self.cfg.min_subtitle {
            return RuleResult::from_finding(fail(
                &self.id(),
                self.cfg.severity,
                format!(
                    "{count} subtitle track(s) present, at least {} required",
                    self.cfg.min_subtitle
                ),
                Some(Value::UInt(count as u64)),
                Some(Value::UInt(self.cfg.min_subtitle as u64)),
            ));
        }
        if let Some(max) = self.cfg.max_subtitle {
            if count > max {
                return RuleResult::from_finding(fail(
                    &self.id(),
                    self.cfg.severity,
                    format!("{count} subtitle tracks present, at most {max} allowed"),
                    Some(Value::UInt(count as u64)),
                    Some(Value::UInt(max as u64)),
                ));
            }
        }
        RuleResult::pass()
    }
}

// ---------------------------------------------------------------------------
// language
// ---------------------------------------------------------------------------

struct SubtitleLanguageRuleImpl {
    cfg: SubtitleLanguageRule,
}

impl QcRule for SubtitleLanguageRuleImpl {
    fn id(&self) -> RuleId {
        RuleId::new("subtitle.language")
    }
    fn description(&self) -> RuleDescription {
        RuleDescription {
            name: "Subtitle language",
            summary: "Each demanded subtitle language is present in the required number of tracks.",
            version: "0.1",
        }
    }
    fn capabilities(&self) -> Capabilities {
        subtitle_capabilities()
    }
    fn run(&self, ctx: &RuleContext<'_>) -> RuleResult {
        if !container_scanned(ctx.inspection) {
            return RuleResult::from_finding(inconclusive(
                &self.id(),
                self.cfg.severity,
                "the container was not scanned, so subtitle languages are unknown",
            ));
        }
        let streams = subtitle_streams(ctx);
        if streams.is_empty() {
            return RuleResult::from_finding(fail(
                &self.id(),
                self.cfg.severity,
                format!(
                    "no subtitle tracks to carry the required languages ({})",
                    self.cfg.required.join(", ")
                ),
                Some(Value::UInt(0)),
                Some(Value::Text(format!("{} track(s)", self.cfg.min_tracks))),
            ));
        }

        let mut findings = Vec::new();
        for wanted in &self.cfg.required {
            let present = streams
                .iter()
                .filter(|(s, _)| {
                    s.language
                        .as_deref()
                        .is_some_and(|tag| language_matches(tag, wanted))
                })
                .count() as u32;
            if present < self.cfg.min_tracks {
                let found: Vec<&str> = streams
                    .iter()
                    .filter_map(|(s, _)| s.language.as_deref())
                    .collect();
                findings.push(fail(
                    &self.id(),
                    self.cfg.severity,
                    format!(
                        "subtitle language '{wanted}' has {present} track(s), {} required (found: {})",
                        self.cfg.min_tracks,
                        if found.is_empty() {
                            "none tagged".to_string()
                        } else {
                            found.join(", ")
                        }
                    ),
                    Some(Value::Text(found.join(", "))),
                    Some(Value::Text(wanted.clone())),
                ));
            }
        }
        RuleResult::findings(findings)
    }
}

// ---------------------------------------------------------------------------
// timing
// ---------------------------------------------------------------------------

struct SubtitleTimingRuleImpl {
    cfg: SubtitleTimingRule,
}

impl QcRule for SubtitleTimingRuleImpl {
    fn id(&self) -> RuleId {
        RuleId::new("subtitle.timing")
    }
    fn description(&self) -> RuleDescription {
        RuleDescription {
            name: "Subtitle cue timing",
            summary: "Cues have valid durations, do not overlap, stay within length limits and leave no long gaps.",
            version: "0.1",
        }
    }
    fn capabilities(&self) -> Capabilities {
        subtitle_capabilities()
    }
    fn run(&self, ctx: &RuleContext<'_>) -> RuleResult {
        let id = self.id();
        let severity = self.cfg.severity;
        let mut findings = Vec::new();
        let mut measured_any = false;

        for (stream, measurement) in subtitle_streams(ctx) {
            // `None` cue count means the cue pass never ran for this stream.
            let Some(m) = measurement.filter(|m| m.cue_count.is_some()) else {
                continue;
            };
            measured_any = true;
            let lang = stream.language.clone().unwrap_or_else(|| "untagged".into());

            if m.invalid_durations.len() as u32 > self.cfg.max_invalid_durations {
                findings.extend(segment_failures(
                    &id,
                    severity,
                    m.invalid_durations.iter().map(|r| {
                        (
                            *r,
                            format!("subtitle cue [{lang}] does not end after it starts"),
                        )
                    }),
                ));
            }
            if m.overlaps.len() as u32 > self.cfg.max_overlaps {
                findings.extend(segment_failures(
                    &id,
                    severity,
                    m.overlaps
                        .iter()
                        .map(|r| (*r, format!("subtitle cue [{lang}] overlaps the next cue"))),
                ));
            }
            if let Some(limit) = self.cfg.max_gap_ms {
                if let Some(gap) = m.max_gap_ms {
                    if gap > limit {
                        findings.push(fail(
                            &id,
                            severity,
                            format!("subtitle track [{lang}] leaves a {gap} ms gap between cues"),
                            Some(Value::DurationMs(gap)),
                            Some(Value::DurationMs(limit)),
                        ));
                    }
                }
            }
            if let Some(limit) = self.cfg.max_cue_duration_ms {
                if let Some(dur) = m.max_cue_duration_ms {
                    if dur > limit {
                        findings.push(fail(
                            &id,
                            severity,
                            format!("subtitle cue [{lang}] stays on screen for {dur} ms"),
                            Some(Value::DurationMs(dur)),
                            Some(Value::DurationMs(limit)),
                        ));
                    }
                }
            }
        }

        if !measured_any {
            return RuleResult::from_finding(inconclusive(
                &id,
                severity,
                "subtitle cue timing was not measured",
            ));
        }
        RuleResult::findings(findings)
    }
}

// ---------------------------------------------------------------------------
// content
// ---------------------------------------------------------------------------

struct SubtitleContentRuleImpl {
    cfg: SubtitleContentRule,
}

impl QcRule for SubtitleContentRuleImpl {
    fn id(&self) -> RuleId {
        RuleId::new("subtitle.content")
    }
    fn description(&self) -> RuleDescription {
        RuleDescription {
            name: "Subtitle cue content",
            summary:
                "Cues decode as text, are not empty and respect the configured character limits.",
            version: "0.1",
        }
    }
    fn capabilities(&self) -> Capabilities {
        subtitle_capabilities()
    }
    fn run(&self, ctx: &RuleContext<'_>) -> RuleResult {
        let id = self.id();
        let severity = self.cfg.severity;
        let mut findings = Vec::new();
        let mut text_measured = false;

        for (stream, measurement) in subtitle_streams(ctx) {
            let Some(m) = measurement.filter(|m| m.cue_count.unwrap_or(0) > 0) else {
                continue;
            };
            let lang = stream.language.clone().unwrap_or_else(|| "untagged".into());

            if m.malformed.len() as u32 > self.cfg.max_malformed {
                findings.extend(segment_failures(
                    &id,
                    severity,
                    m.malformed.iter().map(|r| {
                        (
                            *r,
                            format!("subtitle cue [{lang}] payload is not decodable text"),
                        )
                    }),
                ));
            }
            if m.empty_cues.len() as u32 > self.cfg.max_empty {
                findings.extend(segment_failures(
                    &id,
                    severity,
                    m.empty_cues.iter().map(|r| {
                        (
                            *r,
                            format!("subtitle cue [{lang}] contains no visible text"),
                        )
                    }),
                ));
            }

            // Character limits need a payload the probe could decode as text.
            if !m.text_decoded {
                continue;
            }
            text_measured = true;
            if let Some(limit) = self.cfg.max_chars_per_line {
                if let Some(chars) = m.max_line_chars {
                    if chars > limit {
                        findings.push(fail(
                            &id,
                            severity,
                            format!("subtitle line [{lang}] is {chars} characters long"),
                            Some(Value::UInt(chars as u64)),
                            Some(Value::UInt(limit as u64)),
                        ));
                    }
                }
            }
            if let Some(limit) = self.cfg.max_lines_per_cue {
                if let Some(lines) = m.max_lines_per_cue {
                    if lines > limit {
                        findings.push(fail(
                            &id,
                            severity,
                            format!("subtitle cue [{lang}] spans {lines} lines"),
                            Some(Value::UInt(lines as u64)),
                            Some(Value::UInt(limit as u64)),
                        ));
                    }
                }
            }
        }

        if !text_measured {
            return RuleResult::from_finding(inconclusive(
                &id,
                severity,
                "subtitle cue text was not decoded, so character limits could not be checked",
            ));
        }
        RuleResult::findings(findings)
    }
}

// ---------------------------------------------------------------------------
// duration_match
// ---------------------------------------------------------------------------

struct SubtitleDurationRuleImpl {
    cfg: SubtitleDurationRule,
}

impl QcRule for SubtitleDurationRuleImpl {
    fn id(&self) -> RuleId {
        RuleId::new("subtitle.duration_match")
    }
    fn description(&self) -> RuleDescription {
        RuleDescription {
            name: "Subtitle duration match",
            summary: "Subtitle coverage lines up with the video duration within tolerance.",
            version: "0.1",
        }
    }
    fn capabilities(&self) -> Capabilities {
        subtitle_capabilities()
    }
    fn run(&self, ctx: &RuleContext<'_>) -> RuleResult {
        let id = self.id();
        let severity = self.cfg.severity;
        let Some(video_ms) = video_duration_ms(ctx) else {
            return RuleResult::from_finding(inconclusive(
                &id,
                severity,
                "the video duration is unknown, so subtitle coverage could not be compared",
            ));
        };

        let mut findings = Vec::new();
        let mut measured_any = false;
        for (stream, measurement) in subtitle_streams(ctx) {
            let Some(covered) = measurement.and_then(|m| m.covered_until_ms) else {
                continue;
            };
            measured_any = true;
            let lang = stream.language.clone().unwrap_or_else(|| "untagged".into());
            if covered + self.cfg.tolerance_ms < video_ms {
                findings.push(fail(
                    &id,
                    severity,
                    format!(
                        "subtitle track [{lang}] ends {covered} ms in, {} ms before the video ends",
                        video_ms - covered
                    ),
                    Some(Value::DurationMs(covered)),
                    Some(Value::DurationMs(video_ms)),
                ));
            } else if !self.cfg.allow_longer && covered > video_ms + self.cfg.tolerance_ms {
                findings.push(fail(
                    &id,
                    severity,
                    format!(
                        "subtitle track [{lang}] runs {} ms past the end of the video",
                        covered - video_ms
                    ),
                    Some(Value::DurationMs(covered)),
                    Some(Value::DurationMs(video_ms)),
                ));
            }
        }

        if !measured_any {
            return RuleResult::from_finding(inconclusive(
                &id,
                severity,
                "subtitle cue timing was not measured, so coverage could not be compared",
            ));
        }
        RuleResult::findings(findings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::subtitle_asset;
    use tpt_app_media_qc_model::finding::TimeRange;
    use tpt_app_media_qc_model::inspection::{
        ContainerInspection, ContainerValidity, DurationMillis, Inspection,
    };
    use tpt_app_media_qc_model::severity::{Severity, VerdictDecision as Status};
    use tpt_app_media_qc_model::time::DurationSeconds;

    /// Context with one `eng` subtitle stream (index 2) and the given cue data.
    fn ctx(sub: Option<SubtitleMeasurements>, video_ms: u64) -> RuleContext<'static> {
        let mut asset = subtitle_asset();
        asset.duration = Some(DurationSeconds::from_millis(video_ms));
        asset.streams[0].duration = Some(DurationSeconds::from_millis(video_ms));
        asset.streams[2].duration = Some(DurationSeconds::from_millis(video_ms));
        let inspection: &'static Inspection = Box::leak(Box::new(Inspection {
            subtitle: sub.into_iter().collect(),
            container: ContainerInspection {
                validity: ContainerValidity::Ok,
                duration: Some(DurationMillis(video_ms)),
                ..Default::default()
            },
            ..Default::default()
        }));
        RuleContext {
            asset: Box::leak(Box::new(asset)),
            inspection,
        }
    }

    /// Context with no subtitle streams at all.
    fn ctx_no_subs(video_ms: u64) -> RuleContext<'static> {
        let mut asset = subtitle_asset();
        asset.streams.retain(|s| s.kind != StreamKind::Subtitle);
        asset.duration = Some(DurationSeconds::from_millis(video_ms));
        asset.streams[0].duration = Some(DurationSeconds::from_millis(video_ms));
        let inspection: &'static Inspection = Box::leak(Box::new(Inspection {
            container: ContainerInspection {
                validity: ContainerValidity::Ok,
                duration: Some(DurationMillis(video_ms)),
                ..Default::default()
            },
            ..Default::default()
        }));
        RuleContext {
            asset: Box::leak(Box::new(asset)),
            inspection,
        }
    }

    fn clean() -> SubtitleMeasurements {
        SubtitleMeasurements {
            stream_idx: 2,
            cue_count: Some(120),
            text_decoded: true,
            max_gap_ms: Some(2_000),
            max_cue_duration_ms: Some(4_000),
            max_line_chars: Some(38),
            max_lines_per_cue: Some(2),
            covered_until_ms: Some(120_000),
            ..Default::default()
        }
    }

    fn problems(result: &RuleResult) -> Vec<String> {
        result
            .findings
            .iter()
            .filter(|f| f.status != Status::Pass)
            .map(|f| f.message.clone())
            .collect()
    }

    fn presence(min: u32, max: Option<u32>) -> SubtitlePresenceRuleImpl {
        SubtitlePresenceRuleImpl {
            cfg: SubtitlePresenceRule {
                min_subtitle: min,
                max_subtitle: max,
                severity: Severity::Error,
            },
        }
    }

    fn language(required: &[&str]) -> SubtitleLanguageRuleImpl {
        SubtitleLanguageRuleImpl {
            cfg: SubtitleLanguageRule {
                required: required.iter().map(|s| s.to_string()).collect(),
                min_tracks: 1,
                severity: Severity::Error,
            },
        }
    }

    fn timing() -> SubtitleTimingRuleImpl {
        SubtitleTimingRuleImpl {
            cfg: SubtitleTimingRule {
                max_overlaps: 0,
                max_invalid_durations: 0,
                max_gap_ms: Some(5_000),
                max_cue_duration_ms: Some(7_000),
                severity: Severity::Warning,
            },
        }
    }

    fn content() -> SubtitleContentRuleImpl {
        SubtitleContentRuleImpl {
            cfg: SubtitleContentRule {
                max_malformed: 0,
                max_empty: 0,
                max_chars_per_line: Some(42),
                max_lines_per_cue: Some(2),
                severity: Severity::Warning,
            },
        }
    }

    fn duration_match() -> SubtitleDurationRuleImpl {
        SubtitleDurationRuleImpl {
            cfg: SubtitleDurationRule {
                tolerance_ms: 1_000,
                allow_longer: true,
                severity: Severity::Warning,
            },
        }
    }

    #[test]
    fn clean_subtitles_pass_every_rule() {
        let c = ctx(Some(clean()), 120_000);
        let rules: Vec<Box<dyn QcRule>> = vec![
            Box::new(presence(1, None)),
            Box::new(language(&["eng"])),
            Box::new(timing()),
            Box::new(content()),
            Box::new(duration_match()),
        ];
        for rule in rules {
            let msgs = problems(&rule.run(&c));
            assert!(msgs.is_empty(), "{}: {msgs:?}", rule.id());
        }
    }

    #[test]
    fn presence_fails_when_no_subtitle_stream_exists() {
        let msgs = problems(&presence(1, None).run(&ctx_no_subs(120_000)));
        assert_eq!(msgs.len(), 1);
        assert!(msgs[0].contains("at least 1 required"), "{msgs:?}");
    }

    #[test]
    fn presence_rejects_too_many_tracks() {
        let msgs = problems(&presence(1, Some(0)).run(&ctx(Some(clean()), 120_000)));
        assert!(msgs[0].contains("at most 0 allowed"), "{msgs:?}");
    }

    #[test]
    fn language_matching_is_loose_and_missing_codes_fail() {
        assert!(language_matches("en", "eng"));
        assert!(language_matches("ENG", "eng"));
        assert!(language_matches("en-US", "eng"));
        assert!(!language_matches("fre", "eng"));
        let msgs = problems(&language(&["eng", "fre"]).run(&ctx(Some(clean()), 120_000)));
        assert_eq!(msgs.len(), 1);
        assert!(msgs[0].contains("'fre'"), "{msgs:?}");
    }

    #[test]
    fn timing_flags_overlaps_invalid_durations_gaps_and_long_cues() {
        let mut m = clean();
        m.overlaps = vec![TimeRange::new(4_000, 6_000)];
        m.invalid_durations = vec![TimeRange::new(9_000, 9_000)];
        m.max_gap_ms = Some(30_000);
        m.max_cue_duration_ms = Some(9_000);
        let msgs = problems(&timing().run(&ctx(Some(m), 120_000)));
        assert_eq!(msgs.len(), 4, "{msgs:?}");
        assert!(msgs.iter().any(|m| m.contains("overlaps the next cue")));
        assert!(msgs
            .iter()
            .any(|m| m.contains("does not end after it starts")));
        assert!(msgs.iter().any(|m| m.contains("30000 ms gap")));
        assert!(msgs.iter().any(|m| m.contains("on screen for 9000 ms")));
    }

    #[test]
    fn timing_is_inconclusive_when_cues_were_not_measured() {
        let result = timing().run(&ctx(Some(SubtitleMeasurements::default()), 120_000));
        assert_eq!(result.findings[0].status, Status::Inconclusive);
    }

    #[test]
    fn timing_counts_only_defects_beyond_the_limit() {
        let mut m = clean();
        m.overlaps = vec![TimeRange::new(4_000, 6_000)];
        let mut rule = timing();
        rule.cfg.max_overlaps = 1;
        assert!(problems(&rule.run(&ctx(Some(m), 120_000))).is_empty());
    }

    #[test]
    fn content_flags_character_and_line_limits() {
        let mut m = clean();
        m.max_line_chars = Some(64);
        m.max_lines_per_cue = Some(3);
        let msgs = problems(&content().run(&ctx(Some(m), 120_000)));
        assert_eq!(msgs.len(), 2, "{msgs:?}");
        assert!(msgs[0].contains("64 characters long"), "{msgs:?}");
        assert!(msgs[1].contains("spans 3 lines"), "{msgs:?}");
    }

    #[test]
    fn content_flags_malformed_and_empty_cues() {
        let mut m = clean();
        m.malformed = vec![TimeRange::new(1_000, 2_000)];
        m.empty_cues = vec![TimeRange::new(7_000, 8_000)];
        let msgs = problems(&content().run(&ctx(Some(m), 120_000)));
        assert_eq!(msgs.len(), 2, "{msgs:?}");
        assert!(msgs[0].contains("not decodable text"), "{msgs:?}");
        assert!(msgs[1].contains("no visible text"), "{msgs:?}");
    }

    #[test]
    fn content_is_inconclusive_for_non_text_codecs() {
        let mut m = clean();
        m.text_decoded = false;
        m.max_line_chars = None;
        let result = content().run(&ctx(Some(m), 120_000));
        assert_eq!(result.findings[0].status, Status::Inconclusive);
        assert!(result.findings[0].message.contains("not decoded"));
    }

    #[test]
    fn duration_match_flags_short_coverage() {
        let mut m = clean();
        m.covered_until_ms = Some(60_000);
        let msgs = problems(&duration_match().run(&ctx(Some(m), 120_000)));
        assert_eq!(msgs.len(), 1, "{msgs:?}");
        assert!(msgs[0].contains("before the video ends"), "{msgs:?}");
    }

    #[test]
    fn duration_match_tolerates_shortfall_within_tolerance() {
        let mut m = clean();
        m.covered_until_ms = Some(119_500);
        assert!(problems(&duration_match().run(&ctx(Some(m), 120_000))).is_empty());
    }

    #[test]
    fn duration_match_flags_longer_only_when_disallowed() {
        let mut m = clean();
        m.covered_until_ms = Some(130_000);
        // Default `allow_longer` tolerates subtitles running past the video.
        assert!(problems(&duration_match().run(&ctx(Some(m.clone()), 120_000))).is_empty());

        let mut strict = duration_match();
        strict.cfg.allow_longer = false;
        let msgs = problems(&strict.run(&ctx(Some(m), 120_000)));
        assert!(msgs[0].contains("past the end of the video"), "{msgs:?}");
    }

    #[test]
    fn duration_match_is_inconclusive_without_video_duration() {
        let mut asset = subtitle_asset();
        asset.streams[0].duration = None;
        asset.duration = None;
        let inspection: &'static Inspection = Box::leak(Box::new(Inspection {
            subtitle: vec![clean()],
            ..Default::default()
        }));
        let ctx = RuleContext {
            asset: Box::leak(Box::new(asset)),
            inspection,
        };
        let result = duration_match().run(&ctx);
        assert_eq!(result.findings[0].status, Status::Inconclusive);
    }
}
