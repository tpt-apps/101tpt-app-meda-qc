//! SDK for third-party TPT Media QC rules (spec § 27 "plugin SDK").
//!
//! A plugin is a separate program. For every rule use the host starts it,
//! writes one [`PluginRequest`] as JSON on its standard input, and reads one
//! [`PluginResponse`] as JSON from its standard output. Running out of
//! process keeps a faulty or hostile plugin from corrupting the host, lets it
//! be written in any language, and needs no `unsafe` code or dynamic loading.
//!
//! A Rust plugin is a few lines:
//!
//! ```no_run
//! use tpt_app_media_qc_plugin_sdk::{serve, PluginFinding, PluginRequest};
//!
//! fn main() {
//!     std::process::exit(serve(|request: &PluginRequest| {
//!         let min = request.config["min_bps"].as_u64().unwrap_or(0);
//!         request
//!             .asset
//!             .streams
//!             .iter()
//!             .filter(|s| s.bitrate.is_some_and(|b| b < min))
//!             .map(|s| PluginFinding::fail(format!("stream {} is below {min} b/s", s.index)))
//!             .collect()
//!     }));
//! }
//! ```
//!
//! # Contract
//!
//! * The request carries the protocol version, the rule id, the profile's
//!   `config` for that rule, the [`Asset`] and the [`Inspection`]. Plugins see
//!   measurements, never media bytes.
//! * Return no findings for a pass. Report [`PluginStatus::Inconclusive`] when
//!   a measurement you need is missing; never guess.
//! * The host decides severity (from the profile) and the rule id; a plugin
//!   only reports status, message and evidence.
//! * Exit with status 0 and valid JSON. Anything else is reported by the host
//!   as an `Inconclusive` finding naming the failure.
//! * The host enforces a timeout and a response size limit
//!   ([`MAX_RESPONSE_BYTES`]).

use std::io::{Read, Write};

use serde::{Deserialize, Serialize};

pub use tpt_app_media_qc_model::asset::{Asset, Stream, StreamKind};
pub use tpt_app_media_qc_model::finding::TimeRange;
pub use tpt_app_media_qc_model::inspection::Inspection;
pub use tpt_app_media_qc_model::value::Value;

/// Wire protocol version spoken by this SDK.
pub const PROTOCOL_VERSION: u32 = 1;

/// Largest response the host accepts, in bytes.
pub const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

/// Most findings the host keeps from one response.
pub const MAX_FINDINGS: usize = 1000;

/// What the host sends a plugin.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PluginRequest {
    pub protocol: u32,
    /// The rule being run (a plugin may provide several).
    pub rule: String,
    /// The profile's `config` for this rule; `null` when none was given.
    #[serde(default)]
    pub config: serde_json::Value,
    pub asset: Asset,
    pub inspection: Inspection,
}

/// Outcome of one finding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginStatus {
    /// Reported explicitly, e.g. to attach a measured value to a pass.
    Pass,
    Warn,
    Fail,
    /// The rule could not decide (missing measurement).
    Inconclusive,
}

/// One result reported by a plugin.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PluginFinding {
    pub status: PluginStatus,
    pub message: String,
    #[serde(default)]
    pub measured: Option<Value>,
    #[serde(default)]
    pub expected: Option<Value>,
    /// Stream the finding is about.
    #[serde(default)]
    pub stream: Option<u64>,
    #[serde(default)]
    pub time_range: Option<TimeRange>,
}

impl PluginFinding {
    fn new(status: PluginStatus, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            measured: None,
            expected: None,
            stream: None,
            time_range: None,
        }
    }

    pub fn fail(message: impl Into<String>) -> Self {
        Self::new(PluginStatus::Fail, message)
    }

    pub fn warn(message: impl Into<String>) -> Self {
        Self::new(PluginStatus::Warn, message)
    }

    pub fn inconclusive(message: impl Into<String>) -> Self {
        Self::new(PluginStatus::Inconclusive, message)
    }

    pub fn measured(mut self, value: Value) -> Self {
        self.measured = Some(value);
        self
    }

    pub fn expected(mut self, value: Value) -> Self {
        self.expected = Some(value);
        self
    }

    pub fn on_stream(mut self, index: u64) -> Self {
        self.stream = Some(index);
        self
    }

    pub fn at(mut self, range: TimeRange) -> Self {
        self.time_range = Some(range);
        self
    }
}

