//! No-reference perceptual image-quality metrics ([spec Â§ 8.4] blockiness, blur
//! and noise).
//!
//! All three are computed from the decoded luma plane with no reference frame,
//! so they work on delivery material:
//!
//! - **blockiness** â€” the classic Wang/Bovik ratio: the mean gradient *across*
//!   transform-block boundaries divided by the mean gradient *inside* blocks. A
//!   ratio near 1.0 means block structure is indistinguishable from picture
//!   detail; higher means visible blocking. Frames that are essentially flat
//!   carry no evidence (their interior gradient is ~0) and are skipped rather
//!   than producing a meaningless ratio.
//! - **blur** â€” mean absolute Laplacian (a high-frequency energy proxy). It
//!   falls when content is over-smoothed, so it is checked against a *minimum*
//!   and is content-dependent by nature.
//! - **noise** â€” standard deviation of luma inside flat regions, which isolates
//!   compression noise and dirt from genuine picture detail.

use tpt_app_media_qc_model::inspection::PerceptualStats;

/// Transform block edge assumed for the blockiness metric. 8 is the 4:2:0 block
/// size used by MPEG-family codecs, VP9 and AV1, which is where these
/// artefacts come from.
const BLOCK: usize = 8;

/// Interior offset used for the comparison gradient: mid-block, as far from
/// either boundary as the block size allows.
const INTERIOR_OFFSET: usize = BLOCK / 2;

/// Below this mean interior gradient a frame is too flat to say anything about
/// blocking, so it is excluded from the average.
const MIN_INTERIOR_GRADIENT: f64 = 0.5;

/// Accumulates the metrics across the decoded frames.
#[derive(Default)]
pub(crate) struct PerceptionAnalyzer {
    block_sum: f64,
    block_frames: u32,
    block_max: f64,
    blur_sum: f64,
    blur_frames: u32,
    noise_sum: f64,
    noise_frames: u32,
    frames: u64,
    resolution_limited: bool,
}

/// Row-major 8-bit luma samples of one frame, already reduced to the analysis
/// resolution.
pub(crate) struct LumaPlane {
    width: u32,
    height: u32,
    values: Vec<u8>,
}

/// Box-average stride for the perceptual grid.
///
/// The metrics are gradient measures, so very large pictures are averaged down
/// (rather than point-sampled) to keep the block structure visible while
/// bounding the work per frame. Up to ~2 M cells are measured at full detail.
pub(crate) fn analysis_stride(width: u32, height: u32) -> u32 {
    // 1080p (2.07 M cells) is measured at full detail; 4K and above are reduced.
    const MAX_CELLS: u64 = 2_500_000;
    let cells = width as u64 * height as u64;
    if cells <= MAX_CELLS {
        return 1;
    }
    let factor = (cells as f64 / MAX_CELLS as f64).sqrt().ceil() as u32;
    factor.clamp(1, 4)
}

impl PerceptionAnalyzer {
    /// Reduce a full-resolution luma iterator to the analysis plane.
    ///
    /// Pictures above the cell budget are box-averaged rather than point-sampled:
    /// blockiness and blur are both *gradient* measures, so averaging preserves
    /// the structure being measured where point sampling would alias it away.
    pub(crate) fn reduce(
        width: u32,
        height: u32,
        stride: u32,
        mut luma: impl Iterator<Item = u8>,
    ) -> Option<LumaPlane> {
        if width == 0 || height == 0 || stride == 0 {
            return None;
        }
        let (gw, gh) = (width / stride, height / stride);
        // Too small for a meaningful boundary/interior comparison.
        if gw < (BLOCK as u32 * 2) || gh < (BLOCK as u32 * 2) {
            return None;
        }
        let mut sums = vec![0u64; (gw * gh) as usize];
        let mut counts = vec![0u32; (gw * gh) as usize];
        for y in 0..height {
            let gy = y / stride;
            for x in 0..width {
                let value = luma.next()? as u64;
                let cell = (gy * gw + x / stride) as usize;
                sums[cell] += value;
                counts[cell] += 1;
            }
        }
        let values = sums
            .iter()
            .zip(&counts)
            .map(|(&sum, &count)| {
                if count == 0 {
                    0
                } else {
                    (sum / count as u64) as u8
                }
            })
            .collect();
        Some(LumaPlane {
            width: gw,
            height: gh,
            values,
        })
    }

    pub(crate) fn push(&mut self, plane: LumaPlane) {
        self.frames += 1;
        let (w, h) = (plane.width as usize, plane.height as usize);
        if let Some(ratio) = blockiness(&plane, w, h) {
            self.block_sum += ratio;
            self.block_frames += 1;
            self.block_max = self.block_max.max(ratio);
        }
        if let Some(sharpness) = blur(&plane, w, h) {
            self.blur_sum += sharpness;
            self.blur_frames += 1;
        }
        if let Some(sigma) = noise(&plane, w, h) {
            self.noise_sum += sigma;
            self.noise_frames += 1;
        }
    }

