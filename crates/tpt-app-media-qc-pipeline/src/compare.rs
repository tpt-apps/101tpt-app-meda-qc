//! File-to-file comparison ([spec § 15]): metadata, streams, duration, frame
//! rate, resolution, codec, audio layout, loudness and measured defects.
//!
//! The comparison is a pure function over two inspections, so it is
//! deterministic and independent of how the inspections were produced. Values
//! that were not measured on both sides are reported as `unmeasured` rather
//! than as a difference.

use serde::{Deserialize, Serialize};
use tpt_app_media_qc_model::asset::{Stream, StreamKind};
use tpt_app_media_qc_model::inspection::{AudioMeasurements, Inspection, VideoMeasurements};

/// Tolerances below which a numeric difference is treated as equal.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompareTolerances {
    /// Duration difference in milliseconds (default: one 25 fps frame).
    pub duration_ms: u64,
    /// Frame-rate difference in frames per second.
    pub frame_rate: f64,
    /// Loudness / loudness-range difference in LU.
    pub loudness_lu: f64,
    /// Peak / true-peak difference in dB.
    pub peak_db: f64,
    /// Relative bitrate difference (0.1 = 10 %).
    pub bitrate_ratio: f64,
}

impl Default for CompareTolerances {
    fn default() -> Self {
        Self {
            duration_ms: 40,
            frame_rate: 0.001,
            loudness_lu: 0.5,
            peak_db: 0.5,
            bitrate_ratio: 0.1,
        }
    }
}

/// How significant a difference is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DifferenceKind {
    /// Informational: expected to vary between encodes (bitrate, container).
    Minor,
    /// Changes what the delivered file is (codec, resolution, layout, level).
    Major,
}

/// One compared field.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Difference {
    /// Group: `container`, `streams`, `video s0`, `audio s1`, ...
    pub scope: String,
    pub field: String,
    pub left: String,
    pub right: String,
    pub kind: DifferenceKind,
}

/// Result of comparing two assets.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Comparison {
    pub differences: Vec<Difference>,
    /// Fields compared and found equal (within tolerance).
    pub matching_fields: usize,
    /// Fields skipped because one or both sides were not measured.
    pub unmeasured_fields: usize,
}

impl Comparison {
    pub fn is_identical(&self) -> bool {
        self.differences.is_empty()
    }

    /// The most significant difference, if any.
    pub fn worst(&self) -> Option<DifferenceKind> {
        self.differences.iter().map(|d| d.kind).max()
    }
}

struct Collector<'a> {
    out: Comparison,
    tolerances: &'a CompareTolerances,
}

impl Collector<'_> {
    fn record(
        &mut self,
        scope: &str,
        field: &str,
        left: String,
        right: String,
        kind: DifferenceKind,
    ) {
        self.out.differences.push(Difference {
            scope: scope.into(),
            field: field.into(),
            left,
            right,
            kind,
        });
    }

    fn text(
        &mut self,
        scope: &str,
        field: &str,
        a: Option<String>,
        b: Option<String>,
        kind: DifferenceKind,
    ) {
        match (a, b) {
            (Some(a), Some(b)) if a.eq_ignore_ascii_case(&b) => self.out.matching_fields += 1,
            (Some(a), Some(b)) => self.record(scope, field, a, b, kind),
            _ => self.out.unmeasured_fields += 1,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn number(
        &mut self,
        scope: &str,
        field: &str,
        a: Option<f64>,
        b: Option<f64>,
        tolerance: f64,
        unit: &str,
        kind: DifferenceKind,
    ) {
        match (a, b) {
            (Some(a), Some(b)) if (a - b).abs() <= tolerance => self.out.matching_fields += 1,
            (Some(a), Some(b)) => self.record(
                scope,
                field,
                format!("{a:.2}{unit}"),
                format!("{b:.2}{unit}"),
                kind,
            ),
            _ => self.out.unmeasured_fields += 1,
        }
    }

    fn count(
        &mut self,
        scope: &str,
        field: &str,
        a: Option<u64>,
        b: Option<u64>,
        kind: DifferenceKind,
    ) {
        self.text(
            scope,
            field,
            a.map(|v| v.to_string()),
            b.map(|v| v.to_string()),
            kind,
        );
    }
}

fn streams_of(inspection: &Inspection, kind: StreamKind) -> Vec<&Stream> {
    inspection
        .streams
        .iter()
        .filter(|s| s.kind == kind)
        .collect()
}

fn kind_label(kind: StreamKind) -> &'static str {
    match kind {
        StreamKind::Video => "video",
        StreamKind::Audio => "audio",
        _ => "other",
    }
}

