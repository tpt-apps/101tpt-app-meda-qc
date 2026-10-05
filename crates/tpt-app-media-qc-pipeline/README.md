# tpt-app-media-qc-pipeline

Analysis pipeline for [TPT Media QC](../../README.md) (spec §10, §11).

The pipeline owns the seam between probe front-ends (`Inspector`) and the QC
engine. It runs a profile's rules over an asset's `Inspection`, aggregates
findings into a per-asset `QcRun`, and resolves a verdict from the profile
policy. It also provides the local job scheduler and file-to-file comparison.

## Modules

- `inspector` — `Inspector` trait, `InspectionLevel`, `NoopInspector`, `arc` helper.
- `engine` — `QcEngine`: builds rules from a profile and runs them over inspections.
- `verdict` — `VerdictPolicy` / `VerdictResolution` (fail-on severity mapping).
- `scheduler` — bounded `Scheduler` / `run_jobs` over cost-classified jobs (spec §11).
- `job` — `Job` unit of scheduled work.
- `compare` — file-to-file delivery comparison with minor/major significance.
- `aggregate` / `run_rules` — per-rule status roll-up and shared rule runner.

## Example

```rust
use std::sync::Arc;
use tpt_app_media_qc_pipeline::{arc, NoopInspector, QcEngine};
use tpt_app_media_qc_profile::Profile;

let profile = Arc::new(Profile::default());
let engine = QcEngine::new(profile, arc(NoopInspector));
assert!(engine.rules.is_empty());
```

## Spec references

- §6.5 (per-rule status), §10 (analysis pipeline), §11 (scheduler), §16 (verdicts/exit codes).

## License

Dual-licensed MIT / Apache-2.0. See `LICENSE-MIT` and `LICENSE-APACHE` at the workspace root.
