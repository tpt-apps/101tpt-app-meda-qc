//! Optional voice/content checks ([spec § 8.8]).
//!
//! Speech and speaker findings come from heuristic detectors and are
//! **probabilistic**: every message says so. The transcript check compares
//! two texts and is exact for the texts it is given. Without voice
//! measurements every rule reports `Inconclusive`; voice analysis is never
//! required for basic QC.

use tpt_app_media_qc_core::cost::CostClass;

use crate::rule::{
    Capabilities, DecodeRequirement, QcRule, RuleContext, RuleDescription, RuleResult,
};
use crate::util::{fail, inconclusive};
use tpt_app_media_qc_model::asset::{StreamId, StreamKind};
use tpt_app_media_qc_model::finding::{QcFinding, RuleId, TimeRange};
use tpt_app_media_qc_model::inspection::VoiceMeasurements;
use tpt_app_media_qc_model::severity::Severity;
use tpt_app_media_qc_model::value::Value;
use tpt_app_media_qc_profile::model::{
    Profile, SpeakerChangeRule, SpeakerCountRule, SpeechExpectation, SpeechRule, TranscriptRule,
    VoiceSilenceRule,
};

pub const RULE_IDS: &[&str] = &[
    "voice.speech",
    "voice.silence",
    "voice.speakers",
    "voice.speaker_changes",
    "voice.transcript",
];

pub fn build(profile: &Profile, out: &mut Vec<Box<dyn QcRule>>) {
    let v = &profile.rules.voice;
    if let Some(cfg) = v.speech {
        out.push(Box::new(SpeechRuleImpl { cfg }));
    }
    if let Some(cfg) = v.silence {
        out.push(Box::new(SilenceRuleImpl { cfg }));
    }
    if let Some(cfg) = v.speakers {
        out.push(Box::new(SpeakersRuleImpl { cfg }));
    }
    if let Some(cfg) = v.speaker_changes {
        out.push(Box::new(SpeakerChangesRuleImpl { cfg }));
    }
    if let Some(cfg) = v.transcript {
        out.push(Box::new(TranscriptRuleImpl { cfg }));
    }
}

fn capabilities() -> Capabilities {
    Capabilities {
        required_streams: &[StreamKind::Audio],
        decode: DecodeRequirement::Stream,
        incremental: false,
        gpu: false,
        cost: CostClass::ExpensiveAnalysis,
    }
}

/// Voice measurements usable by a rule, or an `Inconclusive` result when
/// there are none.
fn measurements<'a>(
    ctx: &'a RuleContext<'_>,
    id: &RuleId,
    severity: Severity,
    needs_speech: bool,
) -> Result<Vec<&'a VoiceMeasurements>, RuleResult> {
    let usable: Vec<_> = ctx
        .inspection
        .voice
        .iter()
        .filter(|m| !needs_speech || m.speech_analysed)
        .collect();
    if usable.is_empty() {
        let why = if needs_speech {
            "no speech analysis was available (voice analysis is optional and was not run, or the audio could not be decoded)"
        } else {
            "no transcript comparison was available (supply a transcript and an expected transcript)"
        };
        return Err(RuleResult::from_finding(inconclusive(id, severity, why)));
    }
    Ok(usable)
}

/// Mark a message as coming from a heuristic detector.
fn label(message: impl std::fmt::Display) -> String {
    format!("[probabilistic] {message}")
}

fn pct(ratio: f64) -> String {
    format!("{:.1}%", ratio * 100.0)
}

fn on_stream(f: QcFinding, m: &VoiceMeasurements) -> QcFinding {
    f.stream(StreamId::new(m.stream_idx))
}

// ---------------------------------------------------------------------------

struct SpeechRuleImpl {
    cfg: SpeechRule,
}

