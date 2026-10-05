# TPT Media QC — Project Todo

Source of truth: [`spec.txt`](./spec.txt). Product: **TPT Media QC** (repo `tpt-app-media-qc`),
offline-first professional media QC application. Dual-licensed MIT / Apache-2.0, copyright TPT Solutions.

---

## Phase 0 — Repository & Licensing Setup

- [x] Initialize Cargo workspace (`Cargo.toml`) with crates:
  - [x] `tpt-app-media-qc-core`
  - [x] `tpt-app-media-qc-model`
  - [x] `tpt-app-media-qc-rules`
  - [x] `tpt-app-media-qc-pipeline`
  - [x] `tpt-app-media-qc-profile`
  - [x] `tpt-app-media-qc-report`
  - [x] `tpt-app-media-qc-store`
  - [x] `tpt-app-media-qc-cli`
  - [x] `tpt-app-media-qc-tauri`
  - [x] `tpt-app-media-qc-test` (stub)
- [x] Create `README.md`
- [x] Create `CHANGELOG.md`
- [x] Create `CONTRIBUTING.md`
- [x] Create `docs/` skeleton:
  - [x] `docs/architecture.md`
  - [x] `docs/qc-model.md`
  - [x] `docs/profile-format.md`
  - [x] `docs/report-format.md`
  - [x] `docs/supported-formats.md`
  - [x] `docs/standards.md`
  - [x] `docs/performance.md`
- [x] Create `profiles/` skeleton: `generic/`, `broadcast/`, `streaming/`, `examples/`
- [x] Create `tests/` skeleton: `fixtures/`, `integration/`, `golden/`, `performance/` (dirs + README placeholders only — no fixtures/tests populated yet, see Phase 1 item 22)
- [x] Licensing:
  - [x] Add `LICENSE-MIT` file
  - [x] Add `LICENSE-APACHE` file
  - [x] Set `license = "MIT OR Apache-2.0"` in workspace `Cargo.toml`
  - [x] Set copyright holder as "TPT Solutions" in license headers/files
  - [x] Add licensing section to `README.md`

---

## Phase 1 — MVP

Ordered per spec §31 (Recommended Implementation Order), scoped per spec §26 (MVP).

1. [x] Establish Cargo workspace and application shell
2. [x] Integrate `tpt-kinetix` and enumerate media capabilities — pinned Kinetix
      demux/decode crates are wired through the decode crate; current full-decode
      coverage is MP4/ISO-BMFF, Matroska/WebM and MPEG-TS with AV1/VP9 video
      (royalty-free; no H.264 decoder ships), with other formats explicitly
      reported as unsupported/incomplete
3. [x] Implement asset fingerprinting (content-based, not path-based — spec §6.1)
4. [x] Implement stream/container inspection (probe boundary + `FfprobeInspector` front-end)
5. [x] Implement the QC domain model (`Asset`, `Stream`, `QcFinding`, `Evidence` — spec §6)
6. [x] Implement profile parsing (human-readable YAML profile format — spec §9)
7. [x] Implement metadata QC rules (container checks — spec §8.1: readability, validity,
      malformed metadata, stream count/duration consistency, bitrate, timecode, timebase,
      timestamp continuity, unexpected/missing streams)
8. [x] Implement video decode pipeline (via Kinetix) — MP4/ISO-BMFF,
      Matroska/WebM and MPEG-TS AV1/VP9 packets are decoded and reduced to
      streaming black/freeze/duplicate, corrupt-frame, luma and frame-rate
      measurements; other codecs remain capability-gated
9. [x] Implement basic video QC rules (metadata/measurement-driven; decode coverage
    is explicit and unsupported measurements report Inconclusive):
   - [x] corrupt/dropped frame detection (detection pathway defined)
   - [x] black frames
   - [x] freeze frames
   - [x] duplicate frames
   - [x] frame-rate consistency
   - [x] resolution
   - [x] aspect ratio
   - [x] luma range
   - [x] colour-space / HDR metadata
   - [x] scanning order / field order (`video.scan_format`)
