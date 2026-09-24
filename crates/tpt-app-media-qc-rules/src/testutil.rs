//! Shared test fixtures for rule tests. Not compiled in release.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::OnceLock;
use tpt_app_media_qc_model::asset::{
    Asset, AssetFingerprint, AssetId, Stream, StreamId, StreamKind,
};
use tpt_app_media_qc_model::time::{DurationSeconds, FrameRate, Rational};

pub fn bundled_asset<'a>() -> &'a Asset {
    static ASSET: OnceLock<Asset> = OnceLock::new();
    ASSET.get_or_init(|| Asset {
        id: AssetId::default(),
        path: PathBuf::from("tests/fixtures/bundled.mov"),
        fingerprint: AssetFingerprint {
            sha256: "ab".repeat(32),
            size_bytes: 4_294_967,
        },
        size_bytes: 4_294_967,
        modified_time: Some(1_700_000_000),
        duration: Some(DurationSeconds::from_millis(120_000)),
        streams: vec![
            Stream {
                index: StreamId::new(0),
                kind: StreamKind::Video,
                codec: Some("h264".into()),
                codec_profile: Some("High".into()),
                width: Some(1920),
                height: Some(1080),
                pixel_format: Some("yuv420p".into()),
                frame_rate: Some(FrameRate::from_parts(25, 1)),
                time_base: Some(Rational::from_parts(1, 12800)),
                bitrate: Some(15_000_000),
                duration: Some(DurationSeconds::from_millis(120_000)),
                language: None,
                channel_layout: None,
                channels: None,
                sample_rate: None,
                bit_depth: None,
                metadata: BTreeMap::new(),
            },
            Stream {
                index: StreamId::new(1),
                kind: StreamKind::Audio,
                codec: Some("pcm_s24le".into()),
                codec_profile: None,
                width: None,
                height: None,
                pixel_format: None,
                frame_rate: None,
                time_base: Some(Rational::from_parts(1, 48000)),
                bitrate: Some(2_304_000),
                duration: Some(DurationSeconds::from_millis(120_000)),
                language: Some("en".into()),
                channel_layout: Some("stereo".into()),
                channels: Some(2),
                sample_rate: Some(48_000),
                bit_depth: Some(24),
                metadata: BTreeMap::new(),
            },
        ],
    })
}
