# tpt-app-media-qc-store

SQLite persistence for [TPT Media QC](../../README.md) local state (spec §18).

Persists projects, assets, fingerprints, QC jobs, profiles, results,
findings, report metadata and user preferences in a single local SQLite
database (WAL mode, foreign keys, versioned schema migrations). Original
media is **never** stored — only paths and content fingerprints. Also
implements the per-rule analysis cache (spec §19): cached results are keyed
on asset fingerprint + application version + ruleset version + profile hash +
per-rule config hash, so retuning one rule invalidates only that rule.

## Modules

- `store` — `Store::open` / `Store::in_memory`, schema migration on open.
- `schema` — versioned migrations (`SCHEMA_VERSION`).
- `projects` / `assets` / `jobs` / `profiles` / `prefs` — entity tables.
- `cache` — per-rule analysis cache (spec §19).

## Example

```rust
let store = tpt_app_media_qc_store::Store::in_memory()?;
assert_eq!(store.schema_version(), tpt_app_media_qc_store::schema::SCHEMA_VERSION);
# Ok::<(), tpt_app_media_qc_store::error::Error>(())
```

## Spec references

- §18 (local state), §19 (analysis cache).

## License

Dual-licensed MIT / Apache-2.0. See `LICENSE-MIT` and `LICENSE-APACHE` at the workspace root.
