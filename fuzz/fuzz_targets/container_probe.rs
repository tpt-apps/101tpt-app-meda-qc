//! Fuzz the in-process container inspector ([spec § 24.4]).
//!
//! Arbitrary bytes go through content sniffing and whichever container reader
//! claims them. The contract: return `Ok` (possibly a `Corrupt` container) or
//! `Err` (unsupported format), never panic, never allocate without bound and
//! never loop forever. Whatever parses must also convert into an `Inspection`.

#![no_main]

use std::io::Cursor;

use libfuzzer_sys::fuzz_target;
use tpt_app_media_qc_probe::{build_inspection, probe};

fuzz_target!(|data: &[u8]| {
    let len = data.len() as u64;
    if let Ok(probed) = probe(&mut Cursor::new(data), len) {
        let _ = build_inspection(probed, len);
    }
});
