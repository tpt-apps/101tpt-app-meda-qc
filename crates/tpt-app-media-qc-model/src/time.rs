//! Time representation: rational numbers, frame rates, timecodes.
//!
//! Timecode handling must be deterministic and property-testable
//! ([spec § 24.3]). Drop-frame conversion follows the well-known
//! SMPTE-12M routine used by libavutil (`av_timecode_adjust_ntsc_framenum2`).

use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::fmt;
use std::time::Duration;

/// A duration with serde support (serialized as millisecond integer).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DurationSeconds(u64);

impl DurationSeconds {
    pub fn from_millis(ms: u64) -> Self {
        Self((ms as f64 / 1000.0).round() as u64)
    }

    pub fn as_secs(&self) -> u64 {
        self.0
    }

    pub fn as_duration(&self) -> Duration {
        Duration::from_secs(self.0)
    }
}

impl From<Duration> for DurationSeconds {
    fn from(d: Duration) -> Self {
        Self(d.as_secs())
    }
}

impl From<DurationSeconds> for Duration {
    fn from(v: DurationSeconds) -> Self {
        Duration::from_secs(v.0)
    }
}

impl std::fmt::Display for DurationSeconds {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A non-negative reduced rational number (`num/den`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rational {
    pub num: u64,
    pub den: u64,
}

impl serde::Serialize for Rational {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for Rational {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Rational::parse(&s).ok_or_else(|| serde::de::Error::custom(format!("invalid rational: {s}")))
    }
}

impl Rational {
    /// Create a rational and reduce it. `den == 0` is rejected so callers
    /// handle bad input before constructing a value.
    pub fn new(num: u64, den: u64) -> Option<Self> {
        if den == 0 {
            return None;
        }
        let g = gcd(num, den);
        Some(Self { num: num / g, den: den / g })
    }

    pub const fn from_parts(num: u64, den: u64) -> Self {
        Self { num, den }
    }

    pub fn value(self) -> f64 {
        self.num as f64 / self.den as f64
    }

    /// Reciprocal, or `None` when the numerator is zero.
    pub fn recip(self) -> Option<Self> {
        Self::new(self.den, self.num)
    }

    /// Parse `"num/den"` or a plain integer string.
    pub fn parse(s: &str) -> Option<Self> {
        if let Some((n, d)) = s.trim().split_once('/') {
            Self::new(n.trim().parse().ok()?, d.trim().parse().ok()?)
        } else {
            let n: u64 = s.trim().parse().ok()?;
            Self::new(n, 1)
        }
    }
}

/// Total order via cross multiplication (works for any magnitudes).
impl PartialOrd for Rational {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Rational {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.num * other.den).cmp(&(other.num * self.den))
    }
}

impl fmt::Display for Rational {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.num, self.den)
    }
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a.max(1)
}

/// Frame rate, always expressed as a rational (e.g. `30000/1001` for 29.97).
pub type FrameRate = Rational;

/// Stream time base (`num/den` seconds per tick).
pub type TimeBase = Rational;

/// An SMPTE-style timecode for a given frame rate.
///
/// `frames` is the absolute frame count. Display follows the SMPTE-12M
/// convention: the on-screen frames/second is the integer nearest the real
/// rate (29.97 → 30), non-drop numbers frames continuously, and drop-frame
/// applies the standard minute-drop adjustment so the wall clock stays near
/// real time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Timecode {
    /// Absolute frame index relative to the timecode origin.
    pub frames: u64,
    /// Frame rate in frames per second.
    pub rate: FrameRate,
    /// Display name-drop convention (`drop_frame == true` for 29.97 material).
    pub drop_frame: bool,
}

impl Timecode {
    pub fn new(frames: u64, rate: FrameRate, drop_frame: bool) -> Self {
        Self { frames, rate, drop_frame }
    }

    /// Seconds represented by this timecode.
    pub fn as_seconds(self) -> f64 {
        self.frames as f64 / self.rate.value()
    }

    /// Integer frames-per-second used for the on-screen counter (29.97 → 30).
    pub fn display_fps(self) -> u64 {
        self.rate.value().round() as u64
    }

    /// Display as `HH:MM:SS:FF` (non-drop) or `HH:MM:SS;FF` (drop-frame).
    pub fn to_smpte(self) -> String {
        let fps = self.display_fps();
        let idx = if self.drop_frame {
            match drop_frame_display_index(self.frames, fps) {
                Some(idx) => idx,
                None => return format!("<unsupported drop-frame rate {}>", self.rate),
            }
        } else {
            self.frames
        };

        let seconds_total = idx / fps;
        let hours = seconds_total / 3600;
        let minutes = (seconds_total % 3600) / 60;
        let seconds = seconds_total % 60;
        let ff = idx % fps;
        let sep = if self.drop_frame { ';' } else { ':' };
        format!("{hours:02}:{minutes:02}:{seconds:02}{sep}{ff:02}")
    }
}

