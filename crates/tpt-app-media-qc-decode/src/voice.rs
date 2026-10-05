//! Optional voice analysis (spec § 8.8, Phase 3).
//!
//! Two independent pieces, both off unless a profile asks for them:
//!
//! * **Transcript comparison** — exact text-against-text WER and
//!   unexpected-word counts from sidecar files ([`crate::transcript`]).
//! * **Speech and speaker analysis** — energy-based voice activity detection
//!   and unsupervised speaker clustering from `tpt-voice`'s weight-free
//!   classical path, available when the crate is built with the `voice`
//!   feature. These are heuristics and are labelled **probabilistic**.
//!
//! The analysis needs a standalone WAV, AIFF/AIFC or FLAC file (the same
//! containers the Cadence adapter decodes) and looks at the first
//! [`MAX_ANALYSED_SECONDS`] of audio. No speech-to-text engine is bundled:
//! `tpt-voice` ships no trained weights yet, so transcripts are supplied.

use tpt_app_media_qc_model::asset::Asset;
use tpt_app_media_qc_model::inspection::{Inspection, VoiceMeasurements};

use crate::transcript;

/// Longest stretch of audio analysed for speech and speakers.
pub const MAX_ANALYSED_SECONDS: u64 = 3600;

/// What to compute.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VoiceConfig {
    /// Run speech detection and speaker clustering.
    pub speech: bool,
    /// Compare sidecar transcripts.
    pub transcript: bool,
}

impl VoiceConfig {
    pub fn is_enabled(&self) -> bool {
        self.speech || self.transcript
    }
}

/// Adds [`VoiceMeasurements`] to an inspection.
#[derive(Clone, Copy, Debug, Default)]
pub struct VoiceAnalyzer {
    config: VoiceConfig,
}

impl VoiceAnalyzer {
    pub fn new(config: VoiceConfig) -> Self {
        Self { config }
    }

    /// Whether this build can run speech and speaker analysis.
    pub const fn speech_available() -> bool {
        cfg!(feature = "voice")
    }

    /// Returns `inspection` plus voice measurements for its first audio
    /// stream. Failures are recorded as diagnostics and never abort the scan.
    pub fn analyze(&self, asset: &Asset, inspection: &Inspection) -> Inspection {
        let mut out = inspection.clone();
        if !self.config.is_enabled() {
            return out;
        }
        let Some(stream_idx) = inspection.audio.iter().map(|a| a.stream_idx).min() else {
            out.diagnostics.insert(
                "voice_analysis".into(),
                serde_json::json!({
                    "status": "not_applicable",
                    "reason": "no audio stream was inspected",
                }),
            );
            return out;
        };

        let mut measurements = VoiceMeasurements {
            stream_idx,
            ..Default::default()
        };
        let mut diagnostics = serde_json::Map::new();

        if self.config.transcript {
            match transcript::compare_sidecars(&asset.path) {
                Ok(Some(t)) => {
                    measurements.transcript = Some(t);
                    diagnostics.insert("transcript".into(), "compared".into());
                }
                Ok(None) => {
                    diagnostics.insert(
                        "transcript".into(),
                        "no <name>.expected.txt / <name>.transcript.txt sidecars".into(),
                    );
                }
                Err(e) => {
                    diagnostics.insert("transcript".into(), format!("not compared: {e}").into());
                }
            }
        }

        if self.config.speech {
            match analyse_speech(asset, &mut measurements) {
                Ok(()) => {
                    diagnostics.insert("speech".into(), "analysed".into());
                }
                Err(reason) => {
                    diagnostics.insert("speech".into(), format!("not analysed: {reason}").into());
                }
            }
        }

        diagnostics.insert("probabilistic".into(), measurements.speech_analysed.into());
        out.diagnostics.insert(
            "voice_analysis".into(),
            serde_json::Value::Object(diagnostics),
        );
        if measurements.speech_analysed || measurements.transcript.is_some() {
            out.voice.retain(|v| v.stream_idx != stream_idx);
            out.voice.push(measurements);
        }
        out
    }
}

#[cfg(not(feature = "voice"))]
fn analyse_speech(_asset: &Asset, _out: &mut VoiceMeasurements) -> Result<(), String> {
    Err("this build does not include the optional `voice` feature".into())
}

#[cfg(feature = "voice")]
fn analyse_speech(asset: &Asset, out: &mut VoiceMeasurements) -> Result<(), String> {
    engine::analyse(asset, out)
}

