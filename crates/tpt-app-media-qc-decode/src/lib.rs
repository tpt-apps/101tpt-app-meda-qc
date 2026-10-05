//! Kinetix-backed video inspection for TPT Media QC.
//!
//! Kinetix owns MP4 and Matroska/WebM demuxing and AV1/VP9 reconstruction;
//! this crate reduces decoded frames to the stable [`Inspection`] model
//! consumed by QC rules. H.264 is deliberately not decoded: the AVC patent
//! pools license decoders as well as encoders. Files in other codecs still get
//! refused as an unsupported format by the metadata inspector.
//! Both decoders run in Kinetix strict mode, so a stream they cannot
//! reconstruct faithfully is reported as unmeasurable instead of being
//! analysed. The pinned demuxers are in-memory, so large inputs are refused
//! rather than loaded without a bound. Decoded frames are analysed one at a
//! time.

mod audio;
pub mod correct;
mod deadpixels;
mod loudness;
mod perception;
mod pse;
pub mod transcript;
mod voice;

pub use audio::{AudioAnalyzerConfig, CadenceAudioInspector};
pub use voice::{VoiceAnalyzer, VoiceConfig, MAX_ANALYSED_SECONDS};

use tpt_app_media_qc_core::error::{Error, Result};
use tpt_app_media_qc_model::asset::Asset;
use tpt_app_media_qc_model::inspection::{Inspection, LumaStats, VideoMeasurements};
use tpt_app_media_qc_model::time::{FrameRate, Rational};
use tpt_app_media_qc_model::TimeRange;
use tpt_app_media_qc_pipeline::{InspectionLevel, Inspector};
use tpt_kinetix_av1::Av1Decoder;
use tpt_kinetix_core::codec::{CodecId, MediaType};
use tpt_kinetix_core::error::KinetixError;
use tpt_kinetix_core::frame::VideoFrame;
use tpt_kinetix_core::packet::Packet;
use tpt_kinetix_core::pixel_format::PixelFormat;
use tpt_kinetix_demux::{Demuxer, MkvDemuxer, Mp4Demuxer, TsDemuxer};
use tpt_kinetix_vp9::Vp9Decoder;

/// Maximum input accepted by the current in-memory Kinetix demuxers.
pub const DEFAULT_MAX_DECODE_INPUT_BYTES: u64 = 512 * 1024 * 1024;

/// Royalty-free video codecs the adapter can decode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VideoCodec {
    Av1,
    Vp9,
}

impl VideoCodec {
    fn label(self) -> &'static str {
        match self {
            Self::Av1 => "AV1",
            Self::Vp9 => "VP9",
        }
    }
}

enum VideoDecoder {
    Av1(Box<Av1Decoder>),
    Vp9(Box<Vp9Decoder>),
}

impl VideoDecoder {
    fn new(codec: VideoCodec) -> Self {
        match codec {
            // Strict mode makes a decoder refuse to hand back frames it cannot
            // reconstruct faithfully (AV1 grey placeholders), so they are never
            // measured as real video.
            VideoCodec::Av1 => Self::Av1(Box::new(Av1Decoder::new().with_strict(true))),
            VideoCodec::Vp9 => Self::Vp9(Box::new(Vp9Decoder::new().with_strict(true))),
        }
    }

    fn decode(&mut self, packet: &Packet) -> std::result::Result<Option<VideoFrame>, KinetixError> {
        match self {
            Self::Av1(decoder) => decoder.decode(packet),
            Self::Vp9(decoder) => decoder.decode(packet),
        }
    }

    fn flush(&mut self) -> std::result::Result<Vec<VideoFrame>, KinetixError> {
        match self {
            Self::Av1(decoder) => decoder.flush(),
            Self::Vp9(_) => Ok(Vec::new()),
        }
    }
}

/// A demuxer positioned on the video track chosen for decode.
struct VideoSource {
    demuxer: Box<dyn Demuxer>,
    codec: VideoCodec,
    /// Index of the track within the container; used as the metadata stream index.
    stream_idx: u64,
    /// `Packet::stream_index` value that belongs to the chosen track.
    packet_stream: u32,
    /// Sample count when the container reports it up front.
    expected_packets: Option<usize>,
}

/// The video stream the metadata pass chose, so decode and metadata agree on
/// which stream a measurement belongs to even when a container parser drops a
/// damaged track the other one kept.
#[derive(Clone, Copy, Debug)]
struct PreferredVideo {
    /// Stream index in the metadata inspection.
    stream_idx: u64,
    /// The container's own identifier (track id, track number or PID).
    native_id: u64,
}

/// First AV1/VP9 video stream in the metadata inspection that carries its
/// container-native id.
fn preferred_video(metadata: &Inspection) -> Option<PreferredVideo> {
    metadata.streams.iter().find_map(|stream| {
        let is_decodable = stream.kind == tpt_app_media_qc_model::asset::StreamKind::Video
            && matches!(stream.codec.as_deref(), Some("av1" | "vp9"));
        let native_id = stream.metadata.get("native_id")?.parse().ok()?;
        is_decodable.then_some(PreferredVideo {
            stream_idx: stream.index.as_u64(),
            native_id,
        })
    })
}

/// A decodable track found in a demuxer.
struct Candidate {
    /// Position in the demuxer's own track list.
    index: usize,
    native_id: u64,
    /// Sample count when the container reports it up front.
    samples: Option<usize>,
    codec: VideoCodec,
}

/// Pick the candidate the metadata pass chose (by native id), else the first.
/// Returns the candidate and the stream index to report measurements under.
fn choose(
    candidates: Vec<Candidate>,
    preferred: Option<PreferredVideo>,
) -> Option<(Candidate, u64)> {
    let by_id = preferred.and_then(|p| {
        candidates
            .iter()
            .position(|c| c.native_id == p.native_id)
            .map(|at| (at, p.stream_idx))
    });
    match by_id {
        Some((at, stream_idx)) => candidates.into_iter().nth(at).map(|c| (c, stream_idx)),
        None => candidates.into_iter().next().map(|c| {
            let idx = c.index as u64;
            (c, idx)
        }),
    }
}

fn is_mkv(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0x1A, 0x45, 0xDF, 0xA3])
}

fn is_iso_bmff(bytes: &[u8]) -> bool {
    bytes.get(4..8).is_some_and(|tag| {
        matches!(
            tag,
            b"ftyp" | b"moov" | b"mdat" | b"free" | b"skip" | b"wide" | b"styp"
        )
    })
}

/// MPEG-TS is a raw transport stream: fixed 188-byte packets each starting with
/// the `0x47` sync byte. Check the first two packets so an incidental leading
/// `0x47` in some other container does not claim the file.
fn is_mpeg_ts(bytes: &[u8]) -> bool {
    const TS_PACKET_LEN: usize = 188;
    const SYNC_BYTE: u8 = 0x47;
    bytes.len() >= TS_PACKET_LEN * 2 && bytes[0] == SYNC_BYTE && bytes[TS_PACKET_LEN] == SYNC_BYTE
}

