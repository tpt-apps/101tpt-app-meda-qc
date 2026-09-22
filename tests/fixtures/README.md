# Media fixtures

Controlled media inputs for integration and golden testing (spec §24.2, §24.5).
Each fixture directory maps to a QC concern:

```
fixtures/
├── valid/        # clean, spec-conformant assets (one per codec/container)
├── corrupt/      # truncated/atom-broken containers, malformed metadata
├── loudness/     # known-LUFS masters (R128/A85 reference workloads)
├── freeze/       # freeze-frame segments of known length
├── black/        # black-frame segments of known length
├── clipping/     # clipping / overs) signals
├── sync/         # audio/video duration and timestamp mismatch cases
├── hdr/          # HDR metadata and colour-space cases
├── interlace/    # interlacing / field-order cases
└── subtitles/    # subtitle/caption validation cases (post-MVP)
```

Every fixture here must have a corresponding expected result in
[`../golden/`](../golden/). Attribution: only add assets you have the right to
include (own content or permissive licence); record provenance in
`ATTRIBUTION.md` if you add one. Keep fixtures as small as possible —
synthetic WAV/raw-RGB heads and short clips are preferred over full-length
media.

Runner rules:

- Files in `fixtures/` without an extension are ignored by the runner.
- A fixture whose golden expectation is "ignore" is a placeholder and is
  skipped, not failed.