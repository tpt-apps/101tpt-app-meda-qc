//! Stuck-pixel analysis ([spec § 8.4] dead pixel detection).
//!
//! A dead pixel is a sensor defect that never responds to the scene: it stays
//! dark in bright shots, or bright in dark shots. A *flicker* pixel alternates
//! between both extremes. Detecting them needs contrast, so a cell is only
//! flagged from frames that actually exercise it:
//!
//! - **dead** — never brighter than `dark_level` across every *bright* frame;
//! - **stuck** — never darker than `bright_level` across every *dark* frame;
//! - **flicker** — reached the dark extreme in a bright frame *and* the bright
//!   extreme in a dark frame.
//!
//! Requiring both bright and dark frames is what keeps legitimately static
//! content (letterbox bars, a black border, a night sky) from being reported as
//! defects: those pixels are only ever judged against frames whose own level
//! makes the comparison meaningful.

use tpt_app_media_qc_model::inspection::{DeadPixelCluster, DeadPixelKind, DeadPixelStats};

/// Maximum cells analysed per frame. A frame larger than this is stride-sampled
/// and the result is marked `resolution_limited` so the rule can say so.
const MAX_CELLS: usize = 4_194_304;

/// Retained clusters. A heavily defective picture would otherwise produce an
/// unbounded finding list.
const MAX_CLUSTERS: usize = 64;

/// Analysis thresholds. Studio range 16–235 is the reference: levels are chosen
/// well outside it so legitimate near-black/near-white content is not flagged.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DeadPixelConfig {
    /// Luma at or below which a cell counts as "dark" (default 8).
    pub dark_level: u8,
    /// Luma at or above which a cell counts as "bright" (default 247).
    pub bright_level: u8,
    /// A frame this bright or brighter is a *bright* frame (default 200).
    pub bright_frame_min: u8,
    /// A frame this dark or darker is a *dark* frame (default 55).
    pub dark_frame_max: u8,
    /// Bright frames required before dead pixels are judged (default 3).
    pub min_bright_frames: u32,
    /// Dark frames required before stuck pixels are judged (default 3).
    pub min_dark_frames: u32,
}

impl Default for DeadPixelConfig {
    fn default() -> Self {
        Self {
            dark_level: 8,
            bright_level: 247,
            bright_frame_min: 200,
            dark_frame_max: 55,
            min_bright_frames: 3,
            min_dark_frames: 3,
        }
    }
}

/// Per-cell accumulators across the sampled frames.
struct Cells {
    width: u32,
    height: u32,
    /// Source pixels represented by one grid cell.
    stride: u32,
    /// Largest value seen in a bright frame (0 until the first bright frame).
    max_in_bright: Vec<u8>,
    /// Smallest value seen in a dark frame (255 until the first dark frame).
    min_in_dark: Vec<u8>,
    /// Cell hit the dark extreme at least once in a bright frame.
    dark_extreme: Vec<bool>,
    /// Cell hit the bright extreme at least once in a dark frame.
    bright_extreme: Vec<bool>,
    bright_frames: u32,
    dark_frames: u32,
    frames: u64,
    resolution_limited: bool,
}

impl Cells {
    fn new(width: u32, height: u32, stride: u32, resolution_limited: bool) -> Self {
        let n = (width as usize) * (height as usize);
        Self {
            width,
            height,
            stride,
            max_in_bright: vec![0; n],
            min_in_dark: vec![255; n],
            dark_extreme: vec![false; n],
            bright_extreme: vec![false; n],
            bright_frames: 0,
            dark_frames: 0,
            frames: 0,
            resolution_limited,
        }
    }
}

pub(crate) struct DeadPixelDetector {
    config: DeadPixelConfig,
    cells: Option<Cells>,
}

/// One frame reduced to the analysis grid.
pub(crate) struct DeadPixelFrame {
    width: u32,
    height: u32,
    stride: u32,
    /// Grid values, row-major, `width * height` long.
    values: Vec<u8>,
}

