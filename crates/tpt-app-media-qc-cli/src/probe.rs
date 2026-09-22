//! ffprobe-backed [`Inspector`] implementation ([spec § 10]).
//!
//! This is the reference probe front-end. It shells out to `ffprobe` for the
//! metadata-only pass. The decode pass currently reuses metadata (decode-based
//! measurements arrive with the TPT foundation crates / a media analyzer);
//! decode rules then correctly report `Inconclusive` ([spec § 3.4]).

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use serde::Deserialize;
use tpt_app_media_qc_core::error::{Error, Result};
use tpt_app_media_qc_model::asset::{Asset, Stream, StreamId, StreamKind};
use tpt_app_media_qc_model::inspection::{
    AudioMeasurements, ContainerInspection, ContainerValidity, DurationMillis, Inspection,
    VideoMeasurements,
};
use tpt_app_media_qc_model::time::Rational;
use tpt_app_media_qc_pipeline::{InspectionLevel, Inspector};

#[derive(Deserialize)]
struct FfprobeOutput {
    #[serde(default)]
    streams: Vec<FfStream>,
    #[serde(default)]
    format: FfFormat,
}

#[derive(Deserialize, Default)]
struct FfFormat {
    format_name: Option<String>,
    format_long_name: Option<String>,
    duration: Option<String>,
    bit_rate: Option<String>,
    #[serde(default)]
    tags: BTreeMap<String, String>,
}

#[derive(Deserialize, Default)]
struct FfStream {
    index: u64,
    codec_type: Option<String>,
    codec_name: Option<String>,
    codec_long_name: Option<String>,
    width: Option<u64>,
    height: Option<u64>,
    pix_fmt: Option<String>,
    r_frame_rate: Option<String>,
    time_base: Option<String>,
    duration: Option<String>,
    bit_rate: Option<String>,
    sample_rate: Option<String>,
    channels: Option<u64>,
    channel_layout: Option<String>,
    bits_per_sample: Option<String>,
    #[serde(default)]
    tags: BTreeMap<String, String>,
}

/// Probe front-end using the system `ffprobe` binary.
pub struct FfprobeInspector;

impl FfprobeInspector {
    /// Whether an `ffprobe` binary can be spawned on this machine.
    pub fn available() -> bool {
        Command::new("ffprobe")
            .arg("-version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn run_probe(&self, path: &Path) -> Result<FfprobeOutput> {
        let output = Command::new("ffprobe")
            .arg("-v")
            .arg("error")
            .arg("-show_format")
            .arg("-show_streams")
            .arg("-of")
            .arg("json")
            .arg(path)
            .output()
            .map_err(|e| Error::Probe(format!("could not run ffprobe on {path:?}: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Probe(format!(
                "ffprobe failed on {path:?}: {}",
                stderr.trim()
            )));
        }
        serde_json::from_slice(&output.stdout)
            .map_err(|e| Error::Probe(format!("ffprobe output was not valid JSON: {e}")))
    }
}

fn parse_duration_ms(s: &str) -> Option<u64> {
    s.trim().parse::<f64>().ok().map(|secs| (secs * 1000.0).round() as u64)
}

fn parse_rational(s: &str) -> Option<Rational> {
    Rational::parse(s)
}

fn stream_kind(codec_type: &str) -> StreamKind {
    match codec_type {
        "video" => StreamKind::Video,
        "audio" => StreamKind::Audio,
        "subtitle" => StreamKind::Subtitle,
        "data" => StreamKind::Data,
        "attachment" | "attached_pic" => StreamKind::Attachment,
        _ => StreamKind::Unknown,
    }
}

impl Inspector for FfprobeInspector {
    fn name(&self) -> &str {
        "ffprobe"
    }