impl QcRule for SpeechRuleImpl {
    fn id(&self) -> RuleId {
        RuleId::new("voice.speech")
    }
    fn description(&self) -> RuleDescription {
        RuleDescription {
            name: "Speech present / absent",
            summary: "Probabilistic energy-based speech detection against an expectation.",
            version: "0.1",
        }
    }
    fn capabilities(&self) -> Capabilities {
        capabilities()
    }
    fn run(&self, ctx: &RuleContext<'_>) -> RuleResult {
        let id = self.id();
        let all = match measurements(ctx, &id, self.cfg.severity, true) {
            Ok(m) => m,
            Err(r) => return r,
        };
        let mut findings = Vec::new();
        for m in all {
            let Some(ratio) = m.speech_ratio() else {
                findings.push(on_stream(
                    inconclusive(&id, self.cfg.severity, "no audio was analysed"),
                    m,
                ));
                continue;
            };
            let (bad, msg, expected) = match self.cfg.expect {
                SpeechExpectation::Present => (
                    ratio < self.cfg.min_ratio,
                    format!(
                        "expected speech but only {} of the audio was detected as speech (minimum {})",
                        pct(ratio),
                        pct(self.cfg.min_ratio)
                    ),
                    format!("speech >= {}", pct(self.cfg.min_ratio)),
                ),
                SpeechExpectation::Absent => (
                    ratio >= self.cfg.min_ratio,
                    format!(
                        "expected no speech but {} of the audio was detected as speech (limit {})",
                        pct(ratio),
                        pct(self.cfg.min_ratio)
                    ),
                    format!("speech < {}", pct(self.cfg.min_ratio)),
                ),
            };
            if bad {
                findings.push(on_stream(
                    fail(
                        &id,
                        self.cfg.severity,
                        label(msg),
                        Value::Ratio(ratio),
                        Value::Text(expected),
                    ),
                    m,
                ));
            }
        }
        RuleResult::findings(findings)
    }
}

// ---------------------------------------------------------------------------

struct SilenceRuleImpl {
    cfg: VoiceSilenceRule,
}

impl QcRule for SilenceRuleImpl {
    fn id(&self) -> RuleId {
        RuleId::new("voice.silence")
    }
    fn description(&self) -> RuleDescription {
        RuleDescription {
            name: "Excessive non-speech",
            summary: "Longest stretch without detected speech.",
            version: "0.1",
        }
    }
    fn capabilities(&self) -> Capabilities {
        capabilities()
    }
    fn run(&self, ctx: &RuleContext<'_>) -> RuleResult {
        let id = self.id();
        let all = match measurements(ctx, &id, self.cfg.severity, true) {
            Ok(m) => m,
            Err(r) => return r,
        };
        let findings = all
            .into_iter()
            .filter_map(|m| {
                let gap: TimeRange = m.longest_non_speech?;
                (gap.duration_ms() > self.cfg.max_non_speech_ms).then(|| {
                    on_stream(
                        fail(
                            &id,
                            self.cfg.severity,
                            label(format!(
                                "{} ms without detected speech (limit {} ms)",
                                gap.duration_ms(),
                                self.cfg.max_non_speech_ms
                            )),
                            Value::DurationMs(gap.duration_ms()),
                            Value::DurationMs(self.cfg.max_non_speech_ms),
                        )
                        .time(gap),
                        m,
                    )
                })
            })
            .collect();
        RuleResult::findings(findings)
    }
}

// ---------------------------------------------------------------------------

struct SpeakersRuleImpl {
    cfg: SpeakerCountRule,
}

impl QcRule for SpeakersRuleImpl {
    fn id(&self) -> RuleId {
        RuleId::new("voice.speakers")
    }
    fn description(&self) -> RuleDescription {
        RuleDescription {
            name: "Speaker count",
            summary: "Number of distinct speakers from unsupervised clustering.",
            version: "0.1",
        }
    }
    fn capabilities(&self) -> Capabilities {
        capabilities()
    }
    fn run(&self, ctx: &RuleContext<'_>) -> RuleResult {
        let id = self.id();
        let all = match measurements(ctx, &id, self.cfg.severity, true) {
            Ok(m) => m,
            Err(r) => return r,
        };
        let mut findings = Vec::new();
        for m in all {
            let Some(count) = m.speaker_count else {
                findings.push(on_stream(
                    inconclusive(&id, self.cfg.severity, "speakers were not counted"),
                    m,
                ));
                continue;
            };
            let low = self.cfg.min.is_some_and(|min| count < min);
            let high = self.cfg.max.is_some_and(|max| count > max);
            if low || high {
                let expected = match (self.cfg.min, self.cfg.max) {
                    (Some(a), Some(b)) => format!("{a}..={b} speakers"),
                    (Some(a), None) => format!(">= {a} speakers"),
                    (None, Some(b)) => format!("<= {b} speakers"),
                    (None, None) => unreachable!("profile parser requires a bound"),
                };
                findings.push(on_stream(
                    fail(
                        &id,
                        self.cfg.severity,
                        label(format!("{count} speaker(s) detected, expected {expected}")),
                        Value::UInt(u64::from(count)),
                        Value::Text(expected),
                    ),
                    m,
                ));
            }
        }
        RuleResult::findings(findings)
    }
}

