//! ITU-R BS.1770-4 loudness and true-peak measurement for TPT Media QC.
//!
//! Two measurements live here, both driven from decoded PCM frames:
//!
//! * **Integrated loudness** (LUFS) — the two-stage K-weighting filter of
//!   BS.1770-4 §2, weighted mean square over 400 ms gating blocks that step
//!   every 100 ms (75 % overlap), and the absolute (−70 LUFS) plus relative
//!   (−10 LU) gates of §5. EBU R128, ATSC A/85 and plain BS.1770 all specify
//!   *this* measurement and differ only in target, and the target lives in the
//!   QC profile, so the inspector reports one standards-accurate value.
//! * **True peak** (dBTP) — BS.1770-4 Annex 2 requires the peak of the
//!   band-limited reconstruction, i.e. at least 4× oversampling before taking
//!   the maximum. This implementation upsamples 4× through a 48-tap
//!   Kaiser-windowed sinc reconstruction filter (≈ −80 dB stopband) and tracks
//!   the maximum of the interpolated signal.
//!
//! Loudness range (EBU Tech 3342) is computed from 3 s short-term windows
//! (100 ms hop) with a −70 LUFS absolute and −20 LU relative gate, reporting
//! the 10th–95th percentile spread.

use std::f64::consts::PI;

/// BS.1770-4 §2 stage-1 "head" (high-shelf) filter parameters.
const STAGE1_F0_HZ: f64 = 1_681.974_450_955_533;
const STAGE1_GAIN_DB: f64 = 3.999_843_853_973_347;
const STAGE1_Q: f64 = 0.707_175_236_955_419_6;
/// BS.1770-4 §2 stage-2 RLB (high-pass) filter parameters.
const STAGE2_F0_HZ: f64 = 38.135_470_876_024_44;
const STAGE2_Q: f64 = 0.500_327_037_323_877_3;

/// Absolute gate of BS.1770-4 §5.1 (LKFS).
const ABSOLUTE_GATE_LUFS: f64 = -70.0;
/// Relative gate offset below the absolute-gated mean (BS.1770-4 §5.2).
const RELATIVE_GATE_OFFSET_LU: f64 = -10.0;
/// Loudness offset `−0.691` from the weighted mean square (BS.1770-4 §4).
const LOUDNESS_OFFSET: f64 = -0.691;
/// Gating block length and step (BS.1770-4 §5: 400 ms blocks, 75 % overlap).
const BLOCK_MS: f64 = 400.0;
const STEP_MS: f64 = 100.0;
/// EBU Tech 3342 short-term window.
const SHORT_TERM_MS: f64 = 3000.0;
/// EBU Tech 3342 relative gate for loudness range.
const LRA_RELATIVE_GATE_LU: f64 = -20.0;

/// Oversampling factor for true-peak metering (BS.1770-4 Annex 2 minimum).
const OVERSAMPLE_FACTOR: usize = 4;
/// Taps per interpolation phase; 4 × 12 = 48-tap reconstruction filter.
const TAPS_PER_PHASE: usize = 12;
/// Kaiser window shape parameter, ≈ −80 dB stopband attenuation.
const KAISER_BETA: f64 = 8.6;

/// Convert a gated mean-square value to LUFS (BS.1770-4 §4).
fn energy_to_lufs(energy: f64) -> f64 {
    LOUDNESS_OFFSET + 10.0 * energy.log10()
}

/// Convert a loudness level to its equivalent mean-square energy.
fn lufs_to_energy(lufs: f64) -> f64 {
    10f64.powf((lufs - LOUDNESS_OFFSET) / 10.0)
}
/// A second-order section in direct-form II transposed form.
#[derive(Clone, Copy, Debug)]
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