    fn inspect_metadata(&self, asset: &Asset) -> Result<Inspection> {
        let out = self.run_probe(&asset.path)?;

        let mut streams = Vec::new();
        let mut video_meas = Vec::new();
        let mut audio_meas = Vec::new();

        for s in &out.streams {
            let kind = s.codec_type.as_deref().map(stream_kind).unwrap_or(StreamKind::Unknown);
            let duration_secs = s
                .duration
                .as_deref()
                .and_then(|d| d.trim().parse::<f64>().ok())
                .map(|secs| tpt_app_media_qc_model::time::DurationSeconds::from_millis((secs * 1000.0).round() as u64));

            let mut metadata: BTreeMap<String, String> = BTreeMap::new();
            for (k, v) in &s.tags {
                metadata.insert(k.clone(), v.clone());
            }

            streams.push(Stream {
                index: StreamId::new(s.index),
                kind,
                codec: s.codec_name.clone(),
                codec_profile: s.codec_long_name.clone(),
                width: s.width,
                height: s.height,
                pixel_format: s.pix_fmt.clone(),
                frame_rate: s.r_frame_rate.as_deref().and_then(parse_rational),
                time_base: s.time_base.as_deref().and_then(parse_rational),
                bitrate: s.bit_rate.as_deref().and_then(|b| b.trim().parse().ok()),
                duration: duration_secs,
                language: s.tags.get("language").cloned(),
                channel_layout: s.channel_layout.clone(),
                channels: s.channels,
                sample_rate: s.sample_rate.as_deref().and_then(|r| r.trim().parse().ok()),
                bit_depth: s.bits_per_sample.as_deref().and_then(|b| b.trim().parse().ok()),
                metadata,
            });

            match kind {
                StreamKind::Video => video_meas.push(VideoMeasurements {
                    stream_idx: s.index,
                    frame_rate_observed: s.r_frame_rate.as_deref().and_then(parse_rational),
                    colorspace: s.tags.get("color_space").cloned(),
                    ..Default::default()
                }),
                StreamKind::Audio => audio_meas.push(AudioMeasurements {
                    stream_idx: s.index,
                    ..Default::default()
                }),
                _ => {}
            }
        }

        let format = &out.format;
        let timecode_present =
            format.tags.iter().any(|(k, _)| {
                k.eq_ignore_ascii_case("start_timecode")
                    || k.eq_ignore_ascii_case("timecode")
            });

        let container = ContainerInspection {
            validity: ContainerValidity::Ok,
            format: format.format_long_name.clone().or_else(|| format.format_name.clone()),
            bitrate_bps: format.bit_rate.as_deref().and_then(|b| b.trim().parse().ok()),
            duration: format.duration.as_deref().and_then(parse_duration_ms).map(DurationMillis),
            timecode_present: Some(timecode_present),
            ..Default::default()
        };

        Ok(Inspection {
            container,
            video: video_meas,
            audio: audio_meas,
            diagnostics: BTreeMap::new(),
        })
    }

    fn inspect_decode(
        &self,
        _asset: &Asset,
        metadata: &Inspection,
        _level: InspectionLevel,
    ) -> Result<Inspection> {
        // The decode pass ships with the analyzer integrations. Keeping the
        // metadata inspection here means decode rules report Inconclusive.
        Ok(metadata.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_parsing() {
        assert_eq!(parse_duration_ms("120.0"), Some(120_000));
        assert_eq!(parse_duration_ms("0.5"), Some(500));
        assert_eq!(parse_duration_ms("garbage"), None);
    }

    #[test]
    fn rational_parsing() {
        assert_eq!(parse_rational("30000/1001"), Rational::parse("30000/1001"));
        assert_eq!(parse_rational("25/1").unwrap().value(), 25.0);
    }

    #[test]
    fn codec_kind_mapping() {
        assert_eq!(stream_kind("video"), StreamKind::Video);
        assert_eq!(stream_kind("audio"), StreamKind::Audio);
        assert_eq!(stream_kind("attached_pic"), StreamKind::Attachment);
        assert_eq!(stream_kind("nope"), StreamKind::Unknown);
    }
}