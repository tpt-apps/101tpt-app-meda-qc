//! The royalty-free allow-list.
//!
//! Every container reader classifies each track through this module before it
//! looks inside the track. A track whose codec is not on the list makes the
//! whole file unsupported ([`Class::Refused`]); no bitstream of such a codec is
//! ever parsed. Codec *names* returned here are the strings stored in
//! `Stream::codec` and matched by rules and custom-rule profiles.

use tpt_app_media_qc_core::error::Error;
use tpt_app_media_qc_model::asset::StreamKind;

/// Outcome of classifying one track.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Class {
    /// An allowed codec. The reader may parse its configuration.
    Supported {
        codec: &'static str,
        kind: StreamKind,
    },
    /// A track that carries no audio or video codec we would refuse (timecode,
    /// metadata, image subtitles, attachments): listed, never interpreted.
    Passive { codec: String, kind: StreamKind },
    /// A codec outside the allow-list. Carries a human description such as
    /// `H.264 video (avc1)`.
    Refused(String),
}

/// The error returned for a refused file. It renders as
/// `unsupported: <what was found>. TPT Media QC inspects royalty-free formats only (...)`.
pub fn unsupported_error(what: &str) -> Error {
    Error::Unsupported(format!(
        "{what}. TPT Media QC inspects royalty-free formats only \
         (AV1 and VP9 video; Opus, Vorbis, FLAC and PCM audio; in MP4/MOV, Matroska/WebM, \
         MPEG-TS, WAV, AIFF, FLAC and Ogg files)"
    ))
}

/// First refused track in `classes`, if any.
pub fn first_refusal<'a>(classes: impl IntoIterator<Item = &'a Class>) -> Option<&'a str> {
    classes.into_iter().find_map(|c| match c {
        Class::Refused(what) => Some(what.as_str()),
        _ => None,
    })
}

/// Name of a PCM sample format as stored in `Stream::codec`
/// (`pcm_s16le`, `pcm_f32be`, `pcm_u8`, ...).
pub fn pcm_codec_name(float: bool, bits: u32, big_endian: bool) -> &'static str {
    match (float, bits, big_endian) {
        (_, 8, _) if !float => "pcm_u8",
        (false, 16, false) => "pcm_s16le",
        (false, 16, true) => "pcm_s16be",
        (false, 24, false) => "pcm_s24le",
        (false, 24, true) => "pcm_s24be",
        (false, 32, false) => "pcm_s32le",
        (false, 32, true) => "pcm_s32be",
        (true, 32, false) => "pcm_f32le",
        (true, 32, true) => "pcm_f32be",
        (true, 64, false) => "pcm_f64le",
        (true, 64, true) => "pcm_f64be",
        _ => "pcm",
    }
}

fn fourcc_text(fourcc: [u8; 4]) -> String {
    fourcc
        .iter()
        .map(|b| {
            if b.is_ascii_graphic() || *b == b' ' {
                *b as char
            } else {
                '?'
            }
        })
        .collect()
}

/// Human description of a refused ISO-BMFF sample-entry fourcc.
fn describe_fourcc(fourcc: [u8; 4], what: &str) -> String {
    let name = match &fourcc {
        b"avc1" | b"avc2" | b"avc3" | b"avc4" => "H.264/AVC",
        b"hvc1" | b"hev1" => "HEVC/H.265",
        b"mp4v" => "MPEG-4/MPEG-2 visual",
        b"apch" | b"apcn" | b"apcs" | b"apco" | b"ap4h" | b"ap4x" => "Apple ProRes",
        b"mp4a" => "AAC (MPEG-4 audio)",
        b"ac-3" | b"ec-3" => "Dolby AC-3/E-AC-3",
        b".mp3" => "MP3",
        b"alac" => "Apple Lossless",
        _ => "",
    };
    if name.is_empty() {
        format!("{what} codec '{}'", fourcc_text(fourcc))
    } else {
        format!("{name} {what} ({})", fourcc_text(fourcc))
    }
}

