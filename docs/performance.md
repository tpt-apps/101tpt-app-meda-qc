# Performance

Performance targets are engineering goals, not guarantees (spec §20). They are
measured on representative fixtures, not optimised blindly.

## 1. Targets

| Area | Target |
|------|--------|
| Metadata scan | Begin displaying results within ~1 second for ordinary files |
| Full QC | Faster-than-real-time for common technical checks on commodity hardware |
| Expensive perceptual checks | May legitimately be slower |
| Memory | Must not scale linearly with media duration |
| Concurrency | Use useful CPU cores while respecting user-configurable limits; never starve the desktop UI |
| GPU | Optional; CPU execution must remain correct |

## 2. Current design notes

- **Bounded memory.** Fingerprinting streams the file in 64 KiB buffers.
  The scheduler never buffers unbounded batches; results are collected
  in input order with bounded parallelism.
- **Two-pass analysis** (spec §10.1): a quick metadata pass produces results
  in seconds without full decode; the decode pass only runs checks that need
  frames/samples.
- **Cost-class concurrency** (spec §11): each job is classified metadata /
  cheap-decode / full-decode / GPU / expensive-analysis, and the scheduler
  limits concurrency per cost class (see
  `tpt-app-media-qc-core::cost::SchedulerLimits`).
- **No repeated decoding.** The pipeline merges one decode inspection into the
  metadata baseline; rules read shared measurements. A rule-level cache
  (spec §19) is planned to avoid re-running unchanged rules across runs.

## 3. Benchmarking (planned)

A benchmark suite against representative HD (1920×1080) and UHD (3840×2160)
fixtures will be added under [`tests/performance/`](../tests/performance/),
measuring:

- metadata-only scan throughput (files/sec),
- full-QC throughput vs. real-time (e.g. a 10-minute file QC'd in N seconds),
- peak RSS on a 2-hour fixture (the memory linearity check),
- concurrent-batch scaling on all cores vs. a constrained worker count,
- the desktop-UI starvation check (keep a probe frame interactive during full
  QC).

Throughput numbers will be recorded in the repo when fixtures exist and the
decode stack is integrated — do not infer performance from placeholder
probes.

## 4. Concurrency model

- `run_jobs` uses `std::thread::scope` with a shared atomic index and
  per-cost-class worker bounds; results are written back in input order.
- The OOM-safe rule: never materialise the full decoded asset in memory
  (spec §20). Decode pipelines will stream frame-by-frame through rules.
- A Tauri desktop build must leave headroom for the UI thread; worker counts
  are user-configurable.

## 5. How to profile

```sh
cargo run --release -- batch --profile profiles/generic/generic.yaml --input ./incoming --out ./reports
```

Instrument with a profiler of your choice and `--release` (the dev profile
enables `opt-level = 1` and `lto = "thin"`; release uses `codegen-units = 1`).
Record results in the benchmark suite when it lands.