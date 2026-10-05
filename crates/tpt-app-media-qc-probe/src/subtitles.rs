//! Subtitle cue measurements ([spec § 8.7]), moved here from the old
//! external-probe front-end so every container shares one implementation.
//!
//! Timing checks run for every subtitle codec; text checks run only when the
//! codec's payload is plain text, otherwise `text_decoded` stays `false` and
//! the content rule reports `Inconclusive` instead of guessing.

use tpt_app_media_qc_model::finding::TimeRange;
use tpt_app_media_qc_model::inspection::SubtitleMeasurements;

use crate::common::{RawCue, MAX_CUES};

/// Subtitle codecs whose payload is plain text, so characters and markup can
/// be inspected.
pub fn is_text_subtitle_codec(codec: &str) -> bool {
    matches!(
        codec.to_ascii_lowercase().as_str(),
        "subrip" | "srt" | "ass" | "ssa" | "webvtt" | "tx3g" | "text"
    )
}

/// Strip SubRip/ASS/WebVTT markup and report the visible line structure:
/// `(longest line in characters, number of non-empty lines)`. `None` when the
/// payload is not valid UTF-8.
fn analyse_cue_text(raw: &[u8]) -> Option<(u32, u32)> {
    let text = std::str::from_utf8(raw).ok()?;
    let mut max_line_chars = 0u32;
    let mut lines = 0u32;
    for line in text.lines() {
        let line = strip_subtitle_markup(line);
        if line.is_empty() {
            continue;
        }
        lines += 1;
        max_line_chars = max_line_chars.max(line.chars().count() as u32);
    }
    Some((max_line_chars, lines))
}

/// Remove SRT/ASS/WebVTT structural markup so character counts reflect what
/// is rendered rather than how the file encodes it.
fn strip_subtitle_markup(line: &str) -> String {
    let mut line = line.trim();
    // WebVTT/SRT cue settings trail the timestamps (`align:start ...`).
    if let Some((_, rest)) = line.rsplit_once("-->") {
        line = rest.trim();
    }
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            // ASS override blocks `{\...}` and HTML-ish tags `<i>`.
            '{' | '<' => {
                let close = if c == '{' { '}' } else { '>' };
                for c in chars.by_ref() {
                    if c == close {
                        break;
                    }
                }
            }
            _ => out.push(c),
        }
    }
    out.trim().to_string()
}

/// Reduce one stream's cues into [`SubtitleMeasurements`].
pub fn measure(stream_idx: u64, is_text: bool, cues: &[RawCue]) -> SubtitleMeasurements {
    let mut meas = SubtitleMeasurements {
        stream_idx,
        text_decoded: is_text,
        ..Default::default()
    };

    // Container order is not guaranteed to be presentation order.
    let mut cues: Vec<&RawCue> = cues.iter().take(MAX_CUES).collect();
    cues.sort_by_key(|c| (c.start_ms, c.end_ms));

    meas.cue_count = Some(cues.len() as u64);
    if cues.is_empty() {
        return meas;
    }

    let mut max_line_chars: Option<u32> = None;
    let mut max_lines: Option<u32> = None;
    let mut saw_text_payload = false;

    for (i, cue) in cues.iter().enumerate() {
        let (start, end) = (cue.start_ms, cue.end_ms);
        if end <= start {
            meas.invalid_durations.push(TimeRange::new(start, start));
        }
        if let Some(next) = cues.get(i + 1) {
            // Overlap counts against the following cue, not itself.
            if end > next.start_ms {
                meas.overlaps
                    .push(TimeRange::new(start, end.min(next.start_ms)));
            } else {
                meas.max_gap_ms = Some(
                    meas.max_gap_ms
                        .unwrap_or(0)
                        .max(next.start_ms.saturating_sub(end)),
                );
            }
        }
        if !is_text {
            continue;
        }
        let Some(payload) = cue.payload.as_deref() else {
            continue;
        };
        saw_text_payload = true;
        match analyse_cue_text(payload) {
            Some((line_chars, lines)) => {
                if line_chars == 0 && lines == 0 {
                    meas.empty_cues.push(TimeRange::new(start, end));
                }
                max_line_chars = Some(max_line_chars.unwrap_or(0).max(line_chars));
                max_lines = Some(max_lines.unwrap_or(0).max(lines));
            }
            // Payload is not valid UTF-8 text.
            None => meas.malformed.push(TimeRange::new(start, end)),
        }
    }

    meas.max_cue_duration_ms = Some(
        cues.iter()
            .map(|c| c.end_ms.saturating_sub(c.start_ms))
            .max()
            .unwrap_or(0),
    );
    meas.covered_until_ms = cues.iter().map(|c| c.end_ms).max();
    if saw_text_payload {
        meas.max_line_chars = max_line_chars;
        meas.max_lines_per_cue = max_lines;
    }
    meas
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cue(start: u64, end: u64, text: Option<&str>) -> RawCue {
        RawCue {
            start_ms: start,
            end_ms: end,
            payload: text.map(|t| t.as_bytes().to_vec()),
        }
    }

    #[test]
    fn timing_and_text_checks() {
        let cues = [
            cue(0, 2000, Some("Hello <i>world</i>")),
            cue(1500, 3000, Some("{\\an8}Top line\nSecond line")),
            cue(5000, 5000, Some("")),
            cue(9000, 12_000, None),
        ];
        let m = measure(2, true, &cues);
        assert_eq!(m.cue_count, Some(4));
        assert_eq!(m.overlaps, vec![TimeRange::new(0, 1500)]);
        assert_eq!(m.invalid_durations, vec![TimeRange::new(5000, 5000)]);
        assert_eq!(m.empty_cues, vec![TimeRange::new(5000, 5000)]);
        assert_eq!(m.max_gap_ms, Some(4000));
        assert_eq!(m.max_lines_per_cue, Some(2));
        assert_eq!(m.max_line_chars, Some(11));
        assert_eq!(m.max_cue_duration_ms, Some(3000));
        assert_eq!(m.covered_until_ms, Some(12_000));
    }

    #[test]
    fn non_text_codecs_report_timing_only() {
        let m = measure(0, false, &[cue(0, 1000, None), cue(2000, 3000, None)]);
        assert!(!m.text_decoded);
        assert_eq!(m.max_line_chars, None);
        assert_eq!(m.max_gap_ms, Some(1000));
    }

    #[test]
    fn invalid_utf8_is_malformed() {
        let c = RawCue {
            start_ms: 0,
            end_ms: 500,
            payload: Some(vec![0xFF, 0xFE, 0xFD]),
        };
        assert_eq!(
            measure(0, true, &[c]).malformed,
            vec![TimeRange::new(0, 500)]
        );
    }

    #[test]
    fn empty_stream_has_zero_cues() {
        assert_eq!(measure(3, true, &[]).cue_count, Some(0));
    }

    #[test]
    fn codec_names() {
        for c in ["subrip", "srt", "ass", "SSA", "webvtt", "tx3g"] {
            assert!(is_text_subtitle_codec(c), "{c}");
        }
        assert!(!is_text_subtitle_codec("hdmv_pgs"));
    }
}