#[cfg(feature = "voice")]
mod engine {
    use std::fs::File;

    use tpt_app_media_qc_model::asset::Asset;
    use tpt_app_media_qc_model::finding::TimeRange;
    use tpt_app_media_qc_model::inspection::{SpeakerTurn, VoiceMeasurements};
    use tpt_av_cadence_aiff::AiffDecoder;
    use tpt_av_cadence_core::Decoder;
    use tpt_av_cadence_flac::FlacDecoder;
    use tpt_av_cadence_wav::WavDecoder;
    use tpt_av_voice_diarize::{Diarizer, DiarizerConfig, VoiceActivityDetector};

    use super::MAX_ANALYSED_SECONDS;

    /// Sample rate the voice engine works at.
    const TARGET_RATE: u32 = 16_000;
    const DECODE_FRAMES: usize = 8192;
    const METHOD: &str =
        "tpt-voice classical: energy VAD + MFCC-statistics speaker clustering (no trained model)";

    pub fn analyse(asset: &Asset, out: &mut VoiceMeasurements) -> Result<(), String> {
        let extension = asset
            .path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .ok_or("file has no extension; use WAV, AIFF/AIFC or FLAC")?;
        let file = File::open(&asset.path).map_err(|e| e.to_string())?;
        let (samples, rate) = match extension.as_str() {
            "wav" | "wave" => read_mono(WavDecoder::from_source(Box::new(file)).map_err(err)?)?,
            "aif" | "aiff" | "aifc" => {
                read_mono(AiffDecoder::from_source(Box::new(file)).map_err(err)?)?
            }
            "flac" => read_mono(FlacDecoder::from_source(Box::new(file)).map_err(err)?)?,
            other => {
                return Err(format!(
                    "voice analysis reads WAV, AIFF/AIFC and FLAC, not .{other}"
                ))
            }
        };
        let samples = to_target_rate(&samples, rate);
        if samples.is_empty() {
            return Err("the file contains no audio".into());
        }
        let analysed_ms = samples.len() as u64 * 1000 / u64::from(TARGET_RATE);

        // The detectors are numerical code over untrusted audio: a panic must
        // cost this stream its voice analysis, not the batch.
        let result = std::panic::catch_unwind(|| detect(&samples));
        let (regions, turns, speakers) = match result {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => return Err(e),
            Err(_) => return Err("the voice engine failed unexpectedly on this audio".into()),
        };

        out.method = METHOD.into();
        out.speech_analysed = true;
        out.analysed_ms = analysed_ms;
        out.speech_ms = regions.iter().map(TimeRange::duration_ms).sum();
        out.longest_non_speech = longest_gap(&regions, analysed_ms);
        out.speaker_changes = turns
            .windows(2)
            .filter(|pair| pair[0].speaker != pair[1].speaker)
            .count() as u32;
        out.speaker_turns = turns;
        out.speaker_count = Some(speakers as u32);
        out.speech_regions = regions;
        Ok(())
    }

    fn err(e: impl std::fmt::Display) -> String {
        e.to_string()
    }

    fn detect(samples: &[f32]) -> Result<(Vec<TimeRange>, Vec<SpeakerTurn>, usize), String> {
        let config = DiarizerConfig::default();
        let regions = VoiceActivityDetector::new(config.vad.clone())
            .map_err(err)?
            .detect(samples, TARGET_RATE)
            .into_iter()
            .map(|r| TimeRange::new(secs_to_ms(r.start), secs_to_ms(r.end)))
            .collect();
        let result = Diarizer::classical(config)
            .map_err(err)?
            .diarize_samples(samples, TARGET_RATE)
            .map_err(err)?;
        let turns = result
            .segments
            .iter()
            .map(|s| SpeakerTurn {
                speaker: s.speaker.to_string(),
                start_ms: secs_to_ms(s.start_time),
                end_ms: secs_to_ms(s.end_time),
                confidence: s.confidence,
            })
            .collect();
        Ok((regions, turns, result.num_speakers))
    }

    fn secs_to_ms(s: f64) -> u64 {
        (s.max(0.0) * 1000.0).round() as u64
    }

    /// Gaps between speech regions, including the audio's start and end.
    pub(super) fn longest_gap(regions: &[TimeRange], total_ms: u64) -> Option<TimeRange> {
        let mut best: Option<TimeRange> = None;
        let mut cursor = 0u64;
        let mut consider = |start: u64, end: u64| {
            if end > start && best.is_none_or(|b| end - start > b.duration_ms()) {
                best = Some(TimeRange::new(start, end));
            }
        };
        for r in regions {
            consider(cursor, r.start_ms);
            cursor = cursor.max(r.end_ms);
        }
        consider(cursor, total_ms);
        best
    }

