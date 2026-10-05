//! Real-file CLI integration coverage. Files are generated in-process (no
//! external tools), then exercised through the real CLI binary.

use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

fn cli_path() -> &'static str {
    env!("CARGO_BIN_EXE_tpt-media-qc")
}

/// Write a 16-bit PCM WAV containing a sine tone.
fn write_tone(path: &Path, frequency: f32, amplitude_db: f32, channels: u16, seconds: u32) {
    const RATE: u32 = 48_000;
    let amplitude = 10f32.powf(amplitude_db / 20.0);
    let frames = RATE * seconds;
    let data_bytes = frames * u32::from(channels) * 2;
    let mut out = Vec::with_capacity(44 + data_bytes as usize);
    out.extend(b"RIFF");
    out.extend((36 + data_bytes).to_le_bytes());
    out.extend(b"WAVEfmt ");
    out.extend(16u32.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(channels.to_le_bytes());
    out.extend(RATE.to_le_bytes());
    out.extend((RATE * u32::from(channels) * 2).to_le_bytes());
    out.extend((channels * 2).to_le_bytes());
    out.extend(16u16.to_le_bytes());
    out.extend(b"data");
    out.extend(data_bytes.to_le_bytes());
    for n in 0..frames {
        let t = n as f32 / RATE as f32;
        let sample = (amplitude * (2.0 * std::f32::consts::PI * frequency * t).sin() * 32767.0)
            .round() as i16;
        for _ in 0..channels {
            out.extend(sample.to_le_bytes());
        }
    }
    std::fs::write(path, out).expect("write WAV fixture");
}

fn generate_wav(path: &Path) {
    write_tone(path, 1000.0, 0.0, 2, 1);
}

#[test]
fn real_wav_info_and_full_check_decode_audio() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let wav = directory.path().join("sample.wav");
    generate_wav(&wav);

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
    let directory = tempfile::tempdir().expect("temporary directory");
    let valid = directory.path().join("valid.wav");
    let corrupt = directory.path().join("corrupt.wav");
    let reports = directory.path().join("reports");
    generate_wav(&valid);
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
    generate_wav(&wav);

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

#[test]
fn real_compare_reports_identity_and_audio_differences() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let (a, b) = (
        directory.path().join("a.wav"),
        directory.path().join("b.wav"),
    );
    write_tone(&a, 1000.0, -10.0, 2, 4);
    write_tone(&b, 1000.0, -20.0, 1, 4);

    let same = Command::new(cli_path())
        .arg("compare")
        .arg(&a)
        .arg(&a)
        .output()
        .expect("compare process starts");
    assert_eq!(
        same.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&same.stdout)
    );

    let json = directory.path().join("diff.json");
    let different = Command::new(cli_path())
        .arg("compare")
        .arg(&a)
        .arg(&b)
        .arg("--json")
        .arg(&json)
        .output()
        .expect("compare process starts");
    assert_eq!(different.status.code(), Some(2));
    let stdout = String::from_utf8_lossy(&different.stdout);
    assert!(stdout.contains("channels"), "{stdout}");
    assert!(stdout.contains("integrated loudness"), "{stdout}");
    assert!(json.is_file());
}

/// Path to a committed encoded fixture.
fn encoded_fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/encoded")
        .join(name)
}

fn mp4_box(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
    out.extend(kind);
    out.extend(payload);
    out
}

/// A minimal MP4 whose only track is `fourcc` video (no sample data).
fn mp4_with_video_codec(fourcc: &[u8; 4]) -> Vec<u8> {
    let mut hdlr = vec![0u8; 8];
    hdlr.extend(b"vide");
    hdlr.extend([0u8; 13]);
    let mut entry = vec![0u8; 78];
    entry[7] = 1;
    let mut stsd = vec![0, 0, 0, 0, 0, 0, 0, 1];
    stsd.extend(mp4_box(fourcc, &entry));
    let stbl = mp4_box(b"stbl", &mp4_box(b"stsd", &stsd));
    let minf = mp4_box(b"minf", &stbl);
    let mdia = mp4_box(b"mdia", &[mp4_box(b"hdlr", &hdlr), minf].concat());
    let trak = mp4_box(b"trak", &mdia);
    let mut file = mp4_box(b"ftyp", b"isom\0\0\0\0isom");
    file.extend(mp4_box(b"moov", &trak));
    file
}

#[test]
fn patent_encumbered_codecs_are_refused_not_inspected() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let h264 = directory.path().join("camera.mp4");
    std::fs::write(&h264, mp4_with_video_codec(b"avc1")).expect("write fixture");
    let hevc = directory.path().join("hevc.mp4");
    std::fs::write(&hevc, mp4_with_video_codec(b"hvc1")).expect("write fixture");

    for (file, needle) in [(&h264, "H.264"), (&hevc, "HEVC")] {
        let out = Command::new(cli_path())
            .args(["check", "--quick"])
            .arg(file)
            .output()
            .expect("CLI starts");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(3), "{stderr}");
        assert!(stderr.contains("unsupported"), "{stderr}");
        assert!(stderr.contains(needle), "{stderr}");
        assert!(stderr.contains("royalty-free"), "{stderr}");
        assert!(
            String::from_utf8_lossy(&out.stdout).trim().is_empty(),
            "a refused file must not produce a verdict"
        );
    }
}

#[test]
fn a_truncated_wav_is_a_container_failure_not_a_crash() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let wav = directory.path().join("cut.wav");
    write_tone(&wav, 440.0, -6.0, 2, 2);
    let full = std::fs::read(&wav).expect("read WAV");
    std::fs::write(&wav, &full[..full.len() / 2]).expect("truncate WAV");

    let out = Command::new(cli_path())
        .args(["check", "--quick"])
        .arg(&wav)
        .output()
        .expect("CLI starts");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(2), "{stdout}");
    assert!(stdout.contains("container.readable"), "{stdout}");
    assert!(stdout.contains("data chunk declares"), "{stdout}");
}

#[test]
fn committed_vp9_fixtures_inspect_without_any_external_tool() {
    for (name, format) in [
        ("vp9-clip.mp4", "mov,mp4"),
        ("vp9-clip.webm", "matroska,webm"),
    ] {
        // An empty PATH proves nothing outside this binary is used.
        let out = Command::new(cli_path())
            .args(["info", "--quick"])
            .arg(encoded_fixture(name))
            .env("PATH", "")
            .output()
            .expect("CLI starts");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.code().is_some_and(|c| c <= 2),
            "{name}: {stdout}"
        );
        assert!(
            stdout.contains(&format!("format        {format}")),
            "{name}: {stdout}"
        );
        assert!(stdout.contains("video s0   fps 10/1"), "{name}: {stdout}");
        assert!(stdout.contains("duration      300 ms"), "{name}: {stdout}");
    }
}
