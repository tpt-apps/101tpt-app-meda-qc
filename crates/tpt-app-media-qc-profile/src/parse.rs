//! YAML profile parsing and validation ([spec § 9]).
//!
//! Parsing is strict: unknown rules, unknown keys and malformed values are
//! rejected with a path-annotated error so profiles stay deterministic,
//! diffable and testable. A rule entry may be a scalar severity
//! (`black_frames: warning`) or a mapping with a `severity` field plus
//! rule-specific keys.

use crate::model::{
    default_aspect_tolerance, default_frame_rate_tolerance, default_max_streams, default_min_phase,
    AspectRatioRule, AudioRules, BitDepthRule, ChannelLayoutRule, ColorSpaceRule, ContainerRules,
    CountThresholdRule, DbThresholdRule, DcOffsetRule, DurationThresholdRule, ExpectValueRule,
    FrameRateRule, LoudnessRule, LoudnessStandard, LumaRangeRule, MinBitrateRule, PhaseRule,
    Policy, Profile, Ratio, Resolution, ResolutionRule, RuleSetConfig, SampleRateRule,
    StreamPresenceRule, SubtitleRules, TimestampContinuityRule, ToleranceRule, VideoRules,
    VoiceRules,
};
use serde_yaml::{Mapping, Value};
use tpt_app_media_qc_model::severity::Severity;
use tpt_app_media_qc_model::Rational;

/// Errors produced while loading a profile.
#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    #[error("YAML parse error in profile: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("profile error at '{path}': {msg}")]
    Field { path: String, msg: String },
    #[error("profile error: {0}")]
    General(String),
}

fn err(path: &str, msg: impl Into<String>) -> ProfileError {
    ProfileError::Field {
        path: path.to_string(),
        msg: msg.into(),
    }
}

/// Parse YAML source into a validated [`Profile`].
pub fn parse_str(source: &str) -> Result<Profile, ProfileError> {
    let root: Value = serde_yaml::from_str(source)?;
    parse_value(&root)
}

fn parse_value(root: &Value) -> Result<Profile, ProfileError> {
    let map = as_mapping(root, "$")?;

    let name = required_string(map, "name", "$")?;
    let version = match map.get(Value::String("version".into())) {
        Some(v) => as_u64(v, "version")? as u32,
        None => 1,
    };

    let rules_map = match map.get(Value::String("rules".into())) {
        Some(v) => as_mapping(v, "rules")?.clone(),
        None => Mapping::new(),
    };

    let policy = match map.get(Value::String("policy".into())) {
        Some(v) => parse_policy(v)?,
        None => Policy::default(),
    };

    // Reject unknown top-level keys to catch typos early.
    for k in map.keys() {
        let k = as_key(k);
        if ![
            "name",
            "version",
            "rules",
            "policy",
            "description",
            "comment",
        ]
        .contains(&k.as_str())
        {
            return Err(err("$", format!("unknown top-level key '{k}'")));
        }
    }

    Ok(Profile {
        name,
        version,
        rules: parse_rules(&rules_map)?,
        policy,
    })
}

fn parse_rules(map: &Mapping) -> Result<RuleSetConfig, ProfileError> {
    for k in map.keys() {
        let k = as_key(k);
        if !["container", "video", "audio", "subtitle", "voice"].contains(&k.as_str()) {
            return Err(err("rules", format!("unknown rule group '{k}'")));
        }
    }

    Ok(RuleSetConfig {
        container: parse_container(&mapping_at(map, "container")?)?,
        video: parse_video(&mapping_at(map, "video")?)?,
        audio: parse_audio(&mapping_at(map, "audio")?)?,
        subtitle: parse_subtitle(&mapping_at(map, "subtitle")?)?,
        voice: VoiceRules::default(),
    })
}

fn mapping_at(map: &Mapping, key: &str) -> Result<Mapping, ProfileError> {
    match map.get(Value::String(key.into())) {
        Some(v) => Ok(as_mapping(v, key)?.clone()),
        None => Ok(Mapping::new()),
    }
}

// ---------------------------------------------------------------------------
// Container
// ---------------------------------------------------------------------------

