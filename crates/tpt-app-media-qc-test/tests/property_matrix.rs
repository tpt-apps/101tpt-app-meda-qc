//! Deterministic property-style regression coverage (spec §24.3).
//!
//! A fixed-seed generator supplies several hundred cases per property. This
//! keeps the test deterministic in CI while covering the boundary-heavy paths
//! that example-based tests can miss: timecode, frame/range arithmetic,
//! thresholds, profile parsing and result aggregation.

use tpt_app_media_qc_model::asset::{Asset, AssetFingerprint, Stream};
use tpt_app_media_qc_model::finding::{FrameRange, QcFinding, RuleId, TimeRange};
use tpt_app_media_qc_model::inspection::{Inspection, VideoMeasurements};
use tpt_app_media_qc_model::severity::{Severity, VerdictDecision};
use tpt_app_media_qc_model::time::{Rational, Timecode};
use tpt_app_media_qc_pipeline::aggregate;
use tpt_app_media_qc_profile::model::{DurationThresholdRule, Profile, RuleSetConfig, VideoRules};
use tpt_app_media_qc_rules::{Capabilities, QcRule, RuleDescription, RuleResult};
use tpt_app_media_qc_test::{run_fixture, MediaFixture};

const CASES: usize = 512;

/// A small xorshift generator keeps these property checks dependency-free and
/// reproducible across platforms.
struct Cases(u64);

impl Cases {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn bounded(&mut self, upper: u64) -> u64 {
        if upper == 0 {
            0
        } else {
            self.next() % upper
        }
    }
}

fn sample_asset() -> Asset {
    Asset {
        id: Default::default(),
        path: "property-fixture.mp4".into(),
        fingerprint: AssetFingerprint {
            sha256: "ab".repeat(32),
            size_bytes: 1,
        },
        size_bytes: 1,
        modified_time: None,
        duration: None,
        streams: vec![Stream::primary_video(0)],
    }
}

fn threshold_fixture(duration_ms: u64) -> MediaFixture {
    MediaFixture {
        asset: sample_asset(),
        inspection: Inspection {
            video: vec![VideoMeasurements {
                stream_idx: 0,
                decoded_frame_count: Some(1),
                black_frames: vec![TimeRange::new(0, duration_ms)],
                ..Default::default()
            }],
            ..Default::default()
        },
    }
}

