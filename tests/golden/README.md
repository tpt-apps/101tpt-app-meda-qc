# Golden test expectations

Expected results for the media fixtures in [`../fixtures/`](../fixtures/)
(spec §24.2). Every fixture must have an expected result.

## Manifest format (planned)

Follows the spec §16 machine-readable shape loosely; one golden entry per
fixture:

```json
{
  "fixture": "black/10s-black-on-25fps.mov",
  "profile": "generic",
  "expected_verdict": "fail",
  "expected_statuses": {
    "video.black_frames": "fail"
  },
  "allow_critical_empty": false
}
```

Field meaning:

- `expected_verdict` — the run's final verdict (`pass` | `warn` | `fail` |
  `inconclusive`).
- `expected_statuses` — the minimum set of per-rule verdicts that must hold,
  keyed by rule id. Rules not listed are unconstrained.
- `allow_critical_empty` — set for fixtures that are placeholders.

The golden runner (to be implemented with the fixture set) runs `generic` plus
any profile named in the manifest and compares against expectations. Verdict
and per-rule status comparisons are order-independent; findings are compared by
rule id + status, not by message text, so thresholds can evolve without churn.

## Golden coverage target

Before MVP is declared complete, golden media must cover **every production
rule** (spec §30): readability, container validity, stream presence, duration
consistency, bitrate, timecode, timestamp continuity, resolution, frame rate,
aspect ratio, black frames, freeze frames, duplicate frames, corrupt frames,
luma range, colour space, sample rate, bit depth, channel layout, silence,
clipping, peak, true peak, loudness and phase.