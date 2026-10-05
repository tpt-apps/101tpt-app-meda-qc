//! ffprobe-backed metadata plus Kinetix/Cadence-backed decode ([spec § 10]).
//!
//! Metadata is still collected by the system `ffprobe` front-end. Full scans
//! route AV1/VP9 video (MP4, Matroska/WebM) through `tpt-kinetix-demux`, and
//! standalone audio through the Cadence readers; unsupported codecs and
//! incomplete coverage remain explicit in the inspection model.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use serde::{de::Error as _, Deserialize, Deserializer};
use tpt_app_media_qc_core::error::{Error, Result};
use tpt_app_media_qc_decode::{
    CadenceAudioInspector, KinetixVideoInspector, VoiceAnalyzer, VoiceConfig,
};
use tpt_app_media_qc_model::asset::{Asset, FieldOrder, Stream, StreamId, StreamKind};
use tpt_app_media_qc_model::finding::TimeRange;
use tpt_app_media_qc_model::inspection::{
    AudioMeasurements, ContainerInspection, ContainerValidity, DurationMillis, HdrMetadata,
    Inspection, MasteringDisplay, SubtitleMeasurements, VideoMeasurements,
};
use tpt_app_media_qc_model::time::Rational;
use tpt_app_media_qc_pipeline::{InspectionLevel, Inspector};

#[derive(Deserialize)]
pub(crate) struct FfprobeOutput {
    #[serde(default)]
    streams: Vec<FfStream>,
    #[serde(default)]
    format: FfFormat,
}

#[derive(Deserialize, Default)]
pub(crate) struct FfFormat {
    format_name: Option<String>,
    format_long_name: Option<String>,
    #[serde(default, deserialize_with = "optional_string_or_number")]
    duration: Option<String>,
    #[serde(default, deserialize_with = "optional_string_or_number")]
    bit_rate: Option<String>,
    #[serde(default)]
    tags: BTreeMap<String, String>,
}

#[derive(Deserialize, Default)]
pub(crate) struct FfStream {
    index: u64,
    codec_type: Option<String>,
    codec_name: Option<String>,
    codec_long_name: Option<String>,
    width: Option<u64>,
    height: Option<u64>,
    pix_fmt: Option<String>,
    field_order: Option<String>,
    #[serde(default, deserialize_with = "optional_string_or_number")]
    r_frame_rate: Option<String>,
    #[serde(default, deserialize_with = "optional_string_or_number")]
    time_base: Option<String>,
    #[serde(default, deserialize_with = "optional_string_or_number")]
    duration: Option<String>,
    #[serde(default, deserialize_with = "optional_string_or_number")]
    bit_rate: Option<String>,
    #[serde(default, deserialize_with = "optional_string_or_number")]
    sample_rate: Option<String>,
    channels: Option<u64>,
    channel_layout: Option<String>,
    #[serde(default, deserialize_with = "optional_string_or_number")]
    bits_per_sample: Option<String>,
    color_space: Option<String>,
    color_transfer: Option<String>,
    color_primaries: Option<String>,
    color_range: Option<String>,
    /// Per-stream side data (mastering display, content light level, DOVI).
    #[serde(default)]
    side_data_list: Vec<BTreeMap<String, serde_json::Value>>,
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

    /// Second probe pass that reads subtitle cue timing and, for plain-text
    /// codecs, the cue payload ([spec § 8.7]).
    ///
    /// `text_codecs` maps subtitle stream index to a flag recording whether the
    /// codec's payload is readable text. Files with no subtitle streams return
    /// an empty map, so they never pay for the extra process spawn.
    fn run_subtitle_probe(
        &self,
        path: &Path,
        text_codecs: &BTreeMap<u64, bool>,
    ) -> Result<BTreeMap<u64, SubtitleMeasurements>> {
        if text_codecs.is_empty() {
            return Ok(BTreeMap::new());
        }
        // Absolute stream indices (`-select_streams 2,5`), not the `s:N`
        // shorthand, which is relative to the subtitle streams only.
        let wanted = text_codecs
            .keys()
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(",");

        let output = Command::new("ffprobe")
            .arg("-v")
            .arg("error")
            .arg("-select_streams")
            .arg(wanted)
            .arg("-show_packets")
            .arg("-show_data")
            .arg("-show_entries")
            .arg("packet=stream_index,pts_time,duration_time,duration,data")
            .arg("-of")
            .arg("json")
            .arg(path)
            .output()
            .map_err(|e| Error::Probe(format!("could not run ffprobe on {path:?}: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Probe(format!(
                "ffprobe subtitle packet read failed on {path:?}: {}",
                stderr.trim()
            )));
        }

