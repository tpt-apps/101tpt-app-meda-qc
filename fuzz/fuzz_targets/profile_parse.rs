//! Fuzz the YAML profile parser ([spec § 24.4]; profile crate).
//!
//! Feed arbitrary bytes as profile YAML. The parser must return an error, never
//! panic. Both lossy UTF-8 and valid-UTF-8 inputs are exercised.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Valid UTF-8 input hits the strict parser directly.
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = tpt_app_media_qc_profile::Profile::from_yaml(s);
    }
    // Lossy conversion keeps exploring when invalid bytes slip in.
    let lossy = String::from_utf8_lossy(data);
    let _ = tpt_app_media_qc_profile::Profile::from_yaml(&lossy);
});