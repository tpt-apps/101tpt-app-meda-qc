# Standards Architecture

Standards configuration is implemented as **profiles and rules, not
hard-coded application logic** (spec §25). Standards whose measurement and
delivery coverage is not implemented remain explicitly marked as roadmap work.
The application distinguishes four layers so a change in one never leaks into
another:

1. **Measurement algorithm** — how a quantity is computed (e.g. integrated
   loudness from BS.1770 gating, true-peak oversampling).
2. **Standard** — the normative document's thresholds and definitions
   (EBU R128 → `-23 LUFS`, ATSC A/85 → `-24 LKFS`, …).
3. **Profile** — which rules are active and at what severity.
4. **Customer tolerance** — a delivery spec that narrows a standard.

## 1. Loudness standards

The profile format's `audio.loudness` rule selects the measurement standard:

| Standard | Key | Nominal target | Notes |
|----------|-----|----------------|-------|
| EBU R128 | `ebu-r128` | `-23 LUFS` | Default; ±1 LU tolerance |
| ATSC A/85 | `atsc-a85` | `-24 LKFS` | North-American broadcast |
| ITU-R BS.1770 | `bs1770` | `-18 LUFS` | Measurement base; used with a custom target |

```yaml
audio:
  loudness:
    standard: ebu-r128
    target_lufs: -23
    tolerance_lu: 1
    severity: error
```

The chosen standard, target and tolerance are attached as JSON evidence to
every loudness finding; the report also embeds the exact profile SHA-256. This
records how the configured threshold was applied without claiming that the
measurement algorithm has been validated for formal compliance (spec §8.6).

## 2. Video standards and scanning format

Interlace/field-order measurements are implemented as metadata-driven
`field_order` observations on video streams, and the delivery side as the
`video.scan_format` profile rule (`scan`: progressive/interlaced/any;
`field_order`: top_field_first/bottom_field_first/any). The rule attaches
measured and expected values to every finding and reports `Inconclusive`
when the field order is not signalled, rather than guessing.

A BT.1700/BT.1702-flavoured SDTV delivery profile
([`profiles/broadcast/bt-1702.yaml`](../profiles/broadcast/bt-1702.yaml))
uses these checks for a 576i25 interlaced, top-field-first delivery with
EBU R128 audio. Additional video standards can now be layered on the same
rule as profile configuration.

## 3. HDR / dynamic-range signalling

Dynamic range is measured from **signalled metadata only** — the `HdrMetadata`
recorded on `VideoMeasurements` by the probe front-end (transfer, primaries,
matrix, sample range, ST 2086 mastering display, MaxCLL/MaxFALL, Dolby Vision
flag) — and judged by the `video.hdr` profile rule (`mode`: `sdr`, `hdr10`/PQ,
`hlg`, or `hdr` for either). Findings record the signalled transfer as the
measured value and the expected mode as the expected value, and an unsignalled
transfer reports `Inconclusive` rather than assuming SDR.

```yaml
video:
  hdr:
    mode: hdr10
    require_static_metadata: true
    max_cll_nits: 4000
    max_fall_nits: 1000
    severity: error
```

For HDR content the rule additionally expects BT.2020 primaries and matrix,
limited (`tv`) range and at least 10-bit samples; for PQ with
`require_static_metadata` it expects an ST 2086 mastering display plus both
MaxCLL and MaxFALL. MaxFALL above MaxCLL, a mastering minimum luminance at or
above its maximum, and the optional `max_cll_nits`/`max_fall_nits` ceilings are
always checked when the values are signalled.

An HDR10 streaming delivery profile
([`profiles/streaming/hdr10.yaml`](../profiles/streaming/hdr10.yaml)) configures
these checks for 3840×2160 PQ/BT.2020 content and is bundled in the desktop app
profile list.

Limitations (spec §25): this is **signalling validation, not HDR compliance
certification**. Light levels are read from the container's static metadata and
are never re-measured from decoded pixels, so content that mislabels its own
signalling is not detected. Dynamic metadata (Dolby Vision RPU, HDR10+) is only
*detected* and recorded — its content is not validated. The profile's thresholds
are commonly-used delivery values, not a normative requirement of any HDR
specification.

