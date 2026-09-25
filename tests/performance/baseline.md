# Benchmark baseline — `qc-bench`

Committed reference run of the `qc-bench` micro-benchmarks (spec §20, todo §24).
Numbers are host-dependent; re-run on the same machine when comparing:

```bash
cargo run -p tpt-app-media-qc-test --release --bin qc-bench
```

## Run metadata

| field      | value                                        |
|------------|----------------------------------------------|
| date       | 2026-09-22                                   |
| host       | 12th Gen Intel(R) Core(TM) i7-12700, 20 logical CPUs |
| OS / arch  | windows / x86_64                             |
| toolchain  | rustc 1.97.1, `--release` (thin LTO, CGU=1)  |
| commit     | workspace @ `c0dc523` + this changeset       |

## Results

```
benchmark                         median/op       min/op       throughput
--------------------------------------------------------------------------
fingerprint_full_scan_8mib          3.86 ms      3.83 ms     2074.1 MiB/s
profile_parse_generic_yaml         75.1 us      72.0 us       19.8 MiB/s
build_rules_generic                  640 ns       622 ns    1563722 ops/s
run_rules_clean_master               1.5 us       1.4 us     689417 ops/s
engine_check_noop_inspection         4.8 us       4.7 us     210084 ops/s
fixture_deserialize                  3.6 us       3.6 us      587.2 MiB/s
report_build_and_json               26.8 us      26.1 us      37332 ops/s
report_render_html                  1.04 ms      1.01 ms        959 ops/s
report_render_pdf                  750.5 us     670.3 us       1333 ops/s

9 samples per bench, 8 iterations per sample; lower is better.
```

## Reading

- **Fingerprinting** sustains ~2 GiB/s SHA-256 — a 4 GiB master hashes in
  ~2 s, comfortably inside the import budget.
- **Rule engine** (`run_rules`, 29 rules over a two-stream inspection) is
  ~1.5 µs — metadata QC is effectively free next to any probe I/O; the
  "~1 second per file" budget (spec §20) is dominated by `ffprobe`, not QC.
- **Report rendering** (PDF ≈ 0.75 ms, HTML ≈ 1 ms per report) is negligible
  per asset; watch-folder/batch routing will not be report-bound.
- These are engine-side numbers only. The pinned Kinetix and Cadence decode
  adapters are now integrated; representative media benchmarks for black/freeze/
  loudness analysis and peak-RSS measurements remain follow-up work.
