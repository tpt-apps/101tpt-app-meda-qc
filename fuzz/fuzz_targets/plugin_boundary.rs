//! Plugin boundary fuzzer (spec § 24.4, § 27): the two documents that cross
//! the plugin trust boundary, a plugin manifest and a plugin response, must be
//! parsed or rejected without panicking, and whatever parses must validate
//! without panicking.

#![no_main]

use libfuzzer_sys::fuzz_target;
use tpt_app_media_qc_plugin::PluginManifest;
use tpt_app_media_qc_plugin_sdk::{PluginRequest, PluginResponse};

fuzz_target!(|data: &[u8]| {
    if let Ok(manifest) = serde_json::from_slice::<PluginManifest>(data) {
        let _ = manifest.validate();
        let _ = manifest.resolve_command(std::path::Path::new("."));
    }
    let _ = serde_json::from_slice::<PluginResponse>(data);
    let _ = serde_json::from_slice::<PluginRequest>(data);
});
