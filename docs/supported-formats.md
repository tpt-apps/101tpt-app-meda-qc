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

Full-decode adapters are deliberately narrow and capability-driven:

- **Video containers:** MP4/ISO-BMFF, Matroska/WebM and MPEG-TS (188-byte transport
  streams, incl. HLS segments) through `tpt-kinetix-demux`.
- **Video codec:** AV1 and VP9 through `tpt-kinetix-av1` / `tpt-kinetix-vp9`.
  **No H.264/AVC decoder ships.** AVC patent pools license decoders as well as
  encoders, so H.264 assets are still fully inspected for metadata and container
  rules by `ffprobe`, while their frame-decode rules report `Inconclusive`.
- **Video measurements:** observed frame rate, black-frame ranges, freeze-frame
  ranges, duplicate-frame ranges, decode-error count and all-sample luma
  statistics (min/max/mean/legal-range fractions).
- **Standalone audio containers:** WAV, AIFF/AIFC and FLAC through the pinned
  Cadence readers.
- **Audio measurements:** decoded-frame coverage, silence ranges, clipping
  events, sample peak, true peak (BS.1770-4 Annex 2), gated integrated
  loudness (BS.1770-4 K-weighting and §5 gating), stereo phase correlation and
  DC offset. Silence ranges are bounded; if their retention limit is reached,
  the silence rule returns `Inconclusive`. Loudness range (EBU Tech 3342) needs
  at least 3 s of audio, and integrated loudness needs at least one complete 400 ms
  gating block — shorter files report it as unmeasured rather than as a
  compliant value.
- **Unsupported/incomplete input:** adapters record no decoded coverage or an
  explicit incomplete state and the affected rules return `Inconclusive`; they
  never turn an empty measurement into a pass. A container or codec that cannot
  be decoded (H.264, HEVC, ProRes, MXF, …) is reported as unsupported and never
  as a decode error, so `video.corrupt_frames` cannot fail an asset simply
  because it was not decoded.

The pinned Kinetix demuxers are currently in-memory. The video adapter
therefore refuses files over 512 MiB rather than allocating without a bound.
Embedded audio, HEVC, MXF and other containers remain follow-up work through
the Kinetix/Cadence bridge and other foundation crates. MPEG-TS coverage is
188-byte packets only: audio elementary streams inside a transport stream are
demuxed but not decoded (the Cadence readers take standalone files), so a
broadcast `.ts` gets full video frame QC while its audio silence/clipping/
loudness findings stay `Inconclusive`.

## 3. Intended target coverage (post-integration)

- **Containers:** MP4/MOV (QuickTime), MXF (OP1a, OP-Atom), MPEG-TS/M2TS, MKV,
  WebM, AVI, WAV/W64, MP3, FLAC, Ogg/Opus, AC-3/E-AC-3, AAC (in supported
  containers).
- **Video codecs:** AV1, VP9 today; H.265/HEVC, ProRes, XAVC, DNxHD/HR,
  MPEG-2, VC-1 as the foundation matures. H.264/AVC is intentionally absent
  (patent licensing); its assets stay metadata-inspectable.
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
- **Subtitle/caption streams:** validated by the `subtitle.*` rules (spec §8.7) —
  presence, language, cue timing (overlaps, invalid durations, gaps, cue length),
  cue content (malformed/empty payloads, characters per line, lines per cue) and
  coverage against the video duration. Cue timing is read for **every** subtitle
  codec. Cue *text* — and therefore the character-limit, empty-cue and
  malformed-payload checks — requires a text-based payload and is limited to
  `subrip`, `srt`, `ass`, `ssa`, `webvtt`, `text`, `microdvd`, `mpl2` and
  `subviewer`. Bitmap/structured formats (`dvdsub`, `hdmv_pgs_subtitle`,
  `dvb_subtitle`, `mov_text`) are timing-only and report `Inconclusive` for the
  text-dependent checks rather than guessing. Cue analysis is capped at 100 000
  cues per stream. Voice streams remain a placeholder (spec §8.8).

## 5. Security note

Media parsers never *execute* embedded content, and parser code is a target
for fuzzing (spec §22, §24.4). The probe boundary runs `ffprobe` as a separate
process; when the in-process TPT stack is integrated, its parsers will be fuzz
tested before being accepted. Media/profile inputs are canonicalized before
use, and report exporters stage output in securely created temporary files
before replacing the requested destination.