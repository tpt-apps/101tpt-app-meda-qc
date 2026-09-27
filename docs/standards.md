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

## 2. Current and planned standard profiles

The currently supported standards configuration lives in
[`profiles/`](../profiles/): EBU R128, ATSC A/85 and a BS.1770-derived
streaming profile. BT.1702 and other video standards remain roadmap items:
they require interlace/field-order measurements and corresponding delivery
rules before they can be represented honestly.

```
profiles/
├── generic/        # general-purpose default
├── broadcast/      # EBU R128 and ATSC A/85 delivery profiles
├── streaming/      # OTT-oriented checks (VOD/CEG R128, targets below -23)
└── examples/       # starter templates for new customers
```

## 3. Compliance policy

We **do not claim formal compliance** with any industry standard until the
implementation has been validated against appropriate reference material / test
suites (spec §25). Until then, profile documentation and reports describe the
measured values and the configured thresholds, not conformance.

Measurement implementation notes:

- Loudness uses BS.1770-4-style gating (integrated, momentary, short-term) as
  realised by `tpt-dsp` once integrated.
- True peak is estimated with adequate oversampling per BS.1770-4 Annex.
- Phase and DC-offset thresholds are heuristic defaults (profile-configurable)
  rather than normative requirements.

## 4. Where the code lives

- Standard constants and enums: `tpt-app-media-qc-profile::model`
  (`LoudnessStandard`, `LoudnessRule`).
- Measurement fields: `tpt-app-media-qc-model::inspection`
  (`AudioMeasurements`).
- Rule evaluation: `tpt-app-media-qc-rules::audio`.
- Bundled example profiles: [`profiles/`](../profiles/).