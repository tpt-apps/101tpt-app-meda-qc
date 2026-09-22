# Performance benchmarks

Benchmark harness for the spec §20 targets. See
[`docs/performance.md`](../../docs/performance.md) for the goals and the
method note: don't infer performance from placeholder probes — numbers only
count once compiled with the real decode stack.

Planned benchmarks (criterion-based, released under `--release`):

- `metadata_scan`: files/sec for the metadata-only pass on ordinary HD files —
  the "results within ~1 second" check.
- `full_qc_throughput`: full-QC speed vs. real-time on a fixture (e.g. a
  10-minute file QC'd in N seconds).
- `peak_rss`: peak resident memory on a 2-hour fixture — the "don't scale
  memory with duration" check.
- `batch_scale`: concurrent-batch throughput at default worker count vs. a
  constrained count.
- `ui_headroom`: a probe loop stays interactive during full QC (Tauri).

Each benchmark writes a JSON report into this directory so results can be
diffed across commits. Do not commit oversized fixture media; synthetic
streams are generated in-process where possible.