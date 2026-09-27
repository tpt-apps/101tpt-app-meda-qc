# Integration tests

End-to-end tests spanning the engine boundary and beyond (spec §24). The
engine-level suites are implemented and run with `cargo test --workspace` from
[`crates/tpt-app-media-qc-test/tests/`](../../crates/tpt-app-media-qc-test/tests/):

- `golden_media.rs` — the golden runner (see [`../golden/`](../golden/)) and
  the every-rule coverage assertion (spec §24.2/§30).
- `integration_engine.rs` — fixture inspector → QC engine → immutable report →
  JSON/HTML/CSV/PDF exports, including the JSON round-trip and the five
  integrity fields (spec §14.1); verdict expectations for every fixture;
  the `Inconclusive`-not-guessed guarantee for unscanned assets (spec §3.4).
- `property_matrix.rs` — fixed-seed property-style coverage for rational
  ordering, SMPTE timecode/frame indexing, range durations, strict thresholds,
  profile parsing and result aggregation.
- `crates/tpt-app-media-qc-tauri/src/local_api.rs` — loopback enforcement plus
  live health/profile/job/result tests using the existing local QC engine.
- `crates/tpt-app-media-qc-cli/tests/real_media.rs` — when `ffmpeg` and
  `ffprobe` are available, generate a deterministic WAV and exercise the real
  `info` and full `check` binary, including the Cadence audio decode path. The
  same target also verifies real-process batch failure isolation and watch-folder
  routing; those cases skip cleanly when the tools are unavailable.

Integration tests that need fixtures take them from
[`../fixtures/`](../fixtures/) with expectations in
[`../golden/`](../golden/).