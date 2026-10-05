//! `decode_bench` — representative HD/UHD AV1 decode + analysis throughput
//! (todo item 24/28, spec § 20).
//!
//! Run: `cargo run -p tpt-app-media-qc-decode --release --example decode_bench [-- <frames>|<file>]`
//!
//! Pass a path to an AV1/VP9 file (MP4, Matroska/WebM or MPEG-TS) to time a real
//! master or a VP9 clip instead of the synthetic content.
//!
//! Synthesises deterministic textured 1080p and 2160p content, encodes it with
//! the pinned Kinetix AV1 encoder into a Matroska file, then times the real
//! `KinetixVideoInspector` (demux, AV1 decode and the full per-frame analysis:
//! black/freeze/duplicate, luma, perceptual metrics, dead pixels, PSE).
//! Synthetic content is a stand-in for professional masters; use it to track
//! regressions on one machine, not as an absolute codec benchmark.

use std::time::Instant;

use tpt_app_media_qc_decode::KinetixVideoInspector;
use tpt_app_media_qc_model::asset::{Asset, AssetFingerprint};
use tpt_app_media_qc_model::inspection::{
    ContainerInspection, ContainerValidity, Inspection, VideoMeasurements,
};
use tpt_app_media_qc_model::time::Rational;
use tpt_app_media_qc_pipeline::{InspectionLevel, Inspector};
use tpt_kinetix_av1::{Av1Encoder, Av1EncoderConfig};
use tpt_kinetix_core::frame::VideoFrame;
use tpt_kinetix_core::packet::Packet;
use tpt_kinetix_core::pixel_format::PixelFormat;
use tpt_kinetix_core::timestamp::Timestamp;

fn frame(width: u32, height: u32, index: u32) -> VideoFrame {
    let (w, h) = (width as usize, height as usize);
    let mut data = vec![128u8; w * h * 3 / 2];
    let mut state = 0x9E37_79B9u32 ^ index.wrapping_mul(2_654_435_761);
    for y in 0..h {
        for x in 0..w {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let gradient = (x * 180 / w + y * 40 / h + index as usize * 7) % 200 + 24;
            let texture = (state & 7) as usize;
            data[y * w + x] = (gradient + texture).min(235) as u8;
        }
    }
    let pts = Timestamp::new(i64::from(index) * 40, (1, 1000));
    VideoFrame {
        pts,
        dts: pts,
        data,
        width,
        height,
        pixel_format: PixelFormat::Yuv420p,
        is_key_frame: index == 0,
    }
}

fn ebml(id: &[u8], body: &[u8]) -> Vec<u8> {
    let len = body.len() as u64;
    let mut out = id.to_vec();
    // 8-byte EBML size: marker 0x01 then 7 bytes.
    out.push(0x01);
    out.extend_from_slice(&len.to_be_bytes()[1..]);
    out.extend_from_slice(body);
    out
}

fn mkv(packets: &[Packet]) -> Vec<u8> {
    let mut track = Vec::new();
    track.extend(ebml(&[0xD7], &[1]));
    track.extend(ebml(&[0x83], &[1]));
    track.extend(ebml(&[0x86], b"V_AV1"));
    let tracks = ebml(&[0x16, 0x54, 0xAE, 0x6B], &ebml(&[0xAE], &track));
    let mut cluster = ebml(&[0xE7], &[0]);
    for (index, packet) in packets.iter().enumerate() {
        let mut block = vec![0x81];
        block.extend_from_slice(&((index as i16) * 40).to_be_bytes());
        block.push(if index == 0 { 0x80 } else { 0x00 });
        block.extend_from_slice(&packet.data);
        cluster.extend(ebml(&[0xA3], &block));
    }
    let mut segment = tracks;
    segment.extend(ebml(&[0x1F, 0x43, 0xB6, 0x75], &cluster));
    let mut file = vec![0x1A, 0x45, 0xDF, 0xA3, 0x80];
    file.extend(ebml(&[0x18, 0x53, 0x80, 0x67], &segment));
    file
}