    pub(crate) fn finish(self) -> Option<PerceptualStats> {
        if self.frames == 0 {
            return None;
        }
        Some(PerceptualStats {
            frames_sampled: self.frames,
            blockiness: (self.block_frames > 0).then(|| self.block_sum / self.block_frames as f64),
            blockiness_max: (self.block_frames > 0).then_some(self.block_max),
            blockiness_frames: self.block_frames,
            blur: (self.blur_frames > 0).then(|| self.blur_sum / self.blur_frames as f64),
            noise: (self.noise_frames > 0).then(|| self.noise_sum / self.noise_frames as f64),
            flat_frames: self.noise_frames,
            resolution_limited: self.resolution_limited,
        })
    }
}

/// Gradient across block boundaries divided by gradient inside blocks.
/// `None` when the frame is too flat to carry evidence.
fn blockiness(plane: &LumaPlane, w: usize, h: usize) -> Option<f64> {
    let mut boundary = 0.0f64;
    let mut interior = 0.0f64;
    let mut samples = 0.0f64;

    for y in 0..h {
        for x in 1..w {
            let d = (plane.values[y * w + x] as f64 - plane.values[y * w + x - 1] as f64).abs();
            match x % BLOCK {
                0 => boundary += d,
                INTERIOR_OFFSET => interior += d,
                _ => continue,
            }
            samples += 1.0;
        }
    }
    for y in 1..h {
        for x in 0..w {
            let d = (plane.values[y * w + x] as f64 - plane.values[(y - 1) * w + x] as f64).abs();
            match y % BLOCK {
                0 => boundary += d,
                INTERIOR_OFFSET => interior += d,
                _ => continue,
            }
            samples += 1.0;
        }
    }
    if samples == 0.0 {
        return None;
    }
    let interior_mean = interior / samples;
    if interior_mean < MIN_INTERIOR_GRADIENT {
        // A flat frame says nothing about block structure.
        return None;
    }
    Some(boundary / samples / interior_mean)
}

/// Mean absolute Laplacian, normalised to roughly 0..1. Falls as content blurs.
fn blur(plane: &LumaPlane, w: usize, h: usize) -> Option<f64> {
    let mut total = 0.0f64;
    let mut n = 0.0f64;
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let i = y * w + x;
            let lap = plane.values[i - 1] as f64
                + plane.values[i + 1] as f64
                + plane.values[i - w] as f64
                + plane.values[i + w] as f64
                - 4.0 * plane.values[i] as f64;
            total += lap.abs();
            n += 1.0;
        }
    }
    (n > 0.0).then(|| total / n / (4.0 * 255.0))
}

