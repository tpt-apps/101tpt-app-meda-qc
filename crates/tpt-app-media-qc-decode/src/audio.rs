//! Cadence-backed standalone audio inspection for TPT Media QC.
//!
//! Cadence owns WAV, AIFF/AIFC and FLAC decoding; this adapter reduces decoded
//! PCM to the stable [`tpt_app_media_qc_model::inspection::Inspection`] model.
//! Audio is decoded in fixed-size blocks, so file length does not determine
//! memory use. True-peak and BS.1770 loudness are intentionally left
//! unmeasured until their standards-accurate algorithms are integrated.

use std::fs::File;
use std::path::Path;

use tpt_app_media_qc_core::error::{Error, Result};
use tpt_app_media_qc_model::asset::Asset;
use tpt_app_media_qc_model::inspection::{AudioMeasurements, Inspection};
use tpt_app_media_qc_model::TimeRange;
use tpt_app_media_qc_pipeline::{InspectionLevel, Inspector};
use tpt_av_cadence_aiff::AiffDecoder;
use tpt_av_cadence_core::{CadenceError, Decoder};
use tpt_av_cadence_flac::FlacDecoder;
use tpt_av_cadence_wav::WavDecoder;

const DECODE_BUFFER_FRAMES: usize = 4096;
const MAX_AUDIO_CHANNELS: usize = 64;
const DIGITAL_SILENCE_DB: f64 = -300.0;

/// Measurement-only audio analysis settings. Rule thresholds remain in the QC
/// profile; this configuration controls how raw measurements are detected.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AudioAnalyzerConfig {
    /// Frames at or below this peak level are candidates for silence.
    pub silence_threshold_db: f64,
    /// Do not retain shorter silence ranges, avoiding millisecond zero-length
    /// segments at high sample rates.
    pub silence_min_duration_ms: u64,
    /// Maximum retained silence ranges. Further ranges set
    /// `silence_truncated`, which makes the silence rule inconclusive.
    pub max_silence_ranges: usize,
}

impl Default for AudioAnalyzerConfig {
    fn default() -> Self {
        Self {
            silence_threshold_db: -60.0,
            silence_min_duration_ms: 1,
            max_silence_ranges: 65_536,
        }
    }
}

/// Full-decode inspector for standalone audio files supported by Cadence.
#[derive(Clone, Copy, Debug, Default)]
pub struct CadenceAudioInspector {
    config: AudioAnalyzerConfig,
}