fn parse_container(map: &Mapping) -> Result<ContainerRules, ProfileError> {
    for k in map.keys() {
        let k = as_key(k);
        if ![
            "readable",
            "container_validity",
            "malformed_metadata",
            "duration_consistency",
            "bitrate",
            "timecode_present",
            "timebase",
            "timestamp_continuity",
            "unexpected_streams",
            "stream_presence",
        ]
        .contains(&k.as_str())
        {
            return Err(err(
                "rules.container",
                format!("unknown container rule '{k}'"),
            ));
        }
    }

    Ok(ContainerRules {
        readable: parse_severity_rule(map, "readable", Severity::Error)?,
        container_validity: parse_severity_rule(map, "container_validity", Severity::Error)?,
        malformed_metadata: parse_severity_rule(map, "malformed_metadata", Severity::Warning)?,
        duration_consistency: parse_tolerance(map, "duration_consistency")?,
        bitrate: parse_min_bitrate(map)?,
        timecode_present: parse_severity_rule(map, "timecode_present", Severity::Info)?,
        timebase: parse_expect_value(map)?,
        timestamp_continuity: parse_timestamp_continuity(map)?,
        unexpected_streams: parse_severity_rule(map, "unexpected_streams", Severity::Warning)?,
        stream_presence: parse_stream_presence(map)?,
    })
}

fn parse_severity_rule(
    map: &Mapping,
    name: &str,
    default: Severity,
) -> Result<Option<Severity>, ProfileError> {
    let Some(v) = map.get(Value::String(name.into())) else {
        return Ok(None);
    };
    let path = format!("rules.container.{name}");
    let sev = match v {
        Value::Mapping(m) => match m.get(Value::String("severity".into())) {
            Some(s) => severity_from_value(s, &path)?,
            None => default,
        },
        _ => severity_from_value(v, &path)?,
    };
    Ok(Some(sev))
}

fn parse_tolerance(map: &Mapping, name: &str) -> Result<Option<ToleranceRule>, ProfileError> {
    let Some(spec) = rule_spec(map, name) else {
        return Ok(None);
    };
    let path = format!("rules.container.{name}");
    let (sev, cfg) = (spec.severity, spec.config);
    let tolerance_ms = match cfg {
        Some(m) => required_u64(m, "tolerance_ms", &path)?,
        None => return Err(err(&path, "missing 'tolerance_ms'")),
    };
    Ok(Some(ToleranceRule {
        tolerance_ms,
        severity: sev,
    }))
}

fn parse_min_bitrate(map: &Mapping) -> Result<Option<MinBitrateRule>, ProfileError> {
    let Some(spec) = rule_spec(map, "bitrate") else {
        return Ok(None);
    };
    let path = "rules.container.bitrate";
    let min_bps = match spec.config {
        Some(m) => required_u64(m, "min_bps", path)?,
        None => return Err(err(path, "missing 'min_bps'")),
    };
    Ok(Some(MinBitrateRule {
        min_bps,
        severity: spec.severity,
    }))
}

fn parse_expect_value(map: &Mapping) -> Result<Option<ExpectValueRule>, ProfileError> {
    let Some(spec) = rule_spec(map, "timebase") else {
        return Ok(None);
    };
    let path = "rules.container.timebase";
    let value = match spec.config {
        Some(m) => required_rational(m, "value", path)?,
        None => match spec.scalar {
            Some(scalar) => rational_from_value(scalar, path)?,
            None => return Err(err(path, "missing 'value'")),
        },
    };
    Ok(Some(ExpectValueRule {
        value,
        severity: spec.severity,
    }))
}

fn parse_timestamp_continuity(
    map: &Mapping,
) -> Result<Option<TimestampContinuityRule>, ProfileError> {
    let Some(spec) = rule_spec(map, "timestamp_continuity") else {
        return Ok(None);
    };
    let path = "rules.container.timestamp_continuity";
    let max_gap_ms = match spec.config {
        Some(m) => optional_u64(m, "max_gap_ms", path)?.unwrap_or(150),
        None => 150,
    };
    Ok(Some(TimestampContinuityRule {
        max_gap_ms,
        severity: spec.severity,
    }))
}