        let packets: SubtitlePackets = serde_json::from_slice(&output.stdout)
            .map_err(|e| Error::Probe(format!("ffprobe packet output was not valid JSON: {e}")))?;

        let mut out = BTreeMap::new();
        for (&idx, &text_decoded) in text_codecs {
            let mut meas = SubtitleMeasurements {
                stream_idx: idx,
                text_decoded,
                ..Default::default()
            };
            measure_cues(&mut meas, text_codecs, &packets.packets);
            out.insert(idx, meas);
        }
        Ok(out)
    }
}

/// Upper bound on cues analysed per subtitle stream. Subtitle files are
/// normally far smaller than this; the bound keeps a pathological input from
/// turning the probe into an unbounded allocation ([spec § 11] memory bounds).
const MAX_SUBTITLE_CUES: usize = 100_000;

/// Subtitle codecs whose container payload is plain text, so characters and
/// markup can be inspected. Bitmap/structured formats (`dvdsub`, `hdmv_pgs`,
/// `dvb_subtitle`, `mov_text`) stay timing-only.
fn is_text_subtitle_codec(codec: &str) -> bool {
    matches!(
        codec.to_ascii_lowercase().as_str(),
        "subrip" | "srt" | "ass" | "ssa" | "webvtt" | "text" | "microdvd" | "mpl2" | "subviewer"
    )
}

#[derive(Deserialize, Default)]
struct SubtitlePackets {
    #[serde(default)]
    packets: Vec<SubtitlePacket>,
}

#[derive(Deserialize)]
struct SubtitlePacket {
    #[serde(default)]
    stream_index: Option<u64>,
    #[serde(default, deserialize_with = "optional_string_or_number")]
    pts_time: Option<String>,
    #[serde(default, deserialize_with = "optional_string_or_number")]
    duration_time: Option<String>,
    #[serde(default, deserialize_with = "optional_string_or_number")]
    duration: Option<String>,
    #[serde(default)]
    data: Option<String>,
}

impl SubtitlePacket {
    fn start_ms(&self) -> Option<u64> {
        self.pts_time
            .as_deref()
            .and_then(|s| s.trim().parse::<f64>().ok())
            .map(|secs| (secs * 1000.0).round().max(0.0) as u64)
    }

    fn duration_ms(&self) -> Option<u64> {
        let secs = self
            .duration_time
            .as_deref()
            .or(self.duration.as_deref())
            .and_then(|s| s.trim().parse::<f64>().ok())?;
        Some((secs * 1000.0).round().max(0.0) as u64)
    }
}

/// Decode ffprobe's `hexdump`-style `-show_data` block back into raw bytes.
///
/// ffprobe emits `<offset>: <hex bytes padded to a fixed width>  <ascii>` per
/// line. The hex column is what matters, and the ASCII column can itself
/// contain hex digits, so the column is terminated by the padding run rather
/// than by "stop when a non-hex character appears".
fn decode_packet_data(dump: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    for line in dump.lines() {
        let Some((_, rest)) = line.split_once(':') else {
            continue;
        };
        let hex_column = rest
            .trim_start_matches(' ')
            .split_once("  ")
            .map_or(rest, |(hex, _)| hex);
        let hex: String = hex_column.chars().filter(char::is_ascii_hexdigit).collect();
        // Hex arrives in whole-byte pairs; a trailing nibble is truncated data.
        let hex = &hex[..hex.len() - hex.len() % 2];
        for pair in hex.as_bytes().chunks(2) {
            let hi = (pair[0] as char).to_digit(16);
            let lo = (pair[1] as char).to_digit(16);
            match (hi, lo) {
                (Some(hi), Some(lo)) => bytes.push((hi * 16 + lo) as u8),
                _ => break,
            }
        }
    }
    bytes
}

/// Strip SubRip/ASS/WebVTT markup and report the visible line structure.
fn analyse_cue_text(raw: &[u8]) -> Option<(u32, u32)> {
    let text = std::str::from_utf8(raw).ok()?;
    let mut max_line_chars = 0u32;
    let mut lines = 0u32;
    for line in text.lines() {
        let line = strip_subtitle_markup(line);
        if line.is_empty() {
            continue;
        }
        lines += 1;
        max_line_chars = max_line_chars.max(line.chars().count() as u32);
    }
    Some((max_line_chars, lines))
}

