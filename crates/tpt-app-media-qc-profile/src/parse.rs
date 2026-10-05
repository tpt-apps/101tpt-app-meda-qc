//! YAML profile parsing and validation ([spec § 9]).
//!
//! Parsing is strict: unknown rules, unknown keys and malformed values are
//! rejected with a path-annotated error so profiles stay deterministic,
//! diffable and testable. A rule entry may be a scalar severity
//! (`black_frames: warning`) or a mapping with a `severity` field plus
//! rule-specific keys.

use crate::custom::{
    metric_kind, metric_names, valid_id, CustomOp, CustomOperand, CustomRule, CustomScope,
    CustomTarget, MetricKind,
};
use crate::model::{
    default_aspect_tolerance, default_frame_rate_tolerance, default_max_streams, default_min_phase,
    AspectRatioRule, AudioRules, BitDepthRule, BlockinessRule, BlurRule, ChannelLayoutRule,
    ColorSpaceRule, ContainerRules, CountThresholdRule, DbThresholdRule, DcOffsetRule,
    DeadPixelRule, DurationThresholdRule, ExpectValueRule, FieldOrderExpectation, FrameRateRule,
    HdrMode, HdrRule, LoudnessRule, LoudnessStandard, LumaRangeRule, MinBitrateRule, NoiseRule,
    PhaseRule, PhotosensitivityRule, Policy, Profile, Ratio, Resolution, ResolutionRule,
    RuleSetConfig, SampleRateRule, ScanExpectation, ScanFormatRule, SpeakerChangeRule,
    SpeakerCountRule, SpeechExpectation, SpeechRule, StreamPresenceRule, SubtitleContentRule,
    SubtitleDurationRule, SubtitleLanguageRule, SubtitlePresenceRule, SubtitleRules,
    SubtitleTimingRule, TimestampContinuityRule, ToleranceRule, TranscriptRule, VideoRules,
    VoiceRules, VoiceSilenceRule,
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
        if !["container", "video", "audio", "subtitle", "voice", "custom"].contains(&k.as_str()) {
            return Err(err("rules", format!("unknown rule group '{k}'")));
        }
    }

    Ok(RuleSetConfig {
        container: parse_container(&mapping_at(map, "container")?)?,
        video: parse_video(&mapping_at(map, "video")?)?,
        audio: parse_audio(&mapping_at(map, "audio")?)?,
        subtitle: parse_subtitle(&mapping_at(map, "subtitle")?)?,
        voice: parse_voice(&mapping_at(map, "voice")?)?,
        custom: parse_custom(map.get(Value::String("custom".into())))?,
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
            "scan_format",
            "photosensitivity",
            "hdr",
            "dead_pixels",
            "blockiness",
            "blur",
            "noise",
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
        scan_format: parse_scan_format(map)?,
        photosensitivity: parse_photosensitivity(map)?,
        hdr: parse_hdr(map)?,
        dead_pixels: parse_dead_pixels(map)?,
        blockiness: parse_blockiness(map)?,
        blur: parse_blur(map)?,
        noise: parse_noise(map)?,
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

fn parse_hdr(map: &Mapping) -> Result<Option<HdrRule>, ProfileError> {
    let Some(spec) = rule_spec(map, "hdr") else {
        return Ok(None);
    };
    let path = "rules.video.hdr";
    let mode_text = match (spec.config, spec.scalar) {
        (Some(m), _) => required_string(m, "mode", path)?,
        (None, Some(scalar)) => string_from_value(scalar, path)?,
        (None, None) => return Err(err(path, "missing 'mode'")),
    };
    let mode = match mode_text.trim().to_ascii_lowercase().as_str() {
        "sdr" => HdrMode::Sdr,
        "hdr10" | "pq" => HdrMode::Hdr10,
        "hlg" => HdrMode::Hlg,
        "hdr" | "any_hdr" => HdrMode::Hdr,
        other => {
            return Err(err(
                path,
                format!("unknown hdr mode '{other}' (expected sdr, hdr10, hlg or hdr)"),
            ))
        }
    };
    let mut rule = HdrRule {
        mode,
        require_static_metadata: true,
        max_cll_nits: None,
        max_fall_nits: None,
        severity: spec.severity,
    };
    if let Some(m) = spec.config {
        match m.get(Value::String("require_static_metadata".into())) {
            Some(Value::Bool(b)) => rule.require_static_metadata = *b,
            Some(_) => return Err(err(path, "require_static_metadata must be true or false")),
            None => {}
        }
        rule.max_cll_nits = optional_u64(m, "max_cll_nits", path)?.map(|v| v as u32);
        rule.max_fall_nits = optional_u64(m, "max_fall_nits", path)?.map(|v| v as u32);
    }
    Ok(Some(rule))
}

fn parse_dead_pixels(map: &Mapping) -> Result<Option<DeadPixelRule>, ProfileError> {
    let Some(spec) = rule_spec(map, "dead_pixels") else {
        return Ok(None);
    };
    let path = "rules.video.dead_pixels";
    let Some(m) = spec.config else {
        return Ok(Some(DeadPixelRule {
            max_pixels: 0,
            max_clusters: 0,
            include_flicker: true,
            fail_on_limited_resolution: false,
            severity: spec.severity,
        }));
    };
    reject_unknown_video_keys(
        m,
        path,
        &[
            "max_pixels",
            "max_clusters",
            "include_flicker",
            "fail_on_limited_resolution",
            "severity",
        ],
    )?;
    let bool_key = |m: &Mapping, key: &str| -> Result<Option<bool>, ProfileError> {
        match m.get(Value::String(key.into())) {
            Some(Value::Bool(b)) => Ok(Some(*b)),
            Some(_) => Err(err(path, format!("{key} must be true or false"))),
            None => Ok(None),
        }
    };
    Ok(Some(DeadPixelRule {
        max_pixels: optional_u64(m, "max_pixels", path)?.unwrap_or(0),
        max_clusters: optional_u64(m, "max_clusters", path)?.unwrap_or(0) as u32,
        include_flicker: bool_key(m, "include_flicker")?.unwrap_or(true),
        fail_on_limited_resolution: bool_key(m, "fail_on_limited_resolution")?.unwrap_or(false),
        severity: spec.severity,
    }))
}

fn parse_blockiness(map: &Mapping) -> Result<Option<BlockinessRule>, ProfileError> {
    let Some(spec) = rule_spec(map, "blockiness") else {
        return Ok(None);
    };
    let path = "rules.video.blockiness";
    let Some(m) = spec.config else {
        return Err(err(path, "blockiness rule requires 'max_ratio'"));
    };
    reject_unknown_video_keys(
        m,
        path,
        &[
            "max_ratio",
            "max_frame_ratio",
            "min_evidence_share",
            "severity",
        ],
    )?;
    let share = optional_f64(m, "min_evidence_share", path)?.unwrap_or_else(default_evidence_share);
    if !(0.0..=1.0).contains(&share) {
        return Err(err(path, "min_evidence_share must be between 0 and 1"));
    }
    Ok(Some(BlockinessRule {
        max_ratio: required_f64(m, "max_ratio", path)?,
        max_frame_ratio: optional_f64(m, "max_frame_ratio", path)?,
        min_evidence_share: share,
        severity: spec.severity,
    }))
}

fn parse_blur(map: &Mapping) -> Result<Option<BlurRule>, ProfileError> {
    let Some(spec) = rule_spec(map, "blur") else {
        return Ok(None);
    };
    let path = "rules.video.blur";
    let Some(m) = spec.config else {
        return Err(err(path, "blur rule requires 'min_sharpness'"));
    };
    reject_unknown_video_keys(m, path, &["min_sharpness", "severity"])?;
    Ok(Some(BlurRule {
        min_sharpness: required_f64(m, "min_sharpness", path)?,
        severity: spec.severity,
    }))
}

fn parse_noise(map: &Mapping) -> Result<Option<NoiseRule>, ProfileError> {
    let Some(spec) = rule_spec(map, "noise") else {
        return Ok(None);
    };
    let path = "rules.video.noise";
    let Some(m) = spec.config else {
        return Err(err(path, "noise rule requires 'max_sigma'"));
    };
    reject_unknown_video_keys(m, path, &["max_sigma", "min_evidence_share", "severity"])?;
    let share = optional_f64(m, "min_evidence_share", path)?.unwrap_or_else(default_full_share);
    if !(0.0..=1.0).contains(&share) {
        return Err(err(path, "min_evidence_share must be between 0 and 1"));
    }
    Ok(Some(NoiseRule {
        max_sigma: required_f64(m, "max_sigma", path)?,
        min_evidence_share: share,
        severity: spec.severity,
    }))
}

fn default_evidence_share() -> f64 {
    0.5
}

fn default_full_share() -> f64 {
    1.0
}

fn reject_unknown_video_keys(
    m: &Mapping,
    path: &str,
    allowed: &[&str],
) -> Result<(), ProfileError> {
    for k in m.keys() {
        let k = as_key(k);
        if !allowed.contains(&k.as_str()) {
            return Err(err(path, format!("unknown key '{k}'")));
        }
    }
    Ok(())
}

fn parse_photosensitivity(map: &Mapping) -> Result<Option<PhotosensitivityRule>, ProfileError> {
    let Some(spec) = rule_spec(map, "photosensitivity") else {
        return Ok(None);
    };
    let path = "rules.video.photosensitivity";
    let max_flashes_per_second = match spec.config {
        Some(m) => optional_f64(m, "max_flashes_per_second", path)?.unwrap_or(3.0),
        None => 3.0,
    };
    if !max_flashes_per_second.is_finite() || max_flashes_per_second < 0.0 {
        return Err(err(
            path,
            "max_flashes_per_second must be finite and non-negative",
        ));
    }
    Ok(Some(PhotosensitivityRule {
        max_flashes_per_second,
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

fn parse_scan_format(map: &Mapping) -> Result<Option<ScanFormatRule>, ProfileError> {
    let Some(spec) = rule_spec(map, "scan_format") else {
        return Ok(None);
    };
    let path = "rules.video.scan_format";
    let mut rule = ScanFormatRule {
        severity: spec.severity,
        ..ScanFormatRule::default()
    };
    if let Some(m) = spec.config {
        if let Some(v) = optional_string(m, "scan", path)? {
            rule.scan = parse_scan_expectation(&v, path)?;
        }
        if let Some(v) = optional_string(m, "field_order", path)? {
            rule.field_order = parse_field_order_expectation(&v, path)?;
        }
    } else if let Some(scalar) = spec.scalar {
        // Scalar shorthand: `scan_format: progressive` / `scan_format: interlaced`.
        rule.scan = parse_scan_expectation(&string_from_value(scalar, path)?, path)?;
    }
    if rule.scan == ScanExpectation::Any && rule.field_order == FieldOrderExpectation::Any {
        return Err(err(
            path,
            "scan_format rule requires 'scan' or 'field_order' expectation",
        ));
    }
    Ok(Some(rule))
}

fn parse_scan_expectation(s: &str, path: &str) -> Result<ScanExpectation, ProfileError> {
    match s.trim().to_ascii_lowercase().as_str() {
        "progressive" => Ok(ScanExpectation::Progressive),
        "interlaced" => Ok(ScanExpectation::Interlaced),
        "any" => Ok(ScanExpectation::Any),
        other => Err(err(path, format!("unknown scan expectation '{other}'"))),
    }
}

fn parse_field_order_expectation(
    s: &str,
    path: &str,
) -> Result<FieldOrderExpectation, ProfileError> {
    match s.trim().to_ascii_lowercase().as_str() {
        "top_field_first" | "tff" => Ok(FieldOrderExpectation::TopFieldFirst),
        "bottom_field_first" | "bff" => Ok(FieldOrderExpectation::BottomFieldFirst),
        "any" => Ok(FieldOrderExpectation::Any),
        other => Err(err(
            path,
            format!("unknown field-order expectation '{other}'"),
        )),
    }
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
        if ![
            "presence",
            "language",
            "timing",
            "content",
            "duration_match",
        ]
        .contains(&k.as_str())
        {
            return Err(err("rules.subtitle", format!("unknown subtitle key '{k}'")));
        }
    }

    Ok(SubtitleRules {
        presence: parse_subtitle_presence(map)?,
        language: parse_subtitle_language(map)?,
        timing: parse_subtitle_timing(map)?,
        content: parse_subtitle_content(map)?,
        duration_match: parse_subtitle_duration(map)?,
    })
}

fn parse_subtitle_presence(map: &Mapping) -> Result<Option<SubtitlePresenceRule>, ProfileError> {
    let Some(spec) = rule_spec(map, "presence") else {
        return Ok(None);
    };
    let path = "rules.subtitle.presence";
    let Some(m) = spec.config else {
        // Scalar shorthand: `presence: error` means "at least one track".
        return Ok(Some(SubtitlePresenceRule {
            min_subtitle: 1,
            max_subtitle: None,
            severity: spec.severity,
        }));
    };
    reject_unknown_keys(m, path, &["min_subtitle", "max_subtitle", "severity"])?;
    Ok(Some(SubtitlePresenceRule {
        min_subtitle: optional_u64(m, "min_subtitle", path)?.unwrap_or(1) as u32,
        max_subtitle: optional_u64(m, "max_subtitle", path)?.map(|v| v as u32),
        severity: spec.severity,
    }))
}

fn parse_subtitle_language(map: &Mapping) -> Result<Option<SubtitleLanguageRule>, ProfileError> {
    let Some(spec) = rule_spec(map, "language") else {
        return Ok(None);
    };
    let path = "rules.subtitle.language";
    let Some(m) = spec.config else {
        return Err(err(
            path,
            "language rule requires a mapping with a 'required' list",
        ));
    };
    reject_unknown_keys(m, path, &["required", "min_tracks", "severity"])?;
    let required = match m.get(Value::String("required".into())) {
        Some(Value::Sequence(seq)) => seq
            .iter()
            .map(|v| string_from_value(v, &format!("{path}.required")))
            .collect::<Result<Vec<_>, _>>()?,
        Some(other) => {
            return Err(err(
                &format!("{path}.required"),
                format!("expected a list of language codes, got {}", ty(other)),
            ))
        }
        None => return Err(err(path, "missing 'required'")),
    };
    if required.is_empty() {
        return Err(err(
            &format!("{path}.required"),
            "at least one language code is required",
        ));
    }
    Ok(Some(SubtitleLanguageRule {
        required,
        min_tracks: optional_u64(m, "min_tracks", path)?.unwrap_or(1) as u32,
        severity: spec.severity,
    }))
}

fn parse_subtitle_timing(map: &Mapping) -> Result<Option<SubtitleTimingRule>, ProfileError> {
    let Some(spec) = rule_spec(map, "timing") else {
        return Ok(None);
    };
    let path = "rules.subtitle.timing";
    let Some(m) = spec.config else {
        return Ok(Some(SubtitleTimingRule {
            max_overlaps: 0,
            max_invalid_durations: 0,
            max_gap_ms: None,
            max_cue_duration_ms: None,
            severity: spec.severity,
        }));
    };
    reject_unknown_keys(
        m,
        path,
        &[
            "max_overlaps",
            "max_invalid_durations",
            "max_gap_ms",
            "max_cue_duration_ms",
            "severity",
        ],
    )?;
    Ok(Some(SubtitleTimingRule {
        max_overlaps: optional_u64(m, "max_overlaps", path)?.unwrap_or(0) as u32,
        max_invalid_durations: optional_u64(m, "max_invalid_durations", path)?.unwrap_or(0) as u32,
        max_gap_ms: optional_u64(m, "max_gap_ms", path)?,
        max_cue_duration_ms: optional_u64(m, "max_cue_duration_ms", path)?,
        severity: spec.severity,
    }))
}

fn parse_subtitle_content(map: &Mapping) -> Result<Option<SubtitleContentRule>, ProfileError> {
    let Some(spec) = rule_spec(map, "content") else {
        return Ok(None);
    };
    let path = "rules.subtitle.content";
    let Some(m) = spec.config else {
        return Ok(Some(SubtitleContentRule {
            max_malformed: 0,
            max_empty: 0,
            max_chars_per_line: None,
            max_lines_per_cue: None,
            severity: spec.severity,
        }));
    };
    reject_unknown_keys(
        m,
        path,
        &[
            "max_malformed",
            "max_empty",
            "max_chars_per_line",
            "max_lines_per_cue",
            "severity",
        ],
    )?;
    Ok(Some(SubtitleContentRule {
        max_malformed: optional_u64(m, "max_malformed", path)?.unwrap_or(0) as u32,
        max_empty: optional_u64(m, "max_empty", path)?.unwrap_or(0) as u32,
        max_chars_per_line: optional_u64(m, "max_chars_per_line", path)?.map(|v| v as u32),
        max_lines_per_cue: optional_u64(m, "max_lines_per_cue", path)?.map(|v| v as u32),
        severity: spec.severity,
    }))
}

fn parse_subtitle_duration(map: &Mapping) -> Result<Option<SubtitleDurationRule>, ProfileError> {
    let Some(spec) = rule_spec(map, "duration_match") else {
        return Ok(None);
    };
    let path = "rules.subtitle.duration_match";
    let Some(m) = spec.config else {
        return Ok(Some(SubtitleDurationRule {
            tolerance_ms: default_subtitle_tolerance(),
            allow_longer: true,
            severity: spec.severity,
        }));
    };
    reject_unknown_keys(m, path, &["tolerance_ms", "allow_longer", "severity"])?;
    let allow_longer = match m.get(Value::String("allow_longer".into())) {
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(err(path, "allow_longer must be true or false")),
        None => true,
    };
    Ok(Some(SubtitleDurationRule {
        tolerance_ms: optional_u64(m, "tolerance_ms", path)?
            .unwrap_or_else(default_subtitle_tolerance),
        allow_longer,
        severity: spec.severity,
    }))
}

fn default_subtitle_tolerance() -> u64 {
    1000
}

fn reject_unknown_keys(m: &Mapping, path: &str, allowed: &[&str]) -> Result<(), ProfileError> {
    for k in m.keys() {
        let k = as_key(k);
        if !allowed.contains(&k.as_str()) {
            return Err(err(path, format!("unknown key '{k}'")));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Voice (optional, probabilistic - spec 8.8)
// ---------------------------------------------------------------------------

fn parse_voice(map: &Mapping) -> Result<VoiceRules, ProfileError> {
    reject_unknown_keys(
        map,
        "rules.voice",
        &[
            "speech",
            "silence",
            "speakers",
            "speaker_changes",
            "transcript",
        ],
    )?;
    Ok(VoiceRules {
        speech: parse_speech(map)?,
        silence: parse_voice_silence(map)?,
        speakers: parse_speakers(map)?,
        speaker_changes: parse_speaker_changes(map)?,
        transcript: parse_transcript(map)?,
    })
}

/// A voice rule has no meaningful scalar form: it needs thresholds.
fn voice_config<'v>(
    map: &'v Mapping,
    name: &str,
    allowed: &[&str],
) -> Result<Option<(&'v Mapping, Severity)>, ProfileError> {
    let Some(spec) = rule_spec(map, name) else {
        return Ok(None);
    };
    let path = format!("rules.voice.{name}");
    let Some(m) = spec.config else {
        return Err(err(&path, "expected a mapping with thresholds"));
    };
    reject_unknown_keys(m, &path, allowed)?;
    Ok(Some((m, spec.severity)))
}

fn parse_speech(map: &Mapping) -> Result<Option<SpeechRule>, ProfileError> {
    let Some((m, severity)) = voice_config(map, "speech", &["expect", "min_ratio", "severity"])?
    else {
        return Ok(None);
    };
    let path = "rules.voice.speech";
    let expect = match required_string(m, "expect", path)?.as_str() {
        "present" => SpeechExpectation::Present,
        "absent" => SpeechExpectation::Absent,
        other => {
            return Err(err(
                &format!("{path}.expect"),
                format!("unknown expectation '{other}' (present, absent)"),
            ))
        }
    };
    let min_ratio = optional_f64(m, "min_ratio", path)?.unwrap_or(0.05);
    if !(0.0..=1.0).contains(&min_ratio) {
        return Err(err(
            &format!("{path}.min_ratio"),
            "must be between 0.0 and 1.0",
        ));
    }
    Ok(Some(SpeechRule {
        expect,
        min_ratio,
        severity,
    }))
}

fn parse_voice_silence(map: &Mapping) -> Result<Option<VoiceSilenceRule>, ProfileError> {
    let Some((m, severity)) = voice_config(map, "silence", &["max_non_speech_ms", "severity"])?
    else {
        return Ok(None);
    };
    Ok(Some(VoiceSilenceRule {
        max_non_speech_ms: required_u64(m, "max_non_speech_ms", "rules.voice.silence")?,
        severity,
    }))
}

fn parse_speakers(map: &Mapping) -> Result<Option<SpeakerCountRule>, ProfileError> {
    let Some((m, severity)) = voice_config(map, "speakers", &["min", "max", "severity"])? else {
        return Ok(None);
    };
    let path = "rules.voice.speakers";
    let min = optional_u64(m, "min", path)?.map(|v| v as u32);
    let max = optional_u64(m, "max", path)?.map(|v| v as u32);
    if min.is_none() && max.is_none() {
        return Err(err(path, "set at least one of 'min' or 'max'"));
    }
    if matches!((min, max), (Some(lo), Some(hi)) if lo > hi) {
        return Err(err(path, "'min' must not exceed 'max'"));
    }
    Ok(Some(SpeakerCountRule { min, max, severity }))
}

fn parse_speaker_changes(map: &Mapping) -> Result<Option<SpeakerChangeRule>, ProfileError> {
    let Some((m, severity)) =
        voice_config(map, "speaker_changes", &["max_per_minute", "severity"])?
    else {
        return Ok(None);
    };
    let path = "rules.voice.speaker_changes";
    let max_per_minute = required_f64(m, "max_per_minute", path)?;
    if !max_per_minute.is_finite() || max_per_minute < 0.0 {
        return Err(err(
            &format!("{path}.max_per_minute"),
            "must be a non-negative number",
        ));
    }
    Ok(Some(SpeakerChangeRule {
        max_per_minute,
        severity,
    }))
}

fn parse_transcript(map: &Mapping) -> Result<Option<TranscriptRule>, ProfileError> {
    let Some((m, severity)) = voice_config(
        map,
        "transcript",
        &["max_wer", "max_unexpected_words", "severity"],
    )?
    else {
        return Ok(None);
    };
    let path = "rules.voice.transcript";
    let max_wer = required_f64(m, "max_wer", path)?;
    if !max_wer.is_finite() || max_wer < 0.0 {
        return Err(err(
            &format!("{path}.max_wer"),
            "must be a non-negative number",
        ));
    }
    Ok(Some(TranscriptRule {
        max_wer,
        max_unexpected_words: optional_u64(m, "max_unexpected_words", path)?.unwrap_or(0),
        severity,
    }))
}

// ---------------------------------------------------------------------------
// Custom rules
// ---------------------------------------------------------------------------

/// Upper bound on user rules per profile, to keep reports and caches sane.
const MAX_CUSTOM_RULES: usize = 256;

fn parse_custom(v: Option<&Value>) -> Result<Vec<CustomRule>, ProfileError> {
    let seq = match v {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Sequence(s)) => s,
        Some(other) => {
            return Err(err(
                "rules.custom",
                format!("expected a list of rules, got {}", ty(other)),
            ))
        }
    };
    if seq.len() > MAX_CUSTOM_RULES {
        return Err(err(
            "rules.custom",
            format!("at most {MAX_CUSTOM_RULES} custom rules are allowed"),
        ));
    }
    let mut rules: Vec<CustomRule> = Vec::with_capacity(seq.len());
    for (i, item) in seq.iter().enumerate() {
        let rule = parse_custom_rule(item, &format!("rules.custom[{i}]"))?;
        if rules.iter().any(|r| r.id == rule.id) {
            return Err(err(
                &format!("rules.custom[{i}].id"),
                format!("duplicate custom rule id '{}'", rule.id),
            ));
        }
        rules.push(rule);
    }
    Ok(rules)
}

fn parse_custom_rule(v: &Value, path: &str) -> Result<CustomRule, ProfileError> {
    let m = as_mapping(v, path)?;
    reject_unknown_keys(
        m,
        path,
        &[
            "id",
            "scope",
            "streams",
            "metric",
            "op",
            "value",
            "tolerance",
            "severity",
            "message",
        ],
    )?;

    let id = required_string(m, "id", path)?;
    if !valid_id(&id) {
        return Err(err(
            &format!("{path}.id"),
            format!("'{id}' must look like 'custom.snake_case_name'"),
        ));
    }

    let scope_str = required_string(m, "scope", path)?;
    let scope = CustomScope::parse(&scope_str).ok_or_else(|| {
        err(
            &format!("{path}.scope"),
            format!("unknown scope '{scope_str}' (container, video, audio, subtitle)"),
        )
    })?;

    let streams = match optional_string(m, "streams", path)?.as_deref() {
        None | Some("all") => CustomTarget::All,
        Some("primary") => CustomTarget::Primary,
        Some(other) => {
            return Err(err(
                &format!("{path}.streams"),
                format!("unknown stream selection '{other}' (all, primary)"),
            ))
        }
    };
    if scope == CustomScope::Container && m.contains_key(Value::String("streams".into())) {
        return Err(err(
            &format!("{path}.streams"),
            "'streams' does not apply to container rules",
        ));
    }

    let metric = required_string(m, "metric", path)?;
    let kind = metric_kind(scope, &metric).ok_or_else(|| {
        err(
            &format!("{path}.metric"),
            format!(
                "unknown {} metric '{metric}' (available: {})",
                scope.as_str(),
                metric_names(scope).join(", ")
            ),
        )
    })?;

    let op_str = required_string(m, "op", path)?;
    let op = CustomOp::parse(&op_str).ok_or_else(|| {
        err(
            &format!("{path}.op"),
            format!("unknown operator '{op_str}' (==, !=, <, <=, >, >=, in, not_in)"),
        )
    })?;

    let value_path = format!("{path}.value");
    let raw = m
        .get(Value::String("value".into()))
        .ok_or_else(|| err(path, "missing 'value'"))?;
    let operands: Vec<CustomOperand> = match raw {
        Value::Sequence(items) => items
            .iter()
            .map(|i| custom_operand(i, &value_path))
            .collect::<Result<_, _>>()?,
        scalar => vec![custom_operand(scalar, &value_path)?],
    };
    if operands.is_empty() {
        return Err(err(&value_path, "at least one value is required"));
    }
    if !op.is_set() && operands.len() != 1 {
        return Err(err(
            &value_path,
            format!("operator '{}' takes exactly one value", op.symbol()),
        ));
    }
    if op.is_ordering() && kind == MetricKind::Text {
        return Err(err(
            &format!("{path}.op"),
            format!("operator '{}' needs a numeric metric", op.symbol()),
        ));
    }
    for o in &operands {
        match (kind, o) {
            (MetricKind::Number, CustomOperand::Text(t)) => {
                return Err(err(
                    &value_path,
                    format!("metric '{metric}' is numeric but value '{t}' is text"),
                ))
            }
            (MetricKind::Text, CustomOperand::Number(n)) => {
                return Err(err(
                    &value_path,
                    format!("metric '{metric}' is text but value {n} is a number; quote it"),
                ))
            }
            _ => {}
        }
    }

    let tolerance = optional_f64(m, "tolerance", path)?.unwrap_or(0.0);
    if !tolerance.is_finite() || tolerance < 0.0 {
        return Err(err(
            &format!("{path}.tolerance"),
            "tolerance must be a non-negative number",
        ));
    }
    if tolerance > 0.0 && (kind == MetricKind::Text || op.is_ordering() || op.is_set()) {
        return Err(err(
            &format!("{path}.tolerance"),
            "tolerance only applies to numeric '==' and '!='",
        ));
    }

    let severity = match m.get(Value::String("severity".into())) {
        Some(v) => severity_from_value(v, &format!("{path}.severity"))?,
        None => Severity::Error,
    };
    let message = optional_string(m, "message", path)?;

    Ok(CustomRule {
        id,
        scope,
        streams,
        metric,
        op,
        value: operands,
        tolerance,
        severity,
        message,
    })
}

fn custom_operand(v: &Value, path: &str) -> Result<CustomOperand, ProfileError> {
    match v {
        Value::Number(n) => n
            .as_f64()
            .filter(|f| f.is_finite())
            .map(CustomOperand::Number)
            .ok_or_else(|| err(path, "number is not finite")),
        Value::String(s) => Ok(CustomOperand::Text(s.clone())),
        Value::Bool(b) => Ok(CustomOperand::Text(b.to_string())),
        other => Err(err(
            path,
            format!("expected a number or string, got {}", ty(other)),
        )),
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
