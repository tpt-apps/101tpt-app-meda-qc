# TPT Media QC

Offline-first professional media quality-control and validation application.

TPT Media QC ingests media files, executes deterministic technical and
perceptual checks, presents failures at exact time/frame locations, and
produces auditable reports suitable for post-production, broadcast, OTT
delivery, archival, advertising and professional media workflows.

The product spec is the source of truth: [`spec.txt`](./spec.txt). It runs
entirely on the user's machine by default — no upload to TPT infrastructure and
no mandatory network access are required.

## Features

- **Content-based asset fingerprinting** — identity is SHA-256 of content, not
  path, so moved/replaced files are detected ([spec §6.1]).
- **Stream/container inspection** — ffprobe-backed metadata plus a Kinetix-backed
  AV1/VP9 video decode pass (MP4/ISO-BMFF, Matroska/WebM and MPEG-TS) and a
  Cadence-backed standalone audio decode pass for WAV, AIFF/AIFC and FLAC. The
  video pass measures frame rate, black/freeze/duplicate segments, corrupt-frame
  errors and luma statistics; the audio pass measures silence, clipping, sample
  peak, stereo phase and DC offset. H.264 is deliberately **not** decoded (AVC
  patent licensing): those assets keep full metadata/container QC and their
  frame-decode rules report `Inconclusive`.
- **Rule-based QC profiles** — human-readable, versioned, deterministic YAML profiles ([spec §9]).
- **Container, video and audio rules** — readability, validity, duration/bitrate
  consistency, timecode/timebase/timestamp continuity, resolution, frame rate,
  aspect ratio, luma range, colour-space metadata, scan format/field order,
  sample rate, bit depth,
  channel layout, silence, clipping, true peak, loudness (EBU R128 / ATSC A/85 /
  BS.1770) and phase. Decode coverage is explicit: unsupported or incomplete
  measurements produce `Inconclusive` findings rather than guessed passes.
- **Deterministic, auditable reports** — JSON, HTML, CSV and PDF export with the
  five integrity fields: asset SHA-256, profile SHA-256, application version,
  ruleset version and analysis ID ([spec §14.1]).
- Native Tauri 2 desktop application with dashboard, file/folder/drag-and-drop
  import, local job queue controls, asset inspection, finding/timeline views and
  JSON/HTML/CSV/PDF report export.
- **First-class CLI** with a stable exit-code contract ([spec §16]) and an
  isolated per-asset failure model so one corrupt file never aborts a batch
  ([spec §21]).

## TPT Media QC Desktop

The Windows desktop application lives in
[`crates/tpt-app-media-qc-tauri`](./crates/tpt-app-media-qc-tauri). It provides
the dashboard, file/folder and drag-and-drop import, bounded local queue,
pause/resume/cancel/retry/report actions, asset inspection, finding evidence and
timeline markers, and report export. It reuses the CLI's inspection and QC
engine; no analysis is duplicated in the frontend.

Development and bundling:

```sh
# Run the native shell
cargo run --manifest-path crates/tpt-app-media-qc-tauri/Cargo.toml --bin tpt-media-qc-desktop

# Build a local release bundle
cargo tauri build --config crates/tpt-app-media-qc-tauri/tauri.conf.json
```

Pushing a `vX.Y.Z` tag that matches the Tauri configuration version runs
[`.github/workflows/release.yml`](./.github/workflows/release.yml). It builds
unsigned Windows, macOS and Linux bundles on native runners and attaches them
to a draft GitHub Release. The workflow does not sign, notarize or upload to
Gumroad; those remain launch gates in [`GUMROAD.md`](./GUMROAD.md).

`ffprobe` must be on `PATH` for full and metadata scans. The current release is
unsigned until a signing certificate and Windows release configuration are
approved.

### Optional local API