impl DeadPixelDetector {
    pub(crate) fn new(config: DeadPixelConfig) -> Self {
        Self {
            config,
            cells: None,
        }
    }

    /// Reduce a frame to the analysis grid, or `None` when it is unusable.
    pub(crate) fn reduce(
        width: u32,
        height: u32,
        mut luma: impl Iterator<Item = u8>,
    ) -> Option<DeadPixelFrame> {
        if width == 0 || height == 0 {
            return None;
        }
        let total = (width as usize).checked_mul(height as usize)?;
        // Smallest stride that brings the picture within the cell budget.
        let mut stride = 1u32;
        while (width as usize / stride as usize) * (height as usize / stride as usize) > MAX_CELLS {
            stride += 1;
        }
        let gw = width.div_ceil(stride).max(1);
        let gh = height.div_ceil(stride).max(1);
        let grid_len = (gw as usize) * (gh as usize);

        let values: Vec<u8> = if stride == 1 {
            luma.take(total).collect()
        } else {
            // Nearest-cell sampling keeps one representative value per grid cell
            // rather than averaging, which would dilute isolated defects.
            let mut out = Vec::with_capacity(grid_len);
            for y in 0..height {
                for x in 0..width {
                    let value = luma.next()?;
                    if x % stride == 0 && y % stride == 0 {
                        out.push(value);
                    }
                }
            }
            out
        };
        if values.len() < grid_len {
            return None;
        }
        Some(DeadPixelFrame {
            width: gw,
            height: gh,
            stride,
            values,
        })
    }

    /// Fold one frame into the accumulators.
    pub(crate) fn push(&mut self, frame: DeadPixelFrame) {
        let cfg = self.config;
        let frame_min = frame.values.iter().copied().min().unwrap_or(0);
        let frame_max = frame.values.iter().copied().max().unwrap_or(0);
        // A frame that is neither bright nor dark teaches us nothing about a
        // stuck pixel, so it counts as sampled but not as contrast.
        let is_bright = frame_max >= cfg.bright_frame_min;
        let is_dark = frame_min <= cfg.dark_frame_max;

        let cells = self.cells.get_or_insert_with(|| {
            Cells::new(frame.width, frame.height, frame.stride, frame.stride > 1)
        });

        if is_bright {
            cells.bright_frames += 1;
            for (i, &value) in frame.values.iter().enumerate() {
                cells.max_in_bright[i] = cells.max_in_bright[i].max(value);
                if value <= cfg.dark_level {
                    cells.dark_extreme[i] = true;
                }
            }
        }
        if is_dark {
            cells.dark_frames += 1;
            for (i, &value) in frame.values.iter().enumerate() {
                cells.min_in_dark[i] = cells.min_in_dark[i].min(value);
                if value >= cfg.bright_level {
                    cells.bright_extreme[i] = true;
                }
            }
        }
        cells.frames += 1;
    }

    /// Classify the accumulated cells. `None` when there was nothing to judge.
    pub(crate) fn finish(self) -> Option<DeadPixelStats> {
        let cfg = self.config;
        let cells = self.cells?;
        if cells.frames == 0 {
            return None;
        }
        let total_cells = (cells.width as usize) * (cells.height as usize);
        let mut stats = DeadPixelStats {
            frames_sampled: cells.frames,
            cells_examined: total_cells as u64,
            resolution_limited: cells.resolution_limited,
            ..Default::default()
        };

        // Flicker first: a cell that reached both extremes is one defect, not two.
        let mut kinds: Vec<Option<DeadPixelKind>> = Vec::with_capacity(total_cells);
        for i in 0..total_cells {
            let flicker = cells.bright_frames >= cfg.min_bright_frames
                && cells.dark_frames >= cfg.min_dark_frames
                && cells.dark_extreme[i]
                && cells.bright_extreme[i];
            let dead = !flicker
                && cells.bright_frames >= cfg.min_bright_frames
                && cells.max_in_bright[i] <= cfg.dark_level;
            let stuck = !flicker
                && !dead
                && cells.dark_frames >= cfg.min_dark_frames
                && cells.min_in_dark[i] >= cfg.bright_level;
            let kind = if flicker {
                Some(DeadPixelKind::Flicker)
            } else if dead {
                Some(DeadPixelKind::Dead)
            } else if stuck {
                Some(DeadPixelKind::Stuck)
            } else {
                None
            };
            match kind {
                Some(DeadPixelKind::Flicker) => stats.flicker_cells += 1,
                Some(DeadPixelKind::Dead) => stats.dark_cells += 1,
                Some(DeadPixelKind::Stuck) => stats.bright_cells += 1,
                None => {}
            }
            kinds.push(kind);
        }

        stats.clusters = cluster(&kinds, cells.width, cells.height, cells.stride);
        Some(stats)
    }
}

