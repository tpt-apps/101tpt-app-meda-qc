//! Fuzz CLI argument parsing ([spec § 24.4]; CLI `cli` crate).
//!
//! Arbitrary tokens are fed through the clap model. Argument *values* that
//! reach subcommand handlers must never panic the parser, and unknown
//! combinations must produce a clap error rather than a crash.

#![no_main]

use clap::Parser;
use libfuzzer_sys::fuzz_target;
use tpt_app_media_qc_cli::cli::Cli;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let argv: Vec<String> = std::iter::once("tpt-media-qc".to_string())
        .chain(text.split_whitespace().map(|t| t.to_string()))
        .collect();
    let _ = Cli::try_parse_from(argv);
});