//! User-defined declarative rules (`rules.custom`, spec § 27 "custom rule
//! builder"). Each rule compares one metadata metric with a threshold; a
//! missing metric is `Inconclusive`, never a guessed pass or fail.

use crate::rule::{Capabilities, QcRule, RuleContext, RuleDescription, RuleResult};
use crate::util::{container_scanned, fail, inconclusive};
use tpt_app_media_qc_model::asset::{Asset, Stream, StreamKind};
use tpt_app_media_qc_model::finding::RuleId;
use tpt_app_media_qc_model::value::Value;
use tpt_app_media_qc_profile::custom::{
    format_number, CustomOp, CustomOperand, CustomRule, CustomScope, CustomTarget, METADATA_PREFIX,
};
use tpt_app_media_qc_profile::model::Profile;

pub fn build(profile: &Profile, out: &mut Vec<Box<dyn QcRule>>) {
    for cfg in &profile.rules.custom {
        out.push(Box::new(CustomRuleImpl { cfg: cfg.clone() }));
    }
}

/// A resolved metric reading.
#[derive(Clone, Debug, PartialEq)]
enum Reading {
    Number(f64),
    Text(String),
}

impl Reading {
    fn label(&self) -> String {
        match self {
            Reading::Number(n) => format_number(*n),
            Reading::Text(t) => t.clone(),
        }
    }

    fn value(&self) -> Value {
        match self {
            Reading::Number(n) if n.fract() == 0.0 && *n >= 0.0 && *n < 1e15 => {
                Value::UInt(*n as u64)
            }
            Reading::Number(n) => Value::Float(*n),
            Reading::Text(t) => Value::Text(t.clone()),
        }
    }
}

fn container_reading(asset: &Asset, metric: &str) -> Option<Reading> {
    let count = |kind: StreamKind| asset.streams.iter().filter(|s| s.kind == kind).count() as f64;
    let n = match metric {
        "duration_seconds" => asset.duration?.as_duration().as_secs_f64(),
        "size_bytes" => asset.size_bytes as f64,
        "stream_count" => asset.streams.len() as f64,
        "video_stream_count" => count(StreamKind::Video),
        "audio_stream_count" => count(StreamKind::Audio),
        "subtitle_stream_count" => count(StreamKind::Subtitle),
        _ => return None,
    };
    Some(Reading::Number(n))
}

fn stream_reading(stream: &Stream, metric: &str) -> Option<Reading> {
    if let Some(key) = metric.strip_prefix(METADATA_PREFIX) {
        return stream
            .metadata
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| Reading::Text(v.clone()));
    }
    let text = |v: &Option<String>| v.clone().map(Reading::Text);
    let num = |v: Option<u64>| v.map(|n| Reading::Number(n as f64));
    match metric {
        "codec" => text(&stream.codec),
        "codec_profile" => text(&stream.codec_profile),
        "language" => text(&stream.language),
        "pixel_format" => text(&stream.pixel_format),
        "channel_layout" => text(&stream.channel_layout),
        "field_order" => stream.field_order.map(|f| Reading::Text(f.as_str().into())),
        "bitrate" => num(stream.bitrate),
        "width" => num(stream.width),
        "height" => num(stream.height),
        "channels" => num(stream.channels),
        "sample_rate" => num(stream.sample_rate),
        "bit_depth" => num(stream.bit_depth),
        "frame_rate" => stream.frame_rate.map(|r| Reading::Number(r.value())),
        "duration_seconds" => stream
            .duration
            .map(|d| Reading::Number(d.as_duration().as_secs_f64())),
        _ => None,
    }
}

struct CustomRuleImpl {
    cfg: CustomRule,
}