/// Group adjacent flagged cells into 4-connected clusters, largest first.
///
/// Coordinates are reported in **source** pixels so an operator can find the
/// defect on the original picture even when the grid was stride-sampled.
fn cluster(
    kinds: &[Option<DeadPixelKind>],
    width: u32,
    height: u32,
    stride: u32,
) -> Vec<DeadPixelCluster> {
    let row = width as usize;
    let mut seen = vec![false; kinds.len()];
    let mut out: Vec<DeadPixelCluster> = Vec::new();

    for start in 0..kinds.len() {
        if seen[start] {
            continue;
        }
        let Some(kind) = kinds[start] else {
            continue;
        };
        let mut stack = vec![start];
        seen[start] = true;
        let (mut min_x, mut max_x) = (u32::MAX, 0u32);
        let (mut min_y, mut max_y) = (u32::MAX, 0u32);
        let mut count = 0u32;

        while let Some(index) = stack.pop() {
            let x = (index % row) as u32;
            let y = (index / row) as u32;
            min_x = min_x.min(x);
            max_x = max_x.max(x);
            min_y = min_y.min(y);
            max_y = max_y.max(y);
            count += 1;

            for (dx, dy) in [(1i64, 0i64), (-1, 0), (0, 1), (0, -1)] {
                let nx = x as i64 + dx;
                let ny = y as i64 + dy;
                if nx < 0 || ny < 0 || nx >= width as i64 || ny >= height as i64 {
                    continue;
                }
                let neighbour = ny as usize * row + nx as usize;
                if !seen[neighbour] && kinds[neighbour] == Some(kind) {
                    seen[neighbour] = true;
                    stack.push(neighbour);
                }
            }
        }
        out.push(DeadPixelCluster {
            x: min_x * stride,
            y: min_y * stride,
            width: (max_x - min_x + 1) * stride,
            height: (max_y - min_y + 1) * stride,
            cells: count,
            kind,
        });
    }

    // Largest defect first, then reading order for a stable report.
    out.sort_by_key(|c| (std::cmp::Reverse(c.cells), c.y, c.x));
    out.truncate(MAX_CLUSTERS);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: u32 = 8;
    const H: u32 = 4;

    /// A picture of `level` everywhere except the listed cell indices, which are
    /// pinned to `pin`.
    fn picture(level: u8, pinned: &[usize], pin: u8) -> Vec<u8> {
        let mut data = vec![level; (W * H) as usize];
        for &index in pinned {
            data[index] = pin;
        }
        data
    }

    fn run(frames: Vec<Vec<u8>>) -> Option<DeadPixelStats> {
        let mut detector = DeadPixelDetector::new(DeadPixelConfig::default());
        for data in frames {
            let frame = DeadPixelDetector::reduce(W, H, data.into_iter()).unwrap();
            detector.push(frame);
        }
        detector.finish()
    }

    /// Alternating bright/dark frames, optionally with pinned cells.
    fn alternating(pinned: &[usize], pin: u8, rounds: usize) -> Vec<Vec<u8>> {
        (0..rounds)
            .map(|i| {
                if i % 2 == 0 {
                    picture(235, pinned, pin)
                } else {
                    picture(16, pinned, pin)
                }
            })
            .collect()
    }

    #[test]
    fn uniform_pictures_yield_no_defects() {
        let stats = run(alternating(&[], 0, 4)).expect("stats");
        assert_eq!(stats.total_cells(), 0, "{stats:?}");
        assert_eq!(stats.cells_examined, (W * H) as u64);
        assert_eq!(stats.frames_sampled, 4);
        assert!(!stats.resolution_limited);
        assert!(stats.clusters.is_empty());
    }

    #[test]
    fn pixel_stuck_dark_in_bright_frames_is_dead() {
        // Cell 9 is always 0; everything else follows the scene.
        let stats = run(alternating(&[9], 0, 6)).expect("stats");
        assert_eq!(stats.dark_cells, 1, "{stats:?}");
        assert_eq!(stats.bright_cells, 0);
        assert_eq!(stats.flicker_cells, 0);
        assert_eq!(stats.clusters.len(), 1);
        let cluster = stats.clusters[0];
        assert_eq!(cluster.kind, DeadPixelKind::Dead);
        assert_eq!(cluster.x, 1);
        assert_eq!(cluster.y, 1);
        assert_eq!(cluster.cells, 1);
    }

    #[test]
    fn pixel_stuck_bright_in_dark_frames_is_stuck() {
        let stats = run(alternating(&[5], 255, 6)).expect("stats");
        assert_eq!(stats.bright_cells, 1, "{stats:?}");
        assert_eq!(stats.dark_cells, 0);
        let cluster = stats.clusters[0];
        assert_eq!(cluster.kind, DeadPixelKind::Stuck);
        assert_eq!(cluster.x, 5);
        assert_eq!(cluster.y, 0);
    }

    #[test]
    fn pixel_hitting_both_extremes_is_one_flicker_defect() {
        // Cell 3 is 0 in bright frames and 255 in dark frames.
        let frames: Vec<Vec<u8>> = (0..6)
            .map(|i| {
                if i % 2 == 0 {
                    picture(235, &[3], 0)
                } else {
                    picture(16, &[3], 255)
                }
            })
            .collect();
        let stats = run(frames).expect("stats");
        assert_eq!(stats.flicker_cells, 1, "{stats:?}");
        // A flicker is one defect, not a dead plus a stuck one.
        assert_eq!(stats.dark_cells, 0, "{stats:?}");
        assert_eq!(stats.bright_cells, 0, "{stats:?}");
        assert_eq!(stats.total_cells(), 1, "{stats:?}");
        assert_eq!(stats.clusters.len(), 1);
        assert_eq!(stats.clusters[0].kind, DeadPixelKind::Flicker);
        assert_eq!(stats.clusters[0].x, 3);
    }

    #[test]
    fn adjacent_defects_form_one_cluster() {
        // Two horizontally adjacent dead cells (2 and 3) plus an isolated one.
        let stats = run(alternating(&[2, 3, 20], 0, 6)).expect("stats");
        assert_eq!(stats.dark_cells, 3, "{stats:?}");
        assert_eq!(stats.clusters.len(), 2, "{stats:?}");
        // Largest first: the pair.
        assert_eq!(stats.clusters[0].cells, 2);
        assert_eq!(stats.clusters[0].width, 2);
        assert_eq!(stats.clusters[1].cells, 1);
    }

    #[test]
    fn too_few_contrast_frames_produce_no_verdict() {
        // A single bright frame cannot establish that a cell is dead.
        let stats = run(vec![picture(235, &[9], 0)]).expect("stats");
        assert_eq!(stats.total_cells(), 0, "{stats:?}");
    }

    #[test]
    fn mid_tone_pixels_are_not_defects() {
        // Studio-range extremes (16/235) sit inside the tolerance band.
        let stats = run(alternating(&[9], 16, 6)).expect("stats");
        assert_eq!(stats.total_cells(), 0, "{stats:?}");
    }

    #[test]
    fn no_frames_means_no_stats() {
        let detector = DeadPixelDetector::new(DeadPixelConfig::default());
        assert!(detector.finish().is_none());
    }
}
