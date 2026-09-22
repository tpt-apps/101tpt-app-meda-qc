# QC Profile Format

A QC profile is the central product abstraction (spec §9): it selects rules,
their thresholds and severities, and the verdict policy. Profiles must be
human-readable, versioned, exportable, importable, deterministic, diffable and
testable — and a user must be able to create one without programming.

The canonical implementation is in
[`tpt-app-media-qc-profile`](../crates/tpt-app-media-qc-profile/) (`model.rs`,
`parse.rs`, `hash.rs`). Parsing is **strict**: unknown top-level keys, unknown
rule groups, unknown rules and unknown rule keys are rejected with
path-annotated errors, keeping profiles deterministic and typo-resistant.

## 1. Structure

```yaml
name: "Client Delivery - Example"
version: 1

rules:
  container: ...
  video: ...
  audio: ...
  subtitle: ...
  voice: ...

policy:
  fail_on: error
```

`name` (required) and `version` (defaults to 1) identify the profile. The
optional top-level keys `description` and `comment` are accepted and ignored.
`policy.fail_on` (default `error`) is the severity at or above which a FAIL
finding forces a FAIL verdict.

## 2. Rule entry forms

Every rule entry is either a **scalar severity** or a **mapping** carrying
rule-specific keys plus an optional `severity`:

```yaml
# scalar form
container:
  readable: error

# mapping form
container:
  readable:
    severity: error
```

Severities are `info`, `warning`, `error`, `critical`.

## 3. Container rules (spec §8.1)

| Key | Form | Meaning |
|-----|------|---------|
| `readable` | severity | File must be readable and a recognised media container |
| `container_validity` | severity | Container structure fully valid |
| `malformed_metadata` | severity | Garbage/malformed metadata entries |
| `duration_consistency` | mapping | `tolerance_ms` — container vs. stream durations |
| `bitrate` | mapping | `min_bps` — minimum overall/container bitrate |
| `timecode_present` | severity | Presence of a start timecode where expected |
| `timebase` | mapping | `value` — expected `num/den` stream time base |
| `timestamp_continuity` | mapping | `max_gap_ms` (default 150) |
| `unexpected_streams` | severity | Stream kinds not expected by the profile |
| `stream_presence` | mapping | `min_video` (default 1), `min_audio` (default 0), `max_streams` (default 32) |

## 4. Video rules (spec §8.3)

| Key | Form | Meaning |
|-----|------|---------|
| `resolution` | mapping / scalar | `expected` as `<width>x<height>` (e.g. `"3840x2160"`) |
| `frame_rate` | mapping / scalar | `expected` as `num/den` or integer (e.g. `25`, `30000/1001`); `tolerance` (default 0.001) |
| `aspect_ratio` | mapping / scalar | `expected` as `<n>/<d>` or `<n>:<d>` (e.g. `16/9`); `tolerance` (default 0.005) |
| `black_frames` | mapping | `max_duration_ms` |
| `freeze_frames` | mapping | `max_duration_ms` |
| `duplicate_frames` | mapping | `max_events` |
| `corrupt_frames` | mapping | `max_events` |
| `luma_range` | mapping | `max_out_of_legal` (fraction outside [16,235], default 0.01) |
| `color_space` | mapping / scalar | `expected` colour-space tag (e.g. `bt709`, `bt2020nc`) |

## 5. Audio rules (spec §8.5, §8.6)

| Key | Form | Meaning |
|-----|------|---------|
| `sample_rate` | mapping / scalar | `expected` Hz (e.g. `48000`) |
| `bit_depth` | mapping / scalar | `expected` bits (e.g. `24`) |
| `channel_layout` | mapping / scalar | `channels` (required); optional `layout` label |
| `silence` | mapping | `max_duration_ms` |
| `clipping` | mapping | `max_events` |
| `peak` | mapping | `max_db` (dBFS) |
| `true_peak` | mapping | `max_db` (dBTP) |
| `loudness` | mapping | `standard` (`ebu-r128` \| `atsc-a85` \| `bs1770`), `target_lufs`, `tolerance_lu`; default standard is `ebu-r128` targeting `-23 LUFS` with `1 LU` tolerance |
| `phase` | mapping | `min_correlation` (default -0.5) |
| `dc_offset` | mapping | `max_offset_percent` (default 5.0) |

Loudness standard defaults: EBU R128 → `-23 LUFS`, ATSC A/85 → `-24 LUFS`,
BS.1770 → `-18 LUFS`.

## 6. Subtitle / voice rules (post-MVP placeholders)

```yaml
subtitle:
  language: "eng"        # demanded subtitle track language
  missing_subtitles: error

voice:
  {}                     # reserved for spec §8.8
```

## 7. Policy

```yaml
policy:
  fail_on: error
```

`fail_on` default is `error`.

## 8. Defaults and determinism

- A profile omits nothing required at parse time; every optional rule key has a
  documented default.
- The **canonical serialization** of the typed model is what gets hashed for
  profile integrity and cache keys. `profile_sha256(&Profile)` returns the
  hex SHA-256 used in report integrity fields (spec §14.1) and cache keys
  (spec §19).
- The bundled `generic` profile (see
  [`profiles/generic/`](../profiles/generic/)) is the CLI default when no
  `--profile` is given.