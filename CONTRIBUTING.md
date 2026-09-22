# Contributing

Thanks for contributing to **TPT Media QC** — the commercial application tier
of the TPT Apps family.

## Licensing

TPT Media QC is **dual-licensed MIT / Apache-2.0**, copyright © TPT Solutions.
By submitting a contribution you agree that it is licensed under both terms
(see [`LICENSE-MIT`](./LICENSE-MIT) and [`LICENSE-APACHE`](./LICENSE-APACHE)).
Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this project is licensed as above.

Product-specific business logic lives in this repository and must **not** be
copied back into the open-source TPT foundation crates. Generic improvements
that are genuinely reusable infrastructure are welcome contributions to the
foundations instead (spec §23).

## Developer setup

Requirements:

- Rust 1.75 or newer (matches the workspace `rust-version`).
- `ffprobe` on `PATH` for the metadata probe front-end (only needed to run
  real-file checks; the test suite does not require it).

Build and test:

```sh
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

The repository lives in one workspace with the layout documented in
[`README.md`](./README.md) and [`docs/architecture.md`](./docs/architecture.md).

## Project conventions

- **Source of truth.** The product spec ([`spec.txt`](./spec.txt)) and the
  phased checklist ([`todo.md`](./todo.md)) define what we build. Update
  `todo.md` when you complete or re-scope work.
- **Determinism.** Same input bytes + same application version + same profile
  must give the same result (spec §3.2). Caching keys and report integrity
  depend on this. Never add wall-clock, randomness or map iteration order where
  it can leak into results or fingerprints.
- **Uncertainty is explicit.** Rules report `Inconclusive` rather than guessing
  on missing measurements (spec §3.4).
- **No unbounded buffering.** Streaming/incremental processing only; a
  two-hour file must not be decoded into RAM wholesale (spec §20).
- **Findings explain themselves.** A finding always carries measured/expected
  values, stream, time/frame range, severity and confidence where applicable
  (spec §3.3).
- **Security by default.** No mandatory network access, no uploads, no
  telemetry, localhost-only API. Malformed media must be handled safely and
  must never crash a batch (spec §21, §22).

## Testing

- Unit tests ship with each QC rule (valid, invalid, boundary, malformed,
  expected result — spec §24.1).
- Golden-media fixtures live in [`tests/golden`](./tests/golden) with an
  expected result per fixture (spec §24.2).
- Property tests cover timecode/frame/duration math, threshold logic, profile
  parsing and result aggregation (spec §24.3).
- Fuzzing targets cover parsers and media-boundary code (spec §24.4); run them
  with `cargo fuzz` and re-run any corpus on regressions.
- **Every production bug gets a regression fixture** (spec §24.5): add the
  reproduction input and a test that documents the expected outcome.

## Commit and review etiquette

- Keep changes small and reviewable; prefer focused commits.
- Add a `CHANGELOG.md` entry for user-visible changes.
- Match existing code style; the workspace lints forbid `unsafe` and warn on
  clippy.
- Update `docs/` when public-facing behaviour, the profile format or the report
  format changes.