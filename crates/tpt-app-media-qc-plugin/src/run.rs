//! Out-of-process execution of one plugin rule.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use tpt_app_media_qc_plugin_sdk::{
    PluginResponse, MAX_FINDINGS, MAX_RESPONSE_BYTES, PROTOCOL_VERSION,
};

/// Longest stderr excerpt kept for error messages.
const MAX_STDERR_BYTES: usize = 2048;

/// Everything needed to start the plugin.
#[derive(Clone, Debug)]
pub struct RunSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub working_dir: PathBuf,
    pub timeout: Duration,
}

/// Environment variables passed through to plugins. Everything else
/// (credentials, tokens, proxies) is withheld.
fn passthrough_env() -> Vec<(String, std::ffi::OsString)> {
    ["PATH", "SystemRoot", "windir"]
        .iter()
        .filter_map(|k| std::env::var_os(k).map(|v| ((*k).to_string(), v)))
        .collect()
}

fn read_limited(mut source: impl Read, limit: usize) -> (Vec<u8>, bool) {
    let mut out = Vec::new();
    let _ = (&mut source).take(limit as u64 + 1).read_to_end(&mut out);
    let over = out.len() > limit;
    out.truncate(limit);
    (out, over)
}

/// Run the plugin once with `request` on its standard input.
pub fn run_plugin(spec: &RunSpec, request: Vec<u8>) -> Result<PluginResponse, String> {
    let mut command = Command::new(&spec.program);
    command
        .args(&spec.args)
        .current_dir(&spec.working_dir)
        .env_clear()
        .envs(passthrough_env())
        .env("TPT_MEDIA_QC_PLUGIN_PROTOCOL", PROTOCOL_VERSION.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|e| format!("could not start '{}': {e}", spec.program.display()))?;

    let mut stdin = child.stdin.take().ok_or("plugin stdin unavailable")?;
    let stdout = child.stdout.take().ok_or("plugin stdout unavailable")?;
    let stderr = child.stderr.take().ok_or("plugin stderr unavailable")?;

    // Feed and drain on their own threads so a plugin that writes before it
    // has read everything cannot deadlock the host.
    let writer = std::thread::spawn(move || {
        // A plugin that exits without reading its input is reported by its
        // exit status, not by a broken pipe here.
        let _ = stdin.write_all(&request);
    });
    let out_reader = std::thread::spawn(move || read_limited(stdout, MAX_RESPONSE_BYTES));
    let err_reader = std::thread::spawn(move || read_limited(stderr, MAX_STDERR_BYTES));

    let deadline = Instant::now() + spec.timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(format!(
                    "timed out after {} ms and was stopped",
                    spec.timeout.as_millis()
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(e) => {
                let _ = child.kill();
                break Err(format!("could not wait for the plugin: {e}"));
            }
        }
    };

    let _ = writer.join();
    let (stdout_bytes, too_large) = out_reader.join().unwrap_or_default();
    let (stderr_bytes, _) = err_reader.join().unwrap_or_default();
    let stderr_text = String::from_utf8_lossy(&stderr_bytes).trim().to_string();

    let status = status?;
    if too_large {
        return Err(format!(
            "response is larger than {MAX_RESPONSE_BYTES} bytes and was discarded"
        ));
    }
    if !status.success() {
        let detail = if stderr_text.is_empty() {
            String::new()
        } else {
            format!(": {stderr_text}")
        };
        return Err(format!("exited with {status}{detail}"));
    }

    let mut response: PluginResponse = serde_json::from_slice(&stdout_bytes)
        .map_err(|e| format!("returned an invalid response: {e}"))?;
    if response.protocol != PROTOCOL_VERSION {
        return Err(format!(
            "answered with protocol {}, expected {PROTOCOL_VERSION}",
            response.protocol
        ));
    }
    response.findings.truncate(MAX_FINDINGS);
    Ok(response)
}
