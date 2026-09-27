//! Real-file CLI integration coverage. Requires `ffmpeg` and `ffprobe` on PATH.

use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

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

#[test]
fn real_batch_continues_after_a_probe_failure() {
    if !tool_available("ffmpeg") || !tool_available("ffprobe") {
        eprintln!("skipping real batch test: ffmpeg/ffprobe is unavailable");
        return;
    }

    let directory = tempfile::tempdir().expect("temporary directory");
    let valid = directory.path().join("valid.wav");
    let corrupt = directory.path().join("corrupt.wav");
    let reports = directory.path().join("reports");
    assert!(
        generate_wav(&valid),
        "ffmpeg could not generate the valid WAV"
    );
    std::fs::write(&corrupt, b"not a media container").expect("write corrupt fixture");

    let output = Command::new(cli_path())
        .args(["batch", "--quick", "--out"])
        .arg(&reports)
        .arg(&valid)
        .arg(&corrupt)
        .output()
        .expect("CLI batch process starts");

    assert_eq!(
        output.status.code(),
        Some(3),
        "batch should return an execution error after the corrupt asset: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        reports.join("valid.json").is_file(),
        "a failure in one asset must not prevent the valid asset's report"
    );
    assert!(
        !reports.join("corrupt.json").exists(),
        "a probe-failed asset must not receive a successful QC report"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("corrupt.wav"),
        "batch should report the isolated asset failure"
    );
}

#[test]
fn real_watch_routes_a_preexisting_asset() {
    if !tool_available("ffmpeg") || !tool_available("ffprobe") {
        eprintln!("skipping real watch test: ffmpeg/ffprobe is unavailable");
        return;
    }

    let directory = tempfile::tempdir().expect("temporary directory");
    let input = directory.path().join("incoming");
    let pass = directory.path().join("approved");
    let warn = directory.path().join("review");
    let fail = directory.path().join("rejected");
    let reports = directory.path().join("reports");
    let profile_path = directory.path().join("watch-profile.yaml");
    std::fs::create_dir_all(&input).expect("create input directory");
    std::fs::write(
        &profile_path,
        "name: watch-pass\nversion: 1\nrules:\n  container:\n    readable: error\n    container_validity: error\npolicy:\n  fail_on: error\n",
    )
    .expect("write watch profile");

    let wav = input.join("sample.wav");
    assert!(
        generate_wav(&wav),
        "ffmpeg could not generate the watch WAV"
    );

    let mut child = Command::new(cli_path())
        .args(["watch", "--input"])
        .arg(&input)
        .args(["--profile"])
        .arg(&profile_path)
        .args(["--pass"])
        .arg(&pass)
        .args(["--warn"])
        .arg(&warn)
        .args(["--fail"])
        .arg(&fail)
        .args(["--report"])
        .arg(&reports)
        .args(["--quick"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("CLI watch process starts");

    let deadline = Instant::now() + Duration::from_secs(20);
    let routed = loop {
        if pass.join("sample.wav").is_file() {
            break true;
        }
        if let Some(status) = child.try_wait().expect("poll watch process") {
            panic!("watch exited before routing the asset: {status}");
        }
        if Instant::now() >= deadline {
            break false;
        }
        thread::sleep(Duration::from_millis(100));
    };

    let _ = child.kill();
    let _ = child.wait();
    assert!(routed, "watch did not route the pre-existing asset in time");
    assert!(!wav.exists(), "routed source file must be moved");
    assert!(
        std::fs::read_dir(&reports)
            .expect("read report directory")
            .filter_map(Result::ok)
            .any(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("json")),
        "watch must write a per-asset report when requested"
    );
}
