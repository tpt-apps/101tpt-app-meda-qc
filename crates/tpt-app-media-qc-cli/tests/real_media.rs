//! Real-file CLI integration coverage. Requires `ffmpeg` and `ffprobe` on PATH.

use std::path::Path;
use std::process::Command;

fn tool_available(name: &str) -> bool {
    Command::new(name)
        .arg("-version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn cli_path() -> &'static str {
    env!("CARGO_BIN_EXE_tpt-media-qc")
}

fn generate_wav(path: &Path) -> bool {
    let output = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=1000:sample_rate=48000:duration=1",
            "-ac",
            "2",
            "-c:a",
            "pcm_s16le",
        ])
        .arg(path)
        .output();
    output
        .map(|result| result.status.success())
        .unwrap_or(false)
}

#[test]
fn real_wav_info_and_full_check_use_ffprobe_and_decode_audio() {
    if !tool_available("ffmpeg") || !tool_available("ffprobe") {
        eprintln!("skipping real-media CLI test: ffmpeg/ffprobe is unavailable");
        return;
    }

    let directory = tempfile::tempdir().expect("temporary directory");
    let wav = directory.path().join("sample.wav");
    assert!(
        generate_wav(&wav),
        "ffmpeg could not generate the WAV fixture"
    );

    let info = Command::new(cli_path())
        .args(["info", "--quick"])
        .arg(&wav)
        .output()
        .expect("CLI info process starts");
    assert!(
        info.status.success(),
        "CLI info failed: {}",
        String::from_utf8_lossy(&info.stderr)
    );
    assert!(String::from_utf8_lossy(&info.stdout).contains("audio s0"));

    let report_path = directory.path().join("report.json");
    let check = Command::new(cli_path())
        .args(["check", "--json"])
        .arg(&report_path)
        .arg(&wav)
        .output()
        .expect("CLI check process starts");
    assert!(check.status.code().is_some());
    assert!(
        report_path.is_file(),
        "full check must write its JSON report"
    );

    let report = std::fs::read_to_string(&report_path).expect("read JSON report");
    let value: serde_json::Value = serde_json::from_str(&report).expect("parse JSON report");
    let findings = value["findings"].as_array().expect("report findings");
    let silence = findings.iter().find(|finding| {
        finding["rule_id"] == "audio.silence" && finding["status"] == "inconclusive"
    });
    assert!(
        silence.is_none(),
        "full WAV check must not report silence as undecoded: {silence:?}"
    );
    assert!(
        !String::from_utf8_lossy(&check.stdout).contains("audio was not decoded"),
        "full WAV check must reach the Cadence decode path"
    );
}
