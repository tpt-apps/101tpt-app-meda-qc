//! Transcript comparison (spec § 8.8 "transcript alignment", "words outside
//! expected transcript").
//!
//! The comparison is plain text-against-text and therefore exact and
//! deterministic. It does not transcribe anything: the hypothesis transcript
//! comes from a sidecar file produced by whatever ASR or captioning workflow
//! the facility already uses, next to the media file:
//!
//! * `<name>.expected.txt` — the script / approved transcript
//! * `<name>.transcript.txt` (or `.json`) — what the audio actually says
//!
//! JSON transcripts may be `{"text": "..."}` or
//! `{"segments": [{"text": "..."}, ...]}`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use tpt_app_media_qc_model::inspection::TranscriptMeasurements;

/// Words compared per side. Alignment is quadratic, so longer texts are
/// refused rather than silently truncated.
pub const MAX_WORDS: usize = 20_000;
/// Largest sidecar file read, in bytes.
pub const MAX_SIDECAR_BYTES: u64 = 8 * 1024 * 1024;
/// Distinct unexpected words kept as evidence.
const MAX_UNEXPECTED_LISTED: usize = 50;

/// Why a comparison could not be produced.
#[derive(Debug, PartialEq, Eq)]
pub enum TranscriptError {
    /// One of the two sidecar files does not exist.
    Missing(PathBuf),
    /// A sidecar exists but is unusable.
    Invalid(String),
}

impl std::fmt::Display for TranscriptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TranscriptError::Missing(p) => write!(f, "sidecar '{}' not found", p.display()),
            TranscriptError::Invalid(m) => f.write_str(m),
        }
    }
}

/// Lower-cased words with surrounding punctuation removed. Apostrophes inside
/// a word are kept (`don't`), curly ones are normalised.
pub fn words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|w| {
            w.chars()
                .map(|c| if c == '\u{2019}' { '\'' } else { c })
                .collect::<String>()
                .trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .filter(|w| !w.is_empty())
        .collect()
}

/// Compare a hypothesis against the expected transcript.
pub fn compare(
    source: &str,
    expected: &str,
    hypothesis: &str,
) -> Result<TranscriptMeasurements, TranscriptError> {
    let reference = words(expected);
    let hyp = words(hypothesis);
    if reference.is_empty() {
        return Err(TranscriptError::Invalid(
            "the expected transcript contains no words".into(),
        ));
    }
    if reference.len() > MAX_WORDS || hyp.len() > MAX_WORDS {
        return Err(TranscriptError::Invalid(format!(
            "transcripts longer than {MAX_WORDS} words are not compared"
        )));
    }

    let (substitutions, deletions, insertions) = edit_counts(&reference, &hyp);
    let wer = (substitutions + deletions + insertions) as f64 / reference.len() as f64;

    let vocabulary: BTreeSet<&str> = reference.iter().map(String::as_str).collect();
    let mut unexpected_total = 0u64;
    let mut listed = BTreeSet::new();
    for word in &hyp {
        if !vocabulary.contains(word.as_str()) {
            unexpected_total += 1;
            if listed.len() < MAX_UNEXPECTED_LISTED {
                listed.insert(word.clone());
            }
        }
    }

    Ok(TranscriptMeasurements {
        source: source.to_string(),
        expected_words: reference.len() as u64,
        hypothesis_words: hyp.len() as u64,
        substitutions,
        deletions,
        insertions,
        wer,
        unexpected_words: listed.into_iter().collect(),
        unexpected_total,
    })
}

/// Minimum-edit substitution / deletion / insertion counts, using two rolling
/// rows so memory stays linear. Ties prefer substitutions, then deletions.
fn edit_counts(reference: &[String], hypothesis: &[String]) -> (u64, u64, u64) {
    #[derive(Clone, Copy)]
    struct Cell {
        cost: u64,
        s: u64,
        d: u64,
        i: u64,
    }
    let mut previous: Vec<Cell> = (0..=hypothesis.len() as u64)
        .map(|j| Cell {
            cost: j,
            s: 0,
            d: 0,
            i: j,
        })
        .collect();
    for (row, r) in reference.iter().enumerate() {
        let mut current = Vec::with_capacity(previous.len());
        current.push(Cell {
            cost: row as u64 + 1,
            s: 0,
            d: row as u64 + 1,
            i: 0,
        });
        for (col, h) in hypothesis.iter().enumerate() {
            let diag = previous[col];
            let up = previous[col + 1];
            let left = current[col];
            let diagonal = if r == h {
                diag
            } else {
                Cell {
                    cost: diag.cost + 1,
                    s: diag.s + 1,
                    ..diag
                }
            };
            let delete = Cell {
                cost: up.cost + 1,
                d: up.d + 1,
                ..up
            };
            let insert = Cell {
                cost: left.cost + 1,
                i: left.i + 1,
                ..left
            };
            let mut best = diagonal;
            if delete.cost < best.cost {
                best = delete;
            }
            if insert.cost < best.cost {
                best = insert;
            }
            current.push(best);
        }
        previous = current;
    }
    let end = previous[hypothesis.len()];
    (end.s, end.d, end.i)
}

/// Sidecar paths for a media file.
pub fn sidecar_paths(media: &Path) -> (PathBuf, [PathBuf; 2]) {
    let with = |suffix: &str| media.with_extension(suffix);
    (
        with("expected.txt"),
        [with("transcript.txt"), with("transcript.json")],
    )
}

