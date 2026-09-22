//! TPT Media QC command-line entry point ([spec § 16], § 21).
//!
//! Exit codes are a stable contract:
//! * `0` — all rules passed
//! * `1` — passed with warnings (or inconclusive findings on a full run)
//! * `2` — failed (findings at/above the profile's `fail_on` severity)
//! * `3` — execution error (probe/I/O failure, invalid input)
//! * `4` — asset path not found
//! * `5` — profile missing or invalid
//! * `6` — no usable inspector available for the requested depth

use clap::Parser;

fn main() {
    let code = tpt_app_media_qc_cli::run(tpt_app_media_qc_cli::Cli::parse());
    std::process::exit(code);
}