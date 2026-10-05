//! Automated correction (Phase 3): loudness gain and DC-offset removal.
//!
//! Deliberately narrow. A correction is only made when it is exact and
//! verifiable:
//!
//! * **Gain** is a single linear scale applied to every sample. It is only
//!   applied when the result still respects the true-peak ceiling, because no
//!   limiter or compressor is implemented; otherwise the plan says why it was
//!   refused.
//! * **DC removal** subtracts each channel's measured mean.
//!
//! The input is never modified: output goes to a new 16- or 24-bit PCM WAV
//! file (written through a staged `.part` file and renamed), and the result is
//! re-measured from the written file so the report shows real before/after
//! values rather than predictions. Inputs are the same standalone WAV,
//! AIFF/AIFC and FLAC files the Cadence adapter reads.

use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tpt_app_media_qc_core::error::{Error, Result};
use tpt_av_cadence_aiff::AiffDecoder;
use tpt_av_cadence_core::Decoder;
use tpt_av_cadence_flac::FlacDecoder;
use tpt_av_cadence_wav::WavDecoder;

use crate::loudness::{LoudnessMeter, TruePeakMeter};

const DECODE_FRAMES: usize = 8192;
const MAX_CHANNELS: usize = 32;
/// Gains smaller than this are inaudible and not worth a rewrite.
const MIN_GAIN_DB: f64 = 0.05;
/// DC below this share of full scale is left alone.
const MIN_DC: f64 = 0.0005;
/// WAV data chunks are limited to 32-bit sizes.
const MAX_DATA_BYTES: u64 = u32::MAX as u64 - 64;

/// Output sample format.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputBits {
    Sixteen,
    #[default]
    TwentyFour,
}

impl OutputBits {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "16" => Some(OutputBits::Sixteen),
            "24" => Some(OutputBits::TwentyFour),
            _ => None,
        }
    }

    fn bytes(self) -> usize {
        match self {
            OutputBits::Sixteen => 2,
            OutputBits::TwentyFour => 3,
        }
    }
}

/// What the caller wants corrected.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CorrectionRequest {
    /// Integrated loudness to reach (LUFS, BS.1770).
    pub target_lufs: Option<f64>,
    /// Highest true peak (dBTP) the corrected file may have.
    pub true_peak_ceiling_db: f64,
    pub remove_dc: bool,
    pub bits: OutputBits,
}

/// Levels measured from a file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Levels {
    pub loudness_lufs: Option<f64>,
    pub true_peak_db: Option<f64>,
    pub sample_peak_db: Option<f64>,
    /// Largest per-channel DC offset, as a fraction of full scale.
    pub max_dc: f64,
}

/// Decided corrections, before anything is written.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CorrectionPlan {
    pub before: Levels,
    /// Linear gain to apply, in dB, when a gain is both wanted and safe.
    pub gain_db: Option<f64>,
    pub remove_dc: bool,
    pub channel_dc: Vec<f64>,
    /// Why a requested correction was not planned.
    pub refused: Vec<String>,
}

impl CorrectionPlan {
    /// Whether anything would change.
    pub fn has_changes(&self) -> bool {
        self.gain_db.is_some() || self.remove_dc
    }
}

/// Result of applying a plan.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CorrectionReport {
    pub plan: CorrectionPlan,
    pub output: PathBuf,
    pub after: Levels,
}

struct StreamInfo {
    sample_rate: u32,
    channels: usize,
}