/// Classify an ISO-BMFF track by its `hdlr` handler type and its first
/// `stsd` sample-entry fourcc.
pub fn classify_iso(handler: [u8; 4], fourcc: [u8; 4]) -> Class {
    match &handler {
        b"vide" => match &fourcc {
            b"av01" => Class::Supported {
                codec: "av1",
                kind: StreamKind::Video,
            },
            b"vp09" => Class::Supported {
                codec: "vp9",
                kind: StreamKind::Video,
            },
            _ => Class::Refused(describe_fourcc(fourcc, "video")),
        },
        b"soun" => match &fourcc {
            b"Opus" => Class::Supported {
                codec: "opus",
                kind: StreamKind::Audio,
            },
            b"fLaC" => Class::Supported {
                codec: "flac",
                kind: StreamKind::Audio,
            },
            // ISO 23003-5 and QuickTime PCM entries. The exact sample format
            // is refined by the reader (`pcm_codec_name`).
            b"ipcm" | b"fpcm" | b"sowt" | b"twos" | b"in24" | b"in32" | b"fl32" | b"fl64"
            | b"raw " | b"lpcm" => Class::Supported {
                codec: "pcm",
                kind: StreamKind::Audio,
            },
            _ => Class::Refused(describe_fourcc(fourcc, "audio")),
        },
        b"text" | b"sbtl" | b"subt" => match &fourcc {
            b"wvtt" => Class::Supported {
                codec: "webvtt",
                kind: StreamKind::Subtitle,
            },
            b"tx3g" => Class::Supported {
                codec: "tx3g",
                kind: StreamKind::Subtitle,
            },
            _ => Class::Passive {
                codec: fourcc_text(fourcc),
                kind: StreamKind::Subtitle,
            },
        },
        b"tmcd" => Class::Passive {
            codec: "tmcd".into(),
            kind: StreamKind::Data,
        },
        _ => Class::Passive {
            codec: fourcc_text(fourcc),
            kind: StreamKind::Data,
        },
    }
}

/// Classify a Matroska/WebM track by its `CodecID` string and `TrackType`
/// (1 video, 2 audio, 17 subtitle, others passive).
pub fn classify_matroska(codec_id: &str, track_type: u8) -> Class {
    match track_type {
        1 => match codec_id {
            "V_AV1" => Class::Supported {
                codec: "av1",
                kind: StreamKind::Video,
            },
            "V_VP9" => Class::Supported {
                codec: "vp9",
                kind: StreamKind::Video,
            },
            other => Class::Refused(format!("video codec '{other}'")),
        },
        2 => match codec_id {
            "A_OPUS" => Class::Supported {
                codec: "opus",
                kind: StreamKind::Audio,
            },
            "A_VORBIS" => Class::Supported {
                codec: "vorbis",
                kind: StreamKind::Audio,
            },
            "A_FLAC" => Class::Supported {
                codec: "flac",
                kind: StreamKind::Audio,
            },
            "A_PCM/INT/LIT" | "A_PCM/INT/BIG" | "A_PCM/FLOAT/IEEE" => Class::Supported {
                codec: "pcm",
                kind: StreamKind::Audio,
            },
            other => Class::Refused(format!("audio codec '{other}'")),
        },
        17 => match codec_id {
            "S_TEXT/UTF8" => Class::Supported {
                codec: "subrip",
                kind: StreamKind::Subtitle,
            },
            "S_TEXT/ASS" => Class::Supported {
                codec: "ass",
                kind: StreamKind::Subtitle,
            },
            "S_TEXT/SSA" => Class::Supported {
                codec: "ssa",
                kind: StreamKind::Subtitle,
            },
            "S_TEXT/WEBVTT" => Class::Supported {
                codec: "webvtt",
                kind: StreamKind::Subtitle,
            },
            other => Class::Passive {
                codec: other.to_string(),
                kind: StreamKind::Subtitle,
            },
        },
        _ => Class::Passive {
            codec: codec_id.to_string(),
            kind: StreamKind::Data,
        },
    }
}