fn read_limited(path: &Path) -> Result<String, TranscriptError> {
    let size = std::fs::metadata(path)
        .map_err(|_| TranscriptError::Missing(path.to_path_buf()))?
        .len();
    if size > MAX_SIDECAR_BYTES {
        return Err(TranscriptError::Invalid(format!(
            "'{}' is larger than {MAX_SIDECAR_BYTES} bytes",
            path.display()
        )));
    }
    std::fs::read_to_string(path)
        .map_err(|e| TranscriptError::Invalid(format!("cannot read '{}': {e}", path.display())))
}

/// Text of a JSON transcript (`text`, or the joined `segments[].text`).
pub fn json_transcript_text(json: &str) -> Result<String, TranscriptError> {
    let value: serde_json::Value = serde_json::from_str(json)
        .map_err(|e| TranscriptError::Invalid(format!("transcript JSON is invalid: {e}")))?;
    if let Some(text) = value.get("text").and_then(|t| t.as_str()) {
        return Ok(text.to_string());
    }
    if let Some(segments) = value.get("segments").and_then(|s| s.as_array()) {
        return Ok(segments
            .iter()
            .filter_map(|s| s.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join(" "));
    }
    Err(TranscriptError::Invalid(
        "transcript JSON needs a 'text' string or a 'segments' array with 'text' fields".into(),
    ))
}

/// Compare the sidecar transcripts next to `media`.
///
/// `Ok(None)` means no sidecars exist at all, which is the normal case and not
/// an error; a half-supplied pair is reported as missing.
pub fn compare_sidecars(media: &Path) -> Result<Option<TranscriptMeasurements>, TranscriptError> {
    let (expected_path, hypothesis_paths) = sidecar_paths(media);
    let hypothesis_path = hypothesis_paths.iter().find(|p| p.is_file());
    if !expected_path.is_file() && hypothesis_path.is_none() {
        return Ok(None);
    }
    let Some(hypothesis_path) = hypothesis_path else {
        return Err(TranscriptError::Missing(hypothesis_paths[0].clone()));
    };
    let expected = read_limited(&expected_path)?;
    let raw = read_limited(hypothesis_path)?;
    let hypothesis = if hypothesis_path.extension().is_some_and(|e| e == "json") {
        json_transcript_text(&raw)?
    } else {
        raw
    };
    let source = hypothesis_path
        .file_name()
        .map(|n| format!("sidecar {}", n.to_string_lossy()))
        .unwrap_or_else(|| "sidecar".into());
    compare(&source, &expected, &hypothesis).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalises_case_punctuation_and_curly_apostrophes() {
        assert_eq!(
            words("Hello, World! It\u{2019}s \"fine\"... -- ok"),
            ["hello", "world", "it's", "fine", "ok"]
        );
    }

    #[test]
    fn identical_texts_have_zero_wer() {
        let m = compare("t", "the quick brown fox", "The quick, brown fox.").unwrap();
        assert_eq!((m.substitutions, m.deletions, m.insertions), (0, 0, 0));
        assert_eq!(m.wer, 0.0);
        assert_eq!(m.unexpected_total, 0);
    }

    #[test]
    fn counts_each_kind_of_edit() {
        let counts = |hyp: &str| {
            let m = compare("t", "one two three four", hyp).unwrap();
            (m.substitutions, m.deletions, m.insertions)
        };
        assert_eq!(counts("one too three four"), (1, 0, 0));
        assert_eq!(counts("one three four"), (0, 1, 0));
        assert_eq!(counts("one two three four five"), (0, 0, 1));
        // Everything missing is all deletions.
        assert_eq!(counts(""), (0, 4, 0));
    }

    #[test]
    fn wer_and_unexpected_words_combine() {
        let m = compare("t", "the quick brown fox", "the quick brown dog").unwrap();
        assert_eq!(m.expected_words, 4);
        assert!((m.wer - 0.25).abs() < 1e-9);
        assert_eq!(m.unexpected_words, ["dog"]);
        assert_eq!(m.unexpected_total, 1);
    }

    #[test]
    fn rejects_empty_reference_and_oversized_input() {
        assert!(compare("t", "  ...  ", "anything").is_err());
        let long = "a ".repeat(MAX_WORDS + 1);
        assert!(compare("t", &long, "a").is_err());
    }

    #[test]
    fn unexpected_words_are_listed_once_but_counted_every_time() {
        let m = compare("t", "alpha beta", "alpha zed zed zed beta").unwrap();
        assert_eq!(m.unexpected_words, ["zed"]);
        assert_eq!(m.unexpected_total, 3);
    }

    #[test]
    fn json_transcripts_accept_text_or_segments() {
        assert_eq!(json_transcript_text(r#"{"text":"a b"}"#).unwrap(), "a b");
        assert_eq!(
            json_transcript_text(r#"{"segments":[{"text":"a"},{"text":"b"}]}"#).unwrap(),
            "a b"
        );
        assert!(json_transcript_text(r#"{"other":1}"#).is_err());
        assert!(json_transcript_text("nope").is_err());
    }

    #[test]
    fn sidecars_are_found_next_to_the_media() {
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("clip.wav");
        assert_eq!(compare_sidecars(&media), Ok(None));

        std::fs::write(dir.path().join("clip.expected.txt"), "one two three").unwrap();
        assert!(matches!(
            compare_sidecars(&media),
            Err(TranscriptError::Missing(_))
        ));

        std::fs::write(
            dir.path().join("clip.transcript.json"),
            r#"{"text":"one too three"}"#,
        )
        .unwrap();
        let m = compare_sidecars(&media).unwrap().unwrap();
        assert_eq!(m.substitutions, 1);
        assert!(m.source.contains("clip.transcript.json"));
    }
}
