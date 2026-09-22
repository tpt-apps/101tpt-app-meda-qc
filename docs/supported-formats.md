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

## 2. Full-decode roadmap

Decode-based measurements (black/freeze/duplicate/corrupt frames, luma,
silence, clipping, peak, true peak, loudness, phase, DC offset) require the TPT
media foundation stack (spec §5, §31):

- **`tpt-kinetix`** — container parsing, demuxing, decoding, frame access,
  timestamps, media pipeline. "The application should never implement its own
  parallel media decoder stack."
- **`tpt-cadence`** — audio packet decoding and PCM extraction.
- **`tpt-dsp`** — signal-level analysis (RMS, peak, true peak, silence,
  clipping, phase, loudness processing).
- **`tpt-visual`** — frame inspection, colour analysis, HDR, image metrics.

Until those are integrated, decode rules correctly report `Inconclusive`
(spec §3.4) for any file whose measurements are absent.

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