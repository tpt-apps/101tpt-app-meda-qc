//! Asset and stream model types ([spec § 6.1, § 6.2]).

use crate::time::{DurationSeconds, FrameRate, TimeBase};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

/// Stable identifier for an asset (`AssetId`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AssetId(pub uuid::Uuid);

impl AssetId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

impl Default for AssetId {
    fn default() -> Self {
        Self::new()
    }
}

/// A media asset under QC ([spec § 6.1]).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Asset {
    pub id: AssetId,
    pub path: PathBuf,
    pub fingerprint: AssetFingerprint,
    pub size_bytes: u64,
    pub modified_time: Option<u64>,
    pub duration: Option<DurationSeconds>,
    pub streams: Vec<Stream>,
}

/// Content-based fingerprint with size, used for identity and cache keys.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetFingerprint {
    /// Hex-encoded SHA-256 of file content (or bounded prefix).
    pub sha256: String,
    pub size_bytes: u64,
}

/// Media stream kinds, following common container conventions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StreamKind {
    Video,
    Audio,
    Subtitle,
    Data,
    Attachment,
    Unknown,
}

impl StreamKind {
    pub fn as_str(self) -> &'static str {
        match self {
            StreamKind::Video => "video",
            StreamKind::Audio => "audio",
            StreamKind::Subtitle => "subtitle",
            StreamKind::Data => "data",
            StreamKind::Attachment => "attachment",
            StreamKind::Unknown => "unknown",
        }
    }
}

/// Monotonic stream index within a container.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StreamId(pub u64);

/// Scanning order (interlacement) reported for a video stream.
///
/// Values mirror what container/probe front-ends expose (the conventional
/// `field_order`). Mixed coded/display orders (`tb`/`bt`) carry no reliable
/// display order and are reported as [`FieldOrder::Unknown`] rather than
/// guessed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldOrder {
    /// Progressive scan — no field structure.
    Progressive,
    /// Interlaced, top field displayed first.
    TopFieldFirst,
    /// Interlaced, bottom field displayed first.
    BottomFieldFirst,
    /// Field order unknown or not signaled by the container/codec. The
    /// picture may be progressive or interlaced; the scan structure simply
    /// was not reported.
    Unknown,
}

impl FieldOrder {
    pub fn as_str(self) -> &'static str {
        match self {
            FieldOrder::Progressive => "progressive",
            FieldOrder::TopFieldFirst => "top_field_first",
            FieldOrder::BottomFieldFirst => "bottom_field_first",
            FieldOrder::Unknown => "unknown",
        }
    }

    /// Whether the stream is *known* to carry an interlaced (field-based)
    /// picture. `Unknown` is not evidence of interlacement.
    pub fn is_interlaced(self) -> bool {
        matches!(
            self,
            FieldOrder::TopFieldFirst | FieldOrder::BottomFieldFirst
        )
    }

    /// Parse a probe-reported field-order tag (e.g. `tff`, `bb`, `progressive`).
    /// Recognised values are matched case-insensitively; unknown tags yield
    /// `None` so callers can distinguish "not reported" from "unrecognised".
    pub fn parse_probe(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "progressive" => Some(FieldOrder::Progressive),
            "tt" | "tff" | "top_field_first" | "topfieldfirst" => Some(FieldOrder::TopFieldFirst),
            "bb" | "bff" | "bottom_field_first" | "bottomfieldfirst" => {
                Some(FieldOrder::BottomFieldFirst)
            }
            // `tb`/`bt` (coded vs display order differs), `unknown` and empty
            // values mean interlaced-but-unspecified or simply unspecified.
            "tb" | "bt" | "unknown" | "unspecified" | "" => Some(FieldOrder::Unknown),
            _ => None,
        }
    }
}

impl StreamId {
    pub fn new(index: u64) -> Self {
        Self(index)
    }

    pub fn as_usize(&self) -> usize {
        self.0 as usize
    }

    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for StreamId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// One elementary stream discovered in a container ([spec § 6.2]).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Stream {
    pub index: StreamId,
    pub kind: StreamKind,
    pub codec: Option<String>,
    pub codec_profile: Option<String>,
    pub width: Option<u64>,
    pub height: Option<u64>,
    pub pixel_format: Option<String>,
    /// Scanning order reported by the container/probe front-end.
    #[serde(default)]
    pub field_order: Option<FieldOrder>,
    /// Real (non-display) frame rate.
    pub frame_rate: Option<FrameRate>,
    pub time_base: Option<TimeBase>,
    /// Bits per second (container-level value for validation).
    pub bitrate: Option<u64>,
    pub duration: Option<DurationSeconds>,
    pub language: Option<String>,
    pub channel_layout: Option<String>,
    pub channels: Option<u64>,
    pub sample_rate: Option<u64>,
    pub bit_depth: Option<u64>,
    /// Is this primary (first of its kind) stream? Primary streams are the
    /// default subjects for default-rule checks.
    pub metadata: BTreeMap<String, String>,
}

impl Stream {
    pub fn dimension_label(&self) -> Option<String> {
        if let (Some(w), Some(h)) = (self.width, self.height) {
            Some(format!("{w}x{h}"))
        } else {
            None
        }
    }
}

/// Convenience helpers for building test/model streams.
impl Stream {
    pub fn primary_video(index: u64) -> Self {
        Self {
            index: StreamId::new(index),
            kind: StreamKind::Video,
            codec: Some("h264".into()),
            codec_profile: None,
            width: Some(1920),
            height: Some(1080),
            pixel_format: Some("yuv420p".into()),
            field_order: Some(FieldOrder::Progressive),
            frame_rate: Some(FrameRate::from_parts(25, 1)),
            time_base: Some(TimeBase::from_parts(1, 12800)),
            bitrate: Some(8_000_000),
            duration: Some(Duration::from_secs(120).into()),
            language: None,
            channel_layout: None,
            channels: None,
            sample_rate: None,
            bit_depth: None,
            metadata: BTreeMap::new(),
        }
    }

    pub fn primary_audio(index: u64) -> Self {
        Self {
            index: StreamId::new(index),
            kind: StreamKind::Audio,
            codec: Some("aac".into()),
            codec_profile: Some("LC".into()),
            width: None,
            height: None,
            pixel_format: None,
            field_order: None,
            frame_rate: None,
            time_base: Some(TimeBase::from_parts(1, 48000)),
            bitrate: Some(320_000),
            duration: Some(Duration::from_secs(120).into()),
            language: Some("eng".into()),
            channel_layout: Some("stereo".into()),
            channels: Some(2),
            sample_rate: Some(48000),
            bit_depth: Some(16),
            metadata: BTreeMap::new(),
        }
    }
}