fn parse_stream_presence(map: &Mapping) -> Result<Option<StreamPresenceRule>, ProfileError> {
    let Some(spec) = rule_spec(map, "stream_presence") else {
        return Ok(None);
    };
    let path = "rules.container.stream_presence";
    let m = match spec.config {
        Some(m) => m,
        None => {
            return Ok(Some(StreamPresenceRule {
                min_video: 1,
                min_audio: 0,
                max_streams: default_max_streams(),
                severity: spec.severity,
            }))
        }
    };
    Ok(Some(StreamPresenceRule {
        min_video: optional_u64(m, "min_video", path)?.unwrap_or(1),
        min_audio: optional_u64(m, "min_audio", path)?.unwrap_or(0),
        max_streams: optional_u64(m, "max_streams", path)?.unwrap_or(default_max_streams()),
        severity: spec.severity,
    }))
}

// ---------------------------------------------------------------------------
// Video
// ---------------------------------------------------------------------------

fn parse_video(map: &Mapping) -> Result<VideoRules, ProfileError> {
    for k in map.keys() {
        let k = as_key(k);
        if ![
            "resolution",
            "frame_rate",
            "aspect_ratio",
            "black_frames",
            "freeze_frames",
            "duplicate_frames",
            "corrupt_frames",
            "luma_range",
            "color_space",
        ]
        .contains(&k.as_str())
        {
            return Err(err("rules.video", format!("unknown video rule '{k}'")));
        }
    }

    Ok(VideoRules {
        resolution: parse_resolution(map)?,
        frame_rate: parse_frame_rate(map)?,
        aspect_ratio: parse_aspect_ratio(map)?,
        black_frames: parse_duration_threshold(map, "black_frames")?,
        freeze_frames: parse_duration_threshold(map, "freeze_frames")?,
        duplicate_frames: parse_count_threshold(map, "duplicate_frames")?,
        corrupt_frames: parse_count_threshold(map, "corrupt_frames")?,
        luma_range: parse_luma_range(map)?,
        color_space: parse_color_space(map)?,
    })
}

fn parse_resolution(map: &Mapping) -> Result<Option<ResolutionRule>, ProfileError> {
    let Some(spec) = rule_spec(map, "resolution") else {
        return Ok(None);
    };
    let path = "rules.video.resolution";
    let expected = match spec.config {
        Some(m) => {
            let s = required_string(m, "expected", path)?;
            parse_resolution_str(&s, path)?
        }
        None => match spec.scalar {
            Some(scalar) => parse_resolution_str(&string_from_value(scalar, path)?, path)?,
            None => return Err(err(path, "missing 'expected'")),
        },
    };
    Ok(Some(ResolutionRule {
        expected,
        severity: spec.severity,
    }))
}

fn parse_resolution_str(s: &str, path: &str) -> Result<Resolution, ProfileError> {
    let (w, h) = s
        .split_once('x')
        .or_else(|| s.split_once('X'))
        .ok_or_else(|| err(path, format!("expected '<width>x<height>', got '{s}'")))?;
    let w = w
        .parse::<u64>()
        .map_err(|_| err(path, format!("invalid width '{w}'")))?;
    let h = h
        .parse::<u64>()
        .map_err(|_| err(path, format!("invalid height '{h}'")))?;
    Ok(Resolution(w, h))
}

fn parse_frame_rate(map: &Mapping) -> Result<Option<FrameRateRule>, ProfileError> {
    let Some(spec) = rule_spec(map, "frame_rate") else {
        return Ok(None);
    };
    let path = "rules.video.frame_rate";
    let expected = match spec.config {
        Some(m) => required_rational(m, "expected", path)?,
        None => match spec.scalar {
            Some(scalar) => rational_from_value(scalar, path)?,
            None => return Err(err(path, "missing 'expected'")),
        },
    };
    let tolerance = match spec.config {
        Some(m) => optional_f64(m, "tolerance", path)?.unwrap_or(default_frame_rate_tolerance()),
        None => default_frame_rate_tolerance(),
    };
    Ok(Some(FrameRateRule {
        expected,
        tolerance,
        severity: spec.severity,
    }))
}

fn parse_aspect_ratio(map: &Mapping) -> Result<Option<AspectRatioRule>, ProfileError> {
    let Some(spec) = rule_spec(map, "aspect_ratio") else {
        return Ok(None);
    };
    let path = "rules.video.aspect_ratio";
    let expected = match spec.config {
        Some(m) => {
            let s = required_string(m, "expected", path)?;
            parse_ratio_str(&s, path)?
        }
        None => match spec.scalar {
            Some(scalar) => {
                let s = string_from_value(scalar, path)?;
                parse_ratio_str(&s, path)?
            }
            None => return Err(err(path, "missing 'expected'")),
        },
    };
    let tolerance =
        optional_f64_m(spec.config, "tolerance", path)?.unwrap_or(default_aspect_tolerance());
    Ok(Some(AspectRatioRule {
        expected,
        tolerance,
        severity: spec.severity,
    }))
}