/// Standard deviation of luma inside flat regions.
///
/// Flatness is judged on a 5x5 **smoothed** copy of the picture. Smoothing
/// removes noise while leaving real detail, so a region that still looks flat
/// after smoothing genuinely has no picture content — and the noise in it can be
/// measured as the RMS deviation from the smoothed value. Judging flatness on
/// the raw pixels instead would classify heavily noisy areas as "not flat" and
/// systematically under-report exactly the noise an operator cares about.
fn noise(plane: &LumaPlane, w: usize, h: usize) -> Option<f64> {
    const RADIUS: usize = 2;
    const MIN_FLAT_SAMPLES: f64 = 64.0;
    // Spread allowed in the smoothed neighbourhood before it counts as detail.
    const FLAT_SPREAD: f64 = 3.0;

    if w < 2 * RADIUS + 2 || h < 2 * RADIUS + 2 {
        return None;
    }
    // 5x5 box smooth.
    let mut smoothed = vec![0.0f64; w * h];
    for y in RADIUS..h - RADIUS {
        for x in RADIUS..w - RADIUS {
            let mut sum = 0.0f64;
            let mut n = 0.0f64;
            for dy in 0..=2 * RADIUS {
                for dx in 0..=2 * RADIUS {
                    sum += plane.values[(y - RADIUS + dy) * w + x - RADIUS + dx] as f64;
                    n += 1.0;
                }
            }
            smoothed[y * w + x] = sum / n;
        }
    }

    let mut sum_sq = 0.0f64;
    let mut n = 0.0f64;
    for y in (RADIUS + 1)..(h - RADIUS - 1) {
        for x in (RADIUS + 1)..(w - RADIUS - 1) {
            let mut lo = f64::MAX;
            let mut hi = f64::MIN;
            for dy in 0..3usize {
                for dx in 0..3usize {
                    let v = smoothed[(y - 1 + dy) * w + x - 1 + dx];
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
            }
            if hi - lo > FLAT_SPREAD {
                continue; // Real detail, not a flat area.
            }
            let deviation = plane.values[y * w + x] as f64 - smoothed[y * w + x];
            sum_sq += deviation * deviation;
            n += 1.0;
        }
    }
    if n < MIN_FLAT_SAMPLES {
        // Not enough flat area to estimate noise from.
        return None;
    }
    Some((sum_sq / n).sqrt())
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: u32 = 64;
    const H: u32 = 32;

    fn plane(values: Vec<u8>) -> LumaPlane {
        assert_eq!(values.len(), (W * H) as usize);
        LumaPlane {
            width: W,
            height: H,
            values,
        }
    }

    fn run(values: Vec<u8>) -> PerceptualStats {
        let mut analyzer = PerceptionAnalyzer::default();
        analyzer.push(plane(values));
        analyzer.finish().expect("stats")
    }

    /// Smooth diagonal ramp: detail present, no block structure at all.
    fn ramp() -> Vec<u8> {
        (0..W * H)
            .map(|i| {
                let x = i % W;
                let y = i / W;
                (40 + x + y).min(235) as u8
            })
            .collect()
    }

    #[test]
    fn smooth_content_measures_low_blockiness() {
        let stats = run(ramp());
        let ratio = stats.blockiness.expect("blockiness measured");
        // No transform structure was introduced, so boundaries and interiors
        // should be indistinguishable.
        assert!(ratio < 1.25, "blockiness {ratio} on a smooth ramp");
        assert!(ratio > 0.5, "blockiness {ratio}");
    }

    #[test]
    fn block_grid_is_detected_as_blocky() {
        // A ramp flattened to one value per 8x8 block, which is what DCT
        // quantisation leaves behind, plus a little residual gradient inside
        // each block so the interior comparison has something to measure.
        let values: Vec<u8> = (0..W * H)
            .map(|i| {
                let x = i % W;
                let y = i / W;
                let block_mean = 40 + (x / 8 * 8) * 4 + (y / 8 * 8) * 2;
                (block_mean + x % 4).min(235) as u8
            })
            .collect();
        let ratio = run(values).blockiness.expect("measured");
        assert!(ratio > 2.0, "blockiness {ratio} on a block-flattened ramp");
    }

    #[test]
    fn perfectly_flat_frames_produce_no_blockiness_verdict() {
        let stats = run(vec![128; (W * H) as usize]);
        assert!(stats.blockiness.is_none(), "{stats:?}");
        assert_eq!(stats.blockiness_frames, 0);
        // Blur is still measurable on flat content.
        assert!(stats.blur.is_some());
    }

    #[test]
    fn blur_falls_as_content_is_smoothed() {
        // A hard checkerboard has strong high-frequency energy; smoothing it
        // must lower the measured sharpness. (A linear ramp has a zero
        // Laplacian everywhere, so it cannot demonstrate blur.)
        let sharp_source: Vec<u8> = (0..W * H)
            .map(|i| {
                let x = i % W;
                let y = i / W;
                if (x / 4 + y / 4) % 2 == 0 {
                    20
                } else {
                    220
                }
            })
            .collect();
        let sharp = run(sharp_source.clone()).blur.expect("sharp");
        let smoothed: Vec<u8> = sharp_source
            .chunks(W as usize)
            .flat_map(|row| {
                (0..W as usize)
                    .map(move |x| {
                        let lo = x.saturating_sub(3);
                        let hi = (x + 3).min(W as usize - 1);
                        let sum: u32 = row[lo..=hi].iter().map(|&v| v as u32).sum();
                        (sum / (hi - lo + 1) as u32) as u8
                    })
                    .collect::<Vec<u8>>()
            })
            .collect();
        let soft = run(smoothed).blur.expect("soft");
        assert!(sharp > 0.01, "checkerboard should be sharp, got {sharp}");
        assert!(
            soft < sharp,
            "smoothed {soft} should be below sharp {sharp}"
        );
    }

    #[test]
    fn noise_is_zero_on_clean_flat_content_and_rises_when_added() {
        let clean = run(vec![128; (W * H) as usize]);
        let clean_sigma = clean.noise.expect("clean sigma");
        assert!(clean_sigma < 0.01, "clean sigma {clean_sigma}");

        // Deterministic +/-6 pattern in a flat field: strong noise that the
        // smoothed-flatness test must still treat as a flat area.
        let noisy: Vec<u8> = (0..W * H)
            .map(|i| if i % 2 == 0 { 134 } else { 122 })
            .collect();
        let noisy_sigma = run(noisy).noise.expect("noisy sigma");
        assert!(
            noisy_sigma > clean_sigma + 2.0,
            "noisy {noisy_sigma} vs clean {clean_sigma}"
        );
    }

    #[test]
    fn noise_ignores_flat_faces_covered_in_detail() {
        // Every neighbourhood has detail, so there is no flat area to measure.
        let detail: Vec<u8> = (0..W * H).map(|i| ((i * 37) % 251) as u8).collect();
        assert!(run(detail).noise.is_none());
    }

    #[test]
    fn analysis_stride_keeps_small_pictures_at_full_detail() {
        assert_eq!(analysis_stride(1920, 1080), 1);
        assert_eq!(analysis_stride(640, 480), 1);
        // 8K is reduced, but never by more than a factor of four.
        assert!((1..=4).contains(&analysis_stride(7680, 4320)));
    }

    #[test]
    fn too_small_a_picture_is_not_reduced() {
        assert!(PerceptionAnalyzer::reduce(8, 8, 1, vec![10u8; 64].into_iter()).is_none());
    }

    #[test]
    fn no_frames_means_no_stats() {
        let analyzer = PerceptionAnalyzer::default();
        assert!(analyzer.finish().is_none());
    }
}