// ---------------------------------------------------------------------------

struct SpeakerChangesRuleImpl {
    cfg: SpeakerChangeRule,
}

impl QcRule for SpeakerChangesRuleImpl {
    fn id(&self) -> RuleId {
        RuleId::new("voice.speaker_changes")
    }
    fn description(&self) -> RuleDescription {
        RuleDescription {
            name: "Speaker changes",
            summary: "Speaker turns per minute of audio.",
            version: "0.1",
        }
    }
    fn capabilities(&self) -> Capabilities {
        capabilities()
    }
    fn run(&self, ctx: &RuleContext<'_>) -> RuleResult {
        let id = self.id();
        let all = match measurements(ctx, &id, self.cfg.severity, true) {
            Ok(m) => m,
            Err(r) => return r,
        };
        let findings = all
            .into_iter()
            .filter(|m| m.analysed_ms > 0)
            .filter_map(|m| {
                let rate = f64::from(m.speaker_changes) / (m.analysed_ms as f64 / 60_000.0);
                (rate > self.cfg.max_per_minute).then(|| {
                    on_stream(
                        fail(
                            &id,
                            self.cfg.severity,
                            label(format!(
                                "{} speaker changes in {:.1} s ({rate:.1}/min, limit {:.1}/min)",
                                m.speaker_changes,
                                m.analysed_ms as f64 / 1000.0,
                                self.cfg.max_per_minute
                            )),
                            Value::Float(rate),
                            Value::Float(self.cfg.max_per_minute),
                        ),
                        m,
                    )
                })
            })
            .collect();
        RuleResult::findings(findings)
    }
}

// ---------------------------------------------------------------------------

struct TranscriptRuleImpl {
    cfg: TranscriptRule,
}

