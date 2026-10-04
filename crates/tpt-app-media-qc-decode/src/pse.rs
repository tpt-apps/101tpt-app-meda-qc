//! Flash detection for photosensitive-epilepsy (PSE) screening, modelled on
//! the Harding test as described in ITU-R BT.1702 and the Ofcom guidance.
//!
//! What is implemented:
//!
//! * **Relative luminance** per pixel: Y'CbCr is converted to R'G'B' with the
//!   BT.709 matrix (BT.601 for SD heights), studio range expanded, gamma
//!   decoded with a 2.2 power law and weighted with the BT.709 primaries.
//! * **General flash:** a pixel makes a transition when its relative luminance
//!   moves by at least 0.1 (10 % of maximum) against its own running extremum
//!   in the opposite direction to its previous change, and the darker of the
//!   two states is below 0.8. A frame carries a screen-level transition when
//!   those pixels cover at least 25 % of the 10° visual field, taken as
//!   341 × 256 pixels of a 1024 × 768 screen (about 2.8 % of the picture).
//! * **Red flash:** a pixel is saturated red when R/(R+G+B) >= 0.8; a
//!   transition occurs when its `(R-G-B)*320` measure changes by at least 20
//!   (non-saturated pixels measure 0), with the same area criterion.
//! * Consecutive screen-level transitions in the same direction are one
//!   change; the rule layer counts opposing changes as flashes (two opposing
//!   transitions each) per one-second window.
//!
//! Approximations (compliance is not claimed): analysis runs on a sampled grid
//! of at most 128 x 96 cells; the field-of-view mapping is a fixed fraction of
//! the picture rather than a function of viewing distance; frame rate is not
//! compensated; full-range, BT.2020 and HDR sources are treated as studio-range
//! BT.709; spatial pattern hazards are not assessed.

use tpt_app_media_qc_model::inspection::{FlashMeasurements, FLASH_CHANNELS};
use tpt_kinetix_core::frame::VideoFrame;
use tpt_kinetix_core::pixel_format::PixelFormat;

/// Minimum relative-luminance swing that counts as a general-flash transition.
const MIN_DELTA: f32 = 0.1;
/// A general transition only counts when its darker state is below this level.
const DARK_LIMIT: f32 = 0.8;
/// Red metric `(R-G-B)*320` swing (Harding) needed for a red transition.
const RED_MIN_DELTA: f32 = 20.0;
/// R/(R+G+B) at which a pixel is saturated red.
const RED_SATURATION: f64 = 0.8;
/// 25 % of the 10° field (341 x 256 px of 1024 x 768) as a fraction of the picture.
const AREA_FRACTION: f64 = 0.25 * (341.0 * 256.0) / (1024.0 * 768.0);
/// Maximum analysis grid.
const GRID_W: usize = 128;
const GRID_H: usize = 96;
/// Sample points per grid cell along each axis.
const SUBSAMPLES: usize = 3;
/// Retention cap per class so pathological strobing cannot exhaust memory.
const MAX_TRANSITIONS: usize = 200_000;

/// Per-cell luminance and red measures for one frame.
pub(crate) struct GridFrame {
    cells: usize,
    luminance: Vec<f32>,
    red: Vec<f32>,
}

#[derive(Clone, Copy)]
enum Layout {
    Gray,
    Planar {
        chroma_w_div: usize,
        chroma_h_div: usize,
    },
    Rgb {
        bgr: bool,
    },
}

struct Pixels<'a> {
    data: &'a [u8],
    width: usize,
    height: usize,
    layout: Layout,
    bytes: usize,
    /// Divisor that brings a sample down to 8-bit scale.
    scale: f64,
    sd: bool,
}

const fn planar(chroma_w_div: usize, chroma_h_div: usize) -> Layout {
    Layout::Planar {
        chroma_w_div,
        chroma_h_div,
    }
}