    /// Decode to mono `f32`, stopping after [`MAX_ANALYSED_SECONDS`].
    fn read_mono<D: Decoder>(mut decoder: D) -> Result<(Vec<f32>, u32), String> {
        let (rate, channels) = {
            let info = decoder.info();
            (info.sample_rate, usize::from(info.channels))
        };
        if rate == 0 || channels == 0 || channels > 32 {
            return Err(format!(
                "unsupported audio layout ({channels} ch @ {rate} Hz)"
            ));
        }
        let limit = (u64::from(rate) * MAX_ANALYSED_SECONDS) as usize;
        let mut buffer = vec![0.0f32; DECODE_FRAMES * channels];
        let mut mono = Vec::new();
        while mono.len() < limit {
            let frames = decoder.decode(&mut buffer).map_err(err)?;
            if frames == 0 {
                break;
            }
            let used = frames
                .checked_mul(channels)
                .filter(|n| *n <= buffer.len())
                .ok_or("the decoder returned an invalid sample count")?;
            mono.extend(
                buffer[..used]
                    .chunks_exact(channels)
                    .map(|f| f.iter().sum::<f32>() / channels as f32),
            );
        }
        mono.truncate(limit);
        Ok((mono, rate))
    }

    /// Resample to 16 kHz. Integer ratios use a box average, which also acts
    /// as the anti-alias filter that plain linear interpolation lacks.
    fn to_target_rate(samples: &[f32], rate: u32) -> Vec<f32> {
        if rate == TARGET_RATE {
            return samples.to_vec();
        }
        if rate > TARGET_RATE && rate % TARGET_RATE == 0 {
            let step = (rate / TARGET_RATE) as usize;
            return samples
                .chunks(step)
                .map(|c| c.iter().sum::<f32>() / c.len() as f32)
                .collect();
        }
        tpt_av_voice_utils::dsp::resample_linear(samples, rate, TARGET_RATE)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// 16-bit mono PCM WAV.
        fn wav(samples: &[f32], rate: u32) -> Vec<u8> {
            let data: Vec<u8> = samples
                .iter()
                .flat_map(|s| ((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes())
                .collect();
            let mut out = b"RIFF".to_vec();
            out.extend((36 + data.len() as u32).to_le_bytes());
            out.extend(b"WAVEfmt ");
            out.extend(16u32.to_le_bytes());
            out.extend(1u16.to_le_bytes());
            out.extend(1u16.to_le_bytes());
            out.extend(rate.to_le_bytes());
            out.extend((rate * 2).to_le_bytes());
            out.extend(2u16.to_le_bytes());
            out.extend(16u16.to_le_bytes());
            out.extend(b"data");
            out.extend((data.len() as u32).to_le_bytes());
            out.extend(data);
            out
        }

        /// A harmonic, syllable-modulated tone: 1 s lead-in silence, 2 s of
        /// voice-like sound, 5 s of silence.
        fn voice_like(rate: u32) -> Vec<f32> {
            let mut out = vec![0.0f32; rate as usize];
            for n in 0..(2 * rate) {
                let t = n as f32 / rate as f32;
                let envelope = 0.5 + 0.5 * (2.0 * std::f32::consts::PI * 4.0 * t).sin();
                let tone: f32 = (1..=6)
                    .map(|h| (2.0 * std::f32::consts::PI * 140.0 * h as f32 * t).sin() / h as f32)
                    .sum();
                out.push(0.4 * envelope * tone);
            }
            out.extend(std::iter::repeat_n(0.0, 5 * rate as usize));
            out
        }

        #[test]
        fn analyses_speech_regions_and_the_silent_gap_from_a_wav() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("talk.wav");
            std::fs::write(&path, wav(&voice_like(48_000), 48_000)).unwrap();
            let asset = Asset {
                id: Default::default(),
                path,
                fingerprint: tpt_app_media_qc_model::asset::AssetFingerprint {
                    sha256: "0".repeat(64),
                    size_bytes: 1,
                },
                size_bytes: 1,
                modified_time: None,
                duration: None,
                streams: vec![],
            };
            let mut out = VoiceMeasurements::default();
            analyse(&asset, &mut out).expect("analysis runs");

            assert!(out.speech_analysed);
            assert_eq!(out.analysed_ms, 8_000);
            assert!(
                (1_000..=3_500).contains(&out.speech_ms),
                "speech_ms {}",
                out.speech_ms
            );
            let gap = out.longest_non_speech.expect("a silent gap");
            assert!(gap.duration_ms() >= 4_000, "{gap:?}");
            assert!(out.speaker_count.is_some());
            assert!(out.method.contains("tpt-voice"));
        }

        #[test]
        fn silence_has_no_speech() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("quiet.wav");
            std::fs::write(&path, wav(&vec![0.0; 16_000 * 3], 16_000)).unwrap();
            let asset = Asset {
                id: Default::default(),
                path,
                fingerprint: tpt_app_media_qc_model::asset::AssetFingerprint {
                    sha256: "0".repeat(64),
                    size_bytes: 1,
                },
                size_bytes: 1,
                modified_time: None,
                duration: None,
                streams: vec![],
            };
            let mut out = VoiceMeasurements::default();
            analyse(&asset, &mut out).expect("analysis runs");
            assert_eq!(out.speech_ms, 0);
            assert_eq!(out.longest_non_speech, Some(TimeRange::new(0, 3_000)));
        }

        #[test]
        fn longest_gap_includes_leading_and_trailing_silence() {
            let regions = [TimeRange::new(2_000, 3_000), TimeRange::new(4_000, 5_000)];
            assert_eq!(
                longest_gap(&regions, 12_000),
                Some(TimeRange::new(5_000, 12_000))
            );
            assert_eq!(longest_gap(&[], 800), Some(TimeRange::new(0, 800)));
            assert_eq!(longest_gap(&[TimeRange::new(0, 800)], 800), None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpt_app_media_qc_model::inspection::AudioMeasurements;

    fn asset(path: std::path::PathBuf) -> Asset {
        Asset {
            id: Default::default(),
            path,
            fingerprint: tpt_app_media_qc_model::asset::AssetFingerprint {
                sha256: "0".repeat(64),
                size_bytes: 1,
            },
            size_bytes: 1,
            modified_time: None,
            duration: None,
            streams: vec![],
        }
    }

    fn with_audio() -> Inspection {
        Inspection {
            audio: vec![AudioMeasurements {
                stream_idx: 1,
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn disabled_config_changes_nothing() {
        let a = VoiceAnalyzer::default().analyze(&asset("x.wav".into()), &with_audio());
        assert!(a.voice.is_empty());
        assert!(!a.diagnostics.contains_key("voice_analysis"));
    }

    #[test]
    fn transcript_comparison_attaches_to_the_first_audio_stream() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.expected.txt"), "hello brave world").unwrap();
        std::fs::write(dir.path().join("a.transcript.txt"), "hello new world").unwrap();
        let analyzer = VoiceAnalyzer::new(VoiceConfig {
            speech: false,
            transcript: true,
        });
        let out = analyzer.analyze(&asset(dir.path().join("a.wav")), &with_audio());
        let v = out.voice_for(1).expect("voice measurements");
        assert!(!v.speech_analysed);
        assert_eq!(v.transcript.as_ref().unwrap().substitutions, 1);
    }

    #[test]
    fn missing_sidecars_leave_no_measurements() {
        let dir = tempfile::tempdir().unwrap();
        let analyzer = VoiceAnalyzer::new(VoiceConfig {
            speech: false,
            transcript: true,
        });
        let out = analyzer.analyze(&asset(dir.path().join("a.wav")), &with_audio());
        assert!(out.voice.is_empty());
        assert!(out.diagnostics.contains_key("voice_analysis"));
    }

    #[test]
    fn no_audio_stream_is_not_applicable() {
        let analyzer = VoiceAnalyzer::new(VoiceConfig {
            speech: true,
            transcript: true,
        });
        let out = analyzer.analyze(&asset("x.wav".into()), &Inspection::default());
        assert_eq!(
            out.diagnostics["voice_analysis"]["status"],
            "not_applicable"
        );
    }

    #[cfg(not(feature = "voice"))]
    #[test]
    fn speech_analysis_reports_when_the_feature_is_missing() {
        let analyzer = VoiceAnalyzer::new(VoiceConfig {
            speech: true,
            transcript: false,
        });
        let out = analyzer.analyze(&asset("x.wav".into()), &with_audio());
        assert!(out.voice.is_empty());
        let note = out.diagnostics["voice_analysis"]["speech"]
            .as_str()
            .unwrap();
        assert!(note.contains("voice"), "{note}");
    }
}