fn open_video_source(
    bytes: Vec<u8>,
    preferred: Option<PreferredVideo>,
) -> std::result::Result<VideoSource, DecodeFailure> {
    if is_mkv(&bytes) {
        let demuxer = MkvDemuxer::new(bytes).map_err(|error| {
            DecodeFailure::Failed(Error::Probe(format!("Kinetix MKV demux failed: {error}")))
        })?;
        let candidates: Vec<Candidate> = demuxer
            .tracks()
            .iter()
            .enumerate()
            .filter_map(|(index, track)| {
                let codec = match track.codec_id.as_str() {
                    "V_AV1" => VideoCodec::Av1,
                    "V_VP9" => VideoCodec::Vp9,
                    _ => return None,
                };
                Some(Candidate {
                    index,
                    native_id: track.track_number,
                    samples: None,
                    codec,
                })
            })
            .collect();
        let Some((found, stream_idx)) = choose(candidates, preferred) else {
            return Err(unsupported_video_codec(
                demuxer.tracks().iter().map(|track| track.codec_id.as_str()),
            ));
        };
        return Ok(VideoSource {
            demuxer: Box::new(demuxer),
            codec: found.codec,
            stream_idx,
            packet_stream: found.native_id as u32,
            expected_packets: None,
        });
    }
    if is_iso_bmff(&bytes) {
        let demuxer = Mp4Demuxer::new(bytes).map_err(|error| {
            DecodeFailure::Failed(Error::Probe(format!("Kinetix MP4 demux failed: {error}")))
        })?;
        let candidates: Vec<Candidate> = demuxer
            .tracks()
            .iter()
            .enumerate()
            .filter_map(|(index, track)| {
                if track.media_type != MediaType::Video {
                    return None;
                }
                let codec = match track.codec {
                    Some(CodecId::Av1) => VideoCodec::Av1,
                    Some(CodecId::Vp9) => VideoCodec::Vp9,
                    _ => return None,
                };
                Some(Candidate {
                    index,
                    native_id: u64::from(track.track_id),
                    samples: Some(track.sample_count()),
                    codec,
                })
            })
            .collect();
        let Some((found, stream_idx)) = choose(candidates, preferred) else {
            return Err(unsupported_video_codec(
                demuxer
                    .tracks()
                    .iter()
                    .filter(|track| track.media_type == MediaType::Video)
                    .map(|track| track.codec.map(|codec| codec.name()).unwrap_or("unknown")),
            ));
        };
        return Ok(VideoSource {
            demuxer: Box::new(demuxer),
            codec: found.codec,
            stream_idx,
            packet_stream: found.index as u32,
            expected_packets: found.samples,
        });
    }
    if is_mpeg_ts(&bytes) {
        let demuxer = TsDemuxer::new(bytes).map_err(|error| {
            DecodeFailure::Failed(Error::Probe(format!(
                "Kinetix MPEG-TS demux failed: {error}"
            )))
        })?;
        // `streams()` is PID-ordered, the same order the metadata pass uses.
        let streams = demuxer.streams();
        let candidates: Vec<Candidate> = streams
            .iter()
            .enumerate()
            .filter_map(|(index, stream)| {
                if stream.media_type != MediaType::Video {
                    return None;
                }
                let codec = match stream.codec {
                    Some(CodecId::Av1) => VideoCodec::Av1,
                    Some(CodecId::Vp9) => VideoCodec::Vp9,
                    _ => return None,
                };
                Some(Candidate {
                    index,
                    native_id: u64::from(stream.pid),
                    samples: None,
                    codec,
                })
            })
            .collect();
        let Some((found, stream_idx)) = choose(candidates, preferred) else {
            return Err(unsupported_video_codec(
                streams
                    .iter()
                    .filter(|stream| stream.media_type == MediaType::Video)
                    .map(|stream| stream.codec.map(|codec| codec.name()).unwrap_or("unknown")),
            ));
        };
        return Ok(VideoSource {
            demuxer: Box::new(demuxer),
            codec: found.codec,
            stream_idx,
            packet_stream: found.native_id as u32,
            expected_packets: None,
        });
    }
    Err(DecodeFailure::Unsupported(
        "frame decode supports MP4, Matroska/WebM and MPEG-TS containers only".into(),
    ))
}

fn unsupported_video_codec<'a>(found: impl Iterator<Item = &'a str>) -> DecodeFailure {
    let found: Vec<&str> = found.collect();
    DecodeFailure::Unsupported(format!(
        "frame decode supports AV1 and VP9 video only (H.264 is not decoded because of \
         patent licensing); found: {}",
        if found.is_empty() {
            "no video track".to_string()
        } else {
            found.join(", ")
        }
    ))
}

fn map_decode_error(codec: VideoCodec, error: KinetixError) -> DecodeFailure {
    match error {
        KinetixError::NotPixelExact(reason) => DecodeFailure::Unsupported(format!(
            "Kinetix {} decoder cannot reconstruct this stream faithfully: {reason}",
            codec.label()
        )),
        other => DecodeFailure::Failed(Error::Probe(format!(
            "Kinetix {} decode failed: {other}",
            codec.label()
        ))),
    }
}

/// Deterministic thresholds for the video analysis pass.
#[derive(Clone, Debug, PartialEq)]
pub struct VideoAnalyzerConfig {
    pub black_luma_threshold: u8,
    pub freeze_max_mean_abs_delta: f64,
    pub duplicate_max_mean_abs_delta: f64,
    pub nominal_frame_rate: Option<FrameRate>,
}

impl Default for VideoAnalyzerConfig {
    fn default() -> Self {
        Self {
            black_luma_threshold: 16,
            freeze_max_mean_abs_delta: 0.5,
            duplicate_max_mean_abs_delta: 0.0,
            nominal_frame_rate: None,
        }
    }
}

/// Full-decode inspector for MP4 and Matroska/WebM files containing AV1 or VP9 video.
pub struct KinetixVideoInspector {
    config: VideoAnalyzerConfig,
    max_input_bytes: u64,
}

impl KinetixVideoInspector {
    pub fn new() -> Self {
        Self::with_config(VideoAnalyzerConfig::default())
    }

    pub fn with_config(config: VideoAnalyzerConfig) -> Self {
        Self {
            config,
            max_input_bytes: DEFAULT_MAX_DECODE_INPUT_BYTES,
        }
    }

    pub fn with_max_input_bytes(mut self, max_input_bytes: u64) -> Self {
        self.max_input_bytes = max_input_bytes;
        self
    }