/// Compares two inspections. Streams of the same kind are paired by order.
pub fn compare(
    left: &Inspection,
    right: &Inspection,
    tolerances: &CompareTolerances,
) -> Comparison {
    use DifferenceKind::{Major, Minor};
    let mut c = Collector {
        out: Comparison::default(),
        tolerances,
    };

    // Container.
    let (lc, rc) = (&left.container, &right.container);
    c.text(
        "container",
        "format",
        lc.format.clone(),
        rc.format.clone(),
        Minor,
    );
    c.number(
        "container",
        "duration",
        lc.duration.map(|d| d.0 as f64),
        rc.duration.map(|d| d.0 as f64),
        tolerances.duration_ms as f64,
        " ms",
        Major,
    );
    match (lc.bitrate_bps, rc.bitrate_bps) {
        (Some(a), Some(b)) => {
            let base = a.max(b).max(1) as f64;
            let within = (a as f64 - b as f64).abs() / base <= c.tolerances.bitrate_ratio;
            if within {
                c.out.matching_fields += 1;
            } else {
                c.record(
                    "container",
                    "bitrate",
                    format!("{a} bps"),
                    format!("{b} bps"),
                    Minor,
                );
            }
        }
        _ => c.out.unmeasured_fields += 1,
    }
    c.text(
        "container",
        "timecode",
        lc.timecode_present.map(|v| v.to_string()),
        rc.timecode_present.map(|v| v.to_string()),
        Minor,
    );

    // Streams.
    for kind in [StreamKind::Video, StreamKind::Audio] {
        let (ls, rs) = (streams_of(left, kind), streams_of(right, kind));
        let label = kind_label(kind);
        c.count(
            "streams",
            &format!("{label} stream count"),
            Some(ls.len() as u64),
            Some(rs.len() as u64),
            Major,
        );
        for (position, (a, b)) in ls.iter().zip(&rs).enumerate() {
            let scope = format!("{label} #{position}");
            c.text(&scope, "codec", a.codec.clone(), b.codec.clone(), Major);
            match kind {
                StreamKind::Video => compare_video(&mut c, &scope, a, b, left, right),
                _ => compare_audio(&mut c, &scope, a, b, left, right),
            }
        }
    }
    c.out
}

fn compare_video(
    c: &mut Collector<'_>,
    scope: &str,
    a: &Stream,
    b: &Stream,
    left: &Inspection,
    right: &Inspection,
) {
    use DifferenceKind::{Major, Minor};
    c.text(
        scope,
        "resolution",
        a.dimension_label(),
        b.dimension_label(),
        Major,
    );
    c.number(
        scope,
        "frame rate",
        a.frame_rate.map(|f| f.value()),
        b.frame_rate.map(|f| f.value()),
        c.tolerances.frame_rate,
        " fps",
        Major,
    );
    c.text(
        scope,
        "pixel format",
        a.pixel_format.clone(),
        b.pixel_format.clone(),
        Minor,
    );
    c.text(
        scope,
        "scan order",
        a.field_order.map(|f| f.as_str().to_string()),
        b.field_order.map(|f| f.as_str().to_string()),
        Major,
    );
    let (ma, mb): (Option<&VideoMeasurements>, Option<&VideoMeasurements>) =
        (left.video_for(a.index.0), right.video_for(b.index.0));
    c.text(
        scope,
        "colour space",
        ma.and_then(|m| m.colorspace.clone()),
        mb.and_then(|m| m.colorspace.clone()),
        Major,
    );
    if let (Some(ma), Some(mb)) = (ma, mb) {
        let measured = |m: &VideoMeasurements| m.decoded_frame_count.filter(|n| *n > 0).is_some();
        if measured(ma) && measured(mb) {
            c.count(
                scope,
                "black segments",
                Some(ma.black_frames.len() as u64),
                Some(mb.black_frames.len() as u64),
                Major,
            );
            c.count(
                scope,
                "freeze segments",
                Some(ma.freeze_frames.len() as u64),
                Some(mb.freeze_frames.len() as u64),
                Major,
            );
            c.count(
                scope,
                "duplicate segments",
                Some(ma.duplicate_frames.len() as u64),
                Some(mb.duplicate_frames.len() as u64),
                Minor,
            );
            c.number(
                scope,
                "mean luma",
                ma.luma.map(|l| l.mean),
                mb.luma.map(|l| l.mean),
                1.0,
                "",
                Minor,
            );
        } else {
            c.out.unmeasured_fields += 1;
        }
    } else {
        c.out.unmeasured_fields += 1;
    }
}

