//! Kinetix-backed video inspection for TPT Media QC.
//!
//! Kinetix owns MP4 demuxing and H.264 reconstruction; this crate reduces
//! decoded frames to the stable [`Inspection`] model consumed by QC rules.
//! The pinned MP4 demuxer is in-memory, so large inputs are refused rather
//! than loaded without a bound. Decoded frames are analysed one at a time.

mod audio;

pub use audio::{AudioAnalyzerConfig, CadenceAudioInspector};

use tpt_app_media_qc_core::error::{Error, Result};
use tpt_app_media_qc_model::asset::Asset;
use tpt_app_media_qc_model::inspection::{Inspection, LumaStats, VideoMeasurements};
use tpt_app_media_qc_model::time::{FrameRate, Rational};
use tpt_app_media_qc_model::TimeRange;
use tpt_app_media_qc_pipeline::{InspectionLevel, Inspector};
use tpt_kinetix_core::codec::{CodecId, MediaType};
use tpt_kinetix_core::frame::VideoFrame;
use tpt_kinetix_core::packet::Packet;
use tpt_kinetix_core::pixel_format::PixelFormat;
use tpt_kinetix_demux::mp4::boxes::parse_box_header;
use tpt_kinetix_demux::{Demuxer, Mp4Demuxer};
use tpt_kinetix_h264::nal::{parse_nal_units_from_avcc, NalUnit};
use tpt_kinetix_h264::H264Decoder;

/// Maximum MP4 input accepted by the current in-memory Kinetix demuxer.
pub const DEFAULT_MAX_DECODE_INPUT_BYTES: u64 = 512 * 1024 * 1024;

/// Deterministic thresholds for the first H.264 video pass.
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

