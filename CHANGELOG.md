# Changelog

All notable changes to TPT Media QC are documented in this file, per
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). This project adheres
to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Cargo workspace with the crates `core`, `model`, `rules`, `pipeline`,
  `profile`, `report`, `cli`, `tauri` and `test`.
- Content-based asset fingerprinting (SHA-256), independent of file path.
- Container/stream inspection back-end (`ffprobe` probe boundary).
- QC domain model: `Asset`, `Stream`, `QcFinding`, `Evidence`, timecode,
  severity and the immutable report structure.
- Human-readable, versioned YAML QC profile format with strict parsing and
  deterministic profile hashing.
- Metadata QC rules: readability, container validity, malformed metadata,
  stream presence/counts, duration consistency, bitrate, timecode presence,
  timebase, timestamp continuity and unexpected streams.
- Basic video QC rules (metadata/measurement-driven): corrupt/dropped frames,
  black frames, freeze frames, duplicate frames, frame-rate consistency,
  resolution, aspect ratio, luma range and colour-space/HDR metadata.
- Basic audio QC rules via the decode measurement pathway: sample rate, bit
  depth, channel layout, silence, clipping, peak/true peak, loudness
  (EBU R128 / ATSC A/85 / BS.1770), phase and DC offset. The pinned Cadence
  adapter decodes standalone WAV, AIFF/AIFC and FLAC into streaming PCM and
  measures silence, clipping, sample peak, stereo phase and DC offset. True
  peak, BS.1770 loudness and embedded audio remain `Inconclusive` until their
  measurement/decode paths are integrated.
- Result aggregation: finding normalization, per-rule status, status counts and
  QC verdict resolution.
- Job scheduler with cost-class concurrency (metadata vs. decode-based
  inspection).
- JSON, HTML, CSV and PDF report generation with the spec §14.1 integrity fields.
- PDF report generation (lopdf, base-14 fonts) with integrity header, findings
  table and automatic pagination; `check --pdf` writes it.
- SQLite persistence crate (`tpt-app-media-qc-store`): assets, projects, jobs,
  findings, profiles, preferences, report metadata and the spec §19 per-rule
  analysis cache whose keys include the individual rule configuration hash.
- CLI with `check`, `batch`, `info` and `list-rules` commands and the stable
  spec §16 exit-code contract.
- Watch-folder automation (`watch` command, spec §13): recursively monitors an
  input folder, runs each new media file against a profile and routes the
  verdict to configurable pass/warn/fail folders with optional per-asset JSON
  reports; pre-existing files are scanned on start and copies are moved after
  write stabilisation.
- Error-handling hardening: isolated per-job failure state; corrupt assets do
  not terminate a batch (spec §21).
- Golden-media test suite (spec §24.2): deterministic measurement fixtures in
  `tests/fixtures/` paired with golden manifests in `tests/golden/` covering
  every built-in MVP rule; runner lives in `tpt-app-media-qc-test`
  (`MEDIA_QC_UPDATE_GOLDEN=1` regenerates manifests).
- End-to-end pipeline integration tests (engine → report → JSON/HTML/CSV/PDF
  round-trip) in `tpt-app-media-qc-test/tests/`.
- `result_parser` fuzz target for the machine-readable result/report parser
  (spec §24.4), completing the `fuzz/` target set: `ffprobe_json`,
  `profile_parse`, `clap_args`, `report_generation`, `media_boundary`,
  `result_parser`.
- `qc-bench` micro-benchmark harness and committed baseline
  (`tests/performance/baseline.md`) covering fingerprinting, profile parsing,
  rule building/execution, engine checks, fixture deserialization and
  report rendering (spec §20). Representative HD/UHD *media* benchmarks follow.
- Kinetix integration through the pinned `tpt-kinetix-demux` and
  `tpt-kinetix-h264` revisions: MP4/ISO-BMFF H.264 video is demuxed and decoded
  into streaming black/freeze/duplicate/luma/frame-rate measurements. The
  adapter enforces a 512 MiB input bound while the foundation demuxer is
  in-memory, and reports unsupported/incomplete coverage explicitly.
- Cadence integration through the pinned `tpt-av-cadence-wav`,
  `tpt-av-cadence-aiff` and `tpt-av-cadence-flac` revisions: standalone audio
  files decode in bounded blocks into silence, clipping, sample-peak,
  stereo-phase and DC-offset measurements. Standards-accurate true peak and
  BS.1770 loudness remain explicitly unmeasured.
- Native Tauri 2 desktop application: dashboard, local media/folder import,
  drag-and-drop, bounded job queue controls, asset inspector, finding evidence,
  timeline markers, and JSON/HTML/CSV/PDF report export.
- Dual MIT / Apache-2.0 licensing, packaging and repository documentation.

### Changed

- None yet.

### Deprecated

- None yet.

### Removed

- None yet.

### Fixed

- The ffprobe parser accepts numeric and string values emitted by current
  FFmpeg releases, and preserves discovered stream metadata for container,
  video and audio rules.
- The Kinetix MP4 adapter bounds reads to the selected video track's declared
  sample count so multi-track inputs cannot repeat the first video sample after
  track exhaustion.
- Restored the container-problems golden fixture's decoded-video coverage
  marker so empty decode results are not mistaken for a completed scan.

### Security

- None yet.