/// Decode `path`, handing every block of interleaved samples to `sink`.
fn decode_blocks(
    path: &Path,
    mut sink: impl FnMut(&StreamInfo, &[f32]) -> Result<()>,
) -> Result<StreamInfo> {
    fn drive<D: Decoder>(
        mut decoder: D,
        sink: &mut impl FnMut(&StreamInfo, &[f32]) -> Result<()>,
    ) -> Result<StreamInfo> {
        let info = {
            let i = decoder.info();
            StreamInfo {
                sample_rate: i.sample_rate,
                channels: usize::from(i.channels),
            }
        };
        if info.channels == 0 || info.channels > MAX_CHANNELS || info.sample_rate == 0 {
            return Err(Error::Unsupported(format!(
                "unsupported audio layout ({} ch @ {} Hz)",
                info.channels, info.sample_rate
            )));
        }
        let mut buffer = vec![0.0f32; DECODE_FRAMES * info.channels];
        loop {
            let frames = decoder
                .decode(&mut buffer)
                .map_err(|e| Error::Probe(format!("audio decode failed: {e}")))?;
            if frames == 0 {
                return Ok(info);
            }
            let used = frames
                .checked_mul(info.channels)
                .filter(|n| *n <= buffer.len())
                .ok_or_else(|| Error::Probe("the decoder returned an invalid length".into()))?;
            sink(&info, &buffer[..used])?;
        }
    }

    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| Error::Unsupported("file has no extension".into()))?;
    let open = || File::open(path).map_err(Error::from);
    let mapped = |e: tpt_av_cadence_core::CadenceError| Error::Probe(format!("cannot open: {e}"));
    match extension.as_str() {
        "wav" | "wave" => drive(
            WavDecoder::from_source(Box::new(open()?)).map_err(mapped)?,
            &mut sink,
        ),
        "aif" | "aiff" | "aifc" => drive(
            AiffDecoder::from_source(Box::new(open()?)).map_err(mapped)?,
            &mut sink,
        ),
        "flac" => drive(
            FlacDecoder::from_source(Box::new(open()?)).map_err(mapped)?,
            &mut sink,
        ),
        other => Err(Error::Unsupported(format!(
            "correction reads WAV, AIFF/AIFC and FLAC, not .{other}"
        ))),
    }
}

fn db(linear: f64) -> Option<f64> {
    (linear > 0.0).then(|| 20.0 * linear.log10())
}

/// Measure loudness, peaks and DC offset of a file.
pub fn measure(path: &Path) -> Result<(Levels, Vec<f64>)> {
    let mut meters: Option<(LoudnessMeter, TruePeakMeter)> = None;
    let mut sums: Vec<f64> = Vec::new();
    let mut frames = 0u64;
    let mut peak = 0.0f64;
    decode_blocks(path, |info, samples| {
        let (loudness, true_peak) = meters.get_or_insert_with(|| {
            sums = vec![0.0; info.channels];
            (
                LoudnessMeter::new(info.sample_rate, info.channels),
                TruePeakMeter::new(info.channels),
            )
        });
        for frame in samples.chunks_exact(info.channels) {
            loudness.push_frame(frame);
            true_peak.push_frame(frame);
            for (channel, s) in frame.iter().enumerate() {
                sums[channel] += f64::from(*s);
                peak = peak.max(f64::from(s.abs()));
            }
            frames += 1;
        }
        Ok(())
    })?;
    let Some((loudness, true_peak)) = meters else {
        return Err(Error::Probe("the file contains no audio".into()));
    };
    let channel_dc: Vec<f64> = sums.iter().map(|s| s / frames.max(1) as f64).collect();
    let levels = Levels {
        loudness_lufs: loudness.integrated_loudness_lufs(),
        true_peak_db: db(true_peak.finish()),
        sample_peak_db: db(peak),
        max_dc: channel_dc.iter().fold(0.0f64, |m, d| m.max(d.abs())),
    };
    Ok((levels, channel_dc))
}

/// Decide what can safely be corrected.
pub fn plan(path: &Path, request: &CorrectionRequest) -> Result<CorrectionPlan> {
    let (before, channel_dc) = measure(path)?;
    let mut refused = Vec::new();

    let remove_dc = request.remove_dc && before.max_dc >= MIN_DC;

    let mut gain_db = None;
    if let Some(target) = request.target_lufs {
        match (before.loudness_lufs, before.true_peak_db) {
            (Some(measured), Some(true_peak)) => {
                let wanted = target - measured;
                if wanted.abs() < MIN_GAIN_DB {
                    // Already on target.
                } else if true_peak + wanted > request.true_peak_ceiling_db {
                    refused.push(format!(
                        "reaching {target:.1} LUFS needs {wanted:+.2} dB, which would raise the \
                         true peak from {true_peak:.2} to {:.2} dBTP (ceiling {:.2} dBTP); no \
                         limiter is implemented, so no gain was applied",
                        true_peak + wanted,
                        request.true_peak_ceiling_db
                    ));
                } else {
                    gain_db = Some(wanted);
                }
            }
            _ => refused.push(
                "loudness could not be measured (file too short or silent), so no gain was applied"
                    .into(),
            ),
        }
    }

    Ok(CorrectionPlan {
        before,
        gain_db,
        remove_dc,
        channel_dc,
        refused,
    })
}

