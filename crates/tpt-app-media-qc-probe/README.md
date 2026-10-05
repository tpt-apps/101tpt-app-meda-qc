# tpt-app-media-qc-probe

In-process container inspector for [TPT Media QC](../../README.md). It reads
**royalty-free formats only** and has no dependency on FFmpeg or any external
program.

| Kind | Inspected |
|------|-----------|
| Containers | ISO-BMFF (MP4/M4V/MOV), Matroska/WebM, MPEG-TS, WAV, AIFF/AIFC, FLAC, Ogg |
| Video | AV1, VP9 |
| Audio | Opus, Vorbis, FLAC, PCM |
| Subtitles | WebVTT, SRT, ASS/SSA, `wvtt`, `tx3g` |

A file whose audio or video uses any other codec (H.264, HEVC, AAC, ProRes,
MPEG-2, AC-3, MP3, ...) is refused with an "unsupported" error; no
bitstream of such a codec is parsed. Format detection is by content, not file
extension. A recognised but damaged file is reported as a container
`Corrupt` finding rather than an error.

`NativeInspector` implements the pipeline's `Inspector::inspect_metadata`.
Decoding (AV1/VP9 frames, audio samples) stays in `tpt-app-media-qc-decode`.
