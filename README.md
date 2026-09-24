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
- **Stream/container inspection** — ffprobe-backed metadata plus a
  Kinetix-backed H.264/MP4 decode pass. The first decode integration measures
  frame rate, black/freeze/duplicate segments, corrupt-frame errors and luma
  statistics.
- **Rule-based QC profiles** — human-readable, versioned, deterministic YAML profiles ([spec §9]).
- **Container, video and audio rules** — readability, validity, duration/bitrate
  consistency, timecode/timebase/timestamp continuity, resolution, frame rate,
  aspect ratio, luma range, colour-space metadata, sample rate, bit depth,
  channel layout, silence, clipping, true peak, loudness (EBU R128 / ATSC A/85 /
  BS.1770) and phase. Decode coverage is explicit: unsupported or incomplete
  measurements produce `Inconclusive` findings rather than guessed passes.
- **Deterministic, auditable reports** — JSON, HTML, CSV and PDF export with the
  five integrity fields: asset SHA-256, profile SHA-256, application version,
  ruleset version and analysis ID ([spec §14.1]).
- **First-class CLI** with a stable exit-code contract ([spec §16]) and an
  isolated per-asset failure model so one corrupt file never aborts a batch
  ([spec §21]).

## Quick start

```sh
# Inspect a file
tpt-media-qc info episode-01.mov

# Run QC with the generic profile
tpt-media-qc check --profile profiles/generic/generic.yaml --json out.json episode-01.mov

# Batch-analyse an incoming folder, writing one JSON report per asset
tpt-media-qc batch --profile profiles/generic/generic.yaml --input ./incoming --out ./reports

# List the rules a profile enables
tpt-media-qc list-rules --profile profiles/generic/generic.yaml
```

Exit codes are stable per spec §16: `0` = PASS, `1` = WARN, `2` = FAIL,
`3` = INCONCLUSIVE, `4` = CONFIGURATION_ERROR, `5` = INPUT_ERROR, `6` =
INTERNAL_ERROR.

Full scans use `ffprobe` for the metadata pass and the pinned TPT Kinetix
H.264 decoder for MP4/ISO-BMFF video. The current Kinetix demuxer is
in-memory, so the adapter refuses inputs over 512 MiB; audio decode through
`tpt-cadence` and non-H.264 video codecs remain future work.

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
  tpt-app-media-qc-decode    Kinetix MP4/H.264 video measurement adapter
  tpt-app-media-qc-cli       Command-line application
  tpt-app-media-qc-tauri     Desktop application shell (stub)
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