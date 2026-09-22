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
  - [x] `tpt-app-media-qc-cli`
  - [x] `tpt-app-media-qc-tauri` (stub)
  - [x] `tpt-app-media-qc-test` (stub)
- [ ] Create `README.md`
- [ ] Create `CHANGELOG.md`
- [ ] Create `CONTRIBUTING.md`
- [ ] Create `docs/` skeleton:
  - [ ] `docs/architecture.md`
  - [ ] `docs/qc-model.md`
  - [ ] `docs/profile-format.md`
  - [ ] `docs/report-format.md`
  - [ ] `docs/supported-formats.md`
  - [ ] `docs/standards.md`
  - [ ] `docs/performance.md`
- [ ] Create `profiles/` skeleton: `generic/`, `broadcast/`, `streaming/`, `examples/`
- [ ] Create `tests/` skeleton: `fixtures/`, `integration/`, `golden/`, `performance/`
- [ ] Licensing:
  - [ ] Add `LICENSE-MIT` file
  - [ ] Add `LICENSE-APACHE` file
  - [ ] Set `license = "MIT OR Apache-2.0"` in workspace `Cargo.toml`
  - [ ] Set copyright holder as "TPT Solutions" in license headers/files
  - [ ] Add licensing section to `README.md`

---

## Phase 1 — MVP

Ordered per spec §31 (Recommended Implementation Order), scoped per spec §26 (MVP).

1. [x] Establish Cargo workspace and application shell
2. [ ] Integrate `tpt-kinetix` and enumerate media capabilities
3. [x] Implement asset fingerprinting (content-based, not path-based — spec §6.1)
4. [x] Implement stream/container inspection (probe boundary + `FfprobeInspector` front-end)
5. [x] Implement the QC domain model (`Asset`, `Stream`, `QcFinding`, `Evidence` — spec §6)
6. [x] Implement profile parsing (human-readable YAML profile format — spec §9)
7. [x] Implement metadata QC rules (container checks — spec §8.1: readability, validity,
      malformed metadata, stream count/duration consistency, bitrate, timecode, timebase,
      timestamp continuity, unexpected/missing streams)
8. [ ] Implement video decode pipeline (via Kinetix)
9. [x] Implement basic video QC rules (metadata/measurement-driven; decode detection stubs
    report Inconclusive until §8 lands):
   - [x] corrupt/dropped frame detection (detection pathway defined)
   - [x] black frames
   - [x] freeze frames
   - [x] duplicate frames
   - [x] frame-rate consistency
   - [x] resolution
   - [x] aspect ratio
   - [x] luma range
   - [x] colour-space / HDR metadata
10. [ ] Implement audio decode through `tpt-cadence`
11. [x] Implement DSP-based audio QC rules (decode measurement pathway defined; rules
    report Inconclusive until §10 lands):
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
15. [ ] Implement desktop queue UI (Tauri):
    - [ ] Dashboard (spec §12.1)
    - [ ] Import — drag-and-drop, file picker, folder/recursive import (spec §12.2)
    - [ ] Job Queue — columns, pause/resume/cancel/retry/open report (spec §12.3)
16. [ ] Implement timeline / evidence viewer and finding inspector (spec §12.5–12.6)
17. [ ] Implement Asset Inspector screen (spec §12.4)
18. [x] Implement JSON/HTML/CSV reporting (spec §14, report integrity fields §14.1)
19. [x] Implement PDF reporting
20. [x] Implement CLI (`check`, `batch`, `info`, `list-rules` commands; stable exit-code
      contract — spec §16; `check`/`batch` rely on `ffprobe` until §2/§8 land)
21. [x] Implement watch folders (spec §13: pass/warn/fail routing, fully local; CLI
      `watch` command)
22. [ ] Add golden-media test suite covering every MVP rule (spec §24.2)
23. [ ] Add fuzzing for parsers, profile parser, result parser, CLI args, report
      generation, media boundary handling (spec §24.4)
24. [ ] Benchmark and profile against representative HD/UHD fixtures
25. [x] Harden error handling — isolated per-job failure state, corrupt asset must not
      terminate batch (spec §21; batch continues past per-asset errors)
26. [ ] Package Windows release
27. [ ] Run a private beta with real professional media
28. [ ] Verify against Definition of Done checklist (spec §30) before declaring MVP complete

---

## Cross-cutting / Ongoing

Applies continuously across all phases, not a one-time gate.

- [ ] **Testing (spec §24)**
  - [ ] Unit tests per QC rule: valid input, invalid input, boundary case, malformed input, expected result
  - [ ] Property tests: timecode conversion, frame indexing, duration calculations, threshold logic, profile parsing, result aggregation
  - [ ] Regression fixture added for every production bug (spec §24.5)
- [ ] **Security & privacy (spec §22)**
  - [ ] No mandatory network access for core operation
  - [ ] No cloud upload, no external telemetry by default
  - [ ] Sandboxed optional AI integrations
  - [ ] Safe handling of malformed media; no execution of embedded media content
  - [ ] Strict path validation; safe temporary-file handling
  - [ ] Media parsers fuzz tested
- [ ] **Standards architecture (spec §25)**
  - [ ] Implement standards (EBU R128, ATSC A/85, BT.1702, etc.) as profiles/rules, not hard-coded UI logic
  - [ ] Distinguish measurement algorithm vs. standard vs. profile vs. customer tolerance
  - [ ] Do not claim formal standards compliance until validated against reference material/test suites
- [ ] **Determinism & caching (spec §19)**
  - [x] Cache keys include: asset fingerprint, application version, ruleset version, profile hash, analysis configuration hash
  - [x] Changing one rule's configuration invalidates only that rule's cached results, not the whole analysis
- [ ] **Local API (spec §17)**
  - [ ] Disabled by default; binds only to `127.0.0.1` when enabled

---

## Phase 2 — Post-MVP (spec §27)

- [ ] HDR analysis
- [ ] PSE (photosensitive epilepsy) risk analysis
- [ ] Advanced compression artefact detection
- [ ] Dead pixel detection
- [ ] Subtitle/caption validation (presence, language, timing, overlaps, invalid durations, malformed data, character limits, duration mismatch — spec §8.7)
- [ ] File comparison mode (spec §15: metadata, streams, duration, frame rate, resolution, codec, audio layout, loudness, visual/audio differences)
- [ ] Custom rule builder
- [ ] Richer report templates
- [ ] GPU acceleration

---

## Phase 3 — Post-MVP (spec §27)

- [ ] Voice integration (`tpt-voice`)
- [ ] Transcription validation
- [ ] Speaker segmentation / analysis
- [ ] Semantic/content checks (dialogue overlap, transcript alignment, words outside expected transcript, intelligibility metrics — spec §8.8)
- [ ] Automated correction
- [ ] Advanced broadcast/OTT profiles

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
