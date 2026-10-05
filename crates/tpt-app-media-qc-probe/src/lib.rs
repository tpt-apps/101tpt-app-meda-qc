//! In-process, royalty-free-only container inspector (replaces ffprobe).
//!
//! [`NativeInspector`] sniffs the container from the file's content, hands it
//! to the matching reader, and turns the reader's [`ProbedContainer`] into the
//! stable [`Inspection`] model the rules consume.
//!
//! Policy: only royalty-free codecs are inspected (see [`codec`]). A file
//! containing any other audio or video codec is refused with an
//! "unsupported" error and none of its bitstreams are parsed. A
//! recognised royalty-free container that is damaged is *not* an error: it
//! yields `ContainerValidity::Corrupt` so the container rules can fail it.
//!
//! # Stream-index contract
//!
//! `Stream::index` is the position of the track in the reader's output, which
//! is the container's natural order: MP4/MOV `trak` order, Matroska
//! `TrackEntry` order, MPEG-TS ascending PID order, and `0` for single-stream
//! audio files. [`ProbedStream::native_id`] carries the container's own
//! identifier so decode adapters can find the same stream.

pub mod audio_files;
pub mod codec;
pub mod common;
pub mod hdr;
pub mod iso_bmff;
pub mod matroska;
pub mod mpegts;
pub mod sniff;
pub mod subtitles;

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek};

use tpt_app_media_qc_core::error::{Error, Result};
use tpt_app_media_qc_model::asset::{Asset, StreamKind};
use tpt_app_media_qc_model::inspection::{
    AudioMeasurements, ContainerInspection, DurationMillis, Inspection, VideoMeasurements,
};
use tpt_app_media_qc_pipeline::Inspector;

pub use common::{ProbedContainer, ProbedStream, RawCue};
pub use sniff::Container;

/// Probe `source` (of `len` bytes) after detecting its container from the
/// first bytes. Pure with respect to the file system apart from reading
/// `source`: this is the fuzzable entry point.
pub fn probe<R: Read + Seek>(source: &mut R, len: u64) -> Result<ProbedContainer> {
    let mut head = vec![0u8; sniff::SNIFF_BYTES.min(len as usize)];
    source.rewind()?;
    source.read_exact(&mut head)?;
    let Some(container) = sniff::sniff(&head) else {
        return Err(codec::unsupported_error(sniff::describe_unreadable(&head)));
    };
    match container {
        Container::Wav => audio_files::probe_wav(source, len),
        Container::Aiff => audio_files::probe_aiff(source, len),
        Container::Flac => audio_files::probe_flac(source, len),
        Container::Ogg => audio_files::probe_ogg(source, len),
        Container::IsoBmff => iso_bmff::probe(source, len),
        Container::Matroska => matroska::probe(source, len),
        Container::MpegTs => mpegts::probe(source, len),
    }
}

/// Turn a reader's result into the shared [`Inspection`] model.
pub fn build_inspection(probed: ProbedContainer, file_len: u64) -> Inspection {
    let mut streams = Vec::with_capacity(probed.streams.len());
    let mut video = Vec::new();
    let mut audio = Vec::new();
    let mut subtitle = Vec::new();

    for (position, ps) in probed.streams.into_iter().enumerate() {
        let index = position as u64;
        let mut stream = ps.stream;
        stream.index = tpt_app_media_qc_model::asset::StreamId::new(index);
        // Decode adapters match their demuxer's track to this stream by the
        // container's own id (track id / track number / PID).
        stream
            .metadata
            .insert("native_id".into(), ps.native_id.to_string());
        match stream.kind {
            StreamKind::Video => {
                let mut m = ps.video.unwrap_or_default();
                m.stream_idx = index;
                video.push(m);
            }
            StreamKind::Audio => audio.push(AudioMeasurements {
                stream_idx: index,
                ..Default::default()
            }),
            StreamKind::Subtitle => {
                let is_text = stream
                    .codec
                    .as_deref()
                    .is_some_and(subtitles::is_text_subtitle_codec);
                subtitle.push(subtitles::measure(index, is_text, &ps.cues));
            }
            _ => {}
        }
        streams.push(stream);
    }

    let bitrate_bps = probed
        .duration_ms
        .filter(|d| *d > 0)
        .map(|d| (u128::from(file_len) * 8 * 1000 / u128::from(d)) as u64);

    let mut diagnostics: BTreeMap<String, serde_json::Value> = probed.diagnostics;
    diagnostics.insert("inspector".into(), "tpt-native-probe".into());

    Inspection {
        streams,
        container: ContainerInspection {
            validity: probed.validity,
            format: Some(probed.format.to_string()),
            bitrate_bps,
            duration: probed.duration_ms.map(DurationMillis),
            timestamps_contiguous: probed.timestamps_contiguous,
            timecode_present: probed.timecode_present,
            timestamp_gaps: probed.timestamp_gaps,
            malformed_metadata: probed.malformed_metadata,
            decode_errors: 0,
        },
        video,
        audio,
        subtitle,
        voice: Vec::new(),
        diagnostics,
    }
}