impl QcRule for TranscriptRuleImpl {
    fn id(&self) -> RuleId {
        RuleId::new("voice.transcript")
    }
    fn description(&self) -> RuleDescription {
        RuleDescription {
            name: "Transcript match",
            summary: "Word error rate and words outside the expected transcript.",
            version: "0.1",
        }
    }
    fn capabilities(&self) -> Capabilities {
        capabilities()
    }
    fn run(&self, ctx: &RuleContext<'_>) -> RuleResult {
        let id = self.id();
        let all = match measurements(ctx, &id, self.cfg.severity, false) {
            Ok(m) => m,
            Err(r) => return r,
        };
        let mut findings = Vec::new();
        for m in all {
            let Some(t) = &m.transcript else {
                findings.push(on_stream(
                    inconclusive(
                        &id,
                        self.cfg.severity,
                        "no transcript comparison for this stream",
                    ),
                    m,
                ));
                continue;
            };
            // The comparison itself is exact for the two texts; how
            // trustworthy the hypothesis is depends on its source.
            let note = format!("transcript from {}", t.source);
            if t.wer > self.cfg.max_wer {
                findings.push(on_stream(
                    fail(
                        &id,
                        self.cfg.severity,
                        format!(
                            "word error rate {} exceeds {} ({note}; {} substitutions, {} deletions, {} insertions over {} expected words)",
                            pct(t.wer),
                            pct(self.cfg.max_wer),
                            t.substitutions,
                            t.deletions,
                            t.insertions,
                            t.expected_words
                        ),
                        Value::Ratio(t.wer),
                        Value::Ratio(self.cfg.max_wer),
                    ),
                    m,
                ));
            }
            if t.unexpected_total > self.cfg.max_unexpected_words {
                let sample = t
                    .unexpected_words
                    .iter()
                    .take(10)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ");
                findings.push(on_stream(
                    fail(
                        &id,
                        self.cfg.severity,
                        format!(
                            "{} word(s) outside the expected transcript ({note}): {sample}",
                            t.unexpected_total
                        ),
                        Value::UInt(t.unexpected_total),
                        Value::UInt(self.cfg.max_unexpected_words),
                    ),
                    m,
                ));
            }
        }
        RuleResult::findings(findings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::bundled_asset;
    use tpt_app_media_qc_model::inspection::{Inspection, SpeakerTurn, TranscriptMeasurements};
    use tpt_app_media_qc_model::severity::VerdictDecision;

    fn voice() -> VoiceMeasurements {
        VoiceMeasurements {
            stream_idx: 1,
            method: "test".into(),
            analysed_ms: 60_000,
            speech_analysed: true,
            speech_regions: vec![TimeRange::new(5_000, 55_000)],
            speech_ms: 50_000,
            longest_non_speech: Some(TimeRange::new(0, 5_000)),
            speaker_turns: vec![SpeakerTurn {
                speaker: "S0".into(),
                start_ms: 5_000,
                end_ms: 55_000,
                confidence: 0.9,
            }],
            speaker_count: Some(2),
            speaker_changes: 6,
            transcript: Some(TranscriptMeasurements {
                source: "sidecar".into(),
                expected_words: 100,
                hypothesis_words: 100,
                substitutions: 10,
                deletions: 0,
                insertions: 0,
                wer: 0.1,
                unexpected_words: vec!["bananas".into()],
                unexpected_total: 1,
            }),
        }
    }

    fn run(yaml: &str, voice: Option<VoiceMeasurements>) -> RuleResult {
        let profile = Profile::from_yaml(&format!("name: t\nrules:\n  voice:\n{yaml}")).unwrap();
        let mut rules = Vec::new();
        build(&profile, &mut rules);
        let inspection = Inspection {
            voice: voice.into_iter().collect(),
            ..Default::default()
        };
        rules[0].execute(&RuleContext {
            asset: bundled_asset(),
            inspection: &inspection,
        })
    }

    #[test]
    fn everything_is_inconclusive_without_measurements() {
        for yaml in [
            "    speech: {expect: present}",
            "    silence: {max_non_speech_ms: 1000}",
            "    speakers: {max: 2}",
            "    speaker_changes: {max_per_minute: 5}",
            "    transcript: {max_wer: 0.2}",
        ] {
            let r = run(yaml, None);
            assert_eq!(
                r.findings[0].status,
                VerdictDecision::Inconclusive,
                "{yaml}"
            );
        }
    }

    #[test]
    fn speech_present_and_absent() {
        assert!(run(
            "    speech: {expect: present, min_ratio: 0.5}",
            Some(voice())
        )
        .is_pass());
        let r = run(
            "    speech: {expect: absent, min_ratio: 0.5}",
            Some(voice()),
        );
        assert_eq!(r.findings[0].status, VerdictDecision::Fail);
        assert!(r.findings[0].message.starts_with("[probabilistic]"));
    }

    #[test]
    fn long_non_speech_fails_with_its_time_range() {
        let r = run("    silence: {max_non_speech_ms: 2000}", Some(voice()));
        assert_eq!(r.findings[0].time_range, Some(TimeRange::new(0, 5_000)));
        assert!(run("    silence: {max_non_speech_ms: 5000}", Some(voice())).is_pass());
    }

    #[test]
    fn speaker_count_and_change_rate() {
        assert!(run("    speakers: {min: 1, max: 2}", Some(voice())).is_pass());
        assert!(!run("    speakers: {max: 1}", Some(voice())).is_pass());
        assert!(run("    speaker_changes: {max_per_minute: 6}", Some(voice())).is_pass());
        assert!(!run("    speaker_changes: {max_per_minute: 5}", Some(voice())).is_pass());
    }

    #[test]
    fn transcript_checks_wer_and_unexpected_words_independently() {
        let r = run("    transcript: {max_wer: 0.2}", Some(voice()));
        assert_eq!(
            r.findings.len(),
            1,
            "unexpected word still fails by default"
        );
        assert!(r.findings[0].message.contains("bananas"));
        let r = run(
            "    transcript: {max_wer: 0.05, max_unexpected_words: 1}",
            Some(voice()),
        );
        assert_eq!(r.findings.len(), 1);
        assert!(r.findings[0].message.contains("word error rate"));
    }

    #[test]
    fn transcript_only_measurements_leave_speech_rules_inconclusive() {
        let mut v = voice();
        v.speech_analysed = false;
        let r = run("    speech: {expect: present}", Some(v.clone()));
        assert_eq!(r.findings[0].status, VerdictDecision::Inconclusive);
        assert!(run(
            "    transcript: {max_wer: 0.5, max_unexpected_words: 5}",
            Some(v)
        )
        .is_pass());
    }
}