fn threshold_profile(max_duration_ms: u64) -> Profile {
    Profile {
        name: "property-threshold".into(),
        rules: RuleSetConfig {
            video: VideoRules {
                black_frames: Some(DurationThresholdRule {
                    max_duration_ms,
                    severity: Severity::Error,
                }),
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    }
}

struct TestRule {
    rule_id: &'static str,
    findings: Vec<QcFinding>,
}

impl QcRule for TestRule {
    fn id(&self) -> RuleId {
        RuleId::new(self.rule_id)
    }

    fn description(&self) -> RuleDescription {
        RuleDescription {
            name: "Property test rule",
            summary: "Exercises aggregation.",
            version: "1",
        }
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::metadata_only()
    }

    fn run(&self, _context: &tpt_app_media_qc_rules::RuleContext<'_>) -> RuleResult {
        RuleResult::findings(self.findings.clone())
    }
}

#[test]
fn rational_order_and_roundtrip_never_overflow() {
    let mut cases = Cases::new(0x4d595df4d0f33173);
    for _ in 0..CASES {
        let a = Rational::new(cases.bounded(u64::MAX), cases.bounded(u64::MAX).max(1)).unwrap();
        let b = Rational::new(cases.bounded(u64::MAX), cases.bounded(u64::MAX).max(1)).unwrap();
        assert_eq!(Rational::parse(&a.to_string()), Some(a));
        let exact_order =
            (u128::from(a.num) * u128::from(b.den)).cmp(&(u128::from(b.num) * u128::from(a.den)));
        assert_eq!(a.cmp(&b), exact_order);
        assert_eq!(a.cmp(&a), std::cmp::Ordering::Equal);
    }
}

#[test]
fn timecode_and_frame_indices_preserve_frame_numbering() {
    let mut cases = Cases::new(0x9e3779b97f4a7c15);
    let rates = [
        Rational::from_parts(24, 1),
        Rational::from_parts(25, 1),
        Rational::from_parts(30, 1),
        Rational::from_parts(50, 1),
        Rational::from_parts(60, 1),
        Rational::from_parts(30000, 1001),
        Rational::from_parts(60000, 1001),
    ];
    for rate in rates {
        for _ in 0..CASES {
            let frames = cases.bounded(1_000_000);
            let timecode = Timecode::new(frames, rate, false);
            assert_eq!(timecode.display_frame_index(), Some(frames));
            let reconstructed = (timecode.as_seconds() * rate.value()).round() as u64;
            assert_eq!(reconstructed, frames, "rate {rate}, frame {frames}");
        }
    }

    for rate in [
        Rational::from_parts(30000, 1001),
        Rational::from_parts(60000, 1001),
    ] {
        for _ in 0..CASES {
            let frame = cases.bounded(100_000);
            let current = Timecode::new(frame, rate, true)
                .display_frame_index()
                .expect("supported SMPTE drop-frame rate");
            let next = Timecode::new(frame + 1, rate, true)
                .display_frame_index()
                .expect("supported SMPTE drop-frame rate");
            assert!(next > current, "drop-frame indexes must be monotonic");
        }
    }
    assert_eq!(
        Timecode::new(17_982, Rational::from_parts(30000, 1001), true).to_smpte(),
        "00:10:00;00"
    );
    assert_eq!(
        Timecode::new(35_964, Rational::from_parts(60000, 1001), true).to_smpte(),
        "00:10:00;00"
    );
}

#[test]
fn range_durations_are_monotonic_and_saturating() {
    let mut cases = Cases::new(0x243f6a8885a308d3);
    for _ in 0..CASES {
        let start = cases.bounded(1_000_000_000);
        let end = start + cases.bounded(10_000);
        assert_eq!(TimeRange::new(start, end).duration_ms(), end - start);
        assert_eq!(FrameRange::new(start, end).length(), end - start);
        assert_eq!(TimeRange::new(end, start).duration_ms(), 0);
        assert_eq!(FrameRange::new(end, start).length(), 0);
    }
}

#[test]
fn duration_thresholds_fail_only_above_their_limit() {
    let mut cases = Cases::new(0x13198a2e03707344);
    for _ in 0..CASES {
        let limit = cases.bounded(2_000);
        let duration = cases.bounded(2_001);
        let run = run_fixture(&threshold_fixture(duration), &threshold_profile(limit));
        let expected = if duration > limit {
            VerdictDecision::Fail
        } else {
            VerdictDecision::Pass
        };
        assert_eq!(
            run.per_rule[0].best, expected,
            "limit {limit}, duration {duration}"
        );
        assert_eq!(run.verdict, expected);
    }
}

#[test]
fn generated_profiles_parse_and_roundtrip_through_the_typed_model() {
    let mut cases = Cases::new(0xa4093822299f31d0);
    for _ in 0..CASES {
        let min_bps = cases.bounded(u32::MAX as u64);
        let tolerance = cases.bounded(10_000) as f64 / 10_000.0;
        let source = format!(
            "name: generated\nversion: 1\nrules:\n  container:\n    bitrate:\n      min_bps: {min_bps}\n  video:\n    aspect_ratio:\n      expected: 16/9\n      tolerance: {tolerance:.4}\n"
        );
        let profile = Profile::from_yaml(&source).expect("generated valid profile");
        assert_eq!(profile.rules.container.bitrate.unwrap().min_bps, min_bps);
        assert_eq!(
            profile.rules.video.aspect_ratio.unwrap().tolerance,
            tolerance
        );
        let json = serde_json::to_string(&profile).unwrap();
        let decoded: Profile = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, profile);
    }
    assert!(Profile::from_yaml("name: [").is_err());
    assert!(
        Profile::from_yaml("name: x\nrules:\n  container:\n    bitrate:\n      min_bps: -1\n")
            .is_err()
    );
}

#[test]
fn aggregation_selects_the_worst_status_and_preserves_counts() {
    let mut cases = Cases::new(0x082efa98ec4e6c89);
    let all_statuses = [
        VerdictDecision::Pass,
        VerdictDecision::Warn,
        VerdictDecision::Inconclusive,
        VerdictDecision::Fail,
    ];
    for _ in 0..CASES {
        let count = 1 + cases.bounded(12) as usize;
        let findings: Vec<_> = (0..count)
            .map(|index| {
                QcFinding::new("property.rule")
                    .status(all_statuses[cases.bounded(4) as usize])
                    .set_message(format!("case {index}"))
            })
            .collect();
        let rules: Vec<Box<dyn QcRule>> = vec![Box::new(TestRule {
            rule_id: "property.rule",
            findings: findings.clone(),
        })];
        let (per_rule, counts) = aggregate(&rules, &findings);
        let expected = if findings.iter().any(|f| f.status == VerdictDecision::Fail) {
            VerdictDecision::Fail
        } else if findings
            .iter()
            .any(|f| f.status == VerdictDecision::Inconclusive)
        {
            VerdictDecision::Inconclusive
        } else if findings.iter().any(|f| f.status == VerdictDecision::Warn) {
            VerdictDecision::Warn
        } else {
            VerdictDecision::Pass
        };
        assert_eq!(per_rule[0].best, expected);
        assert_eq!(per_rule[0].findings, count);
        assert_eq!(
            counts.pass + counts.warn + counts.fail + counts.inconclusive,
            count
        );
    }
}
