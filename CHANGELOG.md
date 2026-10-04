# Changelog

All notable changes to TPT Media QC are documented in this file, per
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). This project adheres
to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `compare` command and `tpt_app_media_qc_pipeline::compare`: file-to-file
  comparison of container metadata, stream counts, codec, resolution, frame
  rate, scan order, colour space, audio layout, loudness/true peak and measured
  defect counts, with tolerances and minor/major significance. Visual and
  audio waveform differences are not compared.

- `video.photosensitivity` rule: general-flash (photosensitive-epilepsy) screening
  over decoded AV1/VP9 frames with a configurable flashes-per-second limit
  (default 3). Follows the Harding/BT.1702 method (per-pixel relative luminance,
  10° field area criterion, general and saturated-red flashes); spatial
  patterns are not assessed and compliance is not claimed.

- `video.hdr` rule: HDR/colorimetry **signalling** validation. Probe front-ends
  record transfer, primaries, matrix, sample range, ST 2086 mastering display,
  MaxCLL/MaxFALL and Dolby Vision presence in `HdrMetadata`, and the new
  `rules.video.hdr` profile key checks the expected dynamic range (`sdr`,
  `hdr10`/`pq`, `hlg` or `hdr`), BT.2020 signalling, limited range, ≥ 10-bit
  depth, static-metadata presence and per-profile `max_cll_nits`/`max_fall_nits`
  ceilings, with MaxFALL ≤ MaxCLL always enforced. An unsignalled transfer is
  `Inconclusive`. Light levels come from static metadata rather than measured
  pixels, and dynamic metadata (Dolby Vision RPU, HDR10+) is only detected, not
  validated; no compliance is claimed. Ships with a bundled
  `profiles/streaming/hdr10.yaml` delivery profile.

- Subtitle/caption validation (spec §8.7): five new rules —
  `subtitle.presence`, `subtitle.language`, `subtitle.timing`,
  `subtitle.content` and `subtitle.duration_match`. Cue timing is measured from
  packet headers and cue text from the packet payload, recorded in
  `SubtitleMeasurements`, so every rule runs in the cheap metadata pass without
  decoding picture or samples. Checks cover track presence, required ISO 639
  languages (loose matching, so `en` satisfies `eng`), cue overlaps, invalid
  durations, inter-cue gaps, cue dwell time, malformed and empty payloads,
  characters per line, lines per cue and subtitle coverage against the video
  duration. Character counts are taken after stripping SRT/ASS/WebVTT markup.
  Text-dependent checks require a text-based subtitle codec and report
  `Inconclusive` otherwise; cue analysis is capped at 100 000 cues per stream.
  Golden fixtures and manifests cover the new rules. The previous
  `subtitle.language`/`subtitle.missing_subtitles` placeholders are replaced by
  the structured `subtitle.*` keys above (breaking for any profile using them).

- `video.dead_pixels` rule: stuck/dead/flickering pixel detection (spec § 8.4)
  over decoded AV1/VP9 frames. Every luma cell is compared across the decode and
  flagged when it never brightens in bright shots (dead), never darkens in dark
  shots (stuck), or alternates between both extremes (flicker). Judging only
  against frames that provide contrast is what keeps static content such as
  letterbox bars from being reported as defects. Flagged cells are clustered
  4-connected and the largest cluster's position and size are attached to the
  finding. Profile keys `max_pixels`, `max_clusters`, `include_flicker` and
  `fail_on_limited_resolution`; analysis is capped at ~4.2 M cells per frame and
  stride-samples above that. Luma-only heuristics — no chroma/subpixel defect
  detection and no claim of compliance with a published standard.

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
- Kinetix integration through the pinned `tpt-kinetix-demux`,
  `tpt-kinetix-av1` and `tpt-kinetix-vp9` revisions: royalty-free AV1 and VP9
  video in MP4/ISO-BMFF and Matroska/WebM is demuxed and decoded (strict,
  pixel-exact decoders) into streaming black/freeze/duplicate/luma/frame-rate
  measurements, including 10/12-bit and monochrome frames. The adapter
  enforces a 512 MiB input bound while the foundation demuxers are in-memory,
  and reports unsupported/incomplete coverage explicitly.
- Cadence integration through the pinned `tpt-av-cadence-wav`,
  `tpt-av-cadence-aiff` and `tpt-av-cadence-flac` revisions: standalone audio
  files decode in bounded blocks into silence, clipping, sample-peak,
  stereo-phase and DC-offset measurements. Standards-accurate true peak
  (BS.1770-4 Annex 2 oversampling) and gated integrated loudness (BS.1770-4
  K-weighting) are now measured too; loudness range (EBU Tech 3342: 3 s
  short-term windows, −70 LUFS / −20 LU gates, 10th–95th percentile) is measured
  as well; audio shorter than 3 s reports it as unmeasured.
