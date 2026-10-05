# tpt-app-media-qc-tauri

Native Tauri 2 desktop application for [TPT Media QC](../../README.md)
(spec §12).

Dashboard, file/folder and drag-and-drop import, bounded local queue with
pause/resume/cancel/retry/report actions, asset inspection, finding evidence
and timeline markers, and JSON/HTML/CSV/PDF report export. The shell reuses
the CLI's inspection and QC engine — no analysis is duplicated in the
frontend. Media scanning runs on Tauri's blocking worker pool so the UI stays
responsive; path validation and staged atomic report writes match the CLI.
Ten delivery profiles are bundled (`generic`, `broadcast-ebu-r128`,
`broadcast-atsc-a85`, `broadcast-sd-576i`, `broadcast-uk-hd`, `streaming-vod`,
`streaming-hdr10`, `streaming-ott-premium`, `streaming-online-upload`,
`streaming-podcast-voice`); custom YAML profiles can be loaded from disk.

## Development and bundling

```sh
# Run the native shell
cargo run --manifest-path crates/tpt-app-media-qc-tauri/Cargo.toml --bin tpt-media-qc-desktop

# Build a local release bundle
cargo tauri build --config crates/tpt-app-media-qc-tauri/tauri.conf.json
```

Commands exposed to the frontend: `list_profiles`, `scan_asset`,
`expand_media_paths`, `export_report`, `open_path`. An optional loopback-only
local API (`TPT_MEDIA_QC_API`) is disabled unless explicitly enabled.

## Spec references

- §12 (desktop app), §13 (watch/queue), §14 (reports), §18 (local state).

## License

Dual-licensed MIT / Apache-2.0. See `LICENSE-MIT` and `LICENSE-APACHE` at the workspace root.
