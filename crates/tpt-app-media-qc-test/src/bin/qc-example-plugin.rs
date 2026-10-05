//! Reference third-party rule plugin (spec § 27). Documented in
//! `docs/plugin-sdk.md`; it doubles as the plugin the integration tests run.
//!
//! Rules:
//!
//! * `example.min_video_bitrate` — config `{"min_bps": N}`: every video stream
//!   must have a bitrate of at least N bits/s.
//! * `example.max_streams` — config `{"max": N}`: the file may not have more
//!   than N streams.

use tpt_app_media_qc_plugin_sdk::{serve, PluginFinding, PluginRequest, StreamKind, Value};

fn min_video_bitrate(request: &PluginRequest) -> Vec<PluginFinding> {
    let Some(min) = request.config["min_bps"].as_u64() else {
        return vec![PluginFinding::inconclusive(
            "config needs a numeric 'min_bps'",
        )];
    };
    request
        .asset
        .streams
        .iter()
        .filter(|s| s.kind == StreamKind::Video)
        .filter_map(|s| match s.bitrate {
            None => Some(
                PluginFinding::inconclusive("video bitrate is not reported")
                    .on_stream(s.index.as_u64()),
            ),
            Some(b) if b < min => Some(
                PluginFinding::fail(format!("video bitrate {b} b/s is below {min} b/s"))
                    .on_stream(s.index.as_u64())
                    .measured(Value::UInt(b))
                    .expected(Value::UInt(min)),
            ),
            Some(_) => None,
        })
        .collect()
}

fn max_streams(request: &PluginRequest) -> Vec<PluginFinding> {
    let max = request.config["max"].as_u64().unwrap_or(8);
    let count = request.asset.streams.len() as u64;
    if count > max {
        vec![
            PluginFinding::fail(format!("{count} streams, at most {max} allowed"))
                .measured(Value::UInt(count))
                .expected(Value::UInt(max)),
        ]
    } else {
        Vec::new()
    }
}

fn main() {
    std::process::exit(serve(|request| match request.rule.as_str() {
        "example.min_video_bitrate" => min_video_bitrate(request),
        "example.max_streams" => max_streams(request),
        other => vec![PluginFinding::inconclusive(format!(
            "unknown rule '{other}'"
        ))],
    }));
}
