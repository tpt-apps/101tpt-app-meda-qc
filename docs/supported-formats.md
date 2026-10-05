# Supported Formats

TPT Media QC reads **royalty-free formats only**, with its own in-process
parsers. It does not use FFmpeg, ffprobe or any other external program, and it
never parses the bitstream of a patent-encumbered codec. This page lists what
is inspected, what is refused, and why.

## 1. What is inspected

| Kind | Inspected |
|------|-----------|
| Containers | ISO-BMFF (`.mp4`, `.m4v`, `.mov`), Matroska/WebM (`.mkv`, `.webm`), MPEG-TS (`.ts`, `.mts`), WAV, AIFF/AIFC, FLAC, Ogg |
| Video codecs | AV1, VP9 |
| Audio codecs | Opus, Vorbis, FLAC, PCM (integer and float) |
| Subtitles | WebVTT, SRT, ASS/SSA (Matroska); `wvtt`, `tx3g` (MP4) |
| Other tracks | MOV/MP4 `tmcd` timecode is read as metadata; other data, image-subtitle and attachment tracks are listed but not interpreted |

The container is identified from the file's **content**, never its extension.

Per stream the metadata pass reports: codec, codec profile, dimensions, pixel
format, frame rate (from the container's sample timing, not a declared value),
time base, bitrate, duration, language, channel layout, channels, sample rate,
bit depth and stream tags; for video also colour tags and HDR static metadata
(ST 2086 mastering display, MaxCLL/MaxFALL) and field order; and per container
the format, duration, bitrate, timecode presence, timestamp gaps and
malformed-metadata notes.

## 2. What is refused

A file containing **any audio or video codec outside the table above** is
refused as an unsupported format. The tool prints
`unsupported: <what was found>. TPT Media QC inspects royalty-free formats only (...)`, exits with code 3, and writes no
report; batch and watch-folder runs continue with the next file. Nothing in
such a file is parsed (the track list is classified by codec name only).

This covers, among others: H.264/AVC, HEVC/H.265, MPEG-2, MPEG-4 visual,
Apple ProRes, DNxHD/HR, XDCAM, AAC, AC-3/E-AC-3, DTS, MP3, Apple Lossless,
and the MXF, AVI, FLV, MPEG-PS and Matroska files that carry them. A file
whose container is not recognised at all (MXF, AVI, raw ADTS/MP3, ...) is
refused the same way.

Why: AVC, HEVC, AAC and the other MPEG-family codecs are covered by patent
pools that license decoders and encoders. Reading even their headers would
mean shipping code that implements part of those standards, so the product
does not. AV1 and VP9 are designed to be royalty-free. If a delivery you need
to QC is in a refused format, transcode it to a supported one first or use a
tool that carries the licences.

## 3. Damaged files are findings, not errors

A file in a **supported** container that is damaged (truncated, missing its
index, bad chunk sizes) is not refused. It is inspected as far as possible and
reported as a container-validity problem, so `container.readable` and
`container.container_validity` fail with an explanation. Timestamp gaps and
backward jumps, MPEG-TS continuity/CRC errors, and inconsistent sample tables
are reported through `container.timestamp_continuity` and
`container.malformed_metadata`.

Fragmented MP4 (CMAF/DASH segments, `moof` boxes) is not read yet: fields that
depend on sample tables are left unmeasured and the affected rules report
`Inconclusive`.

## 4. Full-decode coverage

Decode adapters are narrow and capability-driven:

- **Video:** AV1 and VP9 in MP4, Matroska/WebM and MPEG-TS, through
  `tpt-kinetix-demux`, `tpt-kinetix-av1` and `tpt-kinetix-vp9`. Measurements:
  observed frame rate, black/freeze/duplicate-frame ranges, decode-error count,
  luma statistics and the perceptual, dead-pixel and flash analyses.
- **Standalone audio:** WAV, AIFF/AIFC and FLAC through the pinned Cadence
  readers. Measurements: decoded-frame coverage, silence ranges, clipping, sample
  peak, true peak (BS.1770-4 Annex 2), gated integrated loudness (BS.1770-4),
  loudness range (EBU Tech 3342), stereo phase and DC offset. Loudness needs at
  least one complete 400 ms gating block and loudness range at least 3 s; shorter
  audio reports them as unmeasured, never as a compliant value.
- **Not decoded:** audio inside MP4/Matroska/MPEG-TS (Opus, Vorbis, FLAC, PCM
  elementary streams) and Ogg files are inspected for metadata only, so their
  silence, clipping, peak and loudness rules report `Inconclusive`. The video
  adapter refuses inputs over 512 MiB because the pinned demuxers are
  in-memory.
- Unsupported or incomplete coverage always yields `Inconclusive`; an empty
  measurement is never turned into a pass.

## 5. Subtitles and captions

The `subtitle.*` rules (spec §8.7) cover presence, language, cue timing
(overlaps, invalid durations, gaps, cue length), cue content (malformed and
empty payloads, characters per line, lines per cue) and coverage against the
video duration. Cue timing is read for every subtitle track. Cue text, and
therefore the character-limit, empty-cue and malformed-payload checks, needs a
text codec: `subrip`, `ass`, `ssa`, `webvtt` or `tx3g`. Image and other
subtitle tracks are listed and checked for timing only and report
`Inconclusive` for text-dependent checks. Cue analysis is capped at 100 000 cues
per stream. Optional voice analysis of standalone audio is described in
[`voice-and-correction.md`](./voice-and-correction.md).

## 6. Batch scanning

Directory scans pick up files with these extensions: `mp4`, `m4v`, `mov`,
`mkv`, `webm`, `ts`, `mts`, `wav`, `aif`, `aiff`, `aifc`, `flac`, `ogg`,
`oga`, `opus`. An explicitly named file is attempted whatever its extension.

## 7. Security note

Parsers never execute embedded content and are fuzz-tested (spec §22, §24.4):
the `container_probe` target feeds arbitrary bytes to the container detector
and every reader. Readers walk files with bounded, seek-based reads and
checked arithmetic, so a hostile or truncated file cannot make the inspector
allocate without bound. Media and profile inputs are canonicalised before use,
and report exporters stage output in securely created temporary files.