/// Apply `plan` to `input`, writing a new WAV at `output` and re-measuring it.
///
/// Refuses to overwrite `input` or an existing `output`.
pub fn apply(
    input: &Path,
    output: &Path,
    plan: &CorrectionPlan,
    bits: OutputBits,
) -> Result<CorrectionReport> {
    if !plan.has_changes() {
        return Err(Error::Unsupported(
            "nothing to correct: the plan contains no changes".into(),
        ));
    }
    if output.exists() {
        return Err(Error::Unsupported(format!(
            "'{}' already exists; choose a new output path",
            output.display()
        )));
    }
    if let (Ok(a), Some(parent)) = (input.canonicalize(), output.parent()) {
        let parent = if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        };
        if let (Ok(p), Some(name)) = (parent.canonicalize(), output.file_name()) {
            if p.join(name) == a {
                return Err(Error::Unsupported(
                    "the output must not be the input file".into(),
                ));
            }
        }
    }

    let gain = plan.gain_db.map_or(1.0, |g| 10f64.powf(g / 20.0));
    let dc: Vec<f64> = if plan.remove_dc {
        plan.channel_dc.clone()
    } else {
        Vec::new()
    };

    let staged = output.with_extension("wav.part");
    let result = write_wav(input, &staged, gain, &dc, bits);
    match result {
        Ok(()) => std::fs::rename(&staged, output)?,
        Err(e) => {
            let _ = std::fs::remove_file(&staged);
            return Err(e);
        }
    }

    let (after, _) = measure(output)?;
    Ok(CorrectionReport {
        plan: plan.clone(),
        output: output.to_path_buf(),
        after,
    })
}

fn write_wav(input: &Path, staged: &Path, gain: f64, dc: &[f64], bits: OutputBits) -> Result<()> {
    let mut out = BufWriter::new(File::create(staged)?);
    let mut data_bytes = 0u64;
    let mut header_written = false;
    let bytes = bits.bytes();
    let scale = f64::from(1u32 << (bytes * 8 - 1));
    let (lo, hi) = (-scale, scale - 1.0);

    let info = decode_blocks(input, |info, samples| {
        if !header_written {
            write_header(&mut out, info, bits, 0)?;
            header_written = true;
        }
        for frame in samples.chunks_exact(info.channels) {
            for (channel, s) in frame.iter().enumerate() {
                let offset = dc.get(channel).copied().unwrap_or(0.0);
                let v = ((f64::from(*s) - offset) * gain * scale)
                    .round()
                    .clamp(lo, hi) as i64;
                out.write_all(&v.to_le_bytes()[..bytes])?;
                data_bytes += bytes as u64;
            }
        }
        if data_bytes > MAX_DATA_BYTES {
            return Err(Error::Unsupported(
                "the corrected audio is too large for a WAV file".into(),
            ));
        }
        Ok(())
    })?;
    if !header_written {
        return Err(Error::Probe("the file contains no audio".into()));
    }
    out.flush()?;
    let mut file = out.into_inner().map_err(|e| Error::from(e.into_error()))?;
    file.seek(SeekFrom::Start(0))?;
    write_header(&mut file, &info, bits, data_bytes as u32)?;
    file.flush()?;
    Ok(())
}