/// Classify an MPEG-TS elementary stream.
///
/// `registration` is the 4-byte format identifier of a registration
/// descriptor (tag `0x05`) on the stream, if any. `descriptor_tags` lists the
/// tags of every descriptor on the stream (to recognise DVB subtitles and
/// teletext carried in private PES).
pub fn classify_ts(
    stream_type: u8,
    registration: Option<[u8; 4]>,
    descriptor_tags: &[u8],
) -> Class {
    match (stream_type, registration.as_ref().map(|r| &r[..])) {
        (0x06 | 0x81, Some(b"AV01")) | (0x06, Some(b"av01")) => Class::Supported {
            codec: "av1",
            kind: StreamKind::Video,
        },
        (0x06, Some(b"vp09")) => Class::Supported {
            codec: "vp9",
            kind: StreamKind::Video,
        },
        (0x06, Some(b"Opus")) => Class::Supported {
            codec: "opus",
            kind: StreamKind::Audio,
        },
        (0x06, Some(b"fLaC")) => Class::Supported {
            codec: "flac",
            kind: StreamKind::Audio,
        },
        // Private PES carrying DVB subtitles (0x59) or teletext (0x56): image
        // or non-text subtitle formats, listed but not interpreted.
        (0x06, _) if descriptor_tags.iter().any(|t| matches!(t, 0x56 | 0x59)) => Class::Passive {
            codec: "dvb_subtitle".into(),
            kind: StreamKind::Subtitle,
        },
        // PES private data without a recognised registration: AC-3/E-AC-3/DTS
        // are signalled this way, so treat unknown private audio as refused.
        (0x06, _)
            if descriptor_tags
                .iter()
                .any(|t| matches!(t, 0x6A | 0x7A | 0x7B | 0x7C)) =>
        {
            Class::Refused("Dolby/DTS audio in MPEG-TS".into())
        }
        // PSI/metadata streams: SCTE-35, ID3, KLV, PES private data.
        (0x06 | 0x05 | 0x14 | 0x15 | 0x86, _) => Class::Passive {
            codec: format!("stream_type_0x{stream_type:02x}"),
            kind: StreamKind::Data,
        },
        (0x01 | 0x02 | 0x10 | 0x1B | 0x24 | 0x42 | 0xD1 | 0xEA, _) => Class::Refused(format!(
            "video stream type 0x{stream_type:02X} (MPEG/AVC/HEVC family)"
        )),
        (0x03 | 0x04 | 0x0F | 0x11 | 0x81 | 0x82 | 0x83 | 0x87 | 0x8A, _) => Class::Refused(
            format!("audio stream type 0x{stream_type:02X} (MPEG/AAC/AC-3/DTS family)"),
        ),
        _ => Class::Passive {
            codec: format!("stream_type_0x{stream_type:02x}"),
            kind: StreamKind::Data,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_allow_list() {
        assert_eq!(
            classify_iso(*b"vide", *b"av01"),
            Class::Supported {
                codec: "av1",
                kind: StreamKind::Video
            }
        );
        assert!(
            matches!(classify_iso(*b"vide", *b"avc1"), Class::Refused(m) if m.contains("H.264"))
        );
        assert!(
            matches!(classify_iso(*b"vide", *b"apch"), Class::Refused(m) if m.contains("ProRes"))
        );
        assert!(matches!(classify_iso(*b"soun", *b"mp4a"), Class::Refused(m) if m.contains("AAC")));
        assert!(
            matches!(classify_iso(*b"soun", *b"zzzz"), Class::Refused(m) if m.contains("zzzz"))
        );
        assert!(matches!(
            classify_iso(*b"tmcd", *b"tmcd"),
            Class::Passive { .. }
        ));
        assert!(matches!(
            classify_iso(*b"sbtl", *b"c608"),
            Class::Passive { .. }
        ));
    }

    #[test]
    fn matroska_allow_list() {
        assert!(matches!(
            classify_matroska("V_VP9", 1),
            Class::Supported { codec: "vp9", .. }
        ));
        assert!(matches!(
            classify_matroska("V_MPEG4/ISO/AVC", 1),
            Class::Refused(_)
        ));
        assert!(matches!(classify_matroska("A_AAC", 2), Class::Refused(_)));
        assert!(matches!(
            classify_matroska("A_OPUS", 2),
            Class::Supported { .. }
        ));
        assert!(matches!(
            classify_matroska("S_HDMV/PGS", 17),
            Class::Passive { .. }
        ));
        assert!(matches!(
            classify_matroska("S_TEXT/UTF8", 17),
            Class::Supported {
                codec: "subrip",
                ..
            }
        ));
    }

    #[test]
    fn ts_allow_list() {
        assert!(matches!(
            classify_ts(0x06, Some(*b"AV01"), &[0x05]),
            Class::Supported { codec: "av1", .. }
        ));
        assert!(matches!(
            classify_ts(0x06, Some(*b"Opus"), &[0x05]),
            Class::Supported { codec: "opus", .. }
        ));
        assert!(matches!(classify_ts(0x1B, None, &[]), Class::Refused(_)));
        assert!(matches!(classify_ts(0x0F, None, &[]), Class::Refused(_)));
        assert!(matches!(
            classify_ts(0x06, None, &[0x6A]),
            Class::Refused(_)
        ));
        assert!(matches!(
            classify_ts(0x06, None, &[0x59]),
            Class::Passive { .. }
        ));
    }

    #[test]
    fn pcm_names() {
        assert_eq!(pcm_codec_name(false, 24, false), "pcm_s24le");
        assert_eq!(pcm_codec_name(true, 32, true), "pcm_f32be");
        assert_eq!(pcm_codec_name(false, 8, false), "pcm_u8");
        assert_eq!(pcm_codec_name(false, 12, false), "pcm");
    }

    #[test]
    fn refusal_message_is_explanatory() {
        let e = unsupported_error("H.264 video (avc1)").to_string();
        assert!(
            e.contains("unsupported") && e.contains("royalty-free"),
            "{e}"
        );
    }
}
