# Supported Formats

This page documents which containers, codecs and stream kinds TPT Media QC
inspects today and how format coverage is governed. **Format support is driven
by the capabilities of the probe stack**, not hard-coded UI logic (spec §8.7).

## 1. Current status: ffprobe metadata front-end

The shipped probe boundary (`FfprobeInspector` in the CLI crate) shells out to
the system `ffprobe`. Every container and codec `ffprobe` can parse is visible
to the metadata rules; the *supported* set is therefore whatever your `ffprobe`
build supports. The media extensions the CLI batch scanner recognises are:

`mov`, `mp4`, `mxf`, `m4v`, `mkv`, `ts`, `mts`, `m2ts`, `wav`, `aac`, `w64`,
`ac3`, `eac3`, `mp3`, `flac`, `opus`, `webm`, `avi`.

Metadata inspection covers, per stream: codec, codec profile, dimensions,
pixel format, frame rate, time base, bitrate, duration, language, channel
layout, channels, sample rate and bit depth, plus container format, duration,
bitrate and timecode presence.

## 2. Full-decode coverage

The first full-decode adapter is deliberately narrow and capability-driven:

- **Container:** MP4/ISO-BMFF through `tpt-kinetix-demux`.
- **Video codec:** H.264/AVC through `tpt-kinetix-h264`.
- **Measurements:** observed frame rate, black-frame ranges, freeze-frame
  ranges, duplicate-frame ranges, decode-error count and all-sample luma
  statistics (min/max/mean/legal-range fractions).
- **Unsupported/incomplete input:** the adapter records no decoded-frame
  coverage and the rules return `Inconclusive`; it never turns an empty
  measurement into a pass.

The pinned Kinetix MP4 demuxer is currently in-memory. The adapter therefore
refuses files over 512 MiB rather than allocating without a bound. This is a
foundation limitation, not a claim that large-file decoding is complete.
Audio decode, HEVC/AV1/VP9, MXF, MPEG-TS and other containers remain follow-up
work through Cadence and the other foundation crates.

## 3. Intended target coverage (post-integration)

- **Containers:** MP4/MOV (QuickTime), MXF (OP1a, OP-Atom), MPEG-TS/M2TS, MKV,
  WebM, AVI, WAV/W64, MP3, FLAC, Ogg/Opus, AC-3/E-AC-3, AAC (in supported
  containers).
- **Video codecs:** H.264/AVC, H.265/HEVC, AV1, VP9, ProRes, XAVC, DNxHD/HR,
  MPEG-2, VC-1 as the foundation matures.
- **Audio codecs:** PCM, AAC, AC-3/E-AC-3, MP3, FLAC, Opus, DTS as handled by
  `tpt-cadence`.

Coverage claims will be updated here when the decode stack is integrated and
validated against fixtures.

## 4. Handling unknown formats

- **Metadata path:** unreadable/non-media files surface as a container
  validity finding (`readable`, `container_validity`) rather than a crash —
  a single corrupt asset must never terminate a batch (spec §21).
- **Batch scanning:** only the extensions listed above are picked up for
  directory scans; explicit files are attempted regardless of extension.
- **Subtitle/caption and voice streams:** structural support exists in the
  model and profile format; validation rules land post-MVP (spec §8.7, §8.8).

## 5. Security note

Media parsers never *execute* embedded content, and parser code is a target
for fuzzing (spec §22, §24.4). The probe boundary runs `ffprobe` as a separate
process; when the in-process TPT stack is integrated, its parsers will be fuzz
tested before being accepted.