    fn decode_video(
        &self,
        asset: &Asset,
        metadata: &Inspection,
    ) -> std::result::Result<(VideoMeasurements, VideoCodec), DecodeFailure> {
        let size = std::fs::metadata(&asset.path)
            .map_err(|error| DecodeFailure::Failed(error.into()))?
            .len();
        if size > self.max_input_bytes {
            return Err(DecodeFailure::Unsupported(format!(
                "input is {size} bytes; the current Kinetix adapter limit is {} bytes",
                self.max_input_bytes
            )));
        }
        let bytes =
            std::fs::read(&asset.path).map_err(|error| DecodeFailure::Failed(error.into()))?;
        let mut source = open_video_source(bytes, preferred_video(metadata))?;
        let codec = source.codec;
        let stream_idx = source.stream_idx;
        let nominal_frame_rate = metadata
            .video_for(stream_idx)
            .and_then(|video| video.frame_rate_observed)
            .or(self.config.nominal_frame_rate);
        let mut analyzer = VideoFrameAnalyzer::new(stream_idx, nominal_frame_rate, &self.config);
        decode_source(&mut source, &mut analyzer)?;

        let mut measurement = analyzer.finish();
        measurement.colorspace = metadata
            .video_for(stream_idx)
            .and_then(|video| video.colorspace.clone());
        measurement.hdr = metadata
            .video_for(stream_idx)
            .and_then(|video| video.hdr.clone());
        Ok((measurement, codec))
    }
}

/// Feeds every packet of the chosen track through the decoder into `analyzer`.
fn decode_source(
    source: &mut VideoSource,
    analyzer: &mut VideoFrameAnalyzer,
) -> std::result::Result<(), DecodeFailure> {
    let codec = source.codec;
    let mut decoder = VideoDecoder::new(codec);
    if source.expected_packets == Some(0) {
        return Err(DecodeFailure::Failed(Error::Probe(
            "Kinetix video track contains no samples".into(),
        )));
    }
    let mut video_packets_seen = 0usize;

    loop {
        let packet = source.demuxer.read_packet().map_err(|error| {
            DecodeFailure::Failed(Error::Probe(format!("Kinetix packet read failed: {error}")))
        })?;
        let Some(packet) = packet else { break };
        if packet.stream_index != source.packet_stream {
            continue;
        }
        video_packets_seen += 1;
        let frame = decoder
            .decode(&packet)
            .map_err(|error| map_decode_error(codec, error))?;
        if let Some(frame) = frame {
            analyzer.push(frame).map_err(DecodeFailure::Failed)?;
        }
        if source
            .expected_packets
            .is_some_and(|expected| video_packets_seen >= expected)
        {
            break;
        }
    }
    if let Some(expected) = source.expected_packets {
        if video_packets_seen < expected {
            return Err(DecodeFailure::Failed(Error::Probe(format!(
                "Kinetix demux ended after {video_packets_seen} of {expected} video samples"
            ))));
        }
    }

    let flushed = decoder
        .flush()
        .map_err(|error| map_decode_error(codec, error))?;
    for frame in flushed {
        analyzer.push(frame).map_err(DecodeFailure::Failed)?;
    }
    if analyzer.frame_count() == 0 {
        // No frames from a track we could open means the stream could not be
        // measured (for example missing sequence headers), not that it is corrupt.
        return Err(DecodeFailure::Unsupported(format!(
            "Kinetix {} decoder produced no frames",
            codec.label()
        )));
    }
    Ok(())
}

enum DecodeFailure {
    Unsupported(String),
    Failed(Error),
}

impl Default for KinetixVideoInspector {
    fn default() -> Self {
        Self::new()
    }
}

impl Inspector for KinetixVideoInspector {
    fn name(&self) -> &str {
        "tpt-kinetix-av1-vp9"
    }

    fn inspect_metadata(&self, _asset: &Asset) -> Result<Inspection> {
        Err(Error::Unsupported(
            "KinetixVideoInspector provides full video decode; use a metadata inspector for pass 1"
                .into(),
        ))
    }

    fn inspect_decode(
        &self,
        asset: &Asset,
        metadata: &Inspection,
        level: InspectionLevel,
    ) -> Result<Inspection> {
        if level != InspectionLevel::Full {
            return Ok(metadata.clone());
        }
        if metadata.video.is_empty() {
            let mut incomplete = metadata.clone();
            incomplete.diagnostics.insert(
                "video_decode".into(),
                serde_json::json!({
                    "status": "not_applicable",
                    "reason": "metadata inspection reported no video streams",
                }),
            );
            return Ok(incomplete);
        }

        match self.decode_video(asset, metadata) {
            Ok((measurement, codec)) => {
                let mut decoded = metadata.clone();
                let stream_idx = measurement.stream_idx;
                let frame_count = measurement.decoded_frame_count;
                decoded.video.retain(|video| video.stream_idx != stream_idx);
                decoded.video.push(measurement);
                decoded.video.sort_by_key(|video| video.stream_idx);
                decoded.diagnostics.insert(
                    "video_decode".into(),
                    serde_json::json!({
                        "status": "complete",
                        "backend": "tpt-kinetix-demux + tpt-kinetix-av1/vp9",
                        "codec": codec.label(),
                        "frames": frame_count,
                        "luma_sampling": "all decoded luma samples",
                        "black_threshold_max_luma": self.config.black_luma_threshold,
                        "freeze_threshold_mean_abs_delta": self.config.freeze_max_mean_abs_delta,
                    }),
                );
                Ok(decoded)
            }
            Err(DecodeFailure::Unsupported(reason)) => {
                Ok(incomplete_video_inspection(metadata, &reason, false))
            }
            Err(DecodeFailure::Failed(error)) => Ok(incomplete_video_inspection(
                metadata,
                &error.to_string(),
                true,
            )),
        }
    }
}

fn incomplete_video_inspection(
    metadata: &Inspection,
    reason: &str,
    count_decode_error: bool,
) -> Inspection {
    let mut inspection = metadata.clone();
    for video in &mut inspection.video {
        video.decoded_frame_count = Some(0);
        if count_decode_error {
            video.decode_errors = video.decode_errors.saturating_add(1);
        }
    }
    inspection.diagnostics.insert(
        "video_decode".into(),
        serde_json::json!({
            "status": "incomplete",
            "reason": reason,
            "decode_error_recorded": count_decode_error,
        }),
    );
    inspection
}

#[derive(Clone, Copy, Debug, Default)]
struct LumaTotals {
    samples: u64,
    min: u8,
    max: u8,
    total: u128,
    below_legal: u64,
    above_legal: u64,
    clipped_white: u64,
}

impl LumaTotals {
    fn push(&mut self, sample: u8) {
        self.samples = self.samples.saturating_add(1);
        self.min = if self.samples == 1 {
            sample
        } else {
            self.min.min(sample)
        };
        self.max = self.max.max(sample);
        self.total = self.total.saturating_add(sample as u128);
        self.below_legal = self.below_legal.saturating_add((sample < 16) as u64);
        self.above_legal = self.above_legal.saturating_add((sample > 235) as u64);
        self.clipped_white = self.clipped_white.saturating_add((sample == 255) as u64);
    }

