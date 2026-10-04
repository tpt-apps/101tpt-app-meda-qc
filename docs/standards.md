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

## 3. Current and planned standard profiles

The currently supported standards configuration lives in
[`profiles/`](../profiles/): EBU R128, ATSC A/85, a BS.1770-derived
streaming profile and the BT.1700/BT.1702-flavoured SD profile.

```
profiles/
├── generic/        # general-purpose default
├── broadcast/      # EBU R128, ATSC A/85 and BT.1702-style SD delivery profiles
├── streaming/      # OTT-oriented checks (VOD/CEG R128, targets below -23)
└── examples/       # starter templates for new customers
```

## 4. Compliance policy

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
- Phase and DC-offset thresholds are heuristic defaults (profile-configurable)
  rather than normative requirements.

## 5. Where the code lives

- Standard constants and enums: `tpt-app-media-qc-profile::model`
  (`LoudnessStandard`, `LoudnessRule`).
- Measurement fields: `tpt-app-media-qc-model::inspection`
  (`AudioMeasurements`).
- Rule evaluation: `tpt-app-media-qc-rules::audio`.
- Bundled example profiles: [`profiles/`](../profiles/).