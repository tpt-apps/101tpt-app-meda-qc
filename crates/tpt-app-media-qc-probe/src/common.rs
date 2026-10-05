//! Types shared by every container reader.

use std::collections::BTreeMap;

use tpt_app_media_qc_model::asset::Stream;
use tpt_app_media_qc_model::finding::TimeRange;
use tpt_app_media_qc_model::inspection::{ContainerValidity, VideoMeasurements};

/// Largest single box / element / chunk header payload a reader will buffer.
/// Readers walk containers with seeks and never read a sample payload larger
/// than this into memory.
pub const MAX_BUFFERED_BYTES: u64 = 16 * 1024 * 1024;

/// Upper bound on cues kept per subtitle stream.
pub const MAX_CUES: usize = 100_000;

/// One subtitle cue as stored in a container.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawCue {
    pub start_ms: u64,
    pub end_ms: u64,
    /// Cue payload for text codecs (container framing already removed where
    /// the container wraps it). `None` for non-text codecs.
    pub payload: Option<Vec<u8>>,
}

/// One track/stream found in a container.
#[derive(Clone, Debug)]
pub struct ProbedStream {
    /// Model stream; `index` is the stream's position in
    /// [`ProbedContainer::streams`] (the stream-index contract).
    pub stream: Stream,
    /// The container's own identifier: MP4/Matroska track id, TS PID, `0` for
    /// single-stream files. Decode adapters use it to find the same stream.
    pub native_id: u64,
    /// Video-only measurements the container can supply (declared/observed
    /// frame rate, colour tags, HDR, field order).
    pub video: Option<VideoMeasurements>,
    /// Subtitle cues, for subtitle streams.
    pub cues: Vec<RawCue>,
    /// Whether `cues` was cut at [`MAX_CUES`].
    pub cues_truncated: bool,
}

/// Everything a reader learned about one file.
#[derive(Clone, Debug)]
pub struct ProbedContainer {
    /// Short format name, e.g. `mp4`, `matroska,webm`, `mpegts`, `wav`.
    pub format: &'static str,
    /// Every track, supported or passive, in the container's natural order.
    pub streams: Vec<ProbedStream>,
    pub duration_ms: Option<u64>,
    /// `Some(true)` if a timecode track/tag exists, `Some(false)` if the
    /// container kind can carry one and none was found, `None` if the
    /// container kind cannot.
    pub timecode_present: Option<bool>,
    /// `Some(true)` when packet timestamps were examined and were contiguous.
    pub timestamps_contiguous: Option<bool>,
    pub timestamp_gaps: Vec<TimeRange>,
    pub malformed_metadata: Vec<String>,
    /// `Ok`, or `Corrupt(reason)` for a recognised but damaged file.
    pub validity: ContainerValidity,
    /// Extra facts for the report body (e.g. TS integrity counters).
    pub diagnostics: BTreeMap<String, serde_json::Value>,
}

impl ProbedContainer {
    /// An empty, valid container of `format`.
    pub fn new(format: &'static str) -> Self {
        Self {
            format,
            streams: Vec::new(),
            duration_ms: None,
            timecode_present: None,
            timestamps_contiguous: None,
            timestamp_gaps: Vec::new(),
            malformed_metadata: Vec::new(),
            validity: ContainerValidity::Ok,
            diagnostics: BTreeMap::new(),
        }
    }

    /// A recognised container that could not be read to the end.
    pub fn corrupt(format: &'static str, reason: impl Into<String>) -> Self {
        let mut c = Self::new(format);
        c.validity = ContainerValidity::Corrupt(reason.into());
        c
    }
}
