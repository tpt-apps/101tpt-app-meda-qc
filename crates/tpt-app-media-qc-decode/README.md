# tpt-app-media-qc-decode

Decode-backed measurement adapters for [TPT Media QC](../../README.md)
(spec §10, §8.2–§8.3).

Kinetix owns MP4/ISO-BMFF, Matroska/WebM and MPEG-TS demuxing plus AV1/VP9
reconstruction; Cadence owns standalone WAV, AIFF/AIFC and FLAC reading.
This crate reduces decoded frames/samples to the stable `Inspection` model
consumed by QC rules: frame rate, black/freeze/duplicate segments,
corrupt-frame errors, luma statistics, dead-pixel clustering, flash
screening, silence, clipping, peak, stereo phase, DC offset, BS.1770
loudness and true peak.

Royalty-free codecs only: H.264 is deliberately **not** decoded (AVC patent
pools license decoders as well as encoders). Such assets keep full
ffprobe metadata/container QC and their frame-decode rules report
`Inconclusive`. Both video decoders run in strict mode — a stream that
cannot be reconstructed faithfully is unmeasurable, not approximated. The
pinned demuxers are in-memory, so inputs over 512 MiB are refused
(`DEFAULT_MAX_DECODE_INPUT_BYTES`); frames are analysed one at a time.

## Items

- `KinetixVideoInspector` / `VideoAnalyzerConfig` — video decode adapter
  (AV1/VP9 over MP4, Matroska/WebM, MPEG-TS).
- `CadenceAudioInspector` / `AudioAnalyzerConfig` — standalone audio adapter
  (WAV, AIFF/AIFC, FLAC).
- `DEFAULT_MAX_DECODE_INPUT_BYTES` — 512 MiB in-memory demux bound.

## Example

```rust
use tpt_app_media_qc_decode::{
    AudioAnalyzerConfig, CadenceAudioInspector, KinetixVideoInspector,
    DEFAULT_MAX_DECODE_INPUT_BYTES,
};

let video = KinetixVideoInspector::new();
let audio = CadenceAudioInspector::with_config(AudioAnalyzerConfig::default());
assert_eq!(DEFAULT_MAX_DECODE_INPUT_BYTES, 512 * 1024 * 1024);
let _ = (video, audio);
```

## Spec references

- §8.2–§8.3 (decode measurements), §8.4 (visual extensions), §10 (pipeline).

## License

Dual-licensed MIT / Apache-2.0. See `LICENSE-MIT` and `LICENSE-APACHE` at the workspace root.
