# Definition of Done status (spec §30)

Point-in-time audit of the spec §30 MVP readiness checklist. "Done" means the
capability is implemented and covered by tests in this repository; "external"
means it requires a human, hardware, or third-party action (signing keys,
real professional media, a clean machine) and cannot be closed from code
alone. Re-run this audit before declaring the MVP complete (todo item 28).

| # | Spec §30 item | Status | Evidence |
|---|---------------|--------|----------|
| 1 | Install without development tooling | **Partial** | `.github/workflows/release.yml` builds unsigned per-OS bundles on tag; `ffprobe` requirement is documented (`GUMROAD.md`, `docs/supported-formats.md`). Signing/notarization and an installer UX check need external credentials — see `GUMROAD.md`. |
| 2 | Drag a media file into the application | Done | Desktop import card + drag-and-drop overlay (`crates/tpt-app-media-qc-tauri/ui/app.js`), folder import, `MAX_IMPORT_FILES` bound. |
| 3 | Inspect all supported streams | Done (declared scope) | AV1/VP9 in MP4, Matroska/WebM and MPEG-TS via Kinetix (H.264 deliberately not decoded, patent licensing), WAV/AIFF/FLAC via Cadence, metadata via `ffprobe`; unsupported codecs are reported as such, never guessed (`docs/supported-formats.md`). |
| 4 | QC profile can be selected or created | Done | Profile selector in the desktop app, custom YAML profile picker, bundled `profiles/` tree with strict parser (`docs/profile-format.md`); creation is editing YAML in any editor by design (spec §9). |
| 5 | Complete QC job runs offline | Done | No network in the engine; the only optional listener is the loopback-only local API (`crates/tpt-app-media-qc-tauri/src/local_api.rs`). |
| 6 | Multiple files processed concurrently | Done | Cost-class scheduler with bounded concurrency and backpressure (`crates/tpt-app-media-qc-pipeline/src/scheduler.rs`). |
| 7 | Failure identifies exact time/frame | Done | `TimeRange`/`FrameRange` evidence on findings (black/freeze/duplicate/corrupt, timestamp gaps, silence). |
| 8 | Findings include measured and expected values | Done | `QcFinding.measured`/`expected` (`Value`) on threshold rules, incl. the scan-format rule's measured vs. expected field order. |
| 9 | Navigate from finding to evidence | Done | Evidence viewer + timeline markers in the desktop app (spec §12.5–12.6). |
| 10 | Export PDF and JSON reports | Done | JSON/HTML/CSV/PDF exporters with staged atomic writes and integrity fields (`crates/tpt-app-media-qc-report`). |
| 11 | CLI runs the same QC engine as the GUI | Done | `crates/tpt-app-media-qc-pipeline` engine shared by CLI, desktop app and local API. |
| 12 | Watch-folder processing works | Done | `watch` command (spec §13) with pass/warn/fail routing; exercised by real-process integration tests. |
| 13 | Corrupt file cannot crash the batch | Done | Per-job failure isolation; covered by `real_media.rs` batch-isolation test and `media_boundary` fuzz target. |
| 14 | Golden-media tests cover every production rule | Done | All-rules `golden-suite` profile + manifests pin verdict/status for the full catalogue (incl. `video.scan_format`), `tests/golden/`. |
| 15 | Fuzz testing covers parsers and media boundaries | Done | Six targets: `profile_parse`, `ffprobe_json`, `result_parser`, `report_generation`, `clap_args`, `media_boundary`; bounded campaigns run on every push/PR in CI (Linux — libFuzzer is not linkable on Windows/MSVC). Long-running scheduled campaigns remain optional hardening. |
| 16 | Reproducible from fingerprint + profile + versions | Done | Cache keys and report integrity fields: asset SHA-256, profile SHA-256, app version, ruleset version (`docs/report-format.md`, `docs/performance.md`). |
| 17 | No internet connection required | Done | Offline-first; no telemetry, no cloud upload (spec §22). |
| 18 | Clean-machine installation tested | **External** | Requires a pristine machine per OS with the produced bundles; blockers are signing (optional) and `ffprobe` availability (`GUMROAD.md`). |
| 19 | Performance benchmarked on representative HD/UHD | **Partial** | `qc-bench` micro-benchmarks with committed baseline (`tests/performance/baseline.md`); representative *media* benchmarks grow with decode coverage (AV1/VP9 decode benchmarks are follow-up work; other codecs capability-gated). |

## Remaining before MVP completion

1. External: signing/notarization decisions and artifacts (item 1) — `GUMROAD.md`.
2. External: clean-machine validation (item 18) and the private beta with real
   professional media (todo item 27).
3. Partial: representative HD/UHD media benchmarks as decode coverage widens (item 19).
4. Ongoing: regression fixture for every production bug found from the beta
   onward (spec §24.5).