fn parse_ratio_str(s: &str, path: &str) -> Result<Ratio, ProfileError> {
    let (n, d) = s
        .split_once('/')
        .or_else(|| s.split_once(':'))
        .ok_or_else(|| err(path, format!("expected '<n>/<d>' ratio, got '{s}'")))?;
    let n = n
        .trim()
        .parse::<f64>()
        .map_err(|_| err(path, format!("invalid ratio numerator '{n}'")))?;
    let d = d
        .trim()
        .parse::<f64>()
        .map_err(|_| err(path, format!("invalid ratio denominator '{d}'")))?;
    if d == 0.0 {
        return Err(err(path, "ratio denominator must be non-zero"));
    }
    Ok(Ratio::from_parts(n, d))
}

fn parse_duration_threshold(
    map: &Mapping,
    name: &str,
) -> Result<Option<DurationThresholdRule>, ProfileError> {
    let Some(spec) = rule_spec(map, name) else {
        return Ok(None);
    };
    let path = format!("rules.video.{name}");
    let max_duration_ms = match spec.config {
        Some(m) => required_u64(m, "max_duration_ms", &path)?,
        None => return Err(err(&path, "missing 'max_duration_ms'")),
    };
    Ok(Some(DurationThresholdRule {
        max_duration_ms,
        severity: spec.severity,
    }))
}

fn parse_count_threshold(
    map: &Mapping,
    name: &str,
) -> Result<Option<CountThresholdRule>, ProfileError> {
    let Some(spec) = rule_spec(map, name) else {
        return Ok(None);
    };
    let path = format!("rules.video.{name}");
    let max_events = match spec.config {
        Some(m) => required_u64(m, "max_events", &path)?,
        None => return Err(err(&path, "missing 'max_events'")),
    };
    Ok(Some(CountThresholdRule {
        max_events,
        severity: spec.severity,
    }))
}

fn parse_luma_range(map: &Mapping) -> Result<Option<LumaRangeRule>, ProfileError> {
    let Some(spec) = rule_spec(map, "luma_range") else {
        return Ok(None);
    };
    let path = "rules.video.luma_range";
    let max_out_of_legal = match spec.config {
        Some(m) => optional_f64(m, "max_out_of_legal", path)?.unwrap_or(0.01),
        None => 0.01,
    };
    Ok(Some(LumaRangeRule {
        max_out_of_legal,
        severity: spec.severity,
    }))
}

fn parse_color_space(map: &Mapping) -> Result<Option<ColorSpaceRule>, ProfileError> {
    let Some(spec) = rule_spec(map, "color_space") else {
        return Ok(None);
    };
    let path = "rules.video.color_space";
    let expected = match spec.config {
        Some(m) => required_string(m, "expected", path)?,
        None => match spec.scalar {
            Some(scalar) => string_from_value(scalar, path)?,
            None => return Err(err(path, "missing 'expected'")),
        },
    };
    Ok(Some(ColorSpaceRule {
        expected,
        severity: spec.severity,
    }))
}

// ---------------------------------------------------------------------------
// Audio
// ---------------------------------------------------------------------------

fn parse_audio(map: &Mapping) -> Result<AudioRules, ProfileError> {
    for k in map.keys() {
        let k = as_key(k);
        if ![
            "sample_rate",
            "bit_depth",
            "channel_layout",
            "silence",
            "clipping",
            "peak",
            "true_peak",
            "loudness",
            "phase",
            "dc_offset",
        ]
        .contains(&k.as_str())
        {
            return Err(err("rules.audio", format!("unknown audio rule '{k}'")));
        }
    }

    Ok(AudioRules {
        sample_rate: parse_sample_rate(map)?,
        bit_depth: parse_bit_depth(map)?,
        channel_layout: parse_channel_layout(map)?,
        silence: parse_duration_threshold_audio(map, "silence")?,
        clipping: parse_count_threshold_audio(map, "clipping")?,
        peak: parse_db_threshold(map, "peak")?,
        true_peak: parse_db_threshold(map, "true_peak")?,
        loudness: parse_loudness(map)?,
        phase: parse_phase(map)?,
        dc_offset: parse_dc_offset(map)?,
    })
}