/// The metadata front-end used by every QC run.
#[derive(Clone, Copy, Debug, Default)]
pub struct NativeInspector;

impl Inspector for NativeInspector {
    fn name(&self) -> &str {
        "tpt-native-probe"
    }

    fn inspect_metadata(&self, asset: &Asset) -> Result<Inspection> {
        let mut file = File::open(&asset.path)?;
        let len = file.metadata()?.len();
        if len == 0 {
            return Err(Error::Probe(format!("'{}' is empty", asset.path.display())));
        }
        let probed = probe(&mut file, len)?;
        Ok(build_inspection(probed, len))
    }
}

// Keep the `VideoMeasurements` import meaningful for downstream docs.
#[allow(dead_code)]
type _VideoMeasurements = VideoMeasurements;

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use tpt_app_media_qc_model::inspection::ContainerValidity;

    fn wav() -> Vec<u8> {
        let mut v = b"RIFF".to_vec();
        v.extend(40u32.to_le_bytes());
        v.extend(b"WAVEfmt ");
        v.extend(16u32.to_le_bytes());
        v.extend(1u16.to_le_bytes());
        v.extend(2u16.to_le_bytes());
        v.extend(48_000u32.to_le_bytes());
        v.extend(192_000u32.to_le_bytes());
        v.extend(4u16.to_le_bytes());
        v.extend(16u16.to_le_bytes());
        v.extend(b"data");
        v.extend(4u32.to_le_bytes());
        v.extend([0u8; 4]);
        v
    }

    #[test]
    fn a_wav_becomes_a_full_inspection() {
        let bytes = wav();
        let len = bytes.len() as u64;
        let probed = probe(&mut Cursor::new(bytes), len).unwrap();
        let inspection = build_inspection(probed, len);
        assert_eq!(inspection.container.validity, ContainerValidity::Ok);
        assert_eq!(inspection.container.format.as_deref(), Some("wav"));
        assert_eq!(inspection.streams.len(), 1);
        assert_eq!(inspection.audio.len(), 1);
        assert_eq!(inspection.audio[0].stream_idx, 0);
        assert!(inspection.video.is_empty());
        assert_eq!(inspection.diagnostics["inspector"], "tpt-native-probe");
    }

    #[test]
    fn unknown_formats_are_refused_with_a_clear_message() {
        let bytes = b"RIFF\0\0\0\0AVI LISTxxxxxxxxxx".to_vec();
        let len = bytes.len() as u64;
        let e = probe(&mut Cursor::new(bytes), len).unwrap_err().to_string();
        assert!(e.contains("royalty-free") && e.contains("AVI"), "{e}");
    }

    #[test]
    fn tiny_garbage_never_panics() {
        for n in 0..64usize {
            let bytes = vec![0xA5u8; n];
            let len = bytes.len() as u64;
            let _ = probe(&mut Cursor::new(bytes), len);
        }
    }
}