impl CadenceAudioInspector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_config(config: AudioAnalyzerConfig) -> Self {
        Self { config }
    }

    fn decode_file(
        &self,
        asset: &Asset,
        stream_idx: u64,
    ) -> std::result::Result<AudioMeasurements, AudioDecodeFailure> {
        let extension = path_extension(&asset.path).ok_or_else(|| {
            AudioDecodeFailure::Unsupported(
                "audio file has no extension; use WAV, AIFF/AIFC or FLAC".into(),
            )
        })?;
        let file =
            File::open(&asset.path).map_err(|error| AudioDecodeFailure::Failed(error.into()))?;

        match extension.as_str() {
            "wav" | "wave" => self.decode_with(
                WavDecoder::from_source(Box::new(file)).map_err(AudioDecodeFailure::from)?,
                stream_idx,
            ),
            "aif" | "aiff" | "aifc" => self.decode_with(
                AiffDecoder::from_source(Box::new(file)).map_err(AudioDecodeFailure::from)?,
                stream_idx,
            ),
            "flac" => self.decode_with(
                FlacDecoder::from_source(Box::new(file)).map_err(AudioDecodeFailure::from)?,
                stream_idx,
            ),
            _ => Err(AudioDecodeFailure::Unsupported(format!(
                "Cadence audio adapter does not decode .{extension}; supported standalone containers are WAV, AIFF/AIFC and FLAC"
            ))),
        }
    }

    fn decode_with<D: Decoder>(
        &self,
        mut decoder: D,
        stream_idx: u64,
    ) -> std::result::Result<AudioMeasurements, AudioDecodeFailure> {
        let (sample_rate, channels) = {
            let info = decoder.info();
            (info.sample_rate, usize::from(info.channels))
        };
        if channels == 0 || channels > MAX_AUDIO_CHANNELS {
            return Err(AudioDecodeFailure::Unsupported(format!(
                "audio channel count {channels} is outside the supported range 1..={MAX_AUDIO_CHANNELS}"
            )));
        }

        let mut samples = vec![0.0f32; DECODE_BUFFER_FRAMES * channels];
        let mut analyzer = AudioFrameAnalyzer::new(stream_idx, sample_rate, channels, self.config)?;
        loop {
            let frames = decoder
                .decode(&mut samples)
                .map_err(AudioDecodeFailure::from)?;
            if frames == 0 {
                break;
            }
            let decoded_samples = frames.checked_mul(channels).ok_or_else(|| {
                AudioDecodeFailure::Failed(Error::Probe(
                    "Cadence returned an overflowing audio sample-frame count".into(),
                ))
            })?;
            if decoded_samples > samples.len() {
                return Err(AudioDecodeFailure::Failed(Error::Probe(
                    "Cadence returned more audio samples than its output buffer can hold".into(),
                )));
            }
            analyzer.push(&samples[..decoded_samples])?;
        }
        if analyzer.frame_count == 0 {
            return Err(AudioDecodeFailure::Failed(Error::Probe(
                "Cadence audio decoder produced no sample frames".into(),
            )));
        }
        Ok(analyzer.finish())
    }
}

#[derive(Debug)]
enum AudioDecodeFailure {
    Unsupported(String),
    Failed(Error),
}

impl From<CadenceError> for AudioDecodeFailure {
    fn from(error: CadenceError) -> Self {
        match error {
            CadenceError::UnsupportedFeature(reason) => Self::Unsupported(reason),
            other => Self::Failed(Error::Probe(format!(
                "Cadence audio decode failed: {other}"
            ))),
        }
    }
}

fn path_extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
}

struct AudioFrameAnalyzer {
    stream_idx: u64,
    sample_rate: u32,
    channels: usize,
    silence_threshold_linear: f64,
    silence_min_duration_ms: u64,
    max_silence_ranges: usize,
    frame_count: u64,
    sample_count: u64,
    peak: f64,
    clipping_events: u64,
    sample_sum: f64,
    left_right_sum: f64,
    left_square_sum: f64,
    right_square_sum: f64,
    silence_start: Option<u64>,
    silence: Vec<TimeRange>,
    silence_truncated: bool,
}

impl AudioFrameAnalyzer {
    fn new(
        stream_idx: u64,
        sample_rate: u32,
        channels: usize,
        config: AudioAnalyzerConfig,
    ) -> std::result::Result<Self, AudioDecodeFailure> {
        if sample_rate == 0 {
            return Err(AudioDecodeFailure::Failed(Error::Probe(
                "Cadence returned a zero audio sample rate".into(),
            )));
        }
        let silence_threshold_linear = if config.silence_threshold_db.is_finite() {
            10f64
                .powf(config.silence_threshold_db / 20.0)
                .clamp(0.0, 1.0)
        } else {
            0.0
        };
        Ok(Self {
            stream_idx,
            sample_rate,
            channels,
            silence_threshold_linear,
            silence_min_duration_ms: config.silence_min_duration_ms,
            max_silence_ranges: config.max_silence_ranges,
            frame_count: 0,
            sample_count: 0,
            peak: 0.0,
            clipping_events: 0,
            sample_sum: 0.0,
            left_right_sum: 0.0,
            left_square_sum: 0.0,
            right_square_sum: 0.0,
            silence_start: None,
            silence: Vec::new(),
            silence_truncated: false,
        })
    }