fn parse_sample_rate(map: &Mapping) -> Result<Option<SampleRateRule>, ProfileError> {
    let Some(spec) = rule_spec(map, "sample_rate") else {
        return Ok(None);
    };
    let path = "rules.audio.sample_rate";
    let expected = match spec.config {
        Some(m) => required_u64(m, "expected", path)?,
        None => match spec.scalar {
            Some(scalar) => as_u64(scalar, path)?,
            None => return Err(err(path, "missing 'expected'")),
        },
    };
    Ok(Some(SampleRateRule {
        expected,
        severity: spec.severity,
    }))
}

fn parse_bit_depth(map: &Mapping) -> Result<Option<BitDepthRule>, ProfileError> {
    let Some(spec) = rule_spec(map, "bit_depth") else {
        return Ok(None);
    };
    let path = "rules.audio.bit_depth";
    let expected = match spec.config {
        Some(m) => required_u64(m, "expected", path)?,
        None => match spec.scalar {
            Some(scalar) => as_u64(scalar, path)?,
            None => return Err(err(path, "missing 'expected'")),
        },
    };
    Ok(Some(BitDepthRule {
        expected,
        severity: spec.severity,
    }))
}

fn parse_duration_threshold_audio(
    map: &Mapping,
    name: &str,
) -> Result<Option<DurationThresholdRule>, ProfileError> {
    let Some(spec) = rule_spec(map, name) else {
        return Ok(None);
    };
    let path = format!("rules.audio.{name}");
    let max_duration_ms = match spec.config {
        Some(m) => required_u64(m, "max_duration_ms", &path)?,
        None => return Err(err(&path, "missing 'max_duration_ms'")),
    };
    Ok(Some(DurationThresholdRule {
        max_duration_ms,
        severity: spec.severity,
    }))
}

fn parse_count_threshold_audio(
    map: &Mapping,
    name: &str,
) -> Result<Option<CountThresholdRule>, ProfileError> {
    let Some(spec) = rule_spec(map, name) else {
        return Ok(None);
    };
    let path = format!("rules.audio.{name}");
    let max_events = match spec.config {
        Some(m) => required_u64(m, "max_events", &path)?,
        None => return Err(err(&path, "missing 'max_events'")),
    };
    Ok(Some(CountThresholdRule {
        max_events,
        severity: spec.severity,
    }))
}

fn parse_channel_layout(map: &Mapping) -> Result<Option<ChannelLayoutRule>, ProfileError> {
    let Some(spec) = rule_spec(map, "channel_layout") else {
        return Ok(None);
    };
    let path = "rules.audio.channel_layout";
    let (channels, layout) = match spec.config {
        Some(m) => (
            required_u64(m, "channels", path)?,
            optional_string(m, "layout", path)?,
        ),
        None => match spec.scalar {
            Some(scalar) => (as_u64(scalar, path)?, None),
            None => return Err(err(path, "missing 'channels'")),
        },
    };
    Ok(Some(ChannelLayoutRule {
        channels,
        layout,
        severity: spec.severity,
    }))
}

fn parse_db_threshold(map: &Mapping, name: &str) -> Result<Option<DbThresholdRule>, ProfileError> {
    let Some(spec) = rule_spec(map, name) else {
        return Ok(None);
    };
    let path = format!("rules.audio.{name}");
    let max_db = match spec.config {
        Some(m) => required_f64(m, "max_db", &path)?,
        None => return Err(err(&path, "missing 'max_db'")),
    };
    Ok(Some(DbThresholdRule {
        max_db,
        severity: spec.severity,
    }))
}

