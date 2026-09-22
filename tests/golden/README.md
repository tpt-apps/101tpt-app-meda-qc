# Golden test expectations

Expected results for the fixtures in [`../fixtures/`](../fixtures/)
(spec §24.2). Every fixture must have an expected result.

## Manifest format (implemented)

One manifest per fixture × profile pairing, named `<fixture>.<profile>.json`:

```json
{
  "fixture": "container-problems",
  "profile": "golden-suite",
  "expected_verdict": "fail",
  "expected_statuses": {
    "container.readable": "fail",
    "container.bitrate": "fail"
  },
  "allow_critical_empty": false
}
```

Field meaning:

- `fixture` — the measurement fixture stem in `tests/fixtures/`.
- `profile` — profile name resolved from the bundled `profiles/` tree or
  `tests/fixtures/` (all-rules `golden-suite` for the full catalogue).
- `expected_verdict` — the run's final verdict (`pass` | `warn` | `fail` |
  `inconclusive`).
- `expected_statuses` — the minimum set of per-rule verdicts that must hold,
  keyed by rule id. Rules not listed are unconstrained; the committed
  manifests deliberately pin *every* enabled rule so any engine regression is
  caught.
- `allow_critical_empty` — set for fixtures that are placeholders; an empty
  status map is then advisory instead of an error.

The runner lives in `crates/tpt-app-media-qc-test/tests/golden_media.rs` and
executes with `cargo test --workspace`. Verdict and per-rule status
comparisons are order-independent; findings are compared by rule id + status,
not by message text, so thresholds can evolve without churn. Set
`MEDIA_QC_UPDATE_GOLDEN=1` to regenerate the manifests from current engine
output and review the diff before committing.

## Golden coverage target

Golden media covers **every production rule** (spec §30) — asserted by the
`golden_suite_covers_every_production_rule` test, which fails when a new
built-in rule is not pinned by at least one manifest: readability, container
validity, stream presence, duration consistency, bitrate, timecode, timebase,
timestamp continuity, unexpected streams, resolution, frame rate, aspect
ratio, black frames, freeze frames, duplicate frames, corrupt frames, luma
range, colour space, sample rate, bit depth, channel layout, silence,
clipping, peak, true peak, loudness, phase and DC offset.