    fn mean(self) -> f64 {
        self.total as f64 / self.samples as f64
    }

    fn finish(self) -> Option<LumaStats> {
        (self.samples > 0).then(|| LumaStats {
            min: self.min,
            max: self.max,
            mean: self.mean(),
            below_legal: self.below_legal as f64 / self.samples as f64,
            above_legal: self.above_legal as f64 / self.samples as f64,
            clipped_white: self.clipped_white as f64 / self.samples as f64,
        })
    }
}

#[derive(Clone, Copy, Debug)]
struct FrameLuma {
    stats: LumaTotals,
    hash: u64,
}

#[derive(Clone, Copy, Debug)]
struct FrameObservation {
    timestamp_ms: u64,
    duration_ms: u64,
    mean: f64,
    hash: u64,
}

#[derive(Debug, Default)]
struct SegmentTracker {
    current: Option<(u64, u64)>,
}

impl SegmentTracker {
    fn push(&mut self, start_ms: u64, end_ms: u64, qualifies: bool, out: &mut Vec<TimeRange>) {
        if !qualifies {
            self.flush(out);
            return;
        }
        let end_ms = end_ms.max(start_ms);
        match &mut self.current {
            Some((_, current_end)) => *current_end = (*current_end).max(end_ms),
            None => self.current = Some((start_ms, end_ms)),
        }
    }

    fn flush(&mut self, out: &mut Vec<TimeRange>) {
        if let Some((start, end)) = self.current.take() {
            if end > start {
                out.push(TimeRange::new(start, end));
            }
        }
    }
}

/// Incrementally reduces decoded frames to QC measurements.
pub struct VideoFrameAnalyzer {
    stream_idx: u64,
    config: VideoAnalyzerConfig,
    nominal_frame_rate: Option<FrameRate>,
    previous: Option<FrameObservation>,
    first_timestamp_ms: Option<u64>,
    frame_count: u64,
    luma: LumaTotals,
    black: SegmentTracker,
    freeze: SegmentTracker,
    duplicate: SegmentTracker,
    black_ranges: Vec<TimeRange>,
    freeze_ranges: Vec<TimeRange>,
    duplicate_ranges: Vec<TimeRange>,
    flash: pse::FlashDetector,
    dead_pixels: deadpixels::DeadPixelDetector,
    perception: perception::PerceptionAnalyzer,
}

impl VideoFrameAnalyzer {
    pub fn new(
        stream_idx: u64,
        nominal_frame_rate: Option<FrameRate>,
        config: &VideoAnalyzerConfig,
    ) -> Self {
        Self {
            stream_idx,
            config: config.clone(),
            nominal_frame_rate,
            previous: None,
            first_timestamp_ms: None,
            frame_count: 0,
            luma: LumaTotals::default(),
            black: SegmentTracker::default(),
            freeze: SegmentTracker::default(),
            duplicate: SegmentTracker::default(),
            black_ranges: Vec::new(),
            freeze_ranges: Vec::new(),
            duplicate_ranges: Vec::new(),
            flash: pse::FlashDetector::default(),
            dead_pixels: deadpixels::DeadPixelDetector::new(deadpixels::DeadPixelConfig::default()),
            perception: perception::PerceptionAnalyzer::default(),
        }
    }

    pub fn frame_count(&self) -> usize {
        self.frame_count as usize
    }

    pub fn push(&mut self, frame: VideoFrame) -> Result<()> {
        if !self.config.freeze_max_mean_abs_delta.is_finite()
            || self.config.freeze_max_mean_abs_delta < 0.0
            || !self.config.duplicate_max_mean_abs_delta.is_finite()
            || self.config.duplicate_max_mean_abs_delta < 0.0
        {
            return Err(Error::Value(
                "video analyzer thresholds must be finite and non-negative".into(),
            ));
        }
        let luma = frame_luma(&frame)?;
        let timestamp_ms = frame
            .pts
            .as_millis()
            .and_then(|value| u64::try_from(value).ok())
            .or_else(|| {
                self.previous
                    .map(|previous| previous.timestamp_ms.saturating_add(previous.duration_ms))
            })
            .unwrap_or(0);
        let duration_ms = self.frame_duration_ms(timestamp_ms);
        if let Some(grid) = pse::grid_frame(&frame) {
            self.flash.push(timestamp_ms, &grid);
        }
        // Stuck-pixel analysis needs its own pass over the luma plane; it keeps
        // per-cell state, so it cannot share the streaming luma totals.
        if let Some(reduced) =
            deadpixels::DeadPixelDetector::reduce(frame.width, frame.height, luma_values(&frame)?)
        {
            self.dead_pixels.push(reduced);
        }
        // Perceptual metrics need the same luma plane again, box-averaged down
        // to the analysis grid. Only worth doing for pictures large enough to
        // compare block boundaries against block interiors.
        if frame.width >= 16 && frame.height >= 16 {
            let stride = perception::analysis_stride(frame.width, frame.height);
            if let Some(plane) = perception::PerceptionAnalyzer::reduce(
                frame.width,
                frame.height,
                stride,
                luma_values(&frame)?,
            ) {
                self.perception.push(plane);
            }
        }
        let end_ms = timestamp_ms.saturating_add(duration_ms);
        let mean = luma.stats.mean();
        let delta = self.previous.map(|previous| (mean - previous.mean).abs());
        let qualifies_freeze =
            delta.is_some_and(|value| value <= self.config.freeze_max_mean_abs_delta);
        let qualifies_duplicate = delta.is_some_and(|value| {
            value <= self.config.duplicate_max_mean_abs_delta
                && self
                    .previous
                    .is_some_and(|previous| previous.hash == luma.hash)
        });
        let qualifies_black = luma.stats.max <= self.config.black_luma_threshold;

        self.black.push(
            timestamp_ms,
            end_ms,
            qualifies_black,
            &mut self.black_ranges,
        );
        if let (Some(previous), Some(_)) = (self.previous, delta) {
            self.freeze.push(
                previous.timestamp_ms,
                end_ms,
                qualifies_freeze,
                &mut self.freeze_ranges,
            );
            self.duplicate.push(
                timestamp_ms,
                end_ms,
                qualifies_duplicate,
                &mut self.duplicate_ranges,
            );
        }
        self.first_timestamp_ms.get_or_insert(timestamp_ms);
        self.luma = merge_luma(self.luma, luma.stats);
        self.previous = Some(FrameObservation {
            timestamp_ms,
            duration_ms,
            mean,
            hash: luma.hash,
        });
        self.frame_count = self.frame_count.saturating_add(1);
        Ok(())
    }

    fn frame_duration_ms(&self, timestamp_ms: u64) -> u64 {
        self.nominal_frame_rate
            .filter(|rate| rate.value().is_finite() && rate.value() > 0.0)
            .map(|rate| (1000.0 / rate.value()).round() as u64)
            .filter(|duration| *duration > 0)
            .or_else(|| {
                self.previous
                    .and_then(|previous| timestamp_ms.checked_sub(previous.timestamp_ms))
            })
            .unwrap_or(1)
    }