/// Remove SRT/ASS/WebVTT structural markup so character counts reflect what is
/// rendered on screen rather than how the file encodes it.
fn strip_subtitle_markup(line: &str) -> String {
    let mut line = line.trim();
    // WebVTT/SRT cue settings trail the timestamps (`align:start ...`).
    if let Some((_, rest)) = line.rsplit_once("-->") {
        line = rest.trim();
    }
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            // ASS override blocks `{\...}` and HTML-ish tags `<i>`.
            '{' | '<' => {
                let close = if c == '{' { '}' } else { '>' };
                for c in chars.by_ref() {
                    if c == close {
                        break;
                    }
                }
            }
            _ => out.push(c),
        }
    }
    out.trim().to_string()
}

/// Reduce one stream's packets into [`SubtitleMeasurements`].
///
/// Timing checks run for every subtitle codec; text checks run only when the
/// codec's payload is plain text, otherwise `text_decoded` stays `false` and the
/// content rule reports `Inconclusive` instead of guessing.
fn measure_cues(
    meas: &mut SubtitleMeasurements,
    text_codecs: &BTreeMap<u64, bool>,
    packets: &[SubtitlePacket],
) {
    let idx = meas.stream_idx;
    let is_text = text_codecs.get(&idx).copied().unwrap_or(false);

    // (start_ms, end_ms, payload)
    let mut cues: Vec<(u64, u64, Option<Vec<u8>>)> = Vec::new();
    for p in packets.iter().filter(|p| p.stream_index == Some(idx)) {
        if cues.len() >= MAX_SUBTITLE_CUES {
            break;
        }
        let Some(start) = p.start_ms() else {
            continue;
        };
        let end = start.saturating_add(p.duration_ms().unwrap_or(0));
        let payload = if is_text {
            p.data.as_deref().map(decode_packet_data)
        } else {
            None
        };
        cues.push((start, end, payload));
    }
    // Container packet order is not guaranteed to be presentation order.
    cues.sort_by_key(|(start, end, _)| (*start, *end));

    meas.cue_count = Some(cues.len() as u64);
    if cues.is_empty() {
        return;
    }

    let mut max_line_chars: Option<u32> = None;
    let mut max_lines: Option<u32> = None;
    let mut saw_text_payload = false;

    for (i, &(start, end, ref payload)) in cues.iter().enumerate() {
        if end <= start {
            meas.invalid_durations.push(TimeRange::new(start, start));
        }
        if let Some((next_start, _, _)) = cues.get(i + 1) {
            // Overlap counts against the following cue, not itself.
            if end > *next_start {
                meas.overlaps
                    .push(TimeRange::new(start, end.min(*next_start)));
            } else {
                meas.max_gap_ms = Some(
                    meas.max_gap_ms
                        .unwrap_or(0)
                        .max(next_start.saturating_sub(end)),
                );
            }
        }
        if !is_text {
            continue;
        }
        let Some(payload) = payload.as_deref() else {
            continue;
        };
        saw_text_payload = true;
        match analyse_cue_text(payload) {
            Some((line_chars, lines)) => {
                if line_chars == 0 && lines == 0 {
                    meas.empty_cues.push(TimeRange::new(start, end));
                }
                max_line_chars = Some(max_line_chars.unwrap_or(0).max(line_chars));
                max_lines = Some(max_lines.unwrap_or(0).max(lines));
            }
            // Payload is not valid UTF-8 text.
            None => meas.malformed.push(TimeRange::new(start, end)),
        }
    }

    meas.max_cue_duration_ms = Some(
        cues.iter()
            .map(|(start, end, _)| end.saturating_sub(*start))
            .max()
            .unwrap_or(0),
    );
    meas.covered_until_ms = cues.last().map(|(_, end, _)| *end);
    if saw_text_payload {
        meas.max_line_chars = max_line_chars;
        meas.max_lines_per_cue = max_lines;
    }
}

fn optional_string_or_number<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    match Option::<serde_json::Value>::deserialize(deserializer)? {
        None => Ok(None),
        Some(serde_json::Value::String(value)) => Ok(Some(value)),
        Some(serde_json::Value::Number(value)) => Ok(Some(value.to_string())),
        Some(other) => Err(D::Error::custom(format!(
            "expected a string or number, got {other}"
        ))),
    }
}

