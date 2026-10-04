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
| `clean-master`        | fully in-spec master (H.264 1080p25 + 48 kHz/24-bit stereo + one `eng` subtitle track)                                          | pass           |
| `container-problems`  | corrupt container, malformed metadata, duration/bitrate/timecode/timebase/timestamp violations, unexpected `data` stream, no audio | fail           |
| `video-defects`       | wrong resolution/frame rate/aspect/colour, signalled top-field-first against a progressive expectation, black/freeze/duplicate/corrupt events, out-of-legal luma, 7 stuck/flickering pixels in 4 clusters, overlapping/invalid/empty/malformed subtitle cues and short subtitle coverage | fail           |
| `audio-defects`       | wrong sample rate/bit depth/layout, silence, clipping, over-limit peak/true-peak, off-target loudness, out-of-phase, DC offset  | fail           |
| `boundary-thresholds` | every threshold value **exactly at its limit** (thresholds compare strictly `>`, so these pass), including the subtitle limits  | pass           |
| `unscanned`           | streams present but no probe/decode measurements — rules report `Inconclusive`, never guess (spec §3.4)                          | warn           |

The all-rules `golden-suite.yaml` profile enables every built-in rule so each
golden manifest pins the whole catalogue. Note: when a stream had decode
errors, black/freeze segment rules report `Inconclusive` even when no segment
exceeds the limit (segment detection may be incomplete) — that is why
`boundary-thresholds` carries `decode_errors: 0` while `video-defects` does not.

Encoded-media coverage includes `encoded/vp9-clip.mp4` and
`encoded/vp9-clip.webm` (royalty-free VP9, generated with `libvpx-vp9` — see
`ATTRIBUTION.md`), which run through the pinned Kinetix demuxers and VP9 decoder
for both the MP4 and Matroska/WebM routes. AV1 coverage is generated in-test with
`tpt-kinetix-av1`'s `Av1Encoder` and wrapped in a minimal Matroska file, so no
AV1 media is checked in. Generated mono and stereo WAV fixtures also exercise
the Cadence streaming decode adapter, including silence, sample peak, phase,
DC offset and coverage reporting. Additional encoded fixtures (including HD/UHD
performance media) can be added as foundation coverage grows; the runner
continues to accept measurement fixtures for rule-level golden tests.

Provenance: only add media assets you have the right to include (own content
or permissive licence); record it in `ATTRIBUTION.md`. Keep fixtures small —
JSON measurement fixtures are checked in directly.