    pub fn finish(mut self) -> VideoMeasurements {
        self.black.flush(&mut self.black_ranges);
        self.freeze.flush(&mut self.freeze_ranges);
        self.duplicate.flush(&mut self.duplicate_ranges);
        let observed_rate = self
            .first_timestamp_ms
            .zip(self.previous.map(|previous| previous.timestamp_ms))
            .and_then(|(first, last)| {
                (self.frame_count > 1 && last > first)
                    .then(|| {
                        Rational::new(1000u64.saturating_mul(self.frame_count - 1), last - first)
                    })
                    .flatten()
            })
            .or(self.nominal_frame_rate);
        VideoMeasurements {
            stream_idx: self.stream_idx,
            frame_rate_observed: observed_rate,
            decoded_frame_count: Some(self.frame_count),
            decode_errors: 0,
            black_frames: self.black_ranges,
            freeze_frames: self.freeze_ranges,
            duplicate_frames: self.duplicate_ranges,
            luma: self.luma.finish(),
            colorspace: None,
            field_order: None,
            flash: Some(self.flash.finish()),
            hdr: None,
            dead_pixels: self.dead_pixels.finish(),
            perceptual: self.perception.finish(),
        }
    }
}

fn merge_luma(mut total: LumaTotals, frame: LumaTotals) -> LumaTotals {
    if frame.samples == 0 {
        return total;
    }
    if total.samples == 0 {
        return frame;
    }
    total.samples = total.samples.saturating_add(frame.samples);
    total.min = total.min.min(frame.min);
    total.max = total.max.max(frame.max);
    total.total = total.total.saturating_add(frame.total);
    total.below_legal = total.below_legal.saturating_add(frame.below_legal);
    total.above_legal = total.above_legal.saturating_add(frame.above_legal);
    total.clipped_white = total.clipped_white.saturating_add(frame.clipped_white);
    total
}

fn frame_luma(frame: &VideoFrame) -> Result<FrameLuma> {
    let values = luma_values(frame)?;
    let mut stats = LumaTotals::default();
    let mut hash = 0xcbf29ce484222325u64;
    for value in values {
        stats.push(value);
        hash ^= value as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    if stats.samples == 0 {
        return Err(Error::Probe(
            "decoded frame contained no luma samples".into(),
        ));
    }
    Ok(FrameLuma { stats, hash })
}

/// Row-major 8-bit luma samples of a decoded frame.
fn luma_values(frame: &VideoFrame) -> Result<Box<dyn Iterator<Item = u8> + '_>> {
    let pixels = (frame.width as usize)
        .checked_mul(frame.height as usize)
        .ok_or_else(|| Error::Probe("decoded frame dimensions overflow".into()))?;
    let values: Box<dyn Iterator<Item = u8> + '_> = match frame.pixel_format {
        PixelFormat::Yuv420p | PixelFormat::Yuv422p | PixelFormat::Yuv444p | PixelFormat::Gray => {
            let plane = frame.data.get(..pixels).ok_or_else(|| {
                Error::Probe(format!(
                    "decoded frame has {} luma bytes; expected {pixels}",
                    frame.data.len()
                ))
            })?;
            Box::new(plane.iter().copied())
        }
        PixelFormat::Yuv420p10le
        | PixelFormat::Yuv422p10le
        | PixelFormat::Yuv444p10le
        | PixelFormat::Gray10le
        | PixelFormat::Yuv420p12le
        | PixelFormat::Yuv422p12le
        | PixelFormat::Yuv444p12le
        | PixelFormat::Gray12le => {
            // Luma is the first plane in every high-bit-depth layout. Scale it
            // to 8 bits so legal-range and black thresholds stay comparable.
            let shift = match frame.pixel_format {
                PixelFormat::Yuv420p10le
                | PixelFormat::Yuv422p10le
                | PixelFormat::Yuv444p10le
                | PixelFormat::Gray10le => 2,
                _ => 4,
            };
            let bytes = pixels
                .checked_mul(2)
                .ok_or_else(|| Error::Probe("high-bit-depth frame size overflow".into()))?;
            let plane = frame.data.get(..bytes).ok_or_else(|| {
                Error::Probe(format!(
                    "decoded frame has {} luma bytes; expected {bytes}",
                    frame.data.len()
                ))
            })?;
            Box::new(
                plane.chunks_exact(2).map(move |word| {
                    (u16::from_le_bytes([word[0], word[1]]) >> shift).min(255) as u8
                }),
            )
        }
        PixelFormat::Rgb24 | PixelFormat::Bgr24 => {
            let bytes = pixels
                .checked_mul(3)
                .ok_or_else(|| Error::Probe("RGB frame size overflow".into()))?;
            let data = frame.data.get(..bytes).ok_or_else(|| {
                Error::Probe(format!(
                    "decoded frame has {} RGB bytes; expected {bytes}",
                    frame.data.len()
                ))
            })?;
            let bgr = frame.pixel_format == PixelFormat::Bgr24;
            Box::new(data.chunks_exact(3).map(move |pixel| rgb_luma(pixel, bgr)))
        }
    };
    Ok(values)
}

