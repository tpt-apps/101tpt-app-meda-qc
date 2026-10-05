# tpt-app-media-qc-profile

Human-readable, versioned, deterministic QC profiles for
[TPT Media QC](../../README.md) (spec §9).

A profile selects rules and their thresholds. The typed `Profile` model is the
canonical form: `parse_str` validates the human YAML form, and the canonical
YAML serialization of that model is what gets hashed for profile integrity
and cache keys (spec §14.1, §19).

## Modules

- `model` — typed `Profile`, `Policy`, container/video/audio/subtitle rule configs.
- `parse` — YAML parsing and validation (`parse_str`, `ProfileError`).
- `hash` — `profile_sha256` and `per_rule_config_hash` for integrity and caching.
- `custom` — user-defined threshold rules evaluated against measurements.

## Example

```rust
use tpt_app_media_qc_profile::Profile;

let profile = Profile::default();
assert_eq!(profile.version, 1);
```

Bundled delivery profiles live in `profiles/` at the workspace root
(`generic`, `broadcast/*`, `streaming/*`); see
[profile format](../../docs/profile-format.md).

## Spec references

- §9 (profiles), §14.1 (integrity), §19 (cache keys).

## License

Dual-licensed MIT / Apache-2.0. See `LICENSE-MIT` and `LICENSE-APACHE` at the workspace root.
