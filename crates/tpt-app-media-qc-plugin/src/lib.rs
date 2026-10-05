//! Plugin host (spec § 27 "plugin SDK", "third-party rules").
//!
//! # Trust model
//!
//! Installing a plugin means placing it in the plugin directory the operator
//! chose; that is the trust decision. A *profile* can only name a rule id, so
//! a profile received from someone else can never make the tool start an
//! arbitrary program. Plugins run as separate processes with a cleared
//! environment (only `PATH` and, on Windows, `SystemRoot`/`windir` are passed),
//! the plugin directory as working directory, a hard timeout and a response
//! size limit. They receive measurements, not media bytes. They are **not**
//! otherwise sandboxed: a plugin can still use the network or files the user
//! can, so only install plugins you trust.

mod manifest;
mod registry;
mod run;

pub use manifest::{ManifestRule, PluginDecode, PluginManifest, MAX_TIMEOUT_MS};
pub use registry::{PluginError, PluginInfo, PluginRegistry};