fn parse_loudness(map: &Mapping) -> Result<Option<LoudnessRule>, ProfileError> {
    let Some(spec) = rule_spec(map, "loudness") else {
        return Ok(None);
    };
    let path = "rules.audio.loudness";
    let mut rule = LoudnessRule {
        severity: spec.severity,
        ..Default::default()
    };
    if let Some(m) = spec.config {
        if let Some(v) = opt_string(m, "standard", path)? {
            rule.standard = match v.as_str() {
                "ebu-r128" | "ebu_r128" => LoudnessStandard::EbuR128,
                "atsc-a85" | "atsc_a85" => LoudnessStandard::AtscA85,
                "bs1770" | "itu-r-bs-1770" => LoudnessStandard::Bs1770,
                other => return Err(err(path, format!("unknown loudness standard '{other}'"))),
            };
        }
        if let Some(v) = optional_f64(m, "target_lufs", path)? {
            rule.target_lufs = v;
        }
        if let Some(v) = optional_f64(m, "tolerance_lu", path)? {
            rule.tolerance_lu = v;
        }
    }
    Ok(Some(rule))
}

fn parse_phase(map: &Mapping) -> Result<Option<PhaseRule>, ProfileError> {
    let Some(spec) = rule_spec(map, "phase") else {
        return Ok(None);
    };
    let path = "rules.audio.phase";
    let min_correlation = match spec.config {
        Some(m) => optional_f64(m, "min_correlation", path)?.unwrap_or(default_min_phase()),
        None => default_min_phase(),
    };
    Ok(Some(PhaseRule {
        min_correlation,
        severity: spec.severity,
    }))
}

fn parse_dc_offset(map: &Mapping) -> Result<Option<DcOffsetRule>, ProfileError> {
    let Some(spec) = rule_spec(map, "dc_offset") else {
        return Ok(None);
    };
    let path = "rules.audio.dc_offset";
    let max_offset_percent = match spec.config {
        Some(m) => optional_f64(m, "max_offset_percent", path)?.unwrap_or(5.0),
        None => 5.0,
    };
    Ok(Some(DcOffsetRule {
        max_offset_percent,
        severity: spec.severity,
    }))
}

// ---------------------------------------------------------------------------
// Subtitle
// ---------------------------------------------------------------------------

fn parse_subtitle(map: &Mapping) -> Result<SubtitleRules, ProfileError> {
    for k in map.keys() {
        let k = as_key(k);
        if !["language", "missing_subtitles"].contains(&k.as_str()) {
            return Err(err("rules.subtitle", format!("unknown subtitle key '{k}'")));
        }
    }

    let language = match map.get(Value::String("language".into())) {
        Some(v) => Some(string_from_value(v, "rules.subtitle.language")?),
        None => None,
    };
    let missing_subtitles = parse_severity_rule_sub(map)?;
    Ok(SubtitleRules {
        language,
        missing_subtitles,
    })
}

fn parse_severity_rule_sub(map: &Mapping) -> Result<Option<Severity>, ProfileError> {
    match map.get(Value::String("missing_subtitles".into())) {
        Some(v) => Ok(Some(severity_from_value(
            v,
            "rules.subtitle.missing_subtitles",
        )?)),
        None => Ok(None),
    }
}

// ---------------------------------------------------------------------------
// Policy
// ---------------------------------------------------------------------------

fn parse_policy(v: &Value) -> Result<Policy, ProfileError> {
    let map = as_mapping(v, "policy")?;
    let fail_on = match map.get(Value::String("fail_on".into())) {
        Some(v) => severity_from_value(v, "policy.fail_on")?,
        None => Severity::Error,
    };
    Ok(Policy { fail_on })
}

// ---------------------------------------------------------------------------
// Low-level value helpers
// ---------------------------------------------------------------------------

struct RuleSpec<'v> {
    severity: Severity,
    config: Option<&'v Mapping>,
    scalar: Option<&'v Value>,
}

/// Extract a rule entry that is either a scalar severity or a mapping.
fn rule_spec<'v>(map: &'v Mapping, name: &str) -> Option<RuleSpec<'v>> {
    let v = map.get(Value::String(name.into()))?;
    match v {
        Value::Mapping(m) => {
            let severity = m
                .get(Value::String("severity".into()))
                .and_then(|s| severity_from_value(s, name).ok())
                .unwrap_or_else(rule_default_severity);
            Some(RuleSpec {
                severity,
                config: Some(m),
                scalar: None,
            })
        }
        other => {
            let severity =
                severity_from_value(other, name).unwrap_or_else(|_| rule_default_severity());
            Some(RuleSpec {
                severity,
                config: None,
                scalar: Some(other),
            })
        }
    }
}