fn compare_audio(
    c: &mut Collector<'_>,
    scope: &str,
    a: &Stream,
    b: &Stream,
    left: &Inspection,
    right: &Inspection,
) {
    use DifferenceKind::{Major, Minor};
    c.count(scope, "sample rate", a.sample_rate, b.sample_rate, Major);
    c.count(scope, "channels", a.channels, b.channels, Major);
    c.text(
        scope,
        "channel layout",
        a.channel_layout.clone(),
        b.channel_layout.clone(),
        Major,
    );
    c.count(scope, "bit depth", a.bit_depth, b.bit_depth, Minor);
    c.text(
        scope,
        "language",
        a.language.clone(),
        b.language.clone(),
        Minor,
    );
    let (ma, mb): (Option<&AudioMeasurements>, Option<&AudioMeasurements>) =
        (left.audio_for(a.index.0), right.audio_for(b.index.0));
    let (la, lb) = (ma, mb);
    let tol = *c.tolerances;
    c.number(
        scope,
        "integrated loudness",
        la.and_then(|m| m.loudness_lufs),
        lb.and_then(|m| m.loudness_lufs),
        tol.loudness_lu,
        " LUFS",
        Major,
    );
    c.number(
        scope,
        "loudness range",
        la.and_then(|m| m.loudness_range_lu),
        lb.and_then(|m| m.loudness_range_lu),
        tol.loudness_lu,
        " LU",
        Minor,
    );
    c.number(
        scope,
        "true peak",
        la.and_then(|m| m.true_peak_db),
        lb.and_then(|m| m.true_peak_db),
        tol.peak_db,
        " dBTP",
        Major,
    );
    c.number(
        scope,
        "sample peak",
        la.and_then(|m| m.peak_db),
        lb.and_then(|m| m.peak_db),
        tol.peak_db,
        " dBFS",
        Minor,
    );
    c.number(
        scope,
        "clipping events",
        la.map(|m| m.clipping_events as f64),
        lb.map(|m| m.clipping_events as f64),
        0.0,
        "",
        Major,
    );
    c.number(
        scope,
        "silence segments",
        la.map(|m| m.silence.len() as f64),
        lb.map(|m| m.silence.len() as f64),
        0.0,
        "",
        Minor,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpt_app_media_qc_model::time::FrameRate;

    fn inspection() -> Inspection {
        let mut i = Inspection::default();
        i.container.format = Some("mov".into());
        i.container.duration = Some(tpt_app_media_qc_model::inspection::DurationMillis(120_000));
        i.container.bitrate_bps = Some(10_000_000);
        i.streams = vec![Stream::primary_video(0), Stream::primary_audio(1)];
        i.audio.push(AudioMeasurements {
            stream_idx: 1,
            loudness_lufs: Some(-23.0),
            true_peak_db: Some(-2.0),
            ..Default::default()
        });
        i
    }

    #[test]
    fn identical_inspections_have_no_differences() {
        let a = inspection();
        let result = compare(&a, &a.clone(), &CompareTolerances::default());
        assert!(result.is_identical());
        assert!(result.matching_fields > 5);
    }

    #[test]
    fn changes_are_reported_with_significance() {
        let a = inspection();
        let mut b = inspection();
        b.container.duration = Some(tpt_app_media_qc_model::inspection::DurationMillis(120_500));
        b.container.bitrate_bps = Some(4_000_000);
        b.streams[0].width = Some(1280);
        b.streams[0].height = Some(720);
        b.streams[0].frame_rate = Some(FrameRate::from_parts(30, 1));
        b.audio[0].loudness_lufs = Some(-20.0);
        let result = compare(&a, &b, &CompareTolerances::default());
        let fields: Vec<_> = result
            .differences
            .iter()
            .map(|d| d.field.as_str())
            .collect();
        for expected in [
            "duration",
            "bitrate",
            "resolution",
            "frame rate",
            "integrated loudness",
        ] {
            assert!(fields.contains(&expected), "missing {expected}: {fields:?}");
        }
        assert_eq!(result.worst(), Some(DifferenceKind::Major));
        let bitrate = result
            .differences
            .iter()
            .find(|d| d.field == "bitrate")
            .unwrap();
        assert_eq!(bitrate.kind, DifferenceKind::Minor);
    }

    #[test]
    fn within_tolerance_is_equal_and_missing_values_are_unmeasured() {
        let a = inspection();
        let mut b = inspection();
        b.container.duration = Some(tpt_app_media_qc_model::inspection::DurationMillis(120_030));
        b.audio[0].loudness_lufs = Some(-23.3);
        b.audio[0].true_peak_db = None;
        let result = compare(&a, &b, &CompareTolerances::default());
        assert!(result.is_identical(), "{:?}", result.differences);
        assert!(result.unmeasured_fields > 0);
    }

    #[test]
    fn stream_count_mismatch_is_major() {
        let a = inspection();
        let mut b = inspection();
        b.streams.pop();
        let result = compare(&a, &b, &CompareTolerances::default());
        assert!(result
            .differences
            .iter()
            .any(|d| d.field == "audio stream count" && d.kind == DifferenceKind::Major));
    }
}
