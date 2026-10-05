# Voice checks and automated correction (Phase 3)

Both are optional. Basic deterministic QC never needs either of them
(spec §8.8, `tpt-voice` section).

## Voice checks (`rules.voice`)

Five rules, configured in a profile (see [`profile-format.md`](./profile-format.md)
§7) and run only when the profile asks for them:

| Rule | Question | Source |
|------|----------|--------|
| `voice.speech` | Is speech present (or absent)? | Probabilistic: energy-based voice activity detection |
| `voice.silence` | Is there a stretch longer than N ms without speech? | Probabilistic: same detector, reported with its time range |
| `voice.speakers` | How many distinct speakers? | Probabilistic: unsupervised speaker clustering |
| `voice.speaker_changes` | How often does the speaker change? | Probabilistic: adjacent speaker turns |
| `voice.transcript` | Does the transcript match the expected one? | Exact text comparison |

Findings from the four detector rules start with `[probabilistic]` and should
be read as "worth a human look", not as verdicts about the audio. Without
measurements every voice rule is `Inconclusive`, never pass or fail.

### Speech and speaker analysis

Needs a build with the `voice` feature, which pulls in `tpt-voice`'s
weight-free classical path (no trained model is used or shipped):

```sh
cargo build --release -p tpt-app-media-qc-cli --features voice
```

* Input must be a standalone **WAV, AIFF/AIFC or FLAC** file. Embedded audio
  in MP4/Matroska/MPEG-TS is not decoded (see
  [`supported-formats.md`](./supported-formats.md)).
* The first **3600 s** of audio are analysed; the report states how much.
* Audio is mixed to mono and reduced to 16 kHz first.
* Not implemented: overlapping-speech (dialogue overlap) detection — the
  classical diarizer produces one speaker per moment — and intelligibility
  metrics.
* Runs with a profile that sets any of `speech`, `silence`, `speakers` or
  `speaker_changes`. Quick (metadata-only) scans never run it.

### Transcript validation

There is **no speech-to-text engine** in the product: `tpt-voice`'s
transcriber has no trained weights yet. Instead the transcript comes from the
facility's own ASR or captioning workflow, as sidecar files next to the media:

* `<name>.expected.txt` — the script or approved transcript
* `<name>.transcript.txt` or `<name>.transcript.json` — what the audio says.
  JSON may be `{"text": "..."}` or `{"segments": [{"text": "..."}]}`.

The comparison is exact and deterministic: words are lower-cased and stripped
of punctuation, then aligned by minimum edit distance. The finding reports the
word error rate with substitution, deletion and insertion counts, and lists
words in the transcript that occur nowhere in the expected text. It does not
check that the transcript matches the audio; that depends on whatever
produced it, and the finding names the source file. Transcripts longer than
20 000 words are refused rather than truncated. This part needs no feature
flag.

```sh
tpt-media-qc check episode.wav -p profiles/streaming/podcast-voice.yaml
```

## Automated correction (`correct`)

```sh
tpt-media-qc correct master.wav -p profiles/broadcast/ebu-r128.yaml          # writes master.corrected.wav
tpt-media-qc correct master.wav --target-lufs -16 --true-peak-db -1 --dry-run
```

Deliberately narrow, because a correction must be exact and checkable:

* **Loudness**: one linear gain to reach the target integrated loudness
  (BS.1770). The target and ceiling default to the profile's `audio.loudness`
  and `audio.true_peak` rules, otherwise `--target-lufs` / `--true-peak-db`
  (ceiling default −1 dBTP).
* **True-peak safety**: if the gain would push the true peak over the ceiling
  the gain is **refused** and the reason printed. There is no limiter or
  compressor, so the tool never quietly changes dynamics. Exit code 1.
* **DC offset**: each channel's measured mean is subtracted (skip with
  `--no-dc`).
* **Output**: a new 16- or 24-bit PCM WAV (`--bits`, default 24), written
  through a `.part` file and renamed. The input is never modified, an existing
  output is never overwritten, and the output cannot be the input.
* **Verification**: the written file is re-measured, so the printed "after"
  values are real. Rounding to 16 bit is not dithered.
* Inputs are WAV, AIFF/AIFC or FLAC. Video, embedded audio and other fixes
  (trims, de-clipping, re-timing) are not corrected.

## Advanced profiles

Added under `profiles/`, each with a header stating that its values are
indicative conventions rather than a conformance claim (spec §25):

| Profile | File | Notes |
|---------|------|-------|
| `broadcast-uk-hd` | `broadcast/uk-hd-file-delivery.yaml` | 1080/25, 16:9, EBU R128 −23 LUFS ±1, −1 dBTP, PSE screening |
| `streaming-ott-premium` | `streaming/ott-premium.yaml` | −27 LUFS ±2 (integrated, **not** dialogue-gated), −2 dBTP, integrity checks |
| `streaming-online-upload` | `streaming/online-video-upload.yaml` | −14 LUFS, −1 dBTP, mostly warnings |
| `streaming-podcast-voice` | `streaming/podcast-voice.yaml` | −16 LUFS, speech present, transcript match |

All four are bundled in the desktop app's profile list.
