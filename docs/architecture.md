# Architecture

This is the living architecture document for **TPT Media QC**
([spec §4, §5, §10]).

## 1. Design goals

- **Analysis and presentation are separate** — the QC engine must be usable
  without the desktop GUI (spec §3.5).
- **Offline-first** — all core analysis works without internet access (spec §3.1).
- **Deterministic** — same input bytes, application version and profile give the
  same result (spec §3.2).
- **Bounded memory** — processing streams incrementally; a two-hour asset is
  never decoded wholesale into RAM (spec §20).

## 2. Overall layout

```
                    QC Application
                         |
             +-----------+-----------+
             |                       |
          Desktop                  CLI/API
             |                       |
             +-----------+-----------+
                         |
                     QC Engine
                         |
        +----------------+----------------+
        |                |                |
     Metadata         Signal          Perceptual
       checks         checks            checks
        |                |                |
        +----------------+----------------+
                         |
                  Result/Event Model
                         |
             +----------+----------+
             |                     |
         Interactive            Reports
            review
```

## 3. Crate map

The workspace is split so reusable analysis logic stays independent of the
application shell.

| Crate | Responsibility | Depends on |
|-------|----------------|------------|
| `tpt-app-media-qc-core` | Shared primitives: error type, cost classification, content-based fingerprinting, application identity/versioning | (none) |
| `tpt-app-media-qc-model` | Pure domain types: `Asset`, `Stream`, `QcFinding`, `Evidence`, `Inspection`, timecode, severity, immutable `Report` | core |
| `tpt-app-media-qc-profile` | YAML profile parsing, validation, canonical model, deterministic profile hashing | core, model |
| `tpt-app-media-qc-rules` | Rule framework (`QcRule` trait) and built-in rule catalogue | core, model, profile |
| `tpt-app-media-qc-pipeline` | Inspection boundary (`Inspector`), `QcEngine`, aggregation, verdict, batch scheduler | core, model, rules, profile |
| `tpt-app-media-qc-decode` | Kinetix MP4/ISO-BMFF demux + H.264 video measurement adapter | core, model, pipeline, Kinetix |
| `tpt-app-media-qc-report` | Immutable `Report` assembly + JSON/HTML/CSV/PDF export | core, model, pipeline, profile |
| `tpt-app-media-qc-cli` | `tpt-media-qc` binary: `check`, `batch`, `info`, `list-rules`, stable exit codes | all application crates |
| `tpt-app-media-qc-tauri` | Desktop application shell (stub — planned per spec §12) | — |
| `tpt-app-media-qc-test` | Shared fixtures, golden manifests and integration-test harness | application model/rules/pipeline/report |

## 4. Data flow

Input → Fingerprint → Container scan → Stream discovery → Metadata rules →
Decode planning → (video / audio / subtitle / voice pipelines) → Rule
aggregation → Finding normalization → Severity evaluation → QC verdict →
UI / JSON / HTML / CSV / PDF (spec §10).

### 4.1 Two-pass strategy (spec §10.1)

- **Pass 1 — quick scan:** metadata, container, streams, codecs, durations,
  timestamps and basic parameters. Produces useful results in seconds without
  a full decode.
- **Pass 2 — full QC:** runs only the checks that require decoded frames or
  samples.

The inspection model ([`Inspection`](../crates/tpt-app-media-qc-model/src/inspection.rs))
is the stable exchange format: a probe front-end fills it in, rules read it
back. Missing measurements are expected on the metadata-only path, and decode
rules report `Inconclusive` rather than guessing (spec §3.4).

### 4.2 Probe boundary

The pipeline owns the seam between probe front-ends and the engine:

```
trait Inspector {
    fn name(&self) -> &str;
    fn inspect_metadata(&self, asset: &Asset) -> Result<Inspection>;
    fn inspect_decode(&self, asset: &Asset, metadata: &Inspection, level: InspectionLevel)
        -> Result<Inspection>;
}
```

- `FfprobeInspector` (CLI crate) remains the metadata front-end and shells out
  to `ffprobe`.
- `HybridInspector` (CLI crate) composes that metadata front-end with
  `KinetixVideoInspector` for full scans. The latter uses Kinetix MP4 demuxing
  and H.264 reconstruction, then streams frames through black/freeze/duplicate,
  corrupt-frame, luma and frame-rate measurements.
- The pinned Kinetix MP4 demuxer is in-memory; `KinetixVideoInspector` applies
  a 512 MiB input bound and records unsupported or incomplete coverage instead
  of claiming a pass. Audio remains on the metadata-only path until Cadence is
  integrated.
- `NoopInspector` produces an empty inspection for tests and for the
  metadata-only path when no probe binary is available.

## 5. Rule execution

`QcEngine::check(asset, level)` (pipeline crate):

1. Inspect metadata.
2. If `Full`, run the decode pass and merge measurements over the metadata
   baseline.
3. Execute every rule configured by the profile in deterministic order
   (container, video, audio) via [`build_rules`](../crates/tpt-app-media-qc-rules/src/lib.rs).
4. Aggregate findings into per-rule statuses and status counts.
5. Resolve the verdict from the profile policy (`fail_on` severity).

Rules emit findings only for **non-pass** outcomes; pass is the absence of
findings. Each finding carries `measured`, `expected`, stream index,
time/frame range, severity and optional confidence (spec §3.3).

## 6. Scheduler

The batch scheduler (pipeline `scheduler`) classifies work by cost class
(metadata / cheap decode / full decode / GPU / expensive analysis — spec §11)
and runs independent assets concurrently with per-cost-class concurrency
limits. Results preserve input order; a panicked worker does not lose the
others' results. Cancellation, priority and resumability are slated for the
persistence milestone (§13).

## 7. Reporting pipeline

A [`QcRun`](../crates/tpt-app-media-qc-pipeline/src/lib.rs) is turned into an
immutable [`Report`](../crates/tpt-app-media-qc-model/src/report.rs) carrying
the five integrity fields mandatory per spec §14.1 (asset SHA-256, profile
SHA-256, application version, ruleset version, analysis ID). The report crate
renders JSON (compact), HTML (with optional embedded JSON), CSV and PDF.

## 8. Persistence & caching

SQLite stores local application state: projects, assets, fingerprints, jobs,
profiles, results, findings, report metadata and preferences (spec §18).
Original media is never stored in the database — only paths and fingerprints.
Derived media is stored separately. Cache keys include the asset fingerprint,
application version, ruleset version, profile hash and analysis-configuration
hash; rule-level entries invalidate only the affected rule.

## 9. Automation interfaces

- **CLI** is first-class with a stable exit-code contract (spec §16):
  `0` PASS, `1` WARN, `2` FAIL, `3` INCONCLUSIVE, `4` CONFIGURATION_ERROR,
  `5` INPUT_ERROR, `6` INTERNAL_ERROR.
- **Watch folders** (spec §13) route pass/warn/fail assets to output folders,
  fully locally. Implemented via the `watch` CLI command.
- **Local API** (spec §17) is disabled by default and binds only to
  `127.0.0.1` when enabled. Planned.

## 10. Failure model

Each job has an isolated failure state. A corrupt asset must never terminate a
batch: `batch` continues past per-asset errors and reports them individually
(spec §21). Internal panics are treated as application defects.