## 4. Current and planned standard profiles

The currently supported standards configuration lives in
[`profiles/`](../profiles/): EBU R128, ATSC A/85, a BS.1770-derived
streaming profile, the BT.1700/BT.1702-flavoured SD profile and an HDR10
streaming profile.

```
profiles/
├── generic/        # general-purpose default
├── broadcast/      # EBU R128, ATSC A/85 and BT.1702-style SD delivery profiles
├── streaming/      # OTT-oriented checks (VOD/CEG R128, targets below -23; HDR10 delivery)
└── examples/       # starter templates for new customers
```

## 5. Compliance policy

We **do not claim formal compliance** with any industry standard until the
implementation has been validated against appropriate reference material / test
suites (spec §25). Until then, profile documentation and reports describe the
measured values and the configured thresholds, not conformance.

Measurement implementation notes:

- Integrated loudness is measured with the BS.1770-4 two-stage K-weighting
  filter over 400 ms gating blocks stepping every 100 ms, with the absolute
  (−70 LUFS) and relative (−10 LU) gates of §5, in
  `crates/tpt-app-media-qc-decode/src/loudness.rs`. EBU R128, ATSC A/85 and
  plain BS.1770 all specify this measurement; they differ in *target*, which the
  profile sets. A file with no complete 400 ms block, or one entirely below the
  absolute gate, reports no loudness rather than a compliant value.
- True peak is measured by 4× oversampling through a polyphase
  Kaiser-windowed sinc reconstruction filter (the BS.1770-4 Annex 2 minimum), so
  intersample overshoot is visible. The meter reads the peak of the band-limited
  reconstruction, which for an isolated impulse sits *below* the sample peak.
- `video.photosensitivity` follows the Harding / ITU-R BT.1702 method: per-pixel relative luminance (YCbCr to RGB with BT.709, or BT.601 for SD, studio range, 2.2 gamma), pixel transitions of at least 0.1 with the darker state below 0.8, a screen-level transition when those pixels cover 25 % of the 10° visual field (341×256 of 1024×768, ≈ 2.8 % of the picture), and a saturated-red test (R/(R+G+B) ≥ 0.8, change in (R−G−B)·320 of at least 20). General and red flashes are judged separately against the flashes-per-second limit. Approximations: analysis runs on a ≤128×96 sampled grid, the visual field is a fixed picture fraction rather than a viewing-distance calculation, frame rate is not compensated, full-range/BT.2020/HDR sources are treated as studio-range BT.709, and spatial-pattern hazards are not assessed. It is a pre-screen, not a certified PSE test.
- Loudness range (EBU Tech 3342) uses 3 s short-term windows, a −70 LUFS
  absolute gate, a −20 LU relative gate and the 10th–95th percentile spread;
  audio shorter than 3 s leaves `loudness_range_lu` unmeasured.
- The measurement code is cross-checked in tests against `ffmpeg -af ebur128`
  reference readings for tone, mono/stereo and gated signals.
- Stuck-pixel analysis (`video.dead_pixels`, spec § 8.4) compares every luma cell
  across decoded frames and flags three signatures: a cell that never brightens
  past 8 in *bright* frames (dead), one that never darkens below 247 in *dark*
  frames (stuck), and one that reaches both extremes (flicker). Requiring both
  bright and dark frames is what keeps static content — letterbox bars, borders,
  a night sky — from being reported as a defect, because those pixels are only
  ever judged against frames whose own level makes the comparison meaningful; at
  least 3 frames of each kind are required before anything is judged. Flagged
  cells are grouped into 4-connected clusters (largest first, capped at 64) and
  reported in **source** pixels so a defect can be located even when the grid
  was reduced. Limits: levels are fixed heuristics rather than a published
  standard, the comparison is on 8-bit luma only (no chroma/subpixel defects),
  chroma-subsampled formats analyse the luma plane alone, and pictures above
  ~4.2 M cells are stride-sampled (the measurement then records
  `resolution_limited`, which a profile can treat as `Inconclusive`). Only the
  decoded picture is examined — defects already baked into the source file are
  indistinguishable from genuine sensor defects after encoding.
