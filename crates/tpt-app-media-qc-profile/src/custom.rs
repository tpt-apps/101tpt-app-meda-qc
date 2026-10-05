//! User-defined declarative rules (the "custom rule builder", spec § 27).
//!
//! A custom rule compares one metadata metric of a container or stream with
//! a threshold and reports a finding when the comparison does not hold:
//!
//! ```yaml
//! rules:
//!   custom:
//!     - id: custom.min_video_bitrate
//!       scope: video
//!       metric: bitrate
//!       op: ">="
//!       value: 5000000
//!       severity: error
//!       message: Delivery requires at least 5 Mb/s video
//! ```
//!
//! The metric catalogue is closed and validated when the profile is parsed,
//! so a typo is a profile error rather than a rule that silently never fires.

use serde::{Deserialize, Serialize};

use tpt_app_media_qc_model::severity::Severity;

/// What a custom rule inspects.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CustomScope {
    /// The container / asset as a whole.
    Container,
    Video,
    Audio,
    Subtitle,
}

impl CustomScope {
    pub fn as_str(self) -> &'static str {
        match self {
            CustomScope::Container => "container",
            CustomScope::Video => "video",
            CustomScope::Audio => "audio",
            CustomScope::Subtitle => "subtitle",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "container" => Some(CustomScope::Container),
            "video" => Some(CustomScope::Video),
            "audio" => Some(CustomScope::Audio),
            "subtitle" => Some(CustomScope::Subtitle),
            _ => None,
        }
    }
}

/// Which streams of the scope's kind a stream-scoped rule applies to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CustomTarget {
    /// Every stream of the kind.
    #[default]
    All,
    /// Only the first stream of the kind.
    Primary,
}

/// Comparison operator.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CustomOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    OneOf,
    NoneOf,
}

impl CustomOp {
    pub fn symbol(self) -> &'static str {
        match self {
            CustomOp::Eq => "==",
            CustomOp::Ne => "!=",
            CustomOp::Lt => "<",
            CustomOp::Le => "<=",
            CustomOp::Gt => ">",
            CustomOp::Ge => ">=",
            CustomOp::OneOf => "in",
            CustomOp::NoneOf => "not_in",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "==" | "=" | "eq" => Some(CustomOp::Eq),
            "!=" | "ne" => Some(CustomOp::Ne),
            "<" | "lt" => Some(CustomOp::Lt),
            "<=" | "le" => Some(CustomOp::Le),
            ">" | "gt" => Some(CustomOp::Gt),
            ">=" | "ge" => Some(CustomOp::Ge),
            "in" | "one_of" => Some(CustomOp::OneOf),
            "not_in" | "none_of" => Some(CustomOp::NoneOf),
            _ => None,
        }
    }

    /// Ordering operators only make sense on numbers.
    pub fn is_ordering(self) -> bool {
        matches!(
            self,
            CustomOp::Lt | CustomOp::Le | CustomOp::Gt | CustomOp::Ge
        )
    }

    pub fn is_set(self) -> bool {
        matches!(self, CustomOp::OneOf | CustomOp::NoneOf)
    }
}

/// A comparison operand.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CustomOperand {
    Number(f64),
    Text(String),
}

impl CustomOperand {
    pub fn label(&self) -> String {
        match self {
            CustomOperand::Number(n) => format_number(*n),
            CustomOperand::Text(s) => s.clone(),
        }
    }
}

/// Render a number without a trailing `.0` for whole values.
pub fn format_number(n: f64) -> String {
    if n.is_finite() && n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// One user-defined rule.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CustomRule {
    /// Rule id; always `custom.<name>` so it can never shadow a built-in.
    pub id: String,
    pub scope: CustomScope,
    #[serde(default)]
    pub streams: CustomTarget,
    pub metric: String,
    pub op: CustomOp,
    /// One operand for scalar operators, one or more for `in` / `not_in`.
    pub value: Vec<CustomOperand>,
    /// Absolute tolerance applied to numeric `==` / `!=`.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub tolerance: f64,
    #[serde(default)]
    pub severity: Severity,
    /// Replaces the generated failure message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

/// Value type of a metric.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetricKind {
    Number,
    Text,
}

const CONTAINER_METRICS: &[(&str, MetricKind)] = &[
    ("duration_seconds", MetricKind::Number),
    ("size_bytes", MetricKind::Number),
    ("stream_count", MetricKind::Number),
    ("video_stream_count", MetricKind::Number),
    ("audio_stream_count", MetricKind::Number),
    ("subtitle_stream_count", MetricKind::Number),
];

const COMMON_STREAM_METRICS: &[(&str, MetricKind)] = &[
    ("codec", MetricKind::Text),
    ("codec_profile", MetricKind::Text),
    ("language", MetricKind::Text),
    ("bitrate", MetricKind::Number),
    ("duration_seconds", MetricKind::Number),
];

const VIDEO_METRICS: &[(&str, MetricKind)] = &[
    ("width", MetricKind::Number),
    ("height", MetricKind::Number),
    ("frame_rate", MetricKind::Number),
    ("pixel_format", MetricKind::Text),
    ("field_order", MetricKind::Text),
];

const AUDIO_METRICS: &[(&str, MetricKind)] = &[
    ("channels", MetricKind::Number),
    ("channel_layout", MetricKind::Text),
    ("sample_rate", MetricKind::Number),
    ("bit_depth", MetricKind::Number),
];

/// Prefix for free-form per-stream container tags (`metadata.title`).
pub const METADATA_PREFIX: &str = "metadata.";

/// Look up a metric's kind for a scope, or `None` if it is not offered.
pub fn metric_kind(scope: CustomScope, metric: &str) -> Option<MetricKind> {
    if scope != CustomScope::Container {
        if let Some(key) = metric.strip_prefix(METADATA_PREFIX) {
            return (!key.is_empty()).then_some(MetricKind::Text);
        }
    }
    let specific: &[(&str, MetricKind)] = match scope {
        CustomScope::Container => &[],
        CustomScope::Video => VIDEO_METRICS,
        CustomScope::Audio => AUDIO_METRICS,
        CustomScope::Subtitle => &[],
    };
    let common = if scope == CustomScope::Container {
        CONTAINER_METRICS
    } else {
        COMMON_STREAM_METRICS
    };
    common
        .iter()
        .chain(specific)
        .find(|(name, _)| *name == metric)
        .map(|(_, kind)| *kind)
}

/// Metric names offered for a scope, for error messages and docs.
pub fn metric_names(scope: CustomScope) -> Vec<String> {
    let mut names: Vec<String> = match scope {
        CustomScope::Container => CONTAINER_METRICS.iter().map(|m| m.0.to_string()).collect(),
        _ => COMMON_STREAM_METRICS
            .iter()
            .chain(match scope {
                CustomScope::Video => VIDEO_METRICS,
                CustomScope::Audio => AUDIO_METRICS,
                _ => &[],
            })
            .map(|m| m.0.to_string())
            .collect(),
    };
    if scope != CustomScope::Container {
        names.push(format!("{METADATA_PREFIX}<key>"));
    }
    names
}

/// Check that a rule id is `custom.<snake_case>`.
pub fn valid_id(id: &str) -> bool {
    match id.strip_prefix("custom.") {
        Some(name) => {
            !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        }
        None => false,
    }
}
