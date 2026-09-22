//! Fuzz the ffprobe JSON result parser ([spec § 24.4]; CLI `probe` crate).
//!
//! Arbitrary JSON is deserialised into the ffprobe shape and mapped into an
//! [`Inspection`]; the mapping must be total and panic-free.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = tpt_app_media_qc_cli::probe::parse_ffprobe_json(data);
});