The desktop process exposes a small HTTP/1.1 automation API only when
`TPT_MEDIA_QC_API=1` is set. It binds to an ephemeral port on `127.0.0.1` and
prints the selected address at startup; leaving the variable unset is the
default. The endpoints are `GET /health`, `GET /profiles`, `POST /jobs`,
`GET /jobs/:id`, `GET /jobs/:id/results`, and `POST /jobs/:id/cancel`.
A job request is local JSON such as
`{"asset":"C:/media/episode.mov","profile":"generic","quick":false}`.
Cancellation is cooperative at job boundaries. The API has no upload endpoint,
does not bind externally, and is not a replacement for OS-level authentication
or sandboxing.

## Quick start

```sh
# Inspect a file
tpt-media-qc info episode-01.mov

# Run QC with the generic profile
tpt-media-qc check --profile profiles/generic/generic.yaml --json out.json episode-01.mov

# Batch-analyse an incoming folder, writing one JSON report per asset
tpt-media-qc batch --profile profiles/generic/generic.yaml --input ./incoming --output ./reports

# Compare two deliveries (metadata, streams, duration, frame rate, resolution,
# codec, audio layout, loudness); exit 0 = match, 1 = minor, 2 = major differences
tpt-media-qc compare master_v1.mov master_v2.mov --json diff.json

# List the rules a profile enables
tpt-media-qc list-rules --profile profiles/generic/generic.yaml
```

Exit codes are stable per spec §16: `0` = PASS, `1` = WARN, `2` = FAIL,
`3` = INCONCLUSIVE, `4` = CONFIGURATION_ERROR, `5` = INPUT_ERROR, `6` =
INTERNAL_ERROR.

Full scans use `ffprobe` for the metadata pass, the pinned TPT Kinetix AV1 and
VP9 decoders for MP4/ISO-BMFF, Matroska/WebM and MPEG-TS video, and pinned TPT
Cadence readers for standalone WAV, AIFF/AIFC and FLAC audio. The current Kinetix
demuxers are in-memory, so that video adapter refuses inputs over 512 MiB.
Embedded audio, true-peak, BS.1770 loudness and undecoded video codecs
(H.264, HEVC, ProRes…) remain explicitly unsupported or `Inconclusive` until
their complete decode/measurement paths are integrated.

## Repository layout

```
Cargo.toml            Workspace manifest (dual-licensed)
docs/                 Architecture, formats and standards documentation
crates/
  tpt-app-media-qc-core      Shared primitives, error type, fingerprinting
  tpt-app-media-qc-model     QC domain model (Asset, Stream, Finding, Report)
  tpt-app-media-qc-rules     Rule framework and built-in rule catalogue
  tpt-app-media-qc-pipeline  Inspection boundary, engine, scheduler, verdict
  tpt-app-media-qc-profile   YAML profile parsing and deterministic hashing
  tpt-app-media-qc-report    JSON / HTML / CSV / PDF report generation
  tpt-app-media-qc-decode    Kinetix video and Cadence audio measurement adapters
  tpt-app-media-qc-cli       Command-line application
  tpt-app-media-qc-tauri     Native Tauri desktop application
  tpt-app-media-qc-test      Shared test utilities and golden-test harness
profiles/             Bundled and example QC profiles
tests/                Fixtures, integration, golden and performance suites
```

## Documentation

- [Architecture](./docs/architecture.md)
- [QC model](./docs/qc-model.md)
- [Profile format](./docs/profile-format.md)
- [Report format](./docs/report-format.md)
- [Supported formats](./docs/supported-formats.md)
- [Standards architecture](./docs/standards.md)
- [Performance](./docs/performance.md)
- [Definition of Done status](./docs/definition-of-done.md)

## Licensing and copyright

TPT Media QC is dual-licensed under the MIT License and the Apache License,
Version 2.0. Copyright © 2026 **TPT Solutions**. You may use this software
under either licence; see [`LICENSE-MIT`](./LICENSE-MIT) and
[`LICENSE-APACHE`](./LICENSE-APACHE).

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this project is licensed under the terms above.

TPT Media QC is the commercial application tier of the TPT Apps family. It
reuses the open-source TPT media foundations (`tpt-kinetix`, `tpt-cadence`,
`tpt-dsp`, `tpt-visual`, `tpt-voice`, `tpt-av-asset`, `tpt-av-test`); product
business logic stays in this repository ([spec §23]).