10. [x] Implement audio decode through `tpt-cadence` — pinned WAV, AIFF/AIFC and
      FLAC readers stream PCM into explicit silence, clipping, sample-peak,
      stereo-phase and DC-offset measurements; embedded MP4 audio and
      standards-accurate true-peak/loudness remain capability-gated
11. [x] Implement DSP-based audio QC rules (decode coverage is explicit;
    unsupported or unmeasured values report Inconclusive):
    - [x] sample rate
    - [x] bit depth
    - [x] channel layout
    - [x] silence / unexpected silence
    - [x] clipping
    - [x] peak / true peak
    - [x] loudness (configurable standard/profile)
    - [x] phase
12. [x] Implement result aggregation (Result/Event model, finding normalization, severity
      evaluation, QC verdict)
13. [x] Implement SQLite persistence (spec §18: projects, assets, fingerprints, jobs,
      profiles, results, findings, report metadata, preferences)
14. [x] Implement job scheduler (cost classes, concurrency, bounded memory, backpressure,
      cancellation, priority, resumability — spec §11; cost-class concurrency done,
      cancellation/persistence pending with §13)
15. [x] Implement desktop queue UI (Tauri) — native Tauri 2 shell with dashboard,
       local import/queue, and report actions; native build launches with
       installed FFmpeg available on `PATH`:
   - [x] Dashboard (spec §12.1)
   - [x] Import — drag-and-drop, file picker, folder/recursive import (spec §12.2)
   - [x] Job Queue — columns, pause/resume/cancel/retry/open report (spec §12.3)
16. [x] Implement timeline / evidence viewer and finding inspector (spec §12.5–12.6)
17. [x] Implement Asset Inspector screen (spec §12.4)
18. [x] Implement JSON/HTML/CSV reporting (spec §14, report integrity fields §14.1)
19. [x] Implement PDF reporting
20. [x] Implement CLI (`check`, `batch`, `info`, `list-rules` commands; stable exit-code
      contract — spec §16; full scans compose `ffprobe` metadata with the
      Kinetix AV1/VP9 video adapter (MP4, Matroska/WebM and MPEG-TS) and Cadence
      standalone-audio adapter)
21. [x] Implement watch folders (spec §13: pass/warn/fail routing, fully local; CLI
      `watch` command)
22. [x] Add golden-media test suite covering every MVP rule (spec §24.2) — deterministic
      *measurement* fixtures (`tests/fixtures/*.json`: `Asset` + canned `Inspection`) run
      through the real engine via a fixture `Inspector`; encoded fixtures also
      exercise the Kinetix decode adapter (AV1 generated in-test via
      `Av1Encoder`, VP9 in MP4 and Matroska/WebM under `tests/fixtures/encoded/`).
      Golden manifests in
      `tests/golden` pin verdict + per-rule status for every built-in rule;
      `MEDIA_QC_UPDATE_GOLDEN=1` regenerates.
23. [x] Add fuzzing for parsers, profile parser, result parser, CLI args, report
      generation, media boundary handling (spec §24.4) — `fuzz/` targets: `ffprobe_json`,
      `profile_parse`, `clap_args`, `report_generation`, `media_boundary`, `result_parser`
      (all compile-checked via `cargo check --all-targets`; run under `cargo fuzz`)
24. [x] Benchmark and profile against representative HD/UHD fixtures — `qc-bench`
      micro-benchmark harness (`cargo run -p tpt-app-media-qc-test --release --bin
      qc-bench`): fingerprinting, profile parse, rule build/run, engine check, fixture
      deserialize, report build + JSON/HTML/PDF render; baseline committed to
      `tests/performance/baseline.md`. AV1 HD/UHD decode + analysis
      benchmarked by `cargo run -p tpt-app-media-qc-decode --release --example
      decode_bench` (synthetic AV1; VP9 from libvpx test clips; real masters and peak RSS pending — pass a file to the example)