impl<'a> Pixels<'a> {
    fn new(frame: &'a VideoFrame) -> Option<Self> {
        let (layout, bytes, bits) = match frame.pixel_format {
            PixelFormat::Gray => (Layout::Gray, 1, 8),
            PixelFormat::Gray10le => (Layout::Gray, 2, 10),
            PixelFormat::Gray12le => (Layout::Gray, 2, 12),
            PixelFormat::Yuv420p => (planar(2, 2), 1, 8),
            PixelFormat::Yuv422p => (planar(2, 1), 1, 8),
            PixelFormat::Yuv444p => (planar(1, 1), 1, 8),
            PixelFormat::Yuv420p10le => (planar(2, 2), 2, 10),
            PixelFormat::Yuv422p10le => (planar(2, 1), 2, 10),
            PixelFormat::Yuv444p10le => (planar(1, 1), 2, 10),
            PixelFormat::Yuv420p12le => (planar(2, 2), 2, 12),
            PixelFormat::Yuv422p12le => (planar(2, 1), 2, 12),
            PixelFormat::Yuv444p12le => (planar(1, 1), 2, 12),
            PixelFormat::Rgb24 => (Layout::Rgb { bgr: false }, 1, 8),
            PixelFormat::Bgr24 => (Layout::Rgb { bgr: true }, 1, 8),
        };
        let (width, height) = (frame.width as usize, frame.height as usize);
        if width == 0 || height == 0 {
            return None;
        }
        Some(Self {
            data: &frame.data,
            width,
            height,
            layout,
            bytes,
            scale: f64::from((1u32 << bits) - 1) / 255.0,
            sd: height <= 576,
        })
    }

    fn sample(&self, index: usize) -> Option<f64> {
        let at = index.checked_mul(self.bytes)?;
        if self.bytes == 1 {
            self.data.get(at).map(|v| f64::from(*v))
        } else {
            let pair = self.data.get(at..at + 2)?;
            Some(f64::from(u16::from_le_bytes([pair[0], pair[1]])))
        }
    }

    /// Gamma-encoded R'G'B' in 0..1 at (x, y). `None` only when the luma (or
    /// RGB) data is missing; absent chroma is treated as neutral.
    fn rgb_prime(&self, x: usize, y: usize) -> Option<[f64; 3]> {
        match self.layout {
            Layout::Rgb { bgr } => {
                let base = (y * self.width + x) * 3;
                let (r, g, b) = (
                    self.sample(base)?,
                    self.sample(base + 1)?,
                    self.sample(base + 2)?,
                );
                let (r, g, b) = if bgr { (b, g, r) } else { (r, g, b) };
                Some([r / 255.0, g / 255.0, b / 255.0])
            }
            Layout::Gray => {
                let luma = self.sample(y * self.width + x)? / self.scale;
                let v = ((luma - 16.0) / 219.0).clamp(0.0, 1.0);
                Some([v, v, v])
            }
            Layout::Planar {
                chroma_w_div,
                chroma_h_div,
            } => {
                let luma_plane = self.width * self.height;
                let chroma_w = self.width.div_ceil(chroma_w_div);
                let chroma_plane = chroma_w * self.height.div_ceil(chroma_h_div);
                let c = (y / chroma_h_div) * chroma_w + x / chroma_w_div;
                let luma = self.sample(y * self.width + x)? / self.scale;
                let cb = self
                    .sample(luma_plane + c)
                    .map_or(128.0, |v| v / self.scale);
                let cr = self
                    .sample(luma_plane + chroma_plane + c)
                    .map_or(128.0, |v| v / self.scale);
                let y_n = (luma - 16.0) / 219.0;
                let (pb, pr) = ((cb - 128.0) / 224.0, (cr - 128.0) / 224.0);
                let (kr, kb) = if self.sd {
                    (0.299, 0.114)
                } else {
                    (0.2126, 0.0722)
                };
                let kg = 1.0 - kr - kb;
                let r = y_n + 2.0 * (1.0 - kr) * pr;
                let b = y_n + 2.0 * (1.0 - kb) * pb;
                let g = (y_n - kr * r - kb * b) / kg;
                Some([r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0)])
            }
        }
    }
}

fn linear(v: f64) -> f64 {
    v.clamp(0.0, 1.0).powf(2.2)
}

