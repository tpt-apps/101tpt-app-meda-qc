# QC Model

The QC domain model ([spec §6]) lives in
[`tpt-app-media-qc-model`](../crates/tpt-app-media-qc-model/). It is a set of
pure data types shared by every layer — probe, rules, engine, reports and the
future UI. The types are `Serialize`/`Deserialize` so they pass cleanly across
the CLI, the local API and the Tauri boundary.

## 1. Asset ([spec §6.1])

```rust
struct Asset {
    id: AssetId,
    path: PathBuf,
    fingerprint: AssetFingerprint,
    size_bytes: u64,
    modified_time: Option<u64>,
    duration: Option<DurationSeconds>,
    streams: Vec<Stream>,
}
```

The fingerprint is **content-based** — a file path alone must not identify an
asset because files can be moved or replaced. Fingerprinting streams the file
in bounded buffers and, on the quick path, hashes a bounded prefix plus the
file size (see `tpt-app-media-qc-core::fingerprint`).

## 2. Stream ([spec §6.2])

`StreamKind` distinguishes `Video`, `Audio`, `Subtitle`, `Data`, `Attachment`
and `Unknown`. Each `Stream` records codec, codec profile, dimensions, pixel
format, frame rate, time base, bitrate, duration, language, channel layout,
sample rate, bit depth, stream index and a metadata map.

## 3. Inspection (measurements)

Rules consume **measurements**, not raw media bytes (spec §3.5). The
`Inspection` type is the stable exchange format:

- `ContainerInspection` — validity, format, bitrate, duration, timecode
  presence, timestamp continuity/gaps, malformed metadata, decode errors.
- `VideoMeasurements` — observed frame rate, decode errors, black/freeze/
  duplicate-frame segments, luma statistics, colour-space tag.
- `AudioMeasurements` — decoded sample-frame coverage, decode errors, bounded
  silence segments and truncation state, clipping events, peak (dBFS), true peak
  (dBTP), integrated loudness (LUFS), loudness range, phase correlation and DC
  offset.

Missing measurements (`None`/empty) are expected on the metadata-only path;
rules must report `Inconclusive` or pass accordingly (spec §3.4).

## 4. Results ([spec §6.3])

```rust
enum Severity { Info, Warning, Error, Critical }
enum ResultStatus { Pass, Warn, Fail, Inconclusive }
```

Every finding carries full provenance:

```rust
struct QcFinding {
    rule_id: RuleId,
    status: ResultStatus,
    severity: Severity,
    message: String,
    measured: Option<Value>,
    expected: Option<Value>,
    stream_id: Option<StreamId>,
    time_range: Option<TimeRange>,
    frame_range: Option<FrameRange>,
    evidence: Vec<Evidence>,
    confidence: Option<f32>,   // heuristic rules only
}
```

A finding must never merely say `FAILED` — it explains what failed, why,
the threshold, the measured value, the expected value, the exact
timecode/frame, the affected stream, the severity and, where applicable,
confidence (spec §3.3).

`Value` is a JSON-like scalar/enum/object value used for `measured` and
`expected` so reports stay human-readable and machine-readable.

## 5. Evidence ([spec §6.4])

`Evidence` supports thumbnails, frame captures, waveform regions, spectra,
numerical measurements, metadata excerpts and JSON diagnostic payloads.
Evidence should be lazily generated where possible (planned with the decode
stack and the image cache).

## 6. Time model

- `Timecode` — frame-number / timebase conversion with sub-frame precision.
- `Rational` — numerator/denominator pair used for frame rates and time bases
  (e.g. `30000/1001` for NTSC).
- `DurationSeconds` / `DurationMillis` — millisecond-precision duration
  language used across findings, inspections and reports.
- `TimeRange`, `FrameRange` — the exact locations a finding refers to; these
  are what make "report → finding → media location in one interaction"
  (spec §12.6) possible.

## 7. Aggregation and verdict

The pipeline aggregates per-rule "best" status plus status counts, and resolves
the asset verdict from the profile policy: findings at or above `policy.fail_on`
with status `Fail` drive a FAIL verdict (see `pipeline::verdict`).

## 8. Report ([spec §14])

The immutable `Report` type is the audit artifact. It embeds the five integrity
fields required by spec §14.1: asset SHA-256, profile SHA-256, application
version, ruleset version and analysis ID. Reports are generated from an
immutable QC result set and are reproducible from
fingerprint + profile + application/ruleset version (spec §19).