//! Print the full `Inspection` the native probe builds for a file, as JSON.
//!
//! `cargo run -p tpt-app-media-qc-probe --example dump -- <file>`
//!
//! Handy when debugging a reader or checking what a rule will see. Streams are
//! shown with their stream-index and native container id.

use std::fs::File;

use tpt_app_media_qc_probe::{build_inspection, probe};

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: dump <file>");
        std::process::exit(2);
    };
    let mut file = File::open(&path).unwrap_or_else(|e| {
        eprintln!("cannot open {path}: {e}");
        std::process::exit(2);
    });
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    match probe(&mut file, len) {
        Ok(probed) => {
            let inspection = build_inspection(probed, len);
            println!("{}", serde_json::to_string_pretty(&inspection).unwrap());
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(3);
        }
    }
}