/// Reduces a frame to a grid (at most 128 x 96) of relative luminance and red
/// measure. `None` when the frame data cannot be read.
pub(crate) fn grid_frame(frame: &VideoFrame) -> Option<GridFrame> {
    let pixels = Pixels::new(frame)?;
    let gw = pixels.width.min(GRID_W);
    let gh = pixels.height.min(GRID_H);
    let mut luminance = Vec::with_capacity(gw * gh);
    let mut red = Vec::with_capacity(gw * gh);
    for gy in 0..gh {
        let y0 = gy * pixels.height / gh;
        let y1 = ((gy + 1) * pixels.height / gh).max(y0 + 1);
        for gx in 0..gw {
            let x0 = gx * pixels.width / gw;
            let x1 = ((gx + 1) * pixels.width / gw).max(x0 + 1);
            let (mut lum_sum, mut red_sum, mut n) = (0.0, 0.0, 0.0);
            for sy in 0..SUBSAMPLES {
                let y = (y0 + (y1 - y0) * (2 * sy + 1) / (2 * SUBSAMPLES)).min(pixels.height - 1);
                for sx in 0..SUBSAMPLES {
                    let x =
                        (x0 + (x1 - x0) * (2 * sx + 1) / (2 * SUBSAMPLES)).min(pixels.width - 1);
                    let [rp, gp, bp] = pixels.rgb_prime(x, y)?;
                    let (r, g, b) = (linear(rp), linear(gp), linear(bp));
                    lum_sum += 0.2126 * r + 0.7152 * g + 0.0722 * b;
                    let total = r + g + b;
                    if total > 0.0 && r / total >= RED_SATURATION {
                        red_sum += (r - g - b).max(0.0) * 320.0;
                    }
                    n += 1.0;
                }
            }
            luminance.push((lum_sum / n) as f32);
            red.push((red_sum / n) as f32);
        }
    }
    Some(GridFrame {
        cells: gw * gh,
        luminance,
        red,
    })
}

#[derive(Clone, Copy, Default)]
struct PixelState {
    anchor: f32,
    /// +1 rising, -1 falling, 0 undecided.
    direction: i8,
}

/// Per-class state: one extremum tracker per grid cell plus the screen-level
/// direction of the last recorded transition.
#[derive(Default)]
struct ClassTracker {
    cells: Vec<PixelState>,
    last_direction: i8,
    transitions: Vec<u64>,
}

impl ClassTracker {
    /// Advances every cell; returns the (rising, falling) counts of cells that
    /// made a qualifying transition on this frame.
    fn step(&mut self, values: &[f32], min_delta: f32, dark_limit: Option<f32>) -> (usize, usize) {
        if self.cells.len() != values.len() {
            self.cells = values
                .iter()
                .map(|v| PixelState {
                    anchor: *v,
                    direction: 0,
                })
                .collect();
            self.last_direction = 0;
            return (0, 0);
        }
        let (mut up, mut down) = (0, 0);
        for (state, &value) in self.cells.iter_mut().zip(values) {
            let moved = value - state.anchor;
            let direction: i8 = match state.direction {
                0 if moved.abs() >= min_delta => {
                    if moved > 0.0 {
                        1
                    } else {
                        -1
                    }
                }
                1 if value > state.anchor => {
                    state.anchor = value;
                    continue;
                }
                1 if -moved >= min_delta => -1,
                -1 if value < state.anchor => {
                    state.anchor = value;
                    continue;
                }
                -1 if moved >= min_delta => 1,
                _ => continue,
            };
            let darker = state.anchor.min(value);
            state.direction = direction;
            state.anchor = value;
            if dark_limit.is_none_or(|limit| darker < limit) {
                if direction > 0 {
                    up += 1;
                } else {
                    down += 1;
                }
            }
        }
        (up, down)
    }

    fn record(&mut self, timestamp_ms: u64, up: usize, down: usize, threshold: usize) {
        let direction = match (up >= threshold, down >= threshold) {
            (true, true) => {
                if up >= down {
                    1
                } else {
                    -1
                }
            }
            (true, false) => 1,
            (false, true) => -1,
            _ => return,
        };
        if direction != self.last_direction {
            self.last_direction = direction;
            if self.transitions.len() < MAX_TRANSITIONS {
                self.transitions.push(timestamp_ms);
            }
        }
    }
}

#[derive(Default)]
pub(crate) struct FlashDetector {
    general: ClassTracker,
    red: ClassTracker,
}

impl FlashDetector {
    pub(crate) fn push(&mut self, timestamp_ms: u64, frame: &GridFrame) {
        let threshold = ((AREA_FRACTION * frame.cells as f64).ceil() as usize).max(1);
        let (up, down) = self
            .general
            .step(&frame.luminance, MIN_DELTA, Some(DARK_LIMIT));
        self.general.record(timestamp_ms, up, down, threshold);
        let (up, down) = self.red.step(&frame.red, RED_MIN_DELTA, None);
        self.red.record(timestamp_ms, up, down, threshold);
    }