fn write_header(
    out: &mut impl Write,
    info: &StreamInfo,
    bits: OutputBits,
    data_bytes: u32,
) -> Result<()> {
    let block_align = (info.channels * bits.bytes()) as u16;
    let byte_rate = info.sample_rate * u32::from(block_align);
    out.write_all(b"RIFF")?;
    out.write_all(&(36 + data_bytes).to_le_bytes())?;
    out.write_all(b"WAVEfmt ")?;
    out.write_all(&16u32.to_le_bytes())?;
    out.write_all(&1u16.to_le_bytes())?;
    out.write_all(&(info.channels as u16).to_le_bytes())?;
    out.write_all(&info.sample_rate.to_le_bytes())?;
    out.write_all(&byte_rate.to_le_bytes())?;
    out.write_all(&block_align.to_le_bytes())?;
    out.write_all(&((bits.bytes() * 8) as u16).to_le_bytes())?;
    out.write_all(b"data")?;
    out.write_all(&data_bytes.to_le_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_test_wav(path: &Path, rate: u32, channels: u16, seconds: u32, amp: f32, dc: f32) {
        let info = StreamInfo {
            sample_rate: rate,
            channels: usize::from(channels),
        };
        let frames = rate * seconds;
        let mut out = BufWriter::new(File::create(path).unwrap());
        let bytes = frames * u32::from(channels) * 2;
        write_header(&mut out, &info, OutputBits::Sixteen, bytes).unwrap();
        for n in 0..frames {
            let t = n as f32 / rate as f32;
            let s = amp * (2.0 * std::f32::consts::PI * 997.0 * t).sin() + dc;
            let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
            for _ in 0..channels {
                out.write_all(&v.to_le_bytes()).unwrap();
            }
        }
        out.flush().unwrap();
    }

    fn request(target: Option<f64>) -> CorrectionRequest {
        CorrectionRequest {
            target_lufs: target,
            true_peak_ceiling_db: -1.0,
            remove_dc: true,
            bits: OutputBits::TwentyFour,
        }
    }

    #[test]
    fn gain_brings_a_quiet_file_to_target_and_is_verified_from_the_output() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("quiet.wav");
        write_test_wav(&input, 48_000, 2, 6, 0.05, 0.0);

        let plan = plan(&input, &request(Some(-23.0))).unwrap();
        let gain = plan.gain_db.expect("a gain is planned");
        assert!(gain > 0.0, "{plan:?}");

        let output = dir.path().join("fixed.wav");
        let report = apply(&input, &output, &plan, OutputBits::TwentyFour).unwrap();
        let after = report.after.loudness_lufs.expect("measured");
        assert!((after - -23.0).abs() < 0.1, "after {after}");
        assert!(report.after.true_peak_db.unwrap() <= -1.0);
        // The original is untouched.
        let (still, _) = measure(&input).unwrap();
        assert_eq!(still, plan.before);
    }

    #[test]
    fn gain_that_would_break_the_true_peak_ceiling_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("hot.wav");
        // A 0.5-amplitude sine peaks at -6 dBFS; a -3 LUFS target needs more
        // gain than the -1 dBTP ceiling allows.
        write_test_wav(&input, 48_000, 1, 4, 0.5, 0.0);
        let plan = plan(&input, &request(Some(-3.0))).unwrap();
        assert!(plan.gain_db.is_none());
        assert_eq!(plan.refused.len(), 1);
        assert!(plan.refused[0].contains("no limiter"), "{:?}", plan.refused);
        assert!(!plan.has_changes());
    }

    #[test]
    fn dc_offset_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("dc.wav");
        write_test_wav(&input, 44_100, 1, 3, 0.2, 0.1);
        let plan = plan(&input, &request(None)).unwrap();
        assert!(plan.remove_dc);
        assert!((plan.before.max_dc - 0.1).abs() < 0.01);

        let output = dir.path().join("clean.wav");
        let report = apply(&input, &output, &plan, OutputBits::Sixteen).unwrap();
        assert!(report.after.max_dc < 0.001, "{}", report.after.max_dc);
    }

    #[test]
    fn a_file_already_on_target_with_no_dc_needs_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("ok.wav");
        write_test_wav(&input, 48_000, 2, 5, 0.1, 0.0);
        let (levels, _) = measure(&input).unwrap();
        let plan = plan(&input, &request(levels.loudness_lufs)).unwrap();
        assert!(!plan.has_changes());
        assert!(matches!(
            apply(
                &input,
                &dir.path().join("x.wav"),
                &plan,
                OutputBits::Sixteen
            ),
            Err(Error::Unsupported(_))
        ));
    }

    #[test]
    fn never_overwrites_the_input_or_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("in.wav");
        write_test_wav(&input, 48_000, 1, 3, 0.05, 0.0);
        let plan = plan(&input, &request(Some(-20.0))).unwrap();
        assert!(plan.has_changes());
        assert!(apply(&input, &input, &plan, OutputBits::Sixteen).is_err());
        let existing = dir.path().join("exists.wav");
        std::fs::write(&existing, b"keep me").unwrap();
        assert!(apply(&input, &existing, &plan, OutputBits::Sixteen).is_err());
        assert_eq!(std::fs::read(&existing).unwrap(), b"keep me");
    }

    #[test]
    fn unsupported_inputs_are_errors_not_panics() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("x.mp3");
        std::fs::write(&bad, b"nope").unwrap();
        assert!(plan(&bad, &request(Some(-23.0))).is_err());
        let garbage = dir.path().join("g.wav");
        std::fs::write(&garbage, b"not a wav").unwrap();
        assert!(plan(&garbage, &request(Some(-23.0))).is_err());
    }
}