impl CustomRuleImpl {
    /// Whether a reading satisfies the rule.
    fn holds(&self, reading: &Reading) -> bool {
        let cfg = &self.cfg;
        match reading {
            Reading::Number(m) => {
                let operands = cfg.value.iter().filter_map(|o| match o {
                    CustomOperand::Number(n) => Some(*n),
                    CustomOperand::Text(_) => None,
                });
                let tol = cfg.tolerance.max(1e-9);
                let mut operands = operands;
                match cfg.op {
                    CustomOp::Eq => operands.next().is_some_and(|n| (m - n).abs() <= tol),
                    CustomOp::Ne => operands.next().is_some_and(|n| (m - n).abs() > tol),
                    CustomOp::Lt => operands.next().is_some_and(|n| *m < n),
                    CustomOp::Le => operands.next().is_some_and(|n| *m <= n),
                    CustomOp::Gt => operands.next().is_some_and(|n| *m > n),
                    CustomOp::Ge => operands.next().is_some_and(|n| *m >= n),
                    CustomOp::OneOf => operands.any(|n| (m - n).abs() <= tol),
                    CustomOp::NoneOf => !operands.any(|n| (m - n).abs() <= tol),
                }
            }
            Reading::Text(m) => {
                let mut matches = cfg.value.iter().any(|o| match o {
                    CustomOperand::Text(t) => t.eq_ignore_ascii_case(m),
                    CustomOperand::Number(_) => false,
                });
                match cfg.op {
                    CustomOp::Eq | CustomOp::OneOf => matches,
                    CustomOp::Ne | CustomOp::NoneOf => {
                        matches = !matches;
                        matches
                    }
                    // Rejected by the profile parser for text metrics.
                    _ => false,
                }
            }
        }
    }

    fn expected(&self) -> Value {
        let labels: Vec<String> = self.cfg.value.iter().map(CustomOperand::label).collect();
        let list = if self.cfg.op.is_set() {
            format!("[{}]", labels.join(", "))
        } else {
            labels.join("")
        };
        Value::Text(format!("{} {}", self.cfg.op.symbol(), list))
    }

    fn evaluate(
        &self,
        subject: &str,
        stream: Option<&Stream>,
        reading: Option<Reading>,
    ) -> Option<tpt_app_media_qc_model::finding::QcFinding> {
        let id = self.id();
        let sev = self.cfg.severity;
        let Some(reading) = reading else {
            let mut f = inconclusive(
                &id,
                sev,
                format!("{subject}: metric '{}' is not available", self.cfg.metric),
            );
            if let Some(s) = stream {
                f = f.stream(s.index);
            }
            return Some(f);
        };
        if self.holds(&reading) {
            return None;
        }
        let expected = self.expected();
        let message = match &self.cfg.message {
            Some(m) => format!(
                "{subject}: {m} ({} was {})",
                self.cfg.metric,
                reading.label()
            ),
            None => format!(
                "{subject}: {} is {}, expected {}",
                self.cfg.metric,
                reading.label(),
                match &expected {
                    Value::Text(t) => t.clone(),
                    _ => String::new(),
                }
            ),
        };
        let mut f = fail(&id, sev, message, reading.value(), expected);
        if let Some(s) = stream {
            f = f.stream(s.index);
        }
        Some(f)
    }
}

impl QcRule for CustomRuleImpl {
    fn id(&self) -> RuleId {
        RuleId::new(self.cfg.id.clone())
    }

    fn description(&self) -> RuleDescription {
        RuleDescription {
            name: "Custom rule",
            summary: "User-defined metric comparison from the profile's rules.custom list.",
            version: "0.1",
        }
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::metadata_only()
    }

