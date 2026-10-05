# tpt-app-media-qc-rules

QC rule framework and built-in rule catalogue for
[TPT Media QC](../../README.md) (spec §7).

Rules are components: they declare identity, description and capabilities,
and execute against a `RuleContext` (asset plus `Inspection` measurements).
Rules emit findings only for **non-pass** outcomes — pass is the absence of
findings. Rules must report `Inconclusive` rather than guessing on missing
measurements (spec §3.4). `Capabilities` routes each rule to the metadata-only
(pass 1) or decode (pass 2) phase.

## Rule families

- `container` — readability, validity, duration/bitrate consistency, timestamp continuity.
- `video` — resolution, frame rate, aspect ratio, luma range, colour-space/HDR
  signalling, scan format/field order, black/freeze/duplicate, corrupt frames,
  dead pixels, photosensitivity screening (general-flash only; spatial patterns
  not assessed, no compliance claimed).
- `audio` — sample rate, bit depth, channel layout, silence, clipping, true
  peak, EBU R128 / ATSC A/85 / BS.1770 loudness, phase, DC offset.
- `subtitle` — presence, language (ISO 639), timing, content, duration match.
- `custom` — user threshold rules over measurements.

## Example

```rust
use tpt_app_media_qc_profile::Profile;
use tpt_app_media_qc_rules::{build_rules, known_rule_ids};

let profile = Profile::default();
let rules = build_rules(&profile);
assert!(!known_rule_ids().is_empty());
let _ = rules.len();
```

Decode coverage is explicit: unsupported or undecoded codecs (H.264, HEVC,
ProRes, …) yield `Inconclusive` findings, never guessed passes.

## Spec references

- §3.4 (Inconclusive over guessing), §7 (rules), §8 (measurements), §9 (profiles).

## License

Dual-licensed MIT / Apache-2.0. See `LICENSE-MIT` and `LICENSE-APACHE` at the workspace root.