/// What a plugin sends back.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PluginResponse {
    pub protocol: u32,
    #[serde(default)]
    pub findings: Vec<PluginFinding>,
}

/// Read a request from `input`, run `handler`, write the response to
/// `output`. Returns the process exit code (`0` on success, `2` when the
/// request could not be read).
pub fn serve_with<F>(mut input: impl Read, mut output: impl Write, handler: F) -> i32
where
    F: FnOnce(&PluginRequest) -> Vec<PluginFinding>,
{
    let mut raw = Vec::new();
    if let Err(e) = input.read_to_end(&mut raw) {
        eprintln!("plugin: cannot read request: {e}");
        return 2;
    }
    let request: PluginRequest = match serde_json::from_slice(&raw) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("plugin: invalid request: {e}");
            return 2;
        }
    };
    if request.protocol != PROTOCOL_VERSION {
        eprintln!(
            "plugin: host speaks protocol {}, this SDK speaks {PROTOCOL_VERSION}",
            request.protocol
        );
        return 2;
    }
    let response = PluginResponse {
        protocol: PROTOCOL_VERSION,
        findings: handler(&request),
    };
    match serde_json::to_writer(&mut output, &response)
        .and_then(|()| output.flush().map_err(serde_json::Error::io))
    {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("plugin: cannot write response: {e}");
            2
        }
    }
}

/// [`serve_with`] on standard input and output. Pass the result to
/// `std::process::exit`.
pub fn serve<F>(handler: F) -> i32
where
    F: FnOnce(&PluginRequest) -> Vec<PluginFinding>,
{
    serve_with(std::io::stdin().lock(), std::io::stdout().lock(), handler)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpt_app_media_qc_model::asset::AssetFingerprint;

    fn request(protocol: u32) -> Vec<u8> {
        let request = PluginRequest {
            protocol,
            rule: "acme.test".into(),
            config: serde_json::json!({"n": 3}),
            asset: Asset {
                id: Default::default(),
                path: "x.mov".into(),
                fingerprint: AssetFingerprint {
                    sha256: "0".repeat(64),
                    size_bytes: 1,
                },
                size_bytes: 1,
                modified_time: None,
                duration: None,
                streams: vec![],
            },
            inspection: Inspection::default(),
        };
        serde_json::to_vec(&request).unwrap()
    }

    #[test]
    fn serve_round_trips_a_request() {
        let input = request(PROTOCOL_VERSION);
        let mut out = Vec::new();
        let code = serve_with(&input[..], &mut out, |r| {
            vec![PluginFinding::fail(format!("n={}", r.config["n"]))
                .on_stream(2)
                .measured(Value::UInt(4))]
        });
        assert_eq!(code, 0);
        let response: PluginResponse = serde_json::from_slice(&out).unwrap();
        assert_eq!(response.protocol, PROTOCOL_VERSION);
        assert_eq!(response.findings[0].message, "n=3");
        assert_eq!(response.findings[0].stream, Some(2));
    }

    #[test]
    fn wrong_protocol_or_garbage_exits_nonzero_without_running_the_handler() {
        for input in [request(99), b"not json".to_vec()] {
            let mut out = Vec::new();
            let code = serve_with(&input[..], &mut out, |_| panic!("must not run"));
            assert_eq!(code, 2);
            assert!(out.is_empty());
        }
    }

    #[test]
    fn findings_without_optional_fields_deserialize() {
        let f: PluginFinding =
            serde_json::from_str(r#"{"status":"inconclusive","message":"m"}"#).unwrap();
        assert_eq!(f.status, PluginStatus::Inconclusive);
        assert!(f.measured.is_none());
    }
}