- **Standard-accurate audio levels:** true peak (BS.1770-4 Annex 2, 4×
  oversampling through a polyphase reconstruction filter) and gated integrated
  loudness (BS.1770-4 two-stage K-weighting, 400 ms blocks with the −70 LUFS
  absolute and −10 LU relative gates) are measured from decoded PCM, so the
  `audio.true_peak` and `audio.loudness` rules now evaluate real values instead
  of reporting `Inconclusive`. Cross-checked against `ffmpeg -af ebur128`
  reference readings in tests. 
- MPEG-TS video decode for broadcast/HLS assets: Kinetix's `TsDemuxer`
  (188-byte packets, PAT/PMT with registration descriptors) now feeds the same
  AV1/VP9 decoders, so `.ts`/HLS segments get black/freeze/duplicate/luma/
  frame-rate QC. Codec identity comes from the PMT registration descriptor
  (`AV01` / `vp09`); H.264 elementary streams stay unsupported. Audio inside a
  transport stream is demuxed but not decoded, so its silence/clipping/loudness
  findings remain `Inconclusive`.
- Native Tauri 2 desktop application: dashboard, local media/folder import,
  drag-and-drop, bounded job queue controls, asset inspector, finding evidence,
  timeline markers, and JSON/HTML/CSV/PDF report export.
- Optional localhost automation API (spec §17): disabled by default, enabled
  with `TPT_MEDIA_QC_API=1`, loopback-only ephemeral binding, bounded JSON
  requests, health/profile/job/result endpoints and cooperative cancellation.
- Tag-triggered unsigned Tauri release workflow (`.github/workflows/release.yml`)
  with a version guard, native Windows/macOS/Linux bundle matrix and draft
  GitHub Release assembly.
- Real-process CLI integration coverage for `info`/`check`, batch failure
  isolation (spec §21) and watch-folder routing (spec §13).
- Deterministic property-style test matrix for rational ordering, SMPTE
  timecode/frame indexing, range duration arithmetic, thresholds, profile parsing
  and result aggregation.
- Standards configuration evidence: loudness findings now record the selected
  standard, target and tolerance as structured evidence alongside the profile hash.
- Interlace/field-order measurement: probe front-ends record the video stream's
  scanning order and the new `video.scan_format` profile rule validates it
  (scan: progressive/interlaced, field order: top/bottom-field-first). An
  unsignalled field order is `Inconclusive` against a progressive expectation
  and a failure against interlaced/pinned-order deliveries.
- Bounded fuzz campaigns (60 s per target) for all six fuzz targets on every
  push/PR in CI, running on Linux where libFuzzer is supported.
- Definition of Done status audit (`docs/definition-of-done.md`) mapping each
  spec §30 item to its implementation evidence and remaining external actions.
- BT.1700/BT.1702-flavoured SDTV delivery profile (`profiles/broadcast/bt-1702.yaml`,
  576i25 interlaced top-field-first with EBU R128 audio), also bundled in the
  desktop app profile list.
- Dual MIT / Apache-2.0 licensing, packaging and repository documentation.

### Changed

- Media/profile paths are canonicalized at the application boundary, and report
  exporters stage output in RAII-managed temporary files before replacement.
- Deterministic time handling now uses wide rational comparison and records
  59.94 drop-frame boundaries without arithmetic underflow.

### Deprecated

- None yet.

### Removed

- H.264 video decoding (`tpt-kinetix-h264`) and its MP4 test fixture, to avoid
  AVC patent-licensing exposure (patent pools cover decoders as well as
  encoders). H.264, HEVC, ProRes and other undecoded video still receive full
  `ffprobe` metadata/container QC; their frame-decode rules report
  `Inconclusive` rather than guessing, and never a false `video.corrupt_frames`
  failure. AAC was never decoded, so no audio behaviour changes.

### Fixed

- `video.corrupt_frames` no longer fails an asset when nothing was decoded:
  with zero decoded frames the rule reports `Inconclusive` ("video decode
  produced no frames…") instead of counting the recorded decode errors against
  the profile's event limit. Decode errors are only counted as corrupt frames
  when at least one frame was actually reconstructed.
- The optional local API now forces accepted loopback sockets into blocking mode
  before parsing requests, preventing Windows connection resets during parallel
  health/profile/job tests.
- The ffprobe parser accepts numeric and string values emitted by current
  FFmpeg releases, and preserves discovered stream metadata for container,
  video and audio rules.
- The Kinetix MP4 adapter bounds reads to the selected video track's declared
  sample count so multi-track inputs cannot repeat the first video sample after
  track exhaustion.
- Restored the container-problems golden fixture's decoded-video coverage
  marker so empty decode results are not mistaken for a completed scan.

### Security

- Strict path validation covers media/profile inputs and desktop report export
  destinations; report writes are staged and replaced atomically.
- The optional local API refuses non-loopback binds, limits request headers and
  bodies, and is disabled unless explicitly enabled.