    fn push(&mut self, samples: &[f32]) -> std::result::Result<(), AudioDecodeFailure> {
        if samples.len() % self.channels != 0 {
            return Err(AudioDecodeFailure::Failed(Error::Probe(
                "Cadence returned an incomplete audio sample frame".into(),
            )));
        }

        for frame in samples.chunks_exact(self.channels) {
            let mut frame_peak = 0.0f64;
            for sample in frame {
                let value = f64::from(*sample);
                if !value.is_finite() {
                    return Err(AudioDecodeFailure::Failed(Error::Probe(
                        "Cadence returned a non-finite audio sample".into(),
                    )));
                }
                let magnitude = value.abs();
                frame_peak = frame_peak.max(magnitude);
                self.peak = self.peak.max(magnitude);
                self.clipping_events = self
                    .clipping_events
                    .saturating_add((magnitude >= 1.0) as u64);
                self.sample_sum += value;
                self.sample_count = self.sample_count.saturating_add(1);
            }
            if self.channels == 2 {
                let left = f64::from(frame[0]);
                let right = f64::from(frame[1]);
                self.left_right_sum += left * right;
                self.left_square_sum += left * left;
                self.right_square_sum += right * right;
            }

            if frame_peak <= self.silence_threshold_linear {
                self.silence_start.get_or_insert(self.frame_count);
            } else {
                self.flush_silence(self.frame_count);
            }
            self.frame_count = self.frame_count.saturating_add(1);
        }
        Ok(())
    }

    fn flush_silence(&mut self, end_frame: u64) {
        let Some(start_frame) = self.silence_start.take() else {
            return;
        };
        let start_ms = frame_timestamp_ms(start_frame, self.sample_rate);
        let end_ms = frame_timestamp_ms(end_frame, self.sample_rate);
        if end_ms.saturating_sub(start_ms) >= self.silence_min_duration_ms {
            if self.silence.len() < self.max_silence_ranges {
                self.silence.push(TimeRange::new(start_ms, end_ms));
            } else {
                self.silence_truncated = true;
            }
        }
    }

    fn finish(mut self) -> AudioMeasurements {
        self.flush_silence(self.frame_count);
        let peak_db = if self.peak > 0.0 {
            20.0 * self.peak.log10()
        } else {
            DIGITAL_SILENCE_DB
        };
        let dc_offset_percent = (self.sample_count > 0)
            .then(|| (self.sample_sum / self.sample_count as f64 * 100.0).clamp(-100.0, 100.0));
        let phase_correlation = (self.channels == 2)
            .then(|| {
                let denominator = (self.left_square_sum * self.right_square_sum).sqrt();
                (denominator > 0.0).then(|| (self.left_right_sum / denominator).clamp(-1.0, 1.0))
            })
            .flatten();

        AudioMeasurements {
            stream_idx: self.stream_idx,
            decoded_frame_count: Some(self.frame_count),
            decode_errors: 0,
            silence: self.silence,
            silence_truncated: self.silence_truncated,
            clipping_events: self.clipping_events,
            peak_db: Some(peak_db),
            true_peak_db: None,
            loudness_lufs: None,
            loudness_range_lu: None,
            phase_correlation,
            dc_offset_percent,
        }
    }
}

fn frame_timestamp_ms(frame: u64, sample_rate: u32) -> u64 {
    let timestamp = (frame as u128 * 1000 + sample_rate as u128 / 2) / sample_rate as u128;
    timestamp.min(u64::MAX as u128) as u64
}

impl Inspector for CadenceAudioInspector {
    fn name(&self) -> &str {
        "tpt-cadence-audio"
    }