- Perceptual metrics (`video.blockiness`, `video.blur`, `video.noise`, spec § 8.4)
  are **no-reference**: all three come from the decoded luma plane with no source
  or reference frame.
  - *Blockiness* is the classic Wang/Bovik ratio — mean gradient **across**
    8-pixel transform-block boundaries ÷ mean gradient **inside** blocks, taken
    horizontally and vertically and averaged. ~1.0 means block structure is
    indistinguishable from picture detail. Frames whose mean interior gradient
    is below 0.5 codes carry no evidence and are excluded; `min_evidence_share`
    then decides whether the remaining frames justify a verdict, so flat content
    reports `Inconclusive` rather than a clean pass.
  - *Blur* is the mean absolute Laplacian normalised by `4 × 255`. It is checked
    against a **minimum** and is inherently content-dependent: a soft-focus or
    deliberately shallow-depth-of-field shot is legitimately soft.
  - *Noise* is the RMS deviation from a 5×5 box-smoothed copy, measured only
    where that smoothed picture is still flat (3×3 spread ≤ 3 codes). Smoothing
    first matters: judging flatness on raw pixels would classify a heavily noisy
    area as "not flat" and under-report exactly the noise an operator needs to
    see.
  - Empirically calibrated on AV1 encodes of detailed synthetic content at
    640×360: blockiness rises 1.41 → 1.43 → 1.54 and sharpness falls
    0.00486 → 0.00474 → 0.00399 as CRF goes 10 → 28 → 45, confirming the metrics
    track compression damage rather than scene content. Noise stayed near 1.1
    codes across the same range, since heavier quantisation smooths noise away.
  - Limits: the 8-pixel block size assumes 4:2:0 (4:4:4 content is measured on
    its true structure, which will read differently), only the luma plane is
    examined, pictures above ~2.5 M cells are box-averaged (at most a factor of
    four) before measuring, and the remaining §8.4 artefacts — ringing, banding
    and image corruption — are not yet implemented. No compliance with a
    published image-quality standard is claimed.
- Phase and DC-offset thresholds are heuristic defaults (profile-configurable)
  rather than normative requirements.
- Subtitle cue timing (spec §8.7) comes from packet headers, not a decode pass,
  so `subtitle.timing` and `subtitle.duration_match` run in the cheap metadata
  pass for every subtitle codec. A cue whose end is not after its start is
  counted as an invalid duration, and `max_gap_ms` measures the largest silence
  *between* cues rather than leading or trailing silence. Cue text is read only
  for text-based codecs; character counts are taken after stripping SRT/ASS/WebVTT
  markup, and a payload that is not valid UTF-8 counts as malformed rather than
  being silently skipped. No reading-rate or minimum-dwell-time standard is
  applied, and no compliance with any captioning specification is claimed.

## 6. Where the code lives

- Standard constants and enums: `tpt-app-media-qc-profile::model`
  (`LoudnessStandard`, `LoudnessRule`, `HdrMode`, `HdrRule`, `DeadPixelRule`,
  `Subtitle*Rule`).
- Measurement fields: `tpt-app-media-qc-model::inspection`
  (`AudioMeasurements`, `HdrMetadata`, `DeadPixelStats`, `SubtitleMeasurements`).
- Measurement algorithms: `tpt-app-media-qc-decode::deadpixels`, `::perception`,
  `::pse`, `::loudness` (`crates/tpt-app-media-qc-decode/src/`).
- Rule evaluation: `tpt-app-media-qc-rules::audio`, `tpt-app-media-qc-rules::video`,
  `tpt-app-media-qc-rules::subtitle`.
- Bundled example profiles: [`profiles/`](../profiles/).