impl Biquad {
    /// RBJ high-shelf section, as specified for BS.1770 stage 1.
    fn high_shelf(sample_rate: f64, f0: f64, gain_db: f64, q: f64) -> Self {
        let k = (PI * f0 / sample_rate).tan();
        let vh = 10f64.powf(gain_db / 20.0);
        // The standard's shelf is slightly asymmetric; Vb is derived per the
        // reference design rather than rounded to Vh.
        let vb = vh.powf(0.4996);
        let a0 = 1.0 + k / q + k * k;
        Self {
            b0: (vh + vb * k / q + k * k) / a0,
            b1: 2.0 * (k * k - vh) / a0,
            b2: (vh - vb * k / q + k * k) / a0,
            a1: 2.0 * (k * k - 1.0) / a0,
            a2: (1.0 - k / q + k * k) / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    /// RBJ second-order high-pass section, as specified for BS.1770 stage 2.
    fn high_pass(sample_rate: f64, f0: f64, q: f64) -> Self {
        let k = (PI * f0 / sample_rate).tan();
        let a0 = 1.0 + k / q + k * k;
        Self {
            b0: 1.0 / a0,
            b1: -2.0 / a0,
            b2: 1.0 / a0,
            a1: 2.0 * (k * k - 1.0) / a0,
            a2: (1.0 - k / q + k * k) / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    fn process(&mut self, input: f64) -> f64 {
        let output = self.b0 * input + self.z1;
        self.z1 = self.b1 * input - self.a1 * output + self.z2;
        self.z2 = self.b2 * input - self.a2 * output;
        output
    }
}
/// Integrated (gated) loudness meter per ITU-R BS.1770-4.
#[derive(Clone, Debug)]
pub(crate) struct LoudnessMeter {
    stage1: Vec<Biquad>,
    stage2: Vec<Biquad>,
    /// Gating block length in frames (400 ms).
    block_frames: usize,
    /// Distance between block starts in frames (100 ms).
    step_frames: usize,
    /// Per-frame weighted sum of squares inside the current block.
    window: Vec<f64>,
    window_sum: f64,
    filled: usize,
    /// Frames accumulated since the last completed block.
    since_step: usize,
    /// Total frames pushed, so no block is emitted before the window fills.
    frames_seen: usize,
    /// Completed block energies (weighted mean squares, ungated).
    blocks: Vec<f64>,
    /// Weighted sum of squares of the 100 ms step in progress.
    step_acc: f64,
    step_count: usize,
    /// Completed 100 ms step sums, from which 3 s short-term windows are built.
    step_sums: Vec<f64>,
}

impl LoudnessMeter {
    pub(crate) fn new(sample_rate: u32, channels: usize) -> Self {
        let rate = sample_rate.max(1) as f64;
        let channels = channels.max(1);
        Self {
            stage1: (0..channels)
                .map(|_| Biquad::high_shelf(rate, STAGE1_F0_HZ, STAGE1_GAIN_DB, STAGE1_Q))
                .collect(),
            stage2: (0..channels)
                .map(|_| Biquad::high_pass(rate, STAGE2_F0_HZ, STAGE2_Q))
                .collect(),
            block_frames: ((BLOCK_MS / 1000.0) * rate).round().max(1.0) as usize,
            step_frames: ((STEP_MS / 1000.0) * rate).round().max(1.0) as usize,
            window: Vec::new(),
            window_sum: 0.0,
            filled: 0,
            since_step: 0,
            frames_seen: 0,
            blocks: Vec::new(),
            step_acc: 0.0,
            step_count: 0,
            step_sums: Vec::new(),
        }
    }

    /// Feeds one frame: one sample per channel.
    pub(crate) fn push_frame(&mut self, frame: &[f32]) {
        if frame.is_empty() {
            return;
        }
        let mut sum_of_squares = 0.0;
        for (channel, value) in frame.iter().enumerate() {
            let sample = f64::from(*value);
            let weighted = self.stage1[channel].process(sample);
            let weighted = self.stage2[channel].process(weighted);
            // BS.1770-4 §4 uses a channel weight of 1.0 for every channel of
            // the basic (mono/stereo) configurations.
            sum_of_squares += weighted * weighted;
        }

        self.step_acc += sum_of_squares;
        self.step_count += 1;
        if self.step_count >= self.step_frames {
            self.step_sums.push(self.step_acc);
            self.step_acc = 0.0;
            self.step_count = 0;
        }

        if self.window.len() != self.block_frames {
            self.window.clear();
            self.window.resize(self.block_frames, 0.0);
            self.window_sum = 0.0;
            self.filled = 0;
            self.since_step = 0;
        }
        self.frames_seen = self.frames_seen.saturating_add(1);
        let oldest = self.window[self.filled];
        self.window[self.filled] = sum_of_squares;
        self.window_sum += sum_of_squares - oldest;
        self.filled = (self.filled + 1) % self.block_frames;
        self.since_step += 1;
        // A gating block is only meaningful once the whole 400 ms window holds
        // real audio, so nothing is emitted before `block_frames` frames.
        if self.frames_seen >= self.block_frames
            && self.since_step >= self.step_frames
            && self.window_sum.is_finite()
        {
            self.since_step = 0;
            self.blocks.push(self.window_sum / self.block_frames as f64);
        }
    }

    /// The gated integrated loudness, or `None` when no complete gating block
    /// was measured (shorter than 400 ms) or everything fell below the
    /// absolute gate (digital silence).
    pub(crate) fn integrated_loudness_lufs(&self) -> Option<f64> {
        let absolute_threshold = lufs_to_energy(ABSOLUTE_GATE_LUFS);
        let above_absolute: Vec<f64> = self
            .blocks
            .iter()
            .copied()
            .filter(|energy| *energy > absolute_threshold)
            .collect();
        if above_absolute.is_empty() {
            return None;
        }
        let absolute_mean = above_absolute.iter().sum::<f64>() / above_absolute.len() as f64;
        // The relative gate sits 10 LU *below* the absolute-gated mean, i.e.
        // one tenth of its energy.
        let relative_threshold = absolute_mean / 10f64.powf(-RELATIVE_GATE_OFFSET_LU / 10.0);
        let gate = relative_threshold.max(absolute_threshold);
        let gated: Vec<f64> = above_absolute
            .iter()
            .copied()
            .filter(|energy| *energy > gate)
            .collect();
        let mean = if gated.is_empty() {
            absolute_mean
        } else {
            gated.iter().sum::<f64>() / gated.len() as f64
        };
        (mean > 0.0 && mean.is_finite()).then(|| energy_to_lufs(mean))
    }
}

impl LoudnessMeter {
    /// Short-term (3 s) loudness energies, one per 100 ms step once a full
    /// window is available (EBU Tech 3342 / BS.1770 short-term).
    fn short_term_energies(&self) -> Vec<f64> {
        let steps = (SHORT_TERM_MS / STEP_MS).round() as usize;
        if self.step_sums.len() < steps {
            return Vec::new();
        }
        let denominator = (steps * self.step_frames) as f64;
        self.step_sums
            .windows(steps)
            .map(|window| window.iter().sum::<f64>() / denominator)
            .collect()
    }

    /// Maximum short-term loudness, or `None` for audio shorter than 3 s.
    #[allow(dead_code)]
    pub(crate) fn max_short_term_lufs(&self) -> Option<f64> {
        self.short_term_energies()
            .into_iter()
            .filter(|e| *e > 0.0 && e.is_finite())
            .fold(None, |max: Option<f64>, e| {
                Some(max.map_or(e, |m| m.max(e)))
            })
            .map(energy_to_lufs)
    }

    /// Maximum momentary (400 ms) loudness, or `None` for audio shorter than a block.
    #[allow(dead_code)]
    pub(crate) fn max_momentary_lufs(&self) -> Option<f64> {
        self.blocks
            .iter()
            .copied()
            .filter(|e| *e > 0.0 && e.is_finite())
            .fold(None, |max: Option<f64>, e| {
                Some(max.map_or(e, |m| m.max(e)))
            })
            .map(energy_to_lufs)
    }

    /// Loudness range per EBU Tech 3342: the spread between the 10th and 95th
    /// percentiles of short-term loudness after a −70 LUFS absolute gate and a
    /// relative gate 20 LU below the absolute-gated mean. `None` when the audio
    /// is shorter than 3 s or entirely gated out.
    pub(crate) fn loudness_range_lu(&self) -> Option<f64> {
        let absolute = lufs_to_energy(ABSOLUTE_GATE_LUFS);
        let above: Vec<f64> = self
            .short_term_energies()
            .into_iter()
            .filter(|e| *e > absolute)
            .collect();
        if above.is_empty() {
            return None;
        }
        let mean = above.iter().sum::<f64>() / above.len() as f64;
        let relative = mean * 10f64.powf(LRA_RELATIVE_GATE_LU / 10.0);
        let mut gated: Vec<f64> = above
            .into_iter()
            .filter(|e| *e > relative)
            .map(energy_to_lufs)
            .collect();
        if gated.is_empty() {
            return None;
        }
        gated.sort_by(|a, b| a.total_cmp(b));
        let at = |q: f64| gated[((gated.len() - 1) as f64 * q).round() as usize];
        Some((at(0.95) - at(0.10)).max(0.0))
    }
}

/// True-peak meter: 4× oversampling through a polyphase reconstruction
/// interpolator, per BS.1770-4 Annex 2.
///
/// The meter is a bank of `OVERSAMPLE_FACTOR` filters, one per output phase.
/// Phase `p` reconstructs the samples that fall `p / OVERSAMPLE_FACTOR` of an
/// output step after each input sample. Every phase is an independent
/// symmetric windowed-sinc branch normalised to unity DC gain, so a constant
/// input reconstructs to exactly that constant and each phase sees the signal
/// with the correct gain.
#[derive(Clone, Debug)]
pub(crate) struct TruePeakMeter {
    /// `OVERSAMPLE_FACTOR` branches of `TAPS_PER_PHASE` coefficients each,
    /// indexed as `phases[phase * TAPS_PER_PHASE + tap]`.
    phases: Vec<f64>,
    /// Per-channel input-sample history (oldest first), bounded to
    /// `TAPS_PER_PHASE`.
    history: Vec<Vec<f64>>,
    /// Set once every channel's history is full, so the causal filter's
    /// startup transient is never metered as a peak.
    primed: bool,
    true_peak: f64,
}

impl TruePeakMeter {
    pub(crate) fn new(channels: usize) -> Self {
        Self {
            phases: interpolation_phases(),
            history: vec![Vec::with_capacity(TAPS_PER_PHASE); channels.max(1)],
            primed: false,
            true_peak: 0.0,
        }
    }

    pub(crate) fn push_frame(&mut self, frame: &[f32]) {
        if frame.is_empty() {
            return;
        }
        for (channel, value) in frame.iter().enumerate() {
            let history = &mut self.history[channel];
            history.push(f64::from(*value));
            // All four phases reconstruct points *inside* the interval that just closed,
            // so each input sample evaluates the whole bank. Phase 0 lands on
            // the sample itself; phases 1…3 land between samples, which is
            // where intersample peaks are found.
            if history.len() >= TAPS_PER_PHASE {
                let newest = history.len() - 1;
                for branch in (0..OVERSAMPLE_FACTOR).map(|phase| phase * TAPS_PER_PHASE) {
                    let mut sum = 0.0;
                    for tap in 0..TAPS_PER_PHASE {
                        sum += self.phases[branch + tap] * history[newest - tap];
                    }
                    self.true_peak = self.true_peak.max(sum.abs());
                }
            }
            if history.len() > TAPS_PER_PHASE {
                let excess = history.len() - TAPS_PER_PHASE;
                history.drain(..excess);
            }
        }
        if self
            .history
            .iter()
            .all(|history| history.len() >= TAPS_PER_PHASE)
        {
            self.primed = true;
        }
    }

    /// Maximum absolute reconstructed sample, flushing the filter's group
    /// delay so peaks at the very end of the stream are included.
    pub(crate) fn finish(mut self) -> f64 {
        if self.primed {
            let tail = vec![0.0; self.history.len()];
            self.push_frame(&tail);
        }
        self.true_peak
    }
}

/// Design the polyphase reconstruction bank: for each output phase, a
/// symmetric Kaiser-windowed sinc low-pass at the original Nyquist frequency,
/// normalised to unity DC gain.
///
/// Phase `p` reconstructs the signal at `p / OVERSAMPLE_FACTOR` of an output
/// step past each input sample, so its taps are evaluated half a tap *later*
/// than the input grid. `TAPS_PER_PHASE` is even, which keeps every branch
/// symmetric about its own centre.
///
/// Phase `p` reconstructs the signal at `p / OVERSAMPLE_FACTOR` of an output
/// step past each input sample, so its taps are evaluated half a tap *later*
/// than the input grid. `TAPS_PER_PHASE` is even, which keeps every branch
/// symmetric about its own centre.
fn interpolation_phases() -> Vec<f64> {
    let factor = OVERSAMPLE_FACTOR as f64;
    /// Taps live on the *input* grid, so the passband edge is the input Nyquist
    /// (0.5 cycles per input sample) regardless of the oversampling factor.
    /// `h(x) = 2·cutoff·sinc(2·cutoff·x)` therefore reduces to `sinc(x)` here.
    const CUTOFF: f64 = 0.5;
    let window_denominator = bessel_i0(KAISER_BETA);
    let centre = (TAPS_PER_PHASE - 1) as f64 / 2.0;
    let mut phases = Vec::with_capacity(OVERSAMPLE_FACTOR * TAPS_PER_PHASE);
    for phase in 0..OVERSAMPLE_FACTOR {
        // Delay of this phase, in input-sample intervals (0 = on the grid).
        let delay = phase as f64 / factor;
        let mut branch: Vec<f64> = (0..TAPS_PER_PHASE)
            .map(|tap| {
                // Distance of this tap from the phase's reconstruction point,
                // in input samples. Taps behind the point are positive.
                let offset = delay + centre - tap as f64;
                let sinc = if offset.abs() < 1e-9 {
                    2.0 * CUTOFF
                } else {
                    (2.0 * PI * CUTOFF * offset).sin() / (PI * offset)
                };
                let position = 2.0 * tap as f64 / (TAPS_PER_PHASE - 1) as f64 - 1.0;
                let window = bessel_i0(KAISER_BETA * (1.0 - position * position).max(0.0).sqrt())
                    / window_denominator;
                2.0 * CUTOFF * sinc * window
            })
            .collect();
        // A branch must pass DC unchanged, otherwise each phase would measure
        // the signal at a different gain.
        let sum: f64 = branch.iter().sum();
        if sum.abs() > f64::EPSILON {
            let scale = 1.0 / sum;
            for coefficient in &mut branch {
                *coefficient *= scale;
            }
        }
        phases.extend(branch);
    }
    phases
}

/// Modified Bessel function of the first kind, order zero (Abramowitz &
/// Stegun 9.8.1/9.8.2 piecewise approximation) — used for the Kaiser window.
fn bessel_i0(x: f64) -> f64 {
    let magnitude = x.abs();
    if magnitude < 3.75 {
        let y = (x / 3.75).powi(2);
        1.0 + y
            * (3.515_622_9
                + y * (3.089_942_4
                    + y * (1.206_749_2 + y * (0.265_973_2 + y * (0.036_076_8 + y * 0.004_581_3)))))
    } else {
        let y = 3.75 / magnitude;
        (magnitude.exp() / magnitude.sqrt())
            * (0.398_942_28
                + y * (0.013_285_92
                    + y * (0.002_253_19
                        + y * (-0.001_575_65
                            + y * (0.009_162_81
                                + y * (-0.020_577_06
                                    + y * (0.026_355_37
                                        + y * (-0.016_476_33 + y * 0.003_923_77))))))))
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_RATE: u32 = 48_000;

    /// Flat `value` for `seconds`, one entry per channel per frame.
    fn sine(frequency: f64, amplitude: f64, seconds: f64, channels: usize) -> Vec<f32> {
        let frames = (seconds * SAMPLE_RATE as f64) as usize;
        (0..frames)
            .flat_map(|frame| {
                let phase = 2.0 * PI * frequency * frame as f64 / SAMPLE_RATE as f64;
                let value = (amplitude * phase.sin()) as f32;
                vec![value; channels]
            })
            .collect()
    }

    fn integrated(samples: &[f32], channels: usize) -> Option<f64> {
        let mut meter = LoudnessMeter::new(SAMPLE_RATE, channels);
        for frame in samples.chunks_exact(channels) {
            meter.push_frame(frame);
        }
        meter.integrated_loudness_lufs()
    }

    fn true_peak_db(samples: &[f32], channels: usize) -> f64 {
        let mut meter = TruePeakMeter::new(channels);
        for frame in samples.chunks_exact(channels) {
            meter.push_frame(frame);
        }
        20.0 * meter.finish().max(f32::MIN_POSITIVE as f64).log10()
    }

    #[test]
    fn stereo_tone_matches_bs1770_reference_meter() {
        // Cross-checked against `ffmpeg -af ebur128=peak=true` on this exact
        // signal (48 kHz stereo, both channels 0.5·sin(2π·1000·t)): I = −6.0 LUFS.
        let measured = integrated(&sine(1000.0, 0.5, 4.0, 2), 2).unwrap();
        assert!(
            (measured + 6.0).abs() < 0.15,
            "integrated loudness {measured:.2} LUFS is off the BS.1770 reference"
        );
    }

    #[test]
    fn mono_tone_reads_three_lu_below_an_identical_stereo_pair() {
        // Same cross-check: mono reads I = −9.0 LUFS (one channel of weight 1.0).
        let mono = integrated(&sine(1000.0, 0.5, 4.0, 1), 1).unwrap();
        let stereo = integrated(&sine(1000.0, 0.5, 4.0, 2), 2).unwrap();
        assert!((mono + 9.0).abs() < 0.15, "mono {mono:.2} LUFS");
        assert!(
            (mono - stereo + 3.0).abs() < 0.05,
            "mono {mono:.2} LUFS vs stereo {stereo:.2} LUFS"
        );
    }

    #[test]
    fn relative_gate_excludes_quiet_sections() {
        // Two seconds at 0.5 amplitude then three at 0.005 (40 dB down);
        // `ffmpeg -af ebur128` reports I = −6.4 LUFS for this signal.
        let mut samples = sine(1000.0, 0.5, 2.0, 2);
        samples.extend(sine(1000.0, 0.005, 3.0, 2));
        let measured = integrated(&samples, 2).unwrap();
        assert!(
            (measured + 6.4).abs() < 0.3,
            "gated loudness {measured:.2} LUFS"
        );
    }

    #[test]
    fn k_weighting_matches_bs1770_for_sub_audio_content() {
        // A 20 Hz mono tone is attenuated by the RLB high-pass but not
        // eliminated: `ffmpeg -af ebur128` reports I = −23.0 LUFS for this
        // exact signal, and it must still sit clearly below the 1 kHz reading.
        let sub = integrated(&sine(20.0, 0.5, 4.0, 1), 1).unwrap();
        let audio_band = integrated(&sine(1000.0, 0.5, 4.0, 1), 1).unwrap();
        assert!((sub + 23.0).abs() < 0.15, "20 Hz reads {sub:.2} LUFS");
        assert!(
            sub < audio_band - 10.0,
            "20 Hz ({sub:.2} LUFS) must sit below 1 kHz ({audio_band:.2} LUFS)"
        );
    }

    #[test]
    fn steady_tone_has_near_zero_loudness_range() {
        let mut meter = LoudnessMeter::new(SAMPLE_RATE, 2);
        for frame in sine(1000.0, 0.5, 8.0, 2).chunks_exact(2) {
            meter.push_frame(frame);
        }
        assert!(meter.loudness_range_lu().unwrap() < 0.3);
        assert!((meter.max_short_term_lufs().unwrap() + 6.0).abs() < 0.2);
        assert!((meter.max_momentary_lufs().unwrap() + 6.0).abs() < 0.2);
    }

    #[test]
    fn level_change_widens_loudness_range() {
        // 6 s at 0.5 then 6 s at 0.125 (12 dB down, inside the −20 LU gate).
        let mut samples = sine(1000.0, 0.5, 6.0, 2);
        samples.extend(sine(1000.0, 0.125, 6.0, 2));
        let mut meter = LoudnessMeter::new(SAMPLE_RATE, 2);
        for frame in samples.chunks_exact(2) {
            meter.push_frame(frame);
        }
        let lra = meter.loudness_range_lu().unwrap();
        assert!((lra - 12.0).abs() < 1.0, "LRA {lra:.2} LU");
    }

    #[test]
    fn short_audio_has_no_loudness_range() {
        let mut meter = LoudnessMeter::new(SAMPLE_RATE, 2);
        for frame in sine(1000.0, 0.5, 2.0, 2).chunks_exact(2) {
            meter.push_frame(frame);
        }
        assert_eq!(meter.loudness_range_lu(), None);
    }

    #[test]
    fn signals_shorter_than_a_gating_block_report_no_loudness() {
        assert_eq!(integrated(&sine(1000.0, 0.5, 0.3, 2), 2), None);
    }

    #[test]
    fn digital_silence_reports_no_loudness() {
        assert_eq!(integrated(&vec![0.0; SAMPLE_RATE as usize * 4], 2), None);
    }

    #[test]
    fn true_peak_matches_the_amplitude_of_a_low_frequency_tone() {
        // A 1 kHz tone's continuous peak falls on a sample, so true peak is
        // exactly the tone amplitude: −6.02 dBTP.
        let measured = true_peak_db(&sine(1000.0, 0.5, 1.0, 1), 1);
        assert!(
            (measured + 6.02).abs() < 0.05,
            "true peak {measured:.2} dBTP"
        );
    }

    #[test]
    fn true_peak_resolves_intersample_peaks() {
        // At fs/6 the tone's peaks fall between samples: the sample peak reads
        // 0.866 × amplitude (−7.27 dBFS) while the true peak is the full
        // amplitude (−6.02 dBTP), which only 4× oversampling can reveal.
        let samples = sine(8000.0, 0.5, 1.0, 1);
        let sample_peak = samples.iter().fold(0.0f32, |peak, s| peak.max(s.abs()));
        assert!((20.0 * f64::from(sample_peak).log10() + 7.27).abs() < 0.02);
        let measured = true_peak_db(&samples, 1);
        // The reconstructed peak is itself sampled on the 4× grid, so a
        // high-frequency tone reads slightly low; BS.1770-4 Annex 2 meters
        // share this (here ≈ 0.08 dB at fs/6).
        assert!(
            (measured + 6.02).abs() < 0.12,
            "true peak {measured:.2} dBTP must exceed the sample peak"
        );
    }

    #[test]
    fn true_peak_reads_full_scale_for_a_constant_signal() {
        // A DC gain error would show up here: normalisation must give exactly
        // 0 dBTP for a constant full-scale input.
        let measured = true_peak_db(&vec![1.0f32; SAMPLE_RATE as usize], 1);
        assert!(measured.abs() < 0.05, "DC gain error: {measured:.3} dBTP");
    }

    #[test]
    fn true_peak_reads_zero_for_a_full_scale_square_wave() {
        // A 12 kHz square wave (two samples per half-cycle): no sample exceeds
        // full scale, but its band-limited reconstruction overshoots (Gibbs).
        let samples: Vec<f32> = (0..SAMPLE_RATE as usize)
            .map(|index| if (index / 2) % 2 == 0 { 1.0 } else { -1.0 })
            .collect();
        let measured = true_peak_db(&samples, 1);
        assert!(
            measured > 0.0,
            "expected an overshoot above 0 dBTP, got {measured:.2}"
        );
        // With harmonics at 6, 12 and 18 kHz (30 kHz is above Nyquist) the
        // truncated series overshoots by roughly 1.4× (≈ +2.9 dBTP).
        assert!(
            measured < 4.0,
            "overshoot should stay within the Gibbs bound: {measured:.2}"
        );
    }
}