    fn inspect_metadata(&self, _asset: &Asset) -> Result<Inspection> {
        Err(Error::Unsupported(
            "CadenceAudioInspector provides audio decode; use a metadata inspector for pass 1"
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
        let Some(stream_idx) = metadata.audio.iter().map(|audio| audio.stream_idx).min() else {
            let mut inspection = metadata.clone();
            inspection.diagnostics.insert(
                "audio_decode".into(),
                serde_json::json!({
                    "status": "not_applicable",
                    "reason": "metadata inspection reported no audio streams",
                }),
            );
            return Ok(inspection);
        };

        match self.decode_file(asset, stream_idx) {
            Ok(measurement) => {
                let mut decoded = metadata.clone();
                decoded.audio.retain(|audio| audio.stream_idx != stream_idx);
                decoded.audio.push(measurement);
                decoded.audio.sort_by_key(|audio| audio.stream_idx);
                decoded.diagnostics.insert(
                    "audio_decode".into(),
                    serde_json::json!({
                        "status": "complete",
                        "backend": "tpt-cadence",
                        "container": path_extension(&asset.path),
                        "frames": decoded
                            .audio_for(stream_idx)
                            .and_then(|audio| audio.decoded_frame_count),
                        "measurements": [
                            "silence",
                            "clipping",
                            "sample_peak",
                            "phase_correlation_stereo",
                            "dc_offset"
                        ],
                        "unmeasured": ["true_peak", "bs1770_loudness"],
                        "silence_range_limit_reached": decoded
                            .audio_for(stream_idx)
                            .is_some_and(|audio| audio.silence_truncated)
                    }),
                );
                Ok(decoded)
            }
            Err(AudioDecodeFailure::Unsupported(reason)) => {
                Ok(incomplete_audio_inspection(metadata, &reason, false))
            }
            Err(AudioDecodeFailure::Failed(error)) => Ok(incomplete_audio_inspection(
                metadata,
                &error.to_string(),
                true,
            )),
        }
    }
}

fn incomplete_audio_inspection(
    metadata: &Inspection,
    reason: &str,
    count_decode_error: bool,
) -> Inspection {
    let mut inspection = metadata.clone();
    let Some(stream_idx) = inspection.audio.iter().map(|audio| audio.stream_idx).min() else {
        return inspection;
    };
    if let Some(audio) = inspection
        .audio
        .iter_mut()
        .find(|audio| audio.stream_idx == stream_idx)
    {
        if count_decode_error {
            audio.decoded_frame_count = Some(0);
            audio.decode_errors = audio.decode_errors.saturating_add(1);
        }
    }
    inspection.diagnostics.insert(
        "audio_decode".into(),
        serde_json::json!({
            "status": "incomplete",
            "backend": "tpt-cadence",
            "reason": reason,
            "decode_error_recorded": count_decode_error
        }),
    );
    inspection
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpt_app_media_qc_model::asset::AssetFingerprint;
    use tpt_app_media_qc_model::inspection::{ContainerInspection, ContainerValidity};

    #[test]
    fn cadence_decodes_a_streaming_wav_into_explicit_measurements() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fixture.wav");
        let mut samples = vec![0.0; 3_000];
        samples.push(1.0);
        let bytes = fixture_wav(&samples);
        std::fs::write(&path, &bytes).unwrap();
        let asset = test_asset(path, bytes.len() as u64);
        let metadata = Inspection {
            container: ContainerInspection {
                validity: ContainerValidity::Ok,
                ..Default::default()
            },
            audio: vec![AudioMeasurements {
                stream_idx: 0,
                ..Default::default()
            }],
            ..Default::default()
        };

        let decoded = CadenceAudioInspector::new()
            .inspect_decode(&asset, &metadata, InspectionLevel::Full)
            .unwrap();
        let audio = decoded.audio_for(0).unwrap();
        assert_eq!(audio.decoded_frame_count, Some(3_001));
        assert_eq!(audio.decode_errors, 0);
        assert_eq!(audio.clipping_events, 0);
        assert!(audio.peak_db.unwrap() < 0.0);
        assert_eq!(audio.silence, vec![TimeRange::new(0, 375)]);
        assert!(audio.true_peak_db.is_none());
        assert!(audio.loudness_lufs.is_none());
        assert!(!audio.silence_truncated);
        assert_eq!(decoded.diagnostics["audio_decode"]["status"], "complete");
    }

    #[test]
    fn cadence_measures_stereo_phase_and_dc_offset() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fixture-stereo.wav");
        let bytes = fixture_stereo_wav(&[0.5, -0.5, 0.25, -0.25]);
        std::fs::write(&path, &bytes).unwrap();
        let asset = test_asset(path, bytes.len() as u64);
        let metadata = Inspection {
            container: ContainerInspection {
                validity: ContainerValidity::Ok,
                ..Default::default()
            },
            audio: vec![AudioMeasurements {
                stream_idx: 0,
                ..Default::default()
            }],
            ..Default::default()
        };

        let decoded = CadenceAudioInspector::new()
            .inspect_decode(&asset, &metadata, InspectionLevel::Full)
            .unwrap();
        let audio = decoded.audio_for(0).unwrap();
        assert_eq!(audio.decoded_frame_count, Some(2));
        assert_eq!(audio.phase_correlation, Some(-1.0));
        assert_eq!(audio.dc_offset_percent, Some(0.0));
        assert!(audio.silence.is_empty());
    }

