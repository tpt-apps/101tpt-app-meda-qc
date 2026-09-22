# Standards Architecture

Standards (EBU R128, ATSC A/85, ITU-R BS.1770, BT.1702, …) are implemented as
**profiles and rules, not hard-coded application logic** (spec §25). The
application distinguishes four layers so a change in one never leaks into
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

The chosen standard is stored with every result so reports record *how* the
number was obtained (spec §8.6).

## 2. Planned standard profiles

Layout envisioned under [`profiles/`](../profiles/) (following spec §25):

```
profiles/
├── generic/        # general-purpose default
├── broadcast/      # e.g. ATSC A/85, BT.1702-flavoured delivery checks
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