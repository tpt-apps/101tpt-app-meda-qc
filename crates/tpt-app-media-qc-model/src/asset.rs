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

/// Media stream abbreviations mirroring ffprobe/container conventions.
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

impl StreamId {
    pub fn new(index: u64) -> Self {
        Self(index)
    }

    pub fn as_usize(&self) -> usize {
        self.0 as usize
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