25. [x] Harden error handling — isolated per-job failure state, corrupt asset must not
      terminate batch (spec §21; batch continues past per-asset errors)
26. [ ] Package Windows release — CI builds/tests on push across
      Linux/Windows/macOS; a tag-triggered **unsigned** multi-platform bundle
      workflow and draft GitHub Release are implemented in
      `.github/workflows/release.yml`. Signing, notarization, Gumroad upload
      and clean-machine validation remain in `GUMROAD.md`.
27. [ ] Run a private beta with real professional media
28. [ ] Verify against Definition of Done checklist (spec §30) before declaring
      MVP complete — code-level audit tracked in `docs/definition-of-done.md`
      (item-by-item status + evidence); remaining gaps are external actions
      (signing, clean-machine validation, private beta) and representative
      HD/UHD media benchmarks

---

## Cross-cutting / Ongoing

Applies continuously across all phases, not a one-time gate.

- [ ] **Testing (spec §24)**
  - [x] Unit tests per QC rule: valid input, invalid input, boundary case, malformed input, expected result
  - [x] Real-media CLI integration coverage (`crates/tpt-app-media-qc-cli/tests/real_media.rs`
        generates fixtures with `ffmpeg` and exercises `info`/`check` through the real
        `ffprobe` + decode path; it also covers real-process batch failure isolation and
        watch-folder routing; skips itself when the tools aren't on `PATH`)
  - [x] Deterministic property-style matrix: timecode conversion (including 59.94
        drop-frame), frame indexing, duration calculations, threshold logic,
        profile parsing and result aggregation
  - [ ] Regression fixture added for every production bug (spec §24.5)
- [ ] **Security & privacy (spec §22)**
  - [x] No mandatory network access for core operation
  - [x] No cloud upload, no external telemetry by default
  - [ ] Sandboxed optional AI integrations
  - [x] Safe handling of malformed media; no execution of embedded media content
  - [x] Strict path validation and safe temporary-file handling (canonical media
        inputs; validated export paths; RAII temporary files and staged report
        replacement)
  - [x] Media parsers fuzz tested (six targets; bounded campaigns run on every
        push/PR in CI on Linux — libFuzzer does not link on Windows/MSVC;
        long-running scheduled campaigns remain optional)
- [ ] **Standards architecture (spec §25)**
  - [x] Implement supported loudness standards (EBU R128, ATSC A/85 and
        BS.1770-derived measurement selection) as profile/rule configuration;
        every loudness finding records the selected standard and tolerances
  - [x] Add video standards on the shared delivery-rule layer: interlace/
        field-order measurements and the `video.scan_format` rule are
        implemented; a BT.1700/BT.1702-flavoured SDTV delivery profile
        (`profiles/broadcast/bt-1702.yaml`) encodes 576i25 interlaced
        top-field-first with EBU R128 audio (compliance not claimed — spec §25)
  - [x] Distinguish measurement algorithm vs. standard vs. profile vs. customer tolerance
  - [x] Do not claim formal standards compliance until validated against reference material/test suites
- [x] **Determinism & caching (spec §19)**
  - [x] Cache keys include: asset fingerprint, application version, ruleset version, profile hash, analysis configuration hash
  - [x] Changing one rule's configuration invalidates only that rule's cached results, not the whole analysis
- [x] **Local API (spec §17)**
  - [x] Disabled by default; explicitly enabled with `TPT_MEDIA_QC_API=1` and
        binds only to an ephemeral `127.0.0.1` port. Health, profiles, jobs,
        results and cooperative cancellation are implemented.

---

## Patent-safe video decode — replace H.264 with AV1 + VP9

H.264 patent pools license decoders as well as encoders, so the app must not ship an
H.264 decoder. AAC is never decoded (and stays out of scope), so nothing to remove there. H.264/AAC files
keep full ffprobe metadata/container QC; frame-decode rules go Inconclusive for them.
AV1/VP9 in Kinetix are royalty-free; the old pin (`a43959c`) reported `pixel_exact: false`
but upstream HEAD (`1a8623c`, 2026-10-02) reports `pixel_exact: true` for both, so the pin
was bumped and both decoders run strict.

- [x] Dependencies: drop `tpt-kinetix-h264` and `tpt-kinetix-mux` (root `Cargo.toml`, decode crate); add `tpt-kinetix-av1` and `tpt-kinetix-vp9`; bump Kinetix pin to `1a8623c`
- [x] Decode crate: delete H.264 code (`H264Decoder`, avcC/avc1/avc3 parsing, SPS/PPS prelude, AnnexB conversion)
- [x] Decode crate: route by track codec (`CodecId::Av1` / `CodecId::Vp9`); both decoders strict (an unfaithful/placeholder frame → `NotPixelExact` → Unsupported)
- [x] Decode crate: add MKV/WebM via `MkvDemuxer` (codec ids `V_AV1` / `V_VP9`) alongside MP4
- [x] Decode crate: unsupported container/codec (H.264, HEVC, ProRes, MXF…) returns `Unsupported`, never `Failed`
- [x] Fix false `video.corrupt_frames` failure on files that simply can't be decoded; regression tests added (decode crate and `rules/src/video.rs`)
- [x] CLI: update `HybridInspector` backend string and `probe.rs` docs (`ffprobe + tpt-kinetix-av1-vp9 + tpt-cadence`)
- [x] Tests: remove the H.264 fixture and its `ATTRIBUTION.md` entry; add AV1 (Kinetix `Av1Encoder` → Matroska), VP9 (MP4 and WebM fixtures), H.264-is-unsupported and unsupported-container tests
- [x] ~~Report AV1/VP9 measurements as approximate~~ — dropped: decoders are pixel-exact at the new pin
- [x] Frame analysis handles the newer Kinetix pixel formats (monochrome, 10/12-bit) by scaling luma to 8 bits
- [x] Docs: `README.md`, `CHANGELOG.md`, `docs/supported-formats.md`, `docs/architecture.md`, `docs/definition-of-done.md`, `docs/performance.md`, `GUMROAD.md` (adds ffprobe-bundling patent caveat), fixture `README.md`/`ATTRIBUTION.md`
- [x] Update existing Phase 1 items 2, 8, 20 and 22 (MP4/H.264 mentions) to reflect the swap
- [x] Distribution check: release workflow / Tauri config do not bundle ffmpeg/ffprobe (CI only installs it for tests); caveat recorded in `GUMROAD.md`
- [x] Verify: `cargo build`/`cargo test --workspace` green, no `h264`/`kinetix-mux` left in `Cargo.lock` / `fuzz/Cargo.lock`, `cargo fmt --all --check` clean, clippy clean with `-D warnings`, and the CLI exercised end to end on real media — AV1 MP4 (frames decoded, freeze segment measured), VP9 MP4 + WebM (frames decoded), H.264 MP4 (full metadata/container QC, frame-decode rules `Inconclusive`, `video.corrupt_frames` never fails) and WAV (Cadence audio decode). The local-API test's 4-byte `RIFF` stub was replaced with a valid minimal WAV so the suite is green on current FFmpeg builds.
- [x] MPEG-TS video decode via Kinetix's `TsDemuxer` (188-byte packets; codec identity from the PMT registration descriptor `AV01`/`vp09`): AV1 decodes end to end, VP9 is routed to the VP9 decoder, and H.264 elementary streams stay `Unsupported` with no decode error recorded. Audio inside a transport stream is demuxed but not decoded, so its audio findings remain `Inconclusive`.
- [x] Standards-accurate audio levels: true peak (BS.1770-4 Annex 2, 4× oversampling through a polyphase reconstruction filter) and gated integrated loudness (BS.1770-4 K-weighting, 400 ms blocks, −70 LUFS absolute and −10 LU relative gates) are now measured in the Cadence adapter, cross-checked against `ffmpeg -af ebur128` reference readings. Loudness range (EBU Tech 3342: 3 s short-term windows, −70 LUFS / −20 LU gates, 10th–95th percentile) is now measured too
- [x] EBU Tech 3342 loudness range
- [ ] Optional follow-up: embedded-audio decode via Kinetix/Cadence (Opus elementary streams inside MP4/MKV/TS) and surfacing momentary/short-term max loudness in reports (meter methods exist)

---

## Phase 2 — Post-MVP (spec §27)

- [x] HDR analysis — static HDR/colorimetry signalling (`HdrMetadata` in the inspection model, populated by the ffprobe front-end) checked by the `video.hdr` rule: expected mode (`sdr` / `hdr10` / `hlg` / `hdr`), PQ/HLG transfer with BT.2020 primaries/matrix, bit depth, ST 2086 mastering display + MaxCLL/MaxFALL presence, and configurable MaxCLL/MaxFALL ceilings; `profiles/streaming/hdr10.yaml` added and bundled in the desktop app profile list, golden fixtures updated. Dynamic metadata (Dolby Vision RPU / HDR10+) is only detected, not validated; per-frame light-level measurement from decoded pixels not implemented
- [x] PSE (photosensitive epilepsy) general-flash analysis (`video.photosensitivity`: Harding/BT.1702-style per-pixel general + red flash; spatial-pattern check not implemented; golden fixtures added)
- [x] Advanced compression artefact detection (spec § 8.4) — three no-reference perceptual rules over decoded AV1/VP9 frames: `video.blockiness` (Wang/Bovik 8-pixel boundary-vs-interior gradient ratio, with `max_ratio`/`max_frame_ratio`/`min_evidence_share`; flat frames excluded as carrying no evidence so flat content reports `Inconclusive` rather than a false pass), `video.blur` (normalised mean absolute Laplacian against `min_sharpness`) and `video.noise` (RMS deviation in flat regions against `max_sigma`, measured against a 5x5 smoothed copy so noisy areas are not misread as detail). Calibrated on AV1 CRF 10/28/45: blockiness 1.41→1.54, sharpness 0.00486→0.00399, confirming the metrics track compression damage. Pictures above ~2.5 M cells box-averaged. Still outstanding from spec §8.4: ringing, banding and image corruption
- [x] Dead pixel detection (spec § 8.4) — `video.dead_pixels` rule over decoded AV1/VP9 frames: per-cell luma comparison across the decode flags dead (never brightens in bright frames), stuck (never darkens in dark frames) and flicker (reaches both extremes) cells. Contrast-gated judging avoids false positives on letterbox bars/static content; ≥3 bright and ≥3 dark frames required before any verdict. Flagged cells are grouped into 4-connected clusters (largest first, capped at 64) reported in source pixels, with the largest cluster attached to the finding. Profile keys `max_pixels`/`max_clusters`/`include_flicker`/`fail_on_limited_resolution`; capped at ~4.2 M cells/frame with stride-sampling above that. Chroma/subpixel defects and comparisons against a published display-defect standard not implemented
- [x] Subtitle/caption validation (spec §8.7) — five metadata-driven rules: `subtitle.presence` (min/max tracks), `subtitle.language` (required ISO 639 codes, loose matching so `en` satisfies `eng`), `subtitle.timing` (cue overlaps, invalid durations, inter-cue gaps, cue dwell time), `subtitle.content` (malformed/empty payloads, characters per line, lines per cue) and `subtitle.duration_match` (coverage vs. video duration). Cue timing comes from packet headers and cue text from the packet payload, so no picture or sample is decoded. Text-dependent checks are limited to text-based codecs (`subrip`/`srt`/`ass`/`ssa`/`webvtt`/`text`/`microdvd`/`mpl2`/`subviewer`) and report `Inconclusive` for bitmap/structured formats; character counts strip SRT/ASS/WebVTT markup. Cue analysis capped at 100 000 cues/stream. Golden fixtures updated. Reading-rate/minimum-dwell-time standards and sidecar (`.srt`/`.vtt`) comparison not implemented
- [x] File comparison mode (spec §15) — `tpt-media-qc compare`: metadata, streams, duration, frame rate, resolution, codec, audio layout, loudness and measured defect counts; pixel/waveform (visual/audio) diffing not implemented
- [x] Custom rule builder (spec §27) — `rules.custom` declarative metadata rules (container/video/audio/subtitle scopes, closed metric catalogue, `== != < <= > >= in not_in`, tolerance, per-rule severity/message); strict parser validation, `Inconclusive` on missing metrics, per-rule cache hash, `profiles/examples/custom-rules.yaml`. Rules over decoded measurements (loudness, black-frame counts, …) and expression/boolean composition are not implemented
- [x] Richer report templates — `detailed` / `summary` / `executive` / `audit` for HTML, PDF and CSV (`check --report-template`, desktop template picker); JSON stays complete. User-authored/branded templates (logo, custom sections) not implemented
- [ ] GPU acceleration

---

## Phase 3 — Post-MVP (spec §27)

- [x] Voice integration (`tpt-voice`) — optional `voice` cargo feature (off by default) using tpt-voice's weight-free VAD and classical diarizer, pinned at `05ffff3`; `VoiceMeasurements` in the inspection model; `rules.voice` profile section. Standalone WAV/AIFF/FLAC only, first 3600 s analysed, findings labelled probabilistic. See `docs/voice-and-correction.md`
- [x] Transcription validation — exact sidecar comparison (`<name>.transcript.txt|json` vs `<name>.expected.txt`): WER with S/D/I counts and words outside the expected transcript (`voice.transcript`). No ASR engine is bundled: tpt-voice has no trained weights yet (its own todo), so producing the transcript is left to the facility's workflow. Timing-aligned transcript checks not implemented
- [x] Speaker segmentation / analysis — speaker turns, speaker count and change rate (`voice.speakers`, `voice.speaker_changes`) from classical MFCC-statistics clustering; accuracy is limited (heuristic, no trained embeddings)
- [ ] Semantic/content checks (dialogue overlap, transcript alignment, words outside expected transcript, intelligibility metrics — spec §8.8) — partial: speech present/absent, excessive non-speech, speaker count/changes, WER and words outside the expected transcript are done; dialogue overlap, time-aligned transcript checks and intelligibility metrics are not
- [x] Automated correction — `tpt-media-qc correct`: single linear loudness gain (refused if it would break the true-peak ceiling; no limiter) and DC-offset removal for WAV/AIFF/FLAC, written to a new 16/24-bit WAV and re-measured. Video, embedded audio, trims and de-clipping not corrected
- [x] Advanced broadcast/OTT profiles — `broadcast-uk-hd`, `streaming-ott-premium` (integrated, not dialogue-gated, loudness), `streaming-online-upload`, `streaming-podcast-voice`; indicative values, no conformance claim

---

## Phase 4 — Post-MVP (spec §27)

- [ ] Plugin SDK
- [ ] Third-party rules
- [ ] Facility management features
- [ ] Distributed local workers
- [ ] Optional LAN processing

---

## Commercial / Packaging Notes (reference, not phase-gated)

- [ ] Validate pricing hypothesis (spec §2.1): Professional $499, Studio $999, Facility $1,999+
- [ ] Implement edition feature gating (spec §29): Professional / Studio / Facility
- [ ] Defer Facility-tier features (LAN workers, central profile distribution) until customer demand is evidenced (spec §29, §32)
- [ ] Gumroad launch checklist tracked in `GUMROAD.md` (remaining blockers are
      licensing model, `ffprobe` packaging/documentation, release artifacts,
      signing and private-beta validation)
