//! TPT Media QC command-line interface ([spec § 16], § 21).
//!
//! Exit codes are a stable contract:
//! * `0` — all rules passed
//! * `1` — passed with warnings (or inconclusive findings on a full run)
//! * `2` — failed (findings at/above the profile's `fail_on` severity)
//! * `3` — execution error (probe/I/O failure, invalid input)
//! * `4` — asset path not found
//! * `5` — profile missing or invalid
//! * `6` — no usable inspector available for the requested depth

mod app;
mod cli;
mod exit;
mod probe;

use clap::Parser;

fn main() {
    let args = cli::Cli::parse();
    let code = app::run(args);
    std::process::exit(code);
}