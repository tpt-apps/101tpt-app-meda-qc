# tpt-app-media-qc-cli

First-class command-line interface for [TPT Media QC](../../README.md)
(spec §16, §21).

The `tpt-media-qc` binary exposes `info`, `check`, `batch`, `compare`,
`list-rules` and `watch` commands with a stable exit-code contract
(`0` pass, `1` warn — including `Inconclusive` verdicts, `2` fail,
`3` execution error, `4` asset path not found, `5` profile invalid,
`6` no usable inspector). One corrupt asset never aborts a
batch (spec §21). The library target (`app`, `cli`, `exit`, `probe`,
`watch`) exposes the argument model and run loop so test, benchmark and
fuzz harnesses can drive the CLI in-process.

## Quick start

```sh
# Inspect a file
tpt-media-qc info episode-01.mov

# Run QC with the generic profile
tpt-media-qc check --profile profiles/generic/generic.yaml --json out.json episode-01.mov

# Batch-analyse a folder, one JSON report per asset
tpt-media-qc batch --profile profiles/generic/generic.yaml --input ./incoming --output ./reports

# Compare two deliveries (exit 0 = match, 1 = minor, 2 = major differences)
tpt-media-qc compare master_v1.mov master_v2.mov --json diff.json

# List the rules a profile enables
tpt-media-qc list-rules --profile profiles/generic/generic.yaml

# Watch a folder and route arrivals into pass/warn/fail directories
tpt-media-qc watch --input ./incoming --pass ./ok --warn ./review --fail ./reject
```

## Example (in-process)

```rust
use clap::Parser;
use tpt_app_media_qc_cli::Cli;

let cli = Cli::try_parse_from(["tpt-media-qc", "list-rules"])?;
let _ = cli.command;
# Ok::<(), clap::Error>(())
```

## Spec references

- §13 (watch folders), §14 (reports), §16 (CLI/exit codes), §21 (failure isolation).

## License

Dual-licensed MIT / Apache-2.0. See `LICENSE-MIT` and `LICENSE-APACHE` at the workspace root.