fn parse_duration_ms(s: &str) -> Option<u64> {
    s.trim()
        .parse::<f64>()
        .ok()
        .map(|secs| (secs * 1000.0).round() as u64)
}

/// `unknown` / `unspecified` / empty mean "not signalled".
fn probe_tag(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|v| !v.is_empty() && !matches!(*v, "unknown" | "unspecified" | "reserved"))
        .map(str::to_string)
}

/// Parses ffprobe's `"num/den"` or plain-number side-data values.
fn side_value(map: &BTreeMap<String, serde_json::Value>, key: &str) -> Option<f64> {
    match map.get(key)? {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => match s.split_once('/') {
            Some((n, d)) => {
                let (n, d) = (n.trim().parse::<f64>().ok()?, d.trim().parse::<f64>().ok()?);
                (d != 0.0).then(|| n / d)
            }
            None => s.trim().parse().ok(),
        },
        _ => None,
    }
}

fn hdr_from_stream(s: &FfStream) -> Option<HdrMetadata> {
    let mut hdr = HdrMetadata {
        transfer: probe_tag(s.color_transfer.as_deref()),
        primaries: probe_tag(s.color_primaries.as_deref()),
        matrix: probe_tag(s.color_space.as_deref()),
        range: probe_tag(s.color_range.as_deref()),
        ..Default::default()
    };
    for side in &s.side_data_list {
        let kind = side
            .get("side_data_type")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if kind.contains("mastering display") {
            let xy = |x: &str, y: &str| Some((side_value(side, x)?, side_value(side, y)?));
            hdr.mastering_display = Some(MasteringDisplay {
                min_luminance_nits: side_value(side, "min_luminance"),
                max_luminance_nits: side_value(side, "max_luminance"),
                primaries_xy: (|| {
                    Some([
                        xy("red_x", "red_y")?,
                        xy("green_x", "green_y")?,
                        xy("blue_x", "blue_y")?,
                    ])
                })(),
                white_point_xy: xy("white_point_x", "white_point_y"),
            });
        } else if kind.contains("content light level") {
            hdr.max_cll = side_value(side, "max_content").map(|v| v.round() as u32);
            hdr.max_fall = side_value(side, "max_average").map(|v| v.round() as u32);
        } else if kind.contains("dovi") || kind.contains("dolby vision") {
            hdr.dolby_vision = true;
        }
    }
    (hdr != HdrMetadata::default()).then_some(hdr)
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

/// Parse ffprobe `-of json` output into an [`Inspection`] without spawning a
/// process. Pure and total: any bytes yield `Err` or a valid inspection.
///
/// This is the fuzzable "result parser" boundary (spec § 24.4).
pub fn parse_ffprobe_json(bytes: &[u8]) -> Result<Inspection> {
    let out: FfprobeOutput = serde_json::from_slice(bytes)
        .map_err(|e| Error::Probe(format!("ffprobe output was not valid JSON: {e}")))?;
    Ok(inspection_from_output(&out))
}

fn inspection_from_output(out: &FfprobeOutput) -> Inspection {
    let mut streams = Vec::new();
    let mut video_meas = Vec::new();
    let mut audio_meas = Vec::new();

    for s in &out.streams {
        let kind = s
            .codec_type
            .as_deref()
            .map(stream_kind)
            .unwrap_or(StreamKind::Unknown);
        let duration_secs = s
            .duration
            .as_deref()
            .and_then(|d| d.trim().parse::<f64>().ok())
            .map(|secs| {
                tpt_app_media_qc_model::time::DurationSeconds::from_millis(
                    (secs * 1000.0).round() as u64
                )
            });

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
            field_order: s.field_order.as_deref().and_then(FieldOrder::parse_probe),
            frame_rate: s.r_frame_rate.as_deref().and_then(parse_rational),
            time_base: s.time_base.as_deref().and_then(parse_rational),
            bitrate: s.bit_rate.as_deref().and_then(|b| b.trim().parse().ok()),
            duration: duration_secs,
            language: s.tags.get("language").cloned(),
            channel_layout: s.channel_layout.clone(),
            channels: s.channels,
            sample_rate: s.sample_rate.as_deref().and_then(|r| r.trim().parse().ok()),
            bit_depth: s
                .bits_per_sample
                .as_deref()
                .and_then(|b| b.trim().parse().ok()),
            metadata,
        });

        match kind {
            StreamKind::Video => video_meas.push(VideoMeasurements {
                stream_idx: s.index,
                frame_rate_observed: s.r_frame_rate.as_deref().and_then(parse_rational),
                colorspace: probe_tag(s.color_space.as_deref())
                    .or_else(|| s.tags.get("color_space").cloned()),
                hdr: hdr_from_stream(s),
                field_order: s.field_order.as_deref().and_then(FieldOrder::parse_probe),
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
    let timecode_present = format.tags.iter().any(|(k, _)| {
        k.eq_ignore_ascii_case("start_timecode") || k.eq_ignore_ascii_case("timecode")
    });

    let container = ContainerInspection {
        validity: ContainerValidity::Ok,
        format: format
            .format_long_name
            .clone()
            .or_else(|| format.format_name.clone()),
        bitrate_bps: format
            .bit_rate
            .as_deref()
            .and_then(|b| b.trim().parse().ok()),
        duration: format
            .duration
            .as_deref()
            .and_then(parse_duration_ms)
            .map(DurationMillis),
        timecode_present: Some(timecode_present),
        ..Default::default()
    };

    Inspection {
        streams,
        container,
        video: video_meas,
        audio: audio_meas,
        subtitle: Vec::new(),
        voice: Vec::new(),
        diagnostics: BTreeMap::new(),
    }
}

/// Composite inspector used by full CLI scans.
pub struct HybridInspector {
    metadata: FfprobeInspector,
    video: KinetixVideoInspector,
    audio: CadenceAudioInspector,
    voice: VoiceAnalyzer,
}

impl Default for HybridInspector {
    fn default() -> Self {
        Self::with_voice(VoiceConfig::default())
    }
}

impl HybridInspector {
    /// An inspector that also runs the optional voice analyses in `voice`.
    pub fn with_voice(voice: VoiceConfig) -> Self {
        Self {
            metadata: FfprobeInspector,
            video: KinetixVideoInspector::new(),
            audio: CadenceAudioInspector::new(),
            voice: VoiceAnalyzer::new(voice),
        }
    }

    /// Voice analyses the profile asks for (spec 8.8). Nothing runs unless
    /// the profile configures a `rules.voice` check.
    pub fn voice_config_for(profile: &tpt_app_media_qc_profile::model::Profile) -> VoiceConfig {
        let v = &profile.rules.voice;
        VoiceConfig {
            speech: v.speech.is_some()
                || v.silence.is_some()
                || v.speakers.is_some()
                || v.speaker_changes.is_some(),
            transcript: v.transcript.is_some(),
        }
    }
}

impl Inspector for HybridInspector {
    fn name(&self) -> &str {
        "ffprobe + tpt-kinetix-av1-vp9 + tpt-cadence"
    }

    fn inspect_metadata(&self, asset: &Asset) -> Result<Inspection> {
        self.metadata.inspect_metadata(asset)
    }

    fn inspect_decode(
        &self,
        asset: &Asset,
        metadata: &Inspection,
        level: InspectionLevel,
    ) -> Result<Inspection> {
        let video = self.video.inspect_decode(asset, metadata, level)?;
        let audio = self.audio.inspect_decode(asset, &video, level)?;
        Ok(self.voice.analyze(asset, &audio))
    }
}

impl Inspector for FfprobeInspector {
    fn name(&self) -> &str {
        "ffprobe"
    }

    fn inspect_metadata(&self, asset: &Asset) -> Result<Inspection> {
        let out = self.run_probe(&asset.path)?;
        let mut inspection = inspection_from_output(&out);

        // Cue timing/text is a second, cheaper packet-level pass ([spec § 8.7]).
        // A failure here must not sink the whole scan: the container rules still
        // have everything they need from the first pass.
        let text_codecs: BTreeMap<u64, bool> = inspection
            .streams
            .iter()
            .filter(|s| s.kind == StreamKind::Subtitle)
            .map(|s| {
                (
                    s.index.as_u64(),
                    is_text_subtitle_codec(s.codec.as_deref().unwrap_or("")),
                )
            })
            .collect();
        match self.run_subtitle_probe(&asset.path, &text_codecs) {
            Ok(cues) => inspection.subtitle = cues.into_values().collect(),
            Err(e) => {
                inspection.diagnostics.insert(
                    "subtitle_probe_error".to_string(),
                    serde_json::Value::String(e.to_string()),
                );
                // Keep the stream entries so presence/language rules still work.
                inspection.subtitle = text_codecs
                    .keys()
                    .map(|idx| SubtitleMeasurements {
                        stream_idx: *idx,
                        ..Default::default()
                    })
                    .collect();
            }
        }
        Ok(inspection)
    }

    fn inspect_decode(
        &self,
        _asset: &Asset,
        metadata: &Inspection,
        _level: InspectionLevel,
    ) -> Result<Inspection> {
        // Metadata-only inspection intentionally has no decode coverage; the
        // hybrid inspector supplies video/audio decode measurements in a full scan.
        Ok(metadata.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_hexdump_payload_bytes() {
        let dump = "\n00000000: 4865 6c6c 6f20 7468 6572 652e            Hello there.\n\
                    00000010: 6f6e 6520 6c69 6e65                     one line\n";
        assert_eq!(decode_packet_data(dump), b"Hello there.one line");
    }

    #[test]
    fn hexdump_stops_at_the_ascii_column_padding() {
        // The ASCII column is itself valid hex text; it must not be decoded.
        let dump = "00000000: 6162 6364                              abcd\n";
        assert_eq!(decode_packet_data(dump), b"abcd");
    }

    #[test]
    fn text_analysis_strips_markup_and_counts_visible_lines() {
        let (chars, lines) = analyse_cue_text(b"{\\an8}<i>Hi</i>\nsecond\n\n").unwrap();
        assert_eq!((chars, lines), (6, 2));
    }

    #[test]
    fn text_analysis_rejects_non_utf8_payload() {
        assert!(analyse_cue_text(&[0xff, 0xfe, 0x00]).is_none());
    }

    #[test]
    fn cue_measurement_reports_timing_and_content() {
        let mut codecs = BTreeMap::new();
        codecs.insert(2u64, true);
        let mut meas = SubtitleMeasurements {
            stream_idx: 2,
            text_decoded: true,
            ..Default::default()
        };
        let packets = vec![
            SubtitlePacket {
                stream_index: Some(2),
                pts_time: Some("5.0".into()),
                duration_time: Some("3.5".into()),
                duration: None,
                // Overlaps the cue starting at 4 s.
                data: Some("00000000: 4869                                     Hi\n".into()),
            },
            SubtitlePacket {
                stream_index: Some(2),
                pts_time: Some("1.0".into()),
                duration_time: Some("2.0".into()),
                duration: None,
                data: Some("00000000: 48656c6c6f                                Hello\n".into()),
            },
        ];
        measure_cues(&mut meas, &codecs, &packets);

        assert_eq!(meas.cue_count, Some(2));
        assert!(meas.overlaps.is_empty(), "{:?}", meas.overlaps);
        assert!(meas.invalid_durations.is_empty());
        assert_eq!(meas.max_gap_ms, Some(2_000));
        assert_eq!(meas.max_cue_duration_ms, Some(3_500));
        assert_eq!(meas.max_line_chars, Some(5));
        assert_eq!(meas.max_lines_per_cue, Some(1));
        assert_eq!(meas.covered_until_ms, Some(8_500));
    }

    #[test]
    fn cue_measurement_detects_overlap_and_invalid_duration() {
        let mut codecs = BTreeMap::new();
        codecs.insert(2u64, true);
        let mut meas = SubtitleMeasurements {
            stream_idx: 2,
            text_decoded: true,
            ..Default::default()
        };
        let packets = vec![
            SubtitlePacket {
                stream_index: Some(2),
                pts_time: Some("1.0".into()),
                duration_time: Some("4.0".into()),
                duration: None,
                data: None,
            },
            SubtitlePacket {
                stream_index: Some(2),
                pts_time: Some("2.0".into()),
                duration_time: Some("0.0".into()),
                duration: None,
                data: None,
            },
        ];
        measure_cues(&mut meas, &codecs, &packets);
        assert_eq!(meas.overlaps.len(), 1, "{:?}", meas.overlaps);
        assert_eq!(
            meas.invalid_durations.len(),
            1,
            "{:?}",
            meas.invalid_durations
        );
        assert_eq!(meas.max_gap_ms, None);
    }

    #[test]
    fn cue_measurement_skips_payload_for_non_text_codecs() {
        let mut codecs = BTreeMap::new();
        codecs.insert(2u64, false);
        let mut meas = SubtitleMeasurements {
            stream_idx: 2,
            text_decoded: false,
            ..Default::default()
        };
        let packets = vec![SubtitlePacket {
            stream_index: Some(2),
            pts_time: Some("1.0".into()),
            duration_time: Some("2.0".into()),
            duration: None,
            data: Some("00000000: deadbeef\n".into()),
        }];
        measure_cues(&mut meas, &codecs, &packets);
        assert_eq!(meas.cue_count, Some(1));
        assert!(meas.max_line_chars.is_none());
        assert!(meas.malformed.is_empty());
        assert!(meas.empty_cues.is_empty());
    }

    #[test]
    fn text_subtitle_codecs_are_classified() {
        assert!(is_text_subtitle_codec("subrip"));
        assert!(is_text_subtitle_codec("ASS"));
        assert!(is_text_subtitle_codec("webvtt"));
        assert!(!is_text_subtitle_codec("hdmv_pgs_subtitle"));
        assert!(!is_text_subtitle_codec("mov_text"));
    }

    #[test]
    fn parses_hdr10_signalling_from_ffprobe() {
        let json = br#"
        { "streams": [ {
            "index": 0, "codec_type": "video", "width": 3840, "height": 2160,
            "pix_fmt": "yuv420p10le", "r_frame_rate": "24/1",
            "color_space": "bt2020nc", "color_transfer": "smpte2084",
            "color_primaries": "bt2020", "color_range": "tv",
            "side_data_list": [
              { "side_data_type": "Mastering display metadata",
                "red_x": "34000/50000", "red_y": "16000/50000",
                "green_x": "13250/50000", "green_y": "34500/50000",
                "blue_x": "7500/50000", "blue_y": "3000/50000",
                "white_point_x": "15635/50000", "white_point_y": "16450/50000",
                "min_luminance": "50/10000", "max_luminance": "10000000/10000" },
              { "side_data_type": "Content light level metadata",
                "max_content": 1000, "max_average": 400 }
            ] } ],
          "format": { "format_name": "matroska" } }"#;
        let inspection = parse_ffprobe_json(json).unwrap();
        let video = &inspection.video[0];
        assert_eq!(video.colorspace.as_deref(), Some("bt2020nc"));
        let hdr = video.hdr.as_ref().unwrap();
        assert_eq!(hdr.transfer.as_deref(), Some("smpte2084"));
        assert_eq!((hdr.max_cll, hdr.max_fall), (Some(1000), Some(400)));
        let mastering = hdr.mastering_display.unwrap();
        assert_eq!(mastering.max_luminance_nits, Some(1000.0));
        assert!((mastering.min_luminance_nits.unwrap() - 0.005).abs() < 1e-9);
        assert!((mastering.primaries_xy.unwrap()[0].0 - 0.68).abs() < 1e-9);
    }

    #[test]
    fn unspecified_colour_tags_are_not_signalled() {
        let json = br#"{ "streams": [ { "index": 0, "codec_type": "video",
            "color_space": "unknown", "color_transfer": "unspecified" } ],
            "format": {} }"#;
        let inspection = parse_ffprobe_json(json).unwrap();
        assert_eq!(inspection.video[0].colorspace, None);
        assert_eq!(inspection.video[0].hdr, None);
    }

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

    #[test]
    fn parses_numeric_ffprobe_metadata_fields() {
        let json = br#"
        {
          "streams": [
            {
              "index": 0,
              "codec_type": "audio",
              "sample_rate": "48000",
              "channels": 2,
              "bits_per_sample": 16,
              "bit_rate": 1536000,
              "duration": "1.0"
            }
          ],
          "format": {
            "format_name": "wav",
            "duration": "1.0",
            "bit_rate": 1536624
          }
        }
        "#;

        let inspection = parse_ffprobe_json(json).unwrap();
        assert_eq!(inspection.container.bitrate_bps, Some(1_536_624));
        assert_eq!(inspection.container.duration, Some(DurationMillis(1_000)));
        assert_eq!(inspection.audio.len(), 1);
        assert_eq!(inspection.audio[0].stream_idx, 0);
        assert_eq!(inspection.streams.len(), 1);
        assert_eq!(inspection.streams[0].channels, Some(2));
        assert_eq!(inspection.streams[0].bit_depth, Some(16));
        assert!(inspection.video.is_empty());
    }
}
