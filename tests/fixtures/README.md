# Measurement fixtures

Deterministic inputs for the golden and integration suites (spec §24.2, §24.5).
Each `*.json` file is a **measurement fixture**: an `Asset` paired with the
`Inspection` a probe front-end would have produced for it (`MediaFixture` in
`tpt-app-media-qc-test`). Rules consume measurements, not raw media bytes
(spec §3.5), so these fixtures exercise the complete rule engine offline — no
encoded media, no `ffprobe` install. The harness lives in
[`crates/tpt-app-media-qc-test`](../../crates/tpt-app-media-qc-test/).

## Fixtures

| fixture               | scenario                                                                                                                       | golden verdict |
|-----------------------|--------------------------------------------------------------------------------------------------------------------------------|----------------|
| `clean-master`        | fully in-spec master (H.264 1080p25 + 48 kHz/24-bit stereo)                                                                     | pass           |
| `container-problems`  | corrupt container, malformed metadata, duration/bitrate/timecode/timebase/timestamp violations, unexpected `data` stream, no audio | fail           |
| `video-defects`       | wrong resolution/frame rate/aspect/colour, black/freeze/duplicate/corrupt events, out-of-legal luma                             | fail           |
| `audio-defects`       | wrong sample rate/bit depth/layout, silence, clipping, over-limit peak/true-peak, off-target loudness, out-of-phase, DC offset  | fail           |
| `boundary-thresholds` | every threshold value **exactly at its limit** (thresholds compare strictly `>`, so these pass)                                  | pass           |
| `unscanned`           | streams present but no probe/decode measurements — rules report `Inconclusive`, never guess (spec §3.4)                          | warn           |

The all-rules `golden-suite.yaml` profile enables every built-in rule so each
golden manifest pins the whole catalogue. Note: when a stream had decode
errors, black/freeze segment rules report `Inconclusive` even when no segment
exceeds the limit (segment detection may be incomplete) — that is why
`boundary-thresholds` carries `decode_errors: 0` while `video-defects` does not.

Encoded-media fixtures (real `.mov`/`.mxf` files) will be added when the TPT
decode stack lands (todo §2/§8/§10); the golden runner accepts any
`Inspector`, so they plug in without runner changes. The originally planned
directory scheme (`valid/`, `corrupt/`, `loudness/`, …) then applies to those.

Provenance: only add media assets you have the right to include (own content
or permissive licence); record it in `ATTRIBUTION.md`. Keep fixtures small —
JSON measurement fixtures are checked in directly.