/// Convert a real frame count into the SMPTE-12M *display* frame index for
/// drop-frame timecode.
///
/// The convention (for 29.97): frame numbers `0` and `1` are omitted at the
/// start of every minute except the first minute of each ten-minute block.
/// Equivalently, within a ten-minute block the first minute is a full
/// `display_fps * 60` frames long and each subsequent minute is
/// `display_fps * 60 - drops_per_minute` frames long, with `drops_per_minute`
/// being 2 for 29.97 and 4 for 59.94. The whole pattern is self-consistent
/// (one-to-one and monotonic) and reproduces the canonical SMPTE anchors:
///
/// * 10 display minutes == 17,982 real frames (29.97);
/// * 1 display hour == 107,892 real frames (29.97);
/// * the first label after the first dropped minute is `00:01:00;02`.
///
/// Only rates with integer display 30/60 fps are supported.
fn drop_frame_display_index(real_frames: u64, display_fps: u64) -> Option<u64> {
    let (drops_per_minute, real_frames_per_block, display_frames_per_block) = match display_fps {
        // 29.97 fps: 10 display minutes = 17,982 real frames; 18000 display frames.
        30 => (2u64, 17_982u64, 18_000u64),
        // 59.94 fps: 10 display minutes = 35,964 real frames; 36000 display frames.
        60 => (4u64, 35_964u64, 36_000u64),
        _ => return None,
    };

    let frames_per_day_real = real_frames_per_block * 24;
    let frames_per_minute0 = display_fps * 60;
    let frames_per_dropped_minute = frames_per_minute0 - drops_per_minute;

    let r = real_frames % frames_per_day_real;
    let blocks = r / real_frames_per_block;
    let m = r % real_frames_per_block;

    let m_adjusted = if m < frames_per_minute0 {
        // First (undropped) minute of the block: no drops yet.
        m
    } else {
        // Subsequent minutes drop `drops_per_minute` frame numbers each.
        let k = (m - frames_per_minute0) / frames_per_dropped_minute;
        let k = k.min(9);
        m + drops_per_minute * (1 + k)
    };

    Some(blocks * display_frames_per_block + m_adjusted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rational_reduces() {
        assert_eq!(Rational::new(2, 4), Some(Rational::from_parts(1, 2)));
        assert_eq!(Rational::new(10, 1), Some(Rational::from_parts(10, 1)));
        assert_eq!(Rational::new(1, 0), None);
    }

    #[test]
    fn rational_parse() {
        assert_eq!(Rational::parse("30000/1001").unwrap().value().round() as u64, 30);
        assert_eq!(Rational::parse("25"), Some(Rational::from_parts(25, 1)));
        assert_eq!(Rational::parse("bogus"), None);
    }

    #[test]
    fn rational_order_and_format() {
        assert!(Rational::from_parts(30000, 1001) > Rational::from_parts(25, 1));
        assert_eq!(Rational::from_parts(30000, 1001).to_string(), "30000/1001");
    }

    #[test]
    fn non_drop_29_97_counts_continuously() {
        // 30 real frames at 29.97 non-drop display as 00:00:01:00.
        let tc = Timecode::new(30, Rational::from_parts(30000, 1001), false);
        assert_eq!(tc.to_smpte(), "00:00:01:00");
        // 30,000 real frames ⟹ 1000 seconds.
        let h = Timecode::new(30000, Rational::from_parts(30000, 1001), false);
        assert_eq!(h.to_smpte(), "00:16:40:00");
    }

    #[test]
    fn drop_frame_matches_smpte_hour_length() {
        // SMPTE drop-frame: exactly 107,892 real frames == 1 hour of display.
        let tc = Timecode::new(107_892, Rational::from_parts(30000, 1001), true);
        assert_eq!(tc.to_smpte(), "01:00:00;00");
    }

    #[test]
    fn drop_frame_minute_boundary_drops_two() {
        // The first minute is a full 1800 frames; the two labels 00:01:00;00
        // and 00:01:00;01 are dropped, so frame 1800 reads 00:01:00;02.
        let tc = Timecode::new(1800, Rational::from_parts(30000, 1001), true);
        assert_eq!(tc.to_smpte(), "00:01:00;02");
        // Last frame of the first minute reads 00:00:59;29.
        let last = Timecode::new(1799, Rational::from_parts(30000, 1001), true);
        assert_eq!(last.to_smpte(), "00:00:59;29");
    }

    #[test]
    fn drop_frame_ten_minute_no_drop() {
        // 10 display minutes == 17,982 real frames; the 10th minute boundary
        // is not dropped and is exactly 00:10:00;00.
        let tc = Timecode::new(17_982, Rational::from_parts(30000, 1001), true);
        assert_eq!(tc.to_smpte(), "00:10:00;00");
    }

    #[test]
    fn drop_frame_is_monotonic() {
        // The mapping real frame -> display label must be strictly increasing
        // (one-to-one) across the whole 24-hour day.
        let mut last_idx = 0u64;
        let mut started = false;
        for f in (0..431_568u64).step_by(17) {
            let idx = drop_frame_display_index(f, 30).unwrap();
            if started {
                assert!(idx > last_idx, "not monotonic at frame {f}");
            }
            started = true;
            last_idx = idx;
        }
    }

    #[test]
    fn drop_frame_inverts_to_real_hour() {
        // The display index at canonical anchors:
        assert_eq!(drop_frame_display_index(107_892, 30), Some(108_000));
        assert_eq!(drop_frame_display_index(17_982, 30), Some(18_000));
        // Random sanity spot: R=5792 sits 396 frames into the third minute
        // (which starts at display 00:03:00;02), so it reads 00:03:13;08.
        assert_eq!(
            Timecode::new(5792, Rational::from_parts(30000, 1001), true).to_smpte(),
            "00:03:13;08"
        );
    }

    #[test]
    fn seconds_conversion_inverse() {
        for n in 0..2000u64 {
            let rate = Rational::from_parts(25, 1);
            let tc = Timecode::new(n, rate, false);
            let secs = tc.as_seconds();
            assert_eq!((secs * rate.value()).round() as u64, n);
        }
    }
}