fn bench(label: &str, width: u32, height: u32, frames: u32) {
    let config = Av1EncoderConfig {
        width,
        height,
        bitrate: 0,
        quantizer: 100,
        speed: 10,
        keyframe_interval: 8,
    };
    let mut encoder = Av1Encoder::new(&config).expect("create AV1 encoder");
    let started = Instant::now();
    let mut packets = Vec::new();
    for index in 0..frames {
        packets.extend(
            encoder
                .encode_frame(&frame(width, height, index))
                .expect("encode"),
        );
    }
    packets.extend(encoder.flush().expect("flush"));
    let encode_time = started.elapsed();
    let bytes = mkv(&packets);

    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("bench.mkv");
    std::fs::write(&path, &bytes).expect("write fixture");
    time_file(
        label,
        &path,
        Some((width, height)),
        encode_time.as_secs_f64(),
    );
}

/// Time the inspector on an existing AV1/VP9 file (MP4, Matroska/WebM, TS).
fn time_file(
    label: &str,
    path: &std::path::Path,
    dimensions: Option<(u32, u32)>,
    encode_secs: f64,
) {
    let size = std::fs::metadata(path).expect("stat").len();
    let asset = Asset {
        id: Default::default(),
        path: path.to_path_buf(),
        fingerprint: AssetFingerprint {
            sha256: "0".repeat(64),
            size_bytes: size,
        },
        size_bytes: size,
        modified_time: None,
        duration: None,
        streams: Vec::new(),
    };
    let metadata = Inspection {
        container: ContainerInspection {
            validity: ContainerValidity::Ok,
            ..Default::default()
        },
        video: vec![VideoMeasurements {
            stream_idx: 0,
            frame_rate_observed: Some(Rational::new(25, 1).unwrap()),
            ..Default::default()
        }],
        ..Default::default()
    };

    let inspector = KinetixVideoInspector::new();
    let mut best = f64::MAX;
    let mut decoded = 0;
    for _ in 0..3 {
        let started = Instant::now();
        let result = inspector
            .inspect_decode(&asset, &metadata, InspectionLevel::Full)
            .expect("inspect");
        best = best.min(started.elapsed().as_secs_f64());
        let status = result.diagnostics["video_decode"]["status"].clone();
        assert_eq!(
            status, "complete",
            "{:?}",
            result.diagnostics["video_decode"]
        );
        decoded = result
            .video_for(0)
            .and_then(|v| v.decoded_frame_count)
            .unwrap_or(0);
    }
    let fps = decoded as f64 / best;
    let (picture, mpix) = match dimensions {
        Some((w, h)) => (
            format!("{w}x{h}"),
            format!("{:.1} Mpx/s", fps * f64::from(w) * f64::from(h) / 1e6),
        ),
        None => ("-".to_string(), "-".to_string()),
    };
    println!(
        "{label:<10} {picture:<10} {decoded:>4} frames  {:>8.1} ms/frame  {fps:>7.2} fps  {mpix:>12}  (file {:.1} KiB{})",
        best * 1000.0 / decoded.max(1) as f64,
        size as f64 / 1024.0,
        if encode_secs > 0.0 {
            format!(", encode {encode_secs:.1}s")
        } else {
            String::new()
        },
    );
}

fn main() {
    let arg = std::env::args().nth(1);
    if let Some(path) = arg
        .as_deref()
        .map(std::path::Path::new)
        .filter(|p| p.is_file())
    {
        println!(
            "Decode + full per-frame analysis of {}, best of 3",
            path.display()
        );
        time_file("file", path, None, 0.0);
        return;
    }
    let frames: u32 = arg.and_then(|s| s.parse().ok()).unwrap_or(8);
    println!("AV1 decode + full per-frame analysis, best of 3 ({frames} frames)");
    bench("HD", 1920, 1080, frames);
    bench("UHD", 3840, 2160, frames);
}