fn rule_default_severity() -> Severity {
    // Reasonable neutral default when a rule mapping omits `severity` and the
    // builder overrides it anyway via its own defaults.
    Severity::Warning
}

fn as_mapping<'v>(v: &'v Value, path: &str) -> Result<&'v Mapping, ProfileError> {
    match v {
        Value::Mapping(m) => Ok(m),
        other => Err(err(path, format!("expected a mapping, got {}", ty(other)))),
    }
}

fn as_key(k: &Value) -> String {
    match k {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        other => ty(other),
    }
}

fn severity_from_value(v: &Value, path: &str) -> Result<Severity, ProfileError> {
    let s = string_from_value(v, path)?;
    s.parse::<Severity>().map_err(|e| err(path, e))
}

fn string_from_value(v: &Value, path: &str) -> Result<String, ProfileError> {
    match v {
        Value::String(s) => Ok(s.clone()),
        Value::Number(n) => Ok(n.to_string()),
        Value::Bool(b) => Ok(b.to_string()),
        other => Err(err(path, format!("expected a string, got {}", ty(other)))),
    }
}

fn as_u64(v: &Value, path: &str) -> Result<u64, ProfileError> {
    match v {
        Value::Number(n) => n
            .as_u64()
            .ok_or_else(|| err(path, format!("expected a non-negative integer, got {n}"))),
        other => Err(err(path, format!("expected an integer, got {}", ty(other)))),
    }
}

fn rational_from_value(v: &Value, path: &str) -> Result<Rational, ProfileError> {
    match v {
        Value::Number(n) => {
            if let Some(i) = n.as_u64() {
                Rational::new(i, 1).ok_or_else(|| err(path, "invalid rational"))
            } else {
                Err(err(path, "expected an integer or '<num>/<den>' string"))
            }
        }
        Value::String(s) => {
            Rational::parse(s).ok_or_else(|| err(path, format!("invalid rational '{s}'")))
        }
        other => Err(err(path, format!("expected a rational, got {}", ty(other)))),
    }
}

fn required_u64(m: &Mapping, key: &str, path: &str) -> Result<u64, ProfileError> {
    let v = m
        .get(Value::String(key.into()))
        .ok_or_else(|| err(path, format!("missing '{key}'")))?;
    as_u64(v, &format!("{path}.{key}"))
}

fn optional_u64(m: &Mapping, key: &str, path: &str) -> Result<Option<u64>, ProfileError> {
    match m.get(Value::String(key.into())) {
        Some(v) => Ok(Some(as_u64(v, &format!("{path}.{key}"))?)),
        None => Ok(None),
    }
}

fn required_f64(m: &Mapping, key: &str, path: &str) -> Result<f64, ProfileError> {
    let v = m
        .get(Value::String(key.into()))
        .ok_or_else(|| err(path, format!("missing '{key}'")))?;
    as_f64(v, &format!("{path}.{key}"))
}

fn optional_f64(m: &Mapping, key: &str, path: &str) -> Result<Option<f64>, ProfileError> {
    match m.get(Value::String(key.into())) {
        Some(v) => Ok(Some(as_f64(v, &format!("{path}.{key}"))?)),
        None => Ok(None),
    }
}

fn optional_f64_m(
    config: Option<&Mapping>,
    key: &str,
    path: &str,
) -> Result<Option<f64>, ProfileError> {
    match config {
        Some(m) => optional_f64(m, key, path),
        None => Ok(None),
    }
}

fn as_f64(v: &Value, path: &str) -> Result<f64, ProfileError> {
    match v {
        Value::Number(n) => n
            .as_f64()
            .ok_or_else(|| err(path, format!("expected a number, got {n}"))),
        other => Err(err(path, format!("expected a number, got {}", ty(other)))),
    }
}

fn required_string(m: &Mapping, key: &str, path: &str) -> Result<String, ProfileError> {
    let v = m
        .get(Value::String(key.into()))
        .ok_or_else(|| err(path, format!("missing '{key}'")))?;
    string_from_value(v, &format!("{path}.{key}"))
}

fn optional_string(m: &Mapping, key: &str, path: &str) -> Result<Option<String>, ProfileError> {
    match m.get(Value::String(key.into())) {
        Some(v) => Ok(Some(string_from_value(v, &format!("{path}.{key}"))?)),
        None => Ok(None),
    }
}