    #[test]
    fn silence_ranges_are_bounded_and_mark_incomplete() {
        let config = AudioAnalyzerConfig {
            max_silence_ranges: 1,
            ..AudioAnalyzerConfig::default()
        };
        let mut analyzer = AudioFrameAnalyzer::new(0, 1_000, 1, config).unwrap();
        analyzer.push(&[0.0, 1.0, 0.0, 1.0, 0.0]).unwrap();
        let measurement = analyzer.finish();
        assert_eq!(measurement.silence.len(), 1);
        assert!(measurement.silence_truncated);
    }

    #[test]
    fn unsupported_audio_container_does_not_create_decode_coverage() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fixture.mp3");
        std::fs::write(&path, b"not decoded").unwrap();
        let asset = test_asset(path, 10);
        let metadata = Inspection {
            audio: vec![AudioMeasurements {
                stream_idx: 0,
                ..Default::default()
            }],
            ..Default::default()
        };

        let decoded = CadenceAudioInspector::new()
            .inspect_decode(&asset, &metadata, InspectionLevel::Full)
            .unwrap();
        assert_eq!(decoded.audio_for(0).unwrap().decoded_frame_count, None);
        assert_eq!(decoded.diagnostics["audio_decode"]["status"], "incomplete");
    }

    fn test_asset(path: std::path::PathBuf, size: u64) -> Asset {
        Asset {
            id: Default::default(),
            path,
            fingerprint: AssetFingerprint {
                sha256: "a".repeat(64),
                size_bytes: size,
            },
            size_bytes: size,
            modified_time: None,
            duration: None,
            streams: Vec::new(),
        }
    }

    fn fixture_wav(samples: &[f32]) -> Vec<u8> {
        fixture_pcm_wav(samples, 1)
    }

    fn fixture_stereo_wav(samples: &[f32]) -> Vec<u8> {
        fixture_pcm_wav(samples, 2)
    }

    fn fixture_pcm_wav(samples: &[f32], channels: u16) -> Vec<u8> {
        let mut pcm = Vec::with_capacity(samples.len() * 2);
        for sample in samples {
            pcm.extend_from_slice(
                &((sample.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16).to_le_bytes(),
            );
        }
        let data_len = pcm.len() as u32;
        let byte_rate = 8_000u32 * u32::from(channels) * 2;
        let block_align = channels * 2;
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_len).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&channels.to_le_bytes());
        wav.extend_from_slice(&8_000u32.to_le_bytes());
        wav.extend_from_slice(&byte_rate.to_le_bytes());
        wav.extend_from_slice(&block_align.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_len.to_le_bytes());
        wav.extend_from_slice(&pcm);
        wav
    }
}