    fn run(&self, ctx: &RuleContext<'_>) -> RuleResult {
        let cfg = &self.cfg;
        if cfg.scope == CustomScope::Container {
            if !container_scanned(ctx.inspection) {
                return RuleResult::from_finding(inconclusive(
                    &self.id(),
                    cfg.severity,
                    "container was not inspected (metadata scan returned no container data)",
                ));
            }
            let reading = container_reading(ctx.asset, &cfg.metric);
            return RuleResult::findings(
                self.evaluate("container", None, reading)
                    .into_iter()
                    .collect(),
            );
        }

        let kind = match cfg.scope {
            CustomScope::Video => StreamKind::Video,
            CustomScope::Audio => StreamKind::Audio,
            _ => StreamKind::Subtitle,
        };
        let limit = match cfg.streams {
            CustomTarget::All => usize::MAX,
            CustomTarget::Primary => 1,
        };
        let findings = ctx
            .streams_of(kind)
            .take(limit)
            .filter_map(|s| {
                let subject = format!("{} stream {}", cfg.scope.as_str(), s.index);
                self.evaluate(&subject, Some(s), stream_reading(s, &cfg.metric))
            })
            .collect();
        RuleResult::findings(findings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::bundled_asset;
    use tpt_app_media_qc_model::inspection::{ContainerValidity, Inspection};
    use tpt_app_media_qc_model::severity::{Severity, VerdictDecision};

    fn scanned() -> Inspection {
        let mut i = Inspection::default();
        i.container.validity = ContainerValidity::Ok;
        i
    }

    fn rule(yaml: &str) -> CustomRuleImpl {
        let profile = Profile::from_yaml(&format!("name: t\nrules:\n  custom:\n{yaml}")).unwrap();
        CustomRuleImpl {
            cfg: profile.rules.custom[0].clone(),
        }
    }

    fn run(r: &CustomRuleImpl, inspection: &Inspection) -> RuleResult {
        r.execute(&RuleContext {
            asset: bundled_asset(),
            inspection,
        })
    }

    #[test]
    fn numeric_threshold_passes_and_fails() {
        let ok =
            rule("    - {id: custom.br, scope: video, metric: bitrate, op: '>=', value: 10000000}");
        assert!(run(&ok, &scanned()).is_pass());

        let bad = rule("    - {id: custom.br, scope: video, metric: bitrate, op: '>=', value: 20000000, severity: warning}");
        let r = run(&bad, &scanned());
        assert_eq!(r.findings.len(), 1);
        let f = &r.findings[0];
        assert_eq!(f.status, VerdictDecision::Fail);
        assert_eq!(f.severity, Severity::Warning);
        assert_eq!(f.measured, Some(Value::UInt(15_000_000)));
        assert!(f.message.contains(">= 20000000"), "{}", f.message);
        assert_eq!(f.stream_idx.map(|s| s.as_u64()), Some(0));
    }

    #[test]
    fn text_metrics_compare_case_insensitively() {
        let ok = rule(
            "    - {id: custom.codec, scope: video, metric: codec, op: in, value: [H264, prores]}",
        );
        assert!(run(&ok, &scanned()).is_pass());
        let bad = rule(
            "    - {id: custom.codec, scope: video, metric: codec, op: not_in, value: [h264]}",
        );
        assert!(!run(&bad, &scanned()).is_pass());
    }

    #[test]
    fn container_scope_uses_asset_totals() {
        let r = rule("    - {id: custom.streams, scope: container, metric: stream_count, op: '==', value: 2}");
        assert!(run(&r, &scanned()).is_pass());
        let r = rule("    - {id: custom.len, scope: container, metric: duration_seconds, op: '<', value: 60, message: Too long}");
        let out = run(&r, &scanned());
        assert!(out.findings[0].message.contains("Too long"));
    }

    #[test]
    fn unscanned_container_is_inconclusive() {
        let r = rule(
            "    - {id: custom.size, scope: container, metric: size_bytes, op: '>', value: 1}",
        );
        let out = run(&r, &Inspection::default());
        assert_eq!(out.findings[0].status, VerdictDecision::Inconclusive);
    }

    #[test]
    fn missing_metric_is_inconclusive_not_failed() {
        // The bundled video stream has no language.
        let r =
            rule("    - {id: custom.lang, scope: video, metric: language, op: '==', value: eng}");
        let out = run(&r, &scanned());
        assert_eq!(out.findings[0].status, VerdictDecision::Inconclusive);
    }

    #[test]
    fn no_matching_streams_is_a_pass() {
        let r =
            rule("    - {id: custom.sub, scope: subtitle, metric: codec, op: '==', value: subrip}");
        assert!(run(&r, &scanned()).is_pass());
    }

    #[test]
    fn tolerance_applies_to_numeric_equality() {
        let r = rule("    - {id: custom.fps, scope: video, metric: frame_rate, op: '==', value: 25.01, tolerance: 0.05}");
        assert!(run(&r, &scanned()).is_pass());
        let r = rule("    - {id: custom.fps, scope: video, metric: frame_rate, op: '==', value: 29.97, tolerance: 0.05}");
        assert!(!run(&r, &scanned()).is_pass());
    }
}