    pub(crate) fn finish(self) -> FlashMeasurements {
        let classes: [Vec<u64>; FLASH_CHANNELS] = [self.general.transitions, self.red.transitions];
        FlashMeasurements {
            transitions_ms: classes.into_iter().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpt_kinetix_core::timestamp::Timestamp;

    /// A uniform grid frame with the given luminance and red measure.
    fn uniform(luminance: f32, red: f32) -> GridFrame {
        GridFrame {
            cells: 1000,
            luminance: vec![luminance; 1000],
            red: vec![red; 1000],
        }
    }

    fn run(frames: &[GridFrame], frame_ms: u64) -> FlashMeasurements {
        let mut detector = FlashDetector::default();
        for (index, frame) in frames.iter().enumerate() {
            detector.push(index as u64 * frame_ms, frame);
        }
        detector.finish()
    }

    fn strobe(low: f32, high: f32, frames: usize) -> Vec<GridFrame> {
        (0..frames)
            .map(|i| uniform(if i % 2 == 0 { low } else { high }, 0.0))
            .collect()
    }

    fn partial_strobe(cells: usize) -> Vec<GridFrame> {
        (0..10)
            .map(|i| {
                let mut f = uniform(0.0, 0.0);
                if i % 2 == 1 {
                    f.luminance[..cells].fill(0.6);
                }
                f
            })
            .collect()
    }

    #[test]
    fn steady_content_does_not_flash() {
        let frames: Vec<_> = (0..60).map(|_| uniform(0.3, 0.0)).collect();
        assert!(run(&frames, 40).transitions_ms[0].is_empty());
    }

    #[test]
    fn a_monotone_ramp_is_never_an_opposing_pair() {
        let frames: Vec<_> = (0..60).map(|i| uniform(i as f32 * 0.005, 0.0)).collect();
        assert!(run(&frames, 40).transitions_ms[0].len() <= 1);
    }

    #[test]
    fn alternating_frames_record_a_transition_each() {
        assert_eq!(run(&strobe(0.0, 0.5, 10), 40).transitions_ms[0].len(), 9);
    }

    #[test]
    fn bright_state_transitions_are_ignored() {
        assert!(run(&strobe(0.82, 1.0, 10), 40).transitions_ms[0].is_empty());
    }

    #[test]
    fn small_swings_are_ignored() {
        assert!(run(&strobe(0.3, 0.35, 10), 40).transitions_ms[0].is_empty());
    }

    #[test]
    fn small_screen_area_does_not_make_a_flash() {
        // 1 % of cells flash: below the ~2.8 % area criterion.
        assert!(run(&partial_strobe(10), 40).transitions_ms[0].is_empty());
    }

    #[test]
    fn sufficient_area_makes_a_flash() {
        assert_eq!(run(&partial_strobe(60), 40).transitions_ms[0].len(), 9);
    }

    #[test]
    fn saturated_red_strobe_is_a_red_flash_but_not_a_general_flash() {
        let frames: Vec<_> = (0..10)
            .map(|i| uniform(0.2, if i % 2 == 0 { 0.0 } else { 100.0 }))
            .collect();
        let m = run(&frames, 40);
        assert_eq!(m.transitions_ms[1].len(), 9);
        assert!(m.transitions_ms[0].is_empty());
    }

    fn frame(format: PixelFormat, data: Vec<u8>) -> VideoFrame {
        VideoFrame {
            pts: Timestamp::new(0, (1, 1000)),
            dts: Timestamp::new(0, (1, 1000)),
            data,
            width: 2,
            height: 2,
            pixel_format: format,
            is_key_frame: false,
        }
    }

    #[test]
    fn studio_black_and_white_map_to_zero_and_one() {
        let black = grid_frame(&frame(PixelFormat::Gray, vec![16; 4])).unwrap();
        let white = grid_frame(&frame(PixelFormat::Gray, vec![235; 4])).unwrap();
        assert!(black.luminance[0].abs() < 1e-6);
        assert!((white.luminance[0] - 1.0).abs() < 1e-3);
    }

    #[test]
    fn saturated_red_yuv_is_detected_as_red() {
        // BT.601 studio red (a 2-row frame is SD): Y=81, Cb=90, Cr=240 (4:4:4).
        let mut data = vec![81u8; 4];
        data.extend([90u8; 4]);
        data.extend([240u8; 4]);
        let grid = grid_frame(&frame(PixelFormat::Yuv444p, data)).unwrap();
        assert!(grid.red[0] > 200.0, "red measure {}", grid.red[0]);
        assert!(
            (grid.luminance[0] - 0.2126).abs() < 0.02,
            "lum {}",
            grid.luminance[0]
        );
    }
}
