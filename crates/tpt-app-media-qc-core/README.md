# tpt-app-media-qc-core

Shared primitives for TPT Media QC (part of the [TPT Media QC](../../README.md) workspace).

This crate holds the infrastructure every other crate depends on: application
identity and versioning (spec §14.1), the unified error type, content-based
asset fingerprinting (spec §6.1), scheduler cost classification (spec §11),
and path validation used at import/export boundaries.

## Modules

- `config` — `APP_NAME`, `APP_VERSION`, ruleset version for integrity fields (spec §14.1, §19).
- `error` — unified `Error`/`Result` type.
- `fingerprint` — SHA-256 content fingerprinting of assets (spec §6.1).
- `cost` — cheap/expensive measurement classification for scheduling (spec §11).
- `path` — strict path validation for media, profile and report paths.

## Optional features

- `sqlite` — SQLite type conversions for the store layer (`rusqlite`).

## Example

```rust
use std::io::Cursor;
use tpt_app_media_qc_core::{config, fingerprint::Fingerprint};

let version = config::app_display_version();
assert!(version.starts_with("TPT Media QC"));
let digest = Fingerprint::from_reader(Cursor::new(b"media-bytes"))?;
assert_eq!(digest.to_hex().len(), 64);
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Spec references

- §6.1 (content fingerprinting), §11 (scheduler cost), §14.1 (integrity fields), §19 (caching).

## License

Dual-licensed MIT / Apache-2.0. See `LICENSE-MIT` and `LICENSE-APACHE` at the workspace root.
