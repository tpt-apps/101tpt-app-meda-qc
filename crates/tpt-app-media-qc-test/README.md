# tpt-app-media-qc-test

Shared test utilities and golden-test harness for
[TPT Media QC](../../README.md) (spec §24).

This crate is the harness side of the workspace-root `tests/` suites:
deterministic measurement fixtures (`MediaFixture`: asset plus the
`Inspection` a probe would have produced), expected-result manifests
(`GoldenManifest`, spec §24.2) verified by the golden runner, and benchmark
baselines produced by the `qc-bench` binary (spec §20). Rules consume
measurements, not raw media bytes (spec §3.5), so pinning measurements
exercises the full rule engine without encoded media; encoded AV1/VP9 and
WAV coverage is generated in-test. The golden runner accepts any `Inspector`.

## Items

- `MediaFixture`, `FixtureInspector` — measurement fixtures and replay inspector.
- `GoldenManifest`, golden runner — manifest verification
  (`MEDIA_QC_UPDATE_GOLDEN=1` regenerates manifests; review the diff).
- `qc-bench` binary — performance baselines in `tests/performance/`.

## Example

```rust
use tpt_app_media_qc_test::UPDATE_GOLDEN_ENV;

assert_eq!(UPDATE_GOLDEN_ENV, "MEDIA_QC_UPDATE_GOLDEN");
```

Integration tests live in this crate's `tests/` directory so
`cargo test --workspace` runs them with the unit suites; see
[tests README](../../tests/README.md).

## Spec references

- §3.5 (measurements over bytes), §20 (performance), §24 (testing).

## License

Dual-licensed MIT / Apache-2.0. See `LICENSE-MIT` and `LICENSE-APACHE` at the workspace root.
