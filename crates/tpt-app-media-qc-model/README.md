# tpt-app-media-qc-model

QC domain model for [TPT Media QC](../../README.md) (spec §6).

Pure, serializable data types shared by every layer: assets and streams,
findings and evidence, timecode/timebase arithmetic, severity and verdicts,
and the immutable report structure. The crate has no I/O and no rule logic —
it is the vocabulary the pipeline, rules, decode adapters, reports and store
all speak.

## Modules

- `asset` — `Asset`, `Stream`, `StreamKind`, content fingerprints (spec §6.1–§6.2).
- `inspection` — `Inspection` with container/video/audio/subtitle measurements.
- `finding` — `QcFinding`, `RuleId`, frame/time ranges.
- `evidence` — structured `Evidence` payloads attached to findings.
- `severity` — `Severity` and `VerdictDecision` (Pass/Warn/Fail/Inconclusive).
- `time` — `Rational`, `TimeBase`, `Timecode`, `FrameRate`, `DurationSeconds`.
- `value` — generic threshold value type used by custom rules.
- `report` — immutable `Report` with the five integrity fields (spec §14.1).

## Example

```rust
use tpt_app_media_qc_model::severity::VerdictDecision;
use tpt_app_media_qc_model::time::Rational;

let rate = Rational::new(30000, 1001).expect("valid rate");
assert!((rate.value() - 29.97).abs() < 0.01);
assert!(VerdictDecision::Pass < VerdictDecision::Fail);
```

## Spec references

- §6 (QC model), §6.5 (status per rule), §14.1 (report integrity).

## License

Dual-licensed MIT / Apache-2.0. See `LICENSE-MIT` and `LICENSE-APACHE` at the workspace root.