/// Full-decode inspector for MP4/ISO-BMFF files containing H.264 video.
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

    fn decode_mp4(
        &self,
        asset: &Asset,
        metadata: &Inspection,
    ) -> std::result::Result<VideoMeasurements, DecodeFailure> {
        let bytes =
            std::fs::read(&asset.path).map_err(|error| DecodeFailure::Failed(error.into()))?;
        if bytes.len() as u64 > self.max_input_bytes {
            return Err(DecodeFailure::Unsupported(format!(
                "MP4 input is {} bytes; the current Kinetix adapter limit is {} bytes",
                bytes.len(),
                self.max_input_bytes
            )));
        }
        let config = avc_decoder_config(&bytes).map_err(DecodeFailure::Failed)?;
        let mut demuxer = Mp4Demuxer::new(bytes).map_err(|error| {
            DecodeFailure::Failed(Error::Probe(format!("Kinetix MP4 demux failed: {error}")))
        })?;
        let track_index = demuxer
            .tracks()
            .iter()
            .position(|track| {
                track.media_type == MediaType::Video && track.codec == Some(CodecId::H264)
            })
            .ok_or_else(|| {
                DecodeFailure::Unsupported(
                    "the pinned Kinetix adapter currently decodes H.264 video in MP4 only".into(),
                )
            })?;
        let stream_idx = track_index as u64;
        let mut decoder = H264Decoder::new().with_display_order().with_strict(true);
        let nominal_frame_rate = metadata
            .video_for(stream_idx)
            .and_then(|video| video.frame_rate_observed)
            .or(self.config.nominal_frame_rate);
        let mut analyzer = VideoFrameAnalyzer::new(stream_idx, nominal_frame_rate, &self.config);
        let expected_video_packets = demuxer.tracks()[track_index].sample_count();
        if expected_video_packets == 0 {
            return Err(DecodeFailure::Failed(Error::Probe(
                "Kinetix MP4 video track contains no samples".into(),
            )));
        }
        let mut sent_parameter_sets = false;
        let mut video_packets_seen = 0usize;

        loop {
            let packet = demuxer.read_packet().map_err(|error| {
                DecodeFailure::Failed(Error::Probe(format!("Kinetix packet read failed: {error}")))
            })?;
            let Some(packet) = packet else { break };
            if packet.stream_index != track_index as u32 {
                continue;
            }
            video_packets_seen += 1;

            let mut data = if sent_parameter_sets {
                Vec::new()
            } else {
                sent_parameter_sets = true;
                avc_parameter_set_prelude(&config)
            };
            data.extend(
                avcc_to_annex_b(&packet.data, config.length_size).map_err(DecodeFailure::Failed)?,
            );
            let h264_packet = Packet {
                pts: packet.pts,
                dts: packet.dts,
                data,
                stream_index: packet.stream_index,
                is_key_frame: packet.is_key_frame,
            };
            let frame = decoder.decode(&h264_packet).map_err(|error| {
                DecodeFailure::Failed(Error::Probe(format!(
                    "Kinetix H.264 decode failed: {error}"
                )))
            })?;
            if let Some(frame) = frame {
                analyzer.push(frame).map_err(DecodeFailure::Failed)?;
            }
            if video_packets_seen >= expected_video_packets {
                break;
            }
        }
        if video_packets_seen < expected_video_packets {
            return Err(DecodeFailure::Failed(Error::Probe(format!(
                "Kinetix MP4 demux ended after {video_packets_seen} of {expected_video_packets} video samples"
            ))));
        }

        let flushed = decoder.flush().map_err(|error| {
            DecodeFailure::Failed(Error::Probe(format!("Kinetix H.264 flush failed: {error}")))
        })?;
        for frame in flushed {
            analyzer.push(frame).map_err(DecodeFailure::Failed)?;
        }
        if analyzer.frame_count() == 0 {
            return Err(DecodeFailure::Failed(Error::Probe(
                "Kinetix H.264 decoder produced no frames".into(),
            )));
        }

        let mut measurement = analyzer.finish();
        measurement.colorspace = metadata
            .video_for(stream_idx)
            .and_then(|video| video.colorspace.clone());
        Ok(measurement)
    }
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
        "tpt-kinetix-h264"
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

        match self.decode_mp4(asset, metadata) {
            Ok(measurement) => {
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
                        "backend": "tpt-kinetix-demux + tpt-kinetix-h264",
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
    let pixels = (frame.width as usize)
        .checked_mul(frame.height as usize)
        .ok_or_else(|| Error::Probe("decoded frame dimensions overflow".into()))?;
    let values: Box<dyn Iterator<Item = u8> + '_> = match frame.pixel_format {
        PixelFormat::Yuv420p | PixelFormat::Yuv422p | PixelFormat::Yuv444p => {
            let plane = frame.data.get(..pixels).ok_or_else(|| {
                Error::Probe(format!(
                    "decoded frame has {} luma bytes; expected {pixels}",
                    frame.data.len()
                ))
            })?;
            Box::new(plane.iter().copied())
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

#[derive(Debug)]
struct AvcDecoderConfig {
    length_size: u8,
    parameter_sets: Vec<Vec<u8>>,
}

fn avc_decoder_config(bytes: &[u8]) -> Result<AvcDecoderConfig> {
    let payload = find_avc_config(bytes)?
        .ok_or_else(|| Error::Probe("MP4 has no readable avcC decoder configuration".into()))?;
    if payload.len() < 6 {
        return Err(Error::Probe("MP4 avcC box is truncated".into()));
    }
    let length_size = (payload[4] & 0x03) + 1;
    let sps_count = (payload[5] & 0x1f) as usize;
    if sps_count == 0 {
        return Err(Error::Probe("MP4 avcC box contains no SPS".into()));
    }
    let mut offset = 6usize;
    let mut parameter_sets = Vec::with_capacity(sps_count + 1);
    for _ in 0..sps_count {
        let (set, next) = read_avc_parameter_set(&payload, offset)?;
        parameter_sets.push(set.to_vec());
        offset = next;
    }
    let pps_count = *payload
        .get(offset)
        .ok_or_else(|| Error::Probe("MP4 avcC box has no PPS count".into()))?
        as usize;
    offset += 1;
    if pps_count == 0 {
        return Err(Error::Probe("MP4 avcC box contains no PPS".into()));
    }
    for _ in 0..pps_count {
        let (set, next) = read_avc_parameter_set(&payload, offset)?;
        parameter_sets.push(set.to_vec());
        offset = next;
    }
    Ok(AvcDecoderConfig {
        length_size,
        parameter_sets,
    })
}

fn read_avc_parameter_set(bytes: &[u8], offset: usize) -> Result<(&[u8], usize)> {
    let length_bytes = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| Error::Probe("MP4 avcC parameter-set length is truncated".into()))?;
    let length = u16::from_be_bytes([length_bytes[0], length_bytes[1]]) as usize;
    let start = offset + 2;
    let end = start
        .checked_add(length)
        .ok_or_else(|| Error::Probe("MP4 avcC parameter-set length overflows".into()))?;
    let value = bytes
        .get(start..end)
        .ok_or_else(|| Error::Probe("MP4 avcC parameter-set payload is truncated".into()))?;
    Ok((value, end))
}

fn find_avc_config(bytes: &[u8]) -> Result<Option<Vec<u8>>> {
    fn walk(data: &[u8], skip: usize) -> Result<Option<Vec<u8>>> {
        let mut rest = data
            .get(skip..)
            .ok_or_else(|| Error::Probe("MP4 box header is truncated".into()))?;
        while !rest.is_empty() {
            if rest.len() < 8 {
                return Ok(None);
            }
            let (after_header, header) = parse_box_header(rest)
                .map_err(|error| Error::Probe(format!("invalid MP4 box header: {error}")))?;
            let header_len = rest.len() - after_header.len();
            let size = header.size as usize;
            if size == 0 || size < header_len || size > rest.len() {
                return Err(Error::Probe(format!(
                    "MP4 box {:?} has invalid size {size} for {} available bytes",
                    String::from_utf8_lossy(&header.box_type),
                    rest.len()
                )));
            }
            let payload_len = size - header_len;
            let payload = &after_header[..payload_len];
            if &header.box_type == b"avcC" {
                return Ok(Some(payload.to_vec()));
            }
            if is_mp4_container(&header.box_type) {
                let child_skip = match &header.box_type {
                    b"stsd" => 8usize,
                    b"avc1" | b"avc3" => 78usize,
                    _ => 0,
                };
                if child_skip <= payload.len() {
                    if let Some(config) = walk(payload, child_skip)? {
                        return Ok(Some(config));
                    }
                }
            }
            rest = &after_header[payload_len..];
        }
        Ok(None)
    }
    walk(bytes, 0)
}

fn is_mp4_container(box_type: &[u8; 4]) -> bool {
    matches!(
        box_type,
        b"moov" | b"trak" | b"mdia" | b"minf" | b"stbl" | b"stsd" | b"avc1" | b"avc3"
    )
}

fn avc_parameter_set_prelude(config: &AvcDecoderConfig) -> Vec<u8> {
    let mut prelude = Vec::new();
    for parameter_set in &config.parameter_sets {
        let length = u32::try_from(parameter_set.len()).unwrap_or(u32::MAX);
        prelude.extend_from_slice(&length.to_be_bytes());
        prelude.extend_from_slice(parameter_set);
    }
    prelude
}

fn avcc_to_annex_b(data: &[u8], length_size: u8) -> Result<Vec<u8>> {
    let nal_units = parse_nal_units_from_avcc(data, length_size);
    if nal_units.is_empty() || !nal_units.iter().any(|nal| nal.nal_unit_type.is_vcl()) {
        return Err(Error::Probe(
            "MP4 H.264 sample has no decodable VCL NAL units".into(),
        ));
    }
    let mut annex_b = Vec::new();
    for nal in nal_units {
        append_annex_b_nal(&mut annex_b, &nal);
    }
    Ok(annex_b)
}

fn append_annex_b_nal(output: &mut Vec<u8>, nal: &NalUnit) {
    output.extend_from_slice(&[0, 0, 0, 1]);
    output.push((nal.nal_ref_idc << 5) | nal.nal_unit_type as u8);
    output.extend_from_slice(&nal.rbsp);
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpt_app_media_qc_model::asset::AssetFingerprint;
    use tpt_app_media_qc_model::inspection::{ContainerInspection, ContainerValidity};
    use tpt_kinetix_core::timestamp::Timestamp;
    use tpt_kinetix_h264::nal::{parse_nal_units_from_annexb, NalUnitType};
    use tpt_kinetix_mux::{Mp4Muxer, Mp4MuxerConfig};

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

    fn raw_nal(nal: &NalUnit) -> Vec<u8> {
        let mut data = vec![(nal.nal_ref_idc << 5) | nal.nal_unit_type as u8];
        data.extend_from_slice(&nal.rbsp);
        data
    }

    fn push_avcc(output: &mut Vec<u8>, nal: &[u8]) {
        output.extend_from_slice(&(nal.len() as u32).to_be_bytes());
        output.extend_from_slice(nal);
    }

    fn fixture_mp4() -> Vec<u8> {
        let source = include_bytes!("../../../tests/fixtures/encoded/kinetix-mbaff-ip-cabac.h264");
        let nals = parse_nal_units_from_annexb(source);
        let sps = nals
            .iter()
            .find(|nal| nal.nal_unit_type == NalUnitType::Sps)
            .unwrap();
        let pps = nals
            .iter()
            .find(|nal| nal.nal_unit_type == NalUnitType::Pps)
            .unwrap();
        let mut muxer = Mp4Muxer::new(Mp4MuxerConfig {
            width: 64,
            height: 64,
            timescale: 30_000,
            sps: raw_nal(sps),
            pps: raw_nal(pps),
        });
        let mut first = true;
        for nal in nals.iter().filter(|nal| nal.nal_unit_type.is_vcl()) {
            let mut sample = Vec::new();
            if first {
                push_avcc(&mut sample, &raw_nal(sps));
                push_avcc(&mut sample, &raw_nal(pps));
                first = false;
            }
            push_avcc(&mut sample, &raw_nal(nal));
            muxer.write_sample(&sample, 1000, nal.nal_unit_type == NalUnitType::IdrSlice);
        }
        muxer.finish()
    }

    #[test]
    fn kinetix_inspector_decodes_a_generated_mp4() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fixture.mp4");
        let bytes = fixture_mp4();
        std::fs::write(&path, &bytes).unwrap();
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

        let decoded = KinetixVideoInspector::new()
            .inspect_decode(&asset, &metadata, InspectionLevel::Full)
            .unwrap();
        let video = decoded.video_for(0).unwrap();
        assert_eq!(
            video.decode_errors, 0,
            "fixture must use a supported H.264 path"
        );
        assert!(video.decoded_frame_count.unwrap_or(0) > 0);
        assert!(video.luma.is_some());
        assert_eq!(video.colorspace.as_deref(), Some("bt709"));
        assert_eq!(
            decoded.diagnostics["video_decode"]["backend"],
            "tpt-kinetix-demux + tpt-kinetix-h264"
        );
    }
}