fn rgb_luma(pixel: &[u8], bgr: bool) -> u8 {
    let (red, green, blue) = if bgr {
        (pixel[2], pixel[1], pixel[0])
    } else {
        (pixel[0], pixel[1], pixel[2])
    };
    (0.2126 * red as f64 + 0.7152 * green as f64 + 0.0722 * blue as f64)
        .round()
        .clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpt_app_media_qc_model::asset::AssetFingerprint;
    use tpt_app_media_qc_model::inspection::{ContainerInspection, ContainerValidity};
    use tpt_kinetix_av1::{Av1Encoder, Av1EncoderConfig};
    use tpt_kinetix_core::timestamp::Timestamp;

    fn decoded_frame(luma: &[u8], timestamp_ms: i64) -> VideoFrame {
        let mut data = luma.to_vec();
        data.resize((luma.len() * 3 / 2).max(luma.len()), 16);
        VideoFrame {
            pts: Timestamp::new(timestamp_ms, (1, 1000)),
            dts: Timestamp::new(timestamp_ms, (1, 1000)),
            data,
            width: luma.len() as u32,
            height: 1,
            pixel_format: PixelFormat::Yuv420p,
            is_key_frame: false,
        }
    }

    /// A full-resolution 64x64 frame whose luma plane is `level`, except the
    /// single cell at `pinned` which is forced to `pin`.
    fn wide_frame(level: u8, pinned: Option<(u32, u32)>, pin: u8, timestamp_ms: i64) -> VideoFrame {
        let (w, h) = (64u32, 64u32);
        let mut luma = vec![level; (w * h) as usize];
        if let Some((x, y)) = pinned {
            luma[(y * w + x) as usize] = pin;
        }
        let mut data = luma;
        data.resize((w * h * 3 / 2) as usize, 128);
        let pts = Timestamp::new(timestamp_ms, (1, 1000));
        VideoFrame {
            pts,
            dts: pts,
            data,
            width: w,
            height: h,
            pixel_format: PixelFormat::Yuv420p,
            is_key_frame: timestamp_ms == 0,
        }
    }

    #[test]
    fn dead_pixel_analysis_runs_on_a_full_decode_and_finds_no_defect_in_clean_content() {
        let mut analyzer = VideoFrameAnalyzer::new(
            0,
            Some(Rational::new(25, 1).unwrap()),
            &VideoAnalyzerConfig::default(),
        );
        // Alternating flat white / flat black: the hardest case for false
        // positives, because every pixel changes level constantly.
        for i in 0..8 {
            let level = if i % 2 == 0 { 235 } else { 16 };
            analyzer.push(wide_frame(level, None, 0, i * 40)).unwrap();
        }
        let stats = analyzer.finish().dead_pixels.expect("analysis ran");
        assert_eq!(stats.frames_sampled, 8);
        assert_eq!(stats.cells_examined, 64 * 64);
        assert!(!stats.resolution_limited);
        assert_eq!(stats.total_cells(), 0, "{stats:?}");
    }

    #[test]
    fn dead_pixel_analysis_finds_a_pinned_cell_across_a_real_decode() {
        let mut analyzer = VideoFrameAnalyzer::new(
            0,
            Some(Rational::new(25, 1).unwrap()),
            &VideoAnalyzerConfig::default(),
        );
        // Cell (10, 20) stays black while the rest of the frame follows the scene.
        for i in 0..8 {
            let level = if i % 2 == 0 { 235 } else { 16 };
            analyzer
                .push(wide_frame(level, Some((10, 20)), 0, i * 40))
                .unwrap();
        }
        let stats = analyzer.finish().dead_pixels.expect("analysis ran");
        assert!(stats.dark_cells >= 1, "{stats:?}");
        assert!(!stats.clusters.is_empty());
        // The cluster is reported in source pixels so it can be located.
        let cluster = stats.clusters[0];
        assert_eq!(
            cluster.kind,
            tpt_app_media_qc_model::inspection::DeadPixelKind::Dead
        );
        assert!(
            cluster.x <= 10 && cluster.x + cluster.width > 10,
            "{cluster:?}"
        );
        assert!(
            cluster.y <= 20 && cluster.y + cluster.height > 20,
            "{cluster:?}"
        );
    }

    #[test]
    fn frame_analyzer_reports_exact_ranges_luma_and_frame_rate() {
        let mut analyzer = VideoFrameAnalyzer::new(
            0,
            Some(Rational::new(10, 1).unwrap()),
            &VideoAnalyzerConfig::default(),
        );
        analyzer.push(decoded_frame(&[0, 0], 0)).unwrap();
        analyzer.push(decoded_frame(&[0, 0], 100)).unwrap();
        analyzer.push(decoded_frame(&[80, 80], 300)).unwrap();

        let result = analyzer.finish();
        assert_eq!(result.decoded_frame_count, Some(3));
        assert_eq!(result.decode_errors, 0);
        assert_eq!(result.black_frames, vec![TimeRange::new(0, 200)]);
        assert_eq!(result.freeze_frames, vec![TimeRange::new(0, 200)]);
        assert_eq!(result.duplicate_frames, vec![TimeRange::new(100, 200)]);
        assert_eq!(result.frame_rate_observed, Rational::new(20, 3));
        assert_eq!(result.luma.unwrap().max, 80);
    }

    /// Encodes `count` moving-gradient 64x64 frames to AV1 packets.
    fn av1_packets(count: u8) -> Vec<Packet> {
        let config = Av1EncoderConfig {
            width: 64,
            height: 64,
            bitrate: 0,
            quantizer: 100,
            speed: 10,
            keyframe_interval: 4,
        };
        let mut encoder = Av1Encoder::new(&config).expect("create AV1 encoder");
        let mut packets = Vec::new();
        for index in 0..count {
            let mut data = Vec::with_capacity(64 * 64 * 3 / 2);
            for y in 0..64u32 {
                for x in 0..64u32 {
                    data.push(((x * 3 + y * 2 + index as u32 * 25) % 200 + 30) as u8);
                }
            }
            data.resize(64 * 64 * 3 / 2, 128);
            let pts = Timestamp::new(index as i64 * 33, (1, 1000));
            let frame = VideoFrame {
                pts,
                dts: pts,
                data,
                width: 64,
                height: 64,
                pixel_format: PixelFormat::Yuv420p,
                is_key_frame: index == 0,
            };
            packets.extend(encoder.encode_frame(&frame).expect("encode frame"));
        }
        packets.extend(encoder.flush().expect("flush encoder"));
        assert!(!packets.is_empty(), "encoder produced no packets");
        packets
    }

    fn ebml(id: &[u8], body: &[u8]) -> Vec<u8> {
        let len = body.len() as u32;
        let mut out = id.to_vec();
        out.extend_from_slice(&[
            0x10 | ((len >> 24) & 0x0F) as u8,
            (len >> 16) as u8,
            (len >> 8) as u8,
            len as u8,
        ]);
        out.extend_from_slice(body);
        out
    }

    /// Builds a one-track Matroska file whose video track carries `packets`.
    fn mkv_file(codec_id: &str, packets: &[Packet]) -> Vec<u8> {
        let mut track = Vec::new();
        track.extend(ebml(&[0xD7], &[1])); // TrackNumber
        track.extend(ebml(&[0x83], &[1])); // TrackType = video
        track.extend(ebml(&[0x86], codec_id.as_bytes())); // CodecID
        let tracks = ebml(&[0x16, 0x54, 0xAE, 0x6B], &ebml(&[0xAE], &track));

        let mut cluster = ebml(&[0xE7], &[0]); // Timestamp
        for (index, packet) in packets.iter().enumerate() {
            let mut block = vec![0x81]; // track number 1
            block.extend_from_slice(&((index as i16) * 33).to_be_bytes());
            block.push(if index == 0 { 0x80 } else { 0x00 });
            block.extend_from_slice(&packet.data);
            cluster.extend(ebml(&[0xA3], &block));
        }
        let mut segment = tracks;
        segment.extend(ebml(&[0x1F, 0x43, 0xB6, 0x75], &cluster));

        let mut file = vec![0x1A, 0x45, 0xDF, 0xA3, 0x80]; // empty EBML header
        file.extend(ebml(&[0x18, 0x53, 0x80, 0x67], &segment));
        file
    }

    /// MPEG-2 CRC-32 (poly `0x04C11DB7`, MSB-first), as PSI sections require.
    fn mpeg_crc32(bytes: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for byte in bytes {
            crc ^= u32::from(*byte) << 24;
            for _ in 0..8 {
                crc = if crc & 0x8000_0000 != 0 {
                    (crc << 1) ^ 0x04C1_1DB7
                } else {
                    crc << 1
                };
            }
        }
        crc
    }

    /// Builds a PSI section (PAT/PMT) with its CRC.
    fn ts_section(table_id: u8, body: &[u8]) -> Vec<u8> {
        let mut section = vec![table_id];
        section.extend_from_slice(&(0xB000u16 | (body.len() + 4) as u16).to_be_bytes());
        section.extend_from_slice(body);
        section.extend_from_slice(&mpeg_crc32(&section).to_be_bytes());
        section
    }

    /// Wraps one PSI section in a single TS packet on `pid`.
    fn ts_psi_packet(pid: u16, section: &[u8]) -> Vec<u8> {
        let mut packet = vec![
            0x47,
            0x40 | ((pid >> 8) as u8 & 0x1F),
            pid as u8,
            0x10,
            0x00,
        ];
        packet.extend_from_slice(section);
        packet.resize(188, 0xFF);
        packet
    }

    /// PES-encapsulates `payload` (PTS only, DTS = PTS) and splits it into TS
    /// packets on `pid`, the first carrying an adaptation field with PCR.
    fn ts_pes_packets(pid: u16, stream_id: u8, pts: u64, payload: &[u8]) -> Vec<Vec<u8>> {
        const TS_PACKET_LEN: usize = 188;
        let header_len = 5u8; // PTS only
        let es_len = payload.len() + 3 + header_len as usize;
        let mut pes = vec![0x00, 0x00, 0x01, stream_id];
        pes.extend_from_slice(&(es_len as u16).to_be_bytes());
        pes.push(0x80); // no scrambling, no priority
        pes.push(0x80); // PTS present
        pes.push(header_len);
        let value = pts & 0x1_FFFF_FFFF;
        // 33-bit PTS, marker bits set (prefix '0011' = PTS only).
        pes.push(0x31 | (((value >> 30) as u8 & 0x07) << 1));
        pes.push((value >> 22) as u8);
        pes.push(0x01 | (((value >> 15) as u8 & 0x7F) << 1));
        pes.push((value >> 7) as u8);
        pes.push(0x01 | ((value as u8 & 0x7F) << 1));
        pes.extend_from_slice(payload);

        let mut out = Vec::new();
        let mut offset = 0;
        let mut continuity = 0u8;
        let mut first = true;
        while offset < pes.len() {
            let mut packet = vec![
                0x47,
                if first { 0x40 } else { 0x00 } | ((pid >> 8) as u8 & 0x1F),
                pid as u8,
            ];
            let remaining = pes.len() - offset;
            if first || remaining < TS_PACKET_LEN - 5 {
                // Adaptation field: PCR only (0x10 flags), then stuffing.
                let mut adaptation = vec![0x10u8];
                let base = value;
                adaptation.extend_from_slice(&[
                    (base >> 25) as u8,
                    (base >> 17) as u8,
                    (base >> 9) as u8,
                    (base >> 1) as u8,
                    (((base & 0x1) as u8) << 7) | 0x7E,
                    0x00,
                ]);
                let space = TS_PACKET_LEN - 5 - adaptation.len();
                let take = remaining.min(space);
                let stuffing = space - take;
                packet.push(0x30 | continuity);
                packet.push((adaptation.len() + stuffing) as u8);
                packet.extend_from_slice(&adaptation);
                packet.extend(std::iter::repeat_n(0xFF, stuffing));
                packet.extend_from_slice(&pes[offset..offset + take]);
                offset += take;
            } else {
                packet.push(0x10 | continuity);
                let take = remaining.min(TS_PACKET_LEN - 4);
                packet.extend_from_slice(&pes[offset..offset + take]);
                offset += take;
            }
            debug_assert_eq!(packet.len(), TS_PACKET_LEN);
            continuity = (continuity + 1) & 0x0F;
            out.push(packet);
            first = false;
        }
        out
    }

    /// Builds a single-program MPEG-TS whose video elementary stream carries
    /// `packets`. `registration` is the four-character registration descriptor
    /// that identifies the codec (`AV01` for AV1, `vp09` for VP9); pass an empty
    /// slice to fall back to `stream_type`.
    fn ts_file(stream_type: u8, registration: &[u8], packets: &[Packet]) -> Vec<u8> {
        const PMT_PID: u16 = 0x1000;
        const VIDEO_PID: u16 = 0x0100;

        let pat_body = {
            let mut body = vec![0x00, 0x01, 0xC1, 0x00, 0x00]; // tsid, version, section
            body.extend_from_slice(&1u16.to_be_bytes()); // program 1
            body.extend_from_slice(&(0xE000 | PMT_PID).to_be_bytes());
            body
        };
        let mut es_info = Vec::new();
        if !registration.is_empty() {
            es_info.push(0x05); // registration_descriptor
            es_info.push(0x04);
            es_info.extend_from_slice(registration);
        }
        let pmt_body = {
            let mut body = vec![0x00, 0x01, 0xC1, 0x00, 0x00];
            body.extend_from_slice(&(0xE000 | VIDEO_PID).to_be_bytes()); // PCR PID
            body.extend_from_slice(&(0xF000u16 | es_info.len() as u16).to_be_bytes());
            body.extend_from_slice(&es_info);
            body.push(stream_type);
            body.extend_from_slice(&(0xE000 | VIDEO_PID).to_be_bytes());
            body.extend_from_slice(&0xF000u16.to_be_bytes()); // ES info length
            body
        };

        let mut file = ts_psi_packet(0x0000, &ts_section(0x00, &pat_body));
        file.extend(ts_psi_packet(PMT_PID, &ts_section(0x02, &pmt_body)));
        for (index, packet) in packets.iter().enumerate() {
            let pts = 90_000u64 + 3_000 * index as u64;
            file.extend(ts_pes_packets(VIDEO_PID, 0xE0, pts, &packet.data).concat());
        }
        file
    }

    fn inspect_bytes(name: &str, bytes: &[u8]) -> Inspection {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        let asset = Asset {
            id: Default::default(),
            path,
            fingerprint: AssetFingerprint {
                sha256: "a".repeat(64),
                size_bytes: bytes.len() as u64,
            },
            size_bytes: bytes.len() as u64,
            modified_time: None,
            duration: None,
            streams: Vec::new(),
        };
        let metadata = Inspection {
            container: ContainerInspection {
                validity: ContainerValidity::Ok,
                ..Default::default()
            },
            video: vec![VideoMeasurements {
                stream_idx: 0,
                frame_rate_observed: Some(Rational::new(30, 1).unwrap()),
                colorspace: Some("bt709".into()),
                ..Default::default()
            }],
            ..Default::default()
        };
        KinetixVideoInspector::new()
            .inspect_decode(&asset, &metadata, InspectionLevel::Full)
            .unwrap()
    }

    #[test]
    fn decodes_av1_in_matroska_end_to_end() {
        let decoded = inspect_bytes("clip.mkv", &mkv_file("V_AV1", &av1_packets(6)));
        assert_eq!(
            decoded.diagnostics["video_decode"]["status"], "complete",
            "{:?}",
            decoded.diagnostics["video_decode"]
        );
        let video = decoded.video_for(0).unwrap();
        assert_eq!(video.decode_errors, 0);
        assert!(video.decoded_frame_count.unwrap_or(0) > 0);
        assert!(video.luma.is_some());
        assert_eq!(video.colorspace.as_deref(), Some("bt709"));
        assert_eq!(decoded.diagnostics["video_decode"]["codec"], "AV1");
        assert_eq!(
            decoded.diagnostics["video_decode"]["backend"],
            "tpt-kinetix-demux + tpt-kinetix-av1/vp9"
        );
    }

    #[test]
    fn h264_is_never_decoded_and_is_not_a_decode_error() {
        let packets = [Packet {
            pts: Timestamp::new(0, (1, 1000)),
            dts: Timestamp::new(0, (1, 1000)),
            data: vec![0, 0, 0, 1, 0x65, 0x88],
            stream_index: 0,
            is_key_frame: true,
        }];
        let decoded = inspect_bytes("clip.mkv", &mkv_file("V_MPEG4/ISO/AVC", &packets));
        let video = decoded.video_for(0).unwrap();
        assert_eq!(
            video.decode_errors, 0,
            "unsupported codec must not look corrupt"
        );
        assert_eq!(video.decoded_frame_count, Some(0));
        let diagnostics = &decoded.diagnostics["video_decode"];
        assert_eq!(diagnostics["status"], "incomplete");
        assert_eq!(diagnostics["decode_error_recorded"], false);
        assert!(diagnostics["reason"].as_str().unwrap().contains("H.264"));
    }

    #[test]
    fn decodes_av1_in_mpeg_ts() {
        // Broadcast/HLS path: AV1 arrives as a private-PES elementary stream
        // identified by the `AV01` registration descriptor.
        let decoded = inspect_bytes("clip.ts", &ts_file(0x06, b"AV01", &av1_packets(6)));
        assert_eq!(
            decoded.diagnostics["video_decode"]["status"], "complete",
            "{:?}",
            decoded.diagnostics["video_decode"]
        );
        let video = decoded.video_for(0).unwrap();
        assert_eq!(video.decode_errors, 0);
        assert!(video.decoded_frame_count.unwrap_or(0) > 0);
        assert!(video.luma.is_some());
        assert_eq!(decoded.diagnostics["video_decode"]["codec"], "AV1");
    }

    #[test]
    fn h264_transport_stream_is_never_decoded_and_is_not_a_decode_error() {
        let packets = [Packet {
            pts: Timestamp::new(90_000, (1, 90_000)),
            dts: Timestamp::new(90_000, (1, 90_000)),
            data: vec![0x00, 0x00, 0x00, 0x01, 0x65, 0x88],
            stream_index: 0x0100,
            is_key_frame: true,
        }];
        // stream_type 0x1B = H.264, declared directly in the PMT.
        let decoded = inspect_bytes("h264.ts", &ts_file(0x1B, b"", &packets));
        assert_eq!(
            decoded.video_for(0).unwrap().decode_errors,
            0,
            "an undecoded transport stream must not look corrupt"
        );
        assert_eq!(decoded.diagnostics["video_decode"]["status"], "incomplete");
        assert_eq!(
            decoded.diagnostics["video_decode"]["decode_error_recorded"],
            false
        );
        assert!(decoded.diagnostics["video_decode"]["reason"]
            .as_str()
            .unwrap()
            .contains("H.264"));
    }

    #[test]
    fn mpeg_ts_routes_vp9_by_registration_descriptor() {
        // `vp09` must be recognised as a VP9 video stream (and routed to the VP9
        // decoder) rather than falling through to the unsupported-codec report.
        let packets = [Packet {
            pts: Timestamp::new(90_000, (1, 90_000)),
            dts: Timestamp::new(90_000, (1, 90_000)),
            data: vec![0x00, 0x00, 0x00, 0x01, 0xDE, 0xAD],
            stream_index: 0x0100,
            is_key_frame: true,
        }];
        let decoded = inspect_bytes("vp9.ts", &ts_file(0x06, b"vp09", &packets));
        let diagnostics = &decoded.diagnostics["video_decode"];
        assert_eq!(diagnostics["status"], "incomplete");
        let reason = diagnostics["reason"].as_str().unwrap().to_string();
        assert!(
            reason.contains("VP9"),
            "expected VP9 routing, got: {reason}"
        );
        assert!(
            !reason.contains("supports AV1 and VP9 video only"),
            "vp09 must not be treated as an unsupported codec: {reason}"
        );
    }

    #[test]
    fn decodes_vp9_in_mp4() {
        let bytes = include_bytes!("../../../tests/fixtures/encoded/vp9-clip.mp4");
        let decoded = inspect_bytes("vp9-clip.mp4", bytes);
        assert_eq!(
            decoded.diagnostics["video_decode"]["status"], "complete",
            "{:?}",
            decoded.diagnostics["video_decode"]
        );
        let video = decoded.video_for(0).unwrap();
        assert_eq!(video.decode_errors, 0);
        assert_eq!(video.decoded_frame_count, Some(3));
        assert!(video.luma.is_some());
        assert_eq!(video.colorspace.as_deref(), Some("bt709"));
        assert_eq!(decoded.diagnostics["video_decode"]["codec"], "VP9");
        assert_eq!(
            decoded.diagnostics["video_decode"]["backend"],
            "tpt-kinetix-demux + tpt-kinetix-av1/vp9"
        );
    }

    #[test]
    fn decodes_vp9_in_matroska() {
        let bytes = include_bytes!("../../../tests/fixtures/encoded/vp9-clip.webm");
        let decoded = inspect_bytes("vp9-clip.webm", bytes);
        assert_eq!(
            decoded.diagnostics["video_decode"]["status"], "complete",
            "{:?}",
            decoded.diagnostics["video_decode"]
        );
        let video = decoded.video_for(0).unwrap();
        assert_eq!(video.decode_errors, 0);
        assert_eq!(video.decoded_frame_count, Some(3));
        assert!(video.luma.is_some());
        assert_eq!(decoded.diagnostics["video_decode"]["codec"], "VP9");
    }

    #[test]
    fn unsupported_container_is_not_a_decode_error() {
        let decoded = inspect_bytes(
            "clip.mxf",
            &[0x06, 0x0E, 0x2B, 0x34, 0x02, 0x05, 0x01, 0x01],
        );
        assert_eq!(decoded.video_for(0).unwrap().decode_errors, 0);
        assert_eq!(decoded.diagnostics["video_decode"]["status"], "incomplete");
        assert_eq!(
            decoded.diagnostics["video_decode"]["decode_error_recorded"],
            false
        );
    }
}