fn opt_string(m: &Mapping, key: &str, path: &str) -> Result<Option<String>, ProfileError> {
    optional_string(m, key, path)
}

fn required_rational(m: &Mapping, key: &str, path: &str) -> Result<Rational, ProfileError> {
    let v = m
        .get(Value::String(key.into()))
        .ok_or_else(|| err(path, format!("missing '{key}'")))?;
    rational_from_value(v, &format!("{path}.{key}"))
}

fn ty(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(_) => "a bool".into(),
        Value::Number(_) => "a number".into(),
        Value::String(_) => "a string".into(),
        Value::Sequence(_) => "a sequence".into(),
        Value::Mapping(_) => "a mapping".into(),
        Value::Tagged(t) => format!("a tagged value ({})", t.tag),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"
name: "Minimal"
version: 1
"#;

    const FULL: &str = r#"
name: "Client Delivery - Example"
version: 1

rules:
  container:
    readable: error
    duration_consistency:
      tolerance_ms: 250
    bitrate:
      min_bps: 15000000

  video:
    resolution:
      expected: "3840x2160"
      severity: error
    frame_rate:
      expected: 25
      tolerance: 0
    black_frames:
      max_duration_ms: 500
      severity: warning

  audio:
    loudness:
      standard: ebu-r128
      target_lufs: -23
      tolerance_lu: 1
    true_peak:
      max_db: -1

policy:
  fail_on: error
"#;

    const WITH_UNKNOWN_RULE: &str = r#"
name: x
rules:
  container:
    bitrate_missing_ok: null
"#;

    #[test]
    fn parses_minimal() {
        let p = parse_str(MINIMAL).unwrap();
        assert_eq!(p.name, "Minimal");
        assert_eq!(p.version, 1);
        assert_eq!(p.policy.fail_on, Severity::Error);
    }

    #[test]
    fn parses_full() {
        let p = parse_str(FULL).unwrap();
        let container = &p.rules.container;
        assert_eq!(container.readable, Some(Severity::Error));
        assert_eq!(container.duration_consistency.unwrap().tolerance_ms, 250);
        assert_eq!(container.bitrate.unwrap().min_bps, 15_000_000);

        let video = &p.rules.video;
        assert_eq!(video.resolution.unwrap().expected.label(), "3840x2160");
        assert_eq!(video.resolution.unwrap().severity, Severity::Error);
        assert_eq!(
            video.frame_rate.unwrap().expected.value().round() as u64,
            25
        );
        assert_eq!(video.black_frames.unwrap().max_duration_ms, 500);
        assert_eq!(video.black_frames.unwrap().severity, Severity::Warning);
        assert!(video.aspect_ratio.is_none());

        let audio = &p.rules.audio;
        let loud = audio.loudness.unwrap();
        assert_eq!(loud.standard, LoudnessStandard::EbuR128);
        assert_eq!(loud.target_lufs, -23.0);
        assert_eq!(loud.tolerance_lu, 1.0);
        assert_eq!(audio.true_peak.unwrap().max_db, -1.0);
    }

    #[test]
    fn rejects_unknown_rule() {
        let r = parse_str(WITH_UNKNOWN_RULE);
        assert!(r.is_err());
        let msg = r.unwrap_err().to_string();
        assert!(msg.contains("bitrate_missing_ok"), "got {msg}");
        assert!(msg.contains("rules.container"), "got {msg}");
    }

    #[test]
    fn rejects_unknown_top_level_key() {
        let r = parse_str("name: x\nunknown_key: 1\n");
        assert!(r.is_err());
    }

    #[test]
    fn rejects_missing_name() {
        let r = parse_str("version: 1\n");
        assert!(r.is_err());
        assert!(r.unwrap_err().to_string().contains("name"));
    }

    #[test]
    fn rejects_bad_severity() {
        let r = parse_str("name: x\nrules:\n  container:\n    readable: severe\n");
        assert!(r.is_err());
        assert!(r.unwrap_err().to_string().contains("severe"));
    }

    #[test]
    fn scalar_rule_form_is_supported() {
        let p = parse_str(
            "name: x\nrules:\n  video:\n    frame_rate:\n      expected: 25\n      severity: error\n",
        )
        .unwrap();
        assert_eq!(p.rules.video.frame_rate.unwrap().severity, Severity::Error);
    }
}
