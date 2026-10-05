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
| `scan_format` | mapping / scalar | `scan`: `progressive`/`interlaced`/`any` (scalar shorthand allowed); `field_order`: `top_field_first`/`bottom_field_first`/`any`. At least one non-`any` expectation required. Unsignalled field order is `Inconclusive` against `progressive` and a failure against `interlaced`/pinned orders |
| `photosensitivity` | mapping | `max_flashes_per_second` (default 3). Harding/BT.1702-style: per-pixel opposing luminance transitions (≥ 0.1, darker state < 0.8) over ≥ 25 % of the 10° field, plus a saturated-red test; general and red flashes judged separately; needs full decode (AV1/VP9) |
| `hdr` | mapping / scalar | `mode` (required): `sdr`, `hdr10` (or `pq`), `hlg` or `hdr` (either PQ or HLG); `require_static_metadata` (default `true` — ST 2086 mastering display and MaxCLL/MaxFALL for PQ); `max_cll_nits`, `max_fall_nits` ceilings. Metadata-driven: also checks BT.2020 primaries/matrix, limited range and ≥ 10-bit depth for HDR, and MaxFALL ≤ MaxCLL always. Unsignalled transfer is `Inconclusive`; light levels are read from static metadata, not measured from pixels |
| `dead_pixels` | mapping / scalar | `max_pixels` (default 0), `max_clusters` (default 0), `include_flicker` (default `true`), `fail_on_limited_resolution` (default `false`). Needs full decode (AV1/VP9). Flagged pixels are clustered and the largest cluster's position and size are attached to the finding |
| `blockiness` | mapping | `max_ratio` (required — highest acceptable boundary-to-interior gradient ratio), `max_frame_ratio`, `min_evidence_share` (default 0.5). Needs full decode. Reports `Inconclusive` when too few frames carried picture detail to measure |
| `blur` | mapping | `min_sharpness` (required — lowest acceptable normalised mean absolute Laplacian). Needs full decode. Content-dependent: a deliberately soft-focus delivery needs a lower minimum |
| `noise` | mapping | `max_sigma` (required — highest acceptable luma codes of noise in flat areas), `min_evidence_share` (default 1.0). Needs full decode. Reports `Inconclusive` when the picture has too little flat area |

```yaml
video:
  blockiness:
    max_ratio: 1.6        # clean 1080p AV1 of detailed content measures ~1.41
    max_frame_ratio: 3.0  # catches localised blocking the mean would hide
    severity: warning
  blur:
    min_sharpness: 0.004
    severity: warning
  noise:
    max_sigma: 2.0        # a clean AV1 encode measures ~1.1
    severity: warning
```

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

## 6. Subtitle rules (spec § 8.7)

| Key | Form | Meaning |
|-----|------|---------|
| `presence` | mapping / scalar | `min_subtitle` (default 1), `max_subtitle`. Scalar shorthand (`presence: error`) means "at least one track". Reports `Inconclusive` when the container was not scanned |
| `language` | mapping | `required`: list of ISO 639 codes that must each be present `min_tracks` times (default 1). Matching is case-insensitive, ignores region subtags, and treats the legacy two-letter codes as their three-letter equivalents (`en` = `eng`) |
| `timing` | mapping | `max_overlaps` (default 0), `max_invalid_durations` (default 0), `max_gap_ms`, `max_cue_duration_ms`. Overlaps and invalid durations are reported per cue with its time range; gaps and cue length are reported as measured-vs-limit values |
| `content` | mapping | `max_malformed` (default 0), `max_empty` (default 0), `max_chars_per_line`, `max_lines_per_cue`. Character limits require a text-based subtitle codec; otherwise the rule reports `Inconclusive` |
| `duration_match` | mapping | `tolerance_ms` (default 1000), `allow_longer` (default `true`). Compares the end of the last cue against the video duration |

```yaml
subtitle:
  presence:
    min_subtitle: 1
    severity: error
  language:
    required: ["eng"]
    severity: error
  timing:
    max_overlaps: 0
    max_gap_ms: 5000
    max_cue_duration_ms: 7000
    severity: warning
  content:
    max_malformed: 0
    max_chars_per_line: 42
    max_lines_per_cue: 2
    severity: warning
  duration_match:
    tolerance_ms: 1000
    severity: warning
```

All five rules are metadata-driven (`CostClass::Metadata`): cue timing comes from
packet headers and cue text from the packet payload, so no picture or sample is
decoded. Character counts are taken **after** stripping SRT/ASS/WebVTT markup
(`{\...}` override blocks, `<i>`-style tags and trailing cue settings), so they
describe what is rendered rather than how the file encodes it. See
[`docs/supported-formats.md`](./supported-formats.md) for which subtitle codecs
are inspected for text.

## 7. Voice rules (optional, spec § 8.8)

```yaml
voice:
  speech:                          # is speech present / absent?
    expect: present                # present | absent
    min_ratio: 0.3                 # share of audio; default 0.05
    severity: error
  silence:                         # longest stretch without speech
    max_non_speech_ms: 8000
  speakers: { min: 1, max: 3 }     # distinct speakers (at least one bound)
  speaker_changes:
    max_per_minute: 20
  transcript:                      # sidecar transcript vs expected text
    max_wer: 0.1
    max_unexpected_words: 2        # default 0
```

Every voice rule needs a mapping (there is no scalar shorthand); `severity`
defaults to `warning`. Detector-based rules are probabilistic and say so in
their findings; they need the `voice` build feature and report `Inconclusive`
without it. See [`voice-and-correction.md`](./voice-and-correction.md).

## 7a. Custom rules (`rules.custom`)

User-defined rules compare one metadata metric with a threshold, so a
facility can encode a delivery requirement without a code change. A worked
profile is in [`profiles/examples/custom-rules.yaml`](../profiles/examples/custom-rules.yaml).

```yaml
rules:
  custom:
    - id: custom.min_video_bitrate    # required; must be custom.<snake_case>
      scope: video                    # container | video | audio | subtitle
      streams: all                    # all (default) | primary; not for container
      metric: bitrate
      op: ">="                        # == != < <= > >= in not_in
      value: 5000000                  # a list for in / not_in
      tolerance: 0                    # numeric == / != only
      severity: error                 # default error
      message: Delivery requires at least 5 Mb/s video   # optional
```

| Scope | Metrics |
|-------|---------|
| `container` | `duration_seconds`, `size_bytes`, `stream_count`, `video_stream_count`, `audio_stream_count`, `subtitle_stream_count` |
| every stream scope | `codec`, `codec_profile`, `language`, `bitrate`, `duration_seconds`, `metadata.<key>` (per-stream container tag) |
| `video` | `width`, `height`, `frame_rate`, `pixel_format`, `field_order` |
| `audio` | `channels`, `channel_layout`, `sample_rate`, `bit_depth` |

Behaviour:

- The profile parser rejects unknown metrics, operators and keys, duplicate
  ids, ordering operators on text metrics and type mismatches (quote
  numeric-looking text such as `"264"`). At most 256 custom rules per profile.
- Text comparison is case-insensitive. A rule that holds produces no finding.
- A metric the stream does not report yields `Inconclusive`, never a guessed
  pass or fail. A scope with no streams of that kind passes (use
  `container.stream_presence` to require streams).
- Each failing stream yields one finding carrying the measured value, the
  expected comparison and the stream index. Custom rules are metadata-only.
- Each rule has its own configuration hash, so editing one custom rule only
  invalidates that rule's cached results.

## 8. Policy

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