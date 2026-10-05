//! Container detection by content, never by file extension.

/// A container family we recognise.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Container {
    IsoBmff,
    Matroska,
    MpegTs,
    Wav,
    Aiff,
    Flac,
    Ogg,
}

/// Magic bytes needed by [`sniff`] (enough for three TS sync bytes).
pub const SNIFF_BYTES: usize = 188 * 3 + 1;

/// Container families this build knows it cannot read, for a clearer error.
pub fn describe_unreadable(head: &[u8]) -> &'static str {
    if head.len() >= 16 && head[..4] == [0x06, 0x0E, 0x2B, 0x34] {
        "MXF container"
    } else if head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"AVI " {
        "AVI container"
    } else if head.len() >= 3 && &head[..3] == b"ID3" {
        "MP3 audio"
    } else if head.len() >= 2 && head[0] == 0xFF && head[1] & 0xE0 == 0xE0 {
        "MPEG audio/ADTS stream"
    } else if head.len() >= 4 && head[..4] == *b"FLV" {
        "FLV container"
    } else {
        "unrecognised file format"
    }
}

/// Identify the container from the first bytes of a file.
pub fn sniff(head: &[u8]) -> Option<Container> {
    if head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WAVE" {
        return Some(Container::Wav);
    }
    if head.len() >= 12 && &head[..4] == b"FORM" && matches!(&head[8..12], b"AIFF" | b"AIFC") {
        return Some(Container::Aiff);
    }
    if head.len() >= 4 && &head[..4] == b"fLaC" {
        return Some(Container::Flac);
    }
    if head.len() >= 4 && &head[..4] == b"OggS" {
        return Some(Container::Ogg);
    }
    if head.len() >= 4 && head[..4] == [0x1A, 0x45, 0xDF, 0xA3] {
        return Some(Container::Matroska);
    }
    if head.len() >= 12 {
        // ISO-BMFF: the first box is normally `ftyp`; QuickTime files may
        // start with `moov`, `mdat`, `wide` or `free`.
        let kind = &head[4..8];
        if matches!(
            kind,
            b"ftyp" | b"moov" | b"mdat" | b"wide" | b"free" | b"skip" | b"styp"
        ) {
            let size = u32::from_be_bytes([head[0], head[1], head[2], head[3]]);
            if size >= 8 || size == 1 || size == 0 {
                return Some(Container::IsoBmff);
            }
        }
    }
    // MPEG-TS: sync byte 0x47 every 188 bytes (also 192 for M2TS, whose
    // 4-byte timestamp prefix we do not accept).
    if head.len() > 188 * 3 && head[0] == 0x47 && head[188] == 0x47 && head[376] == 0x47 {
        return Some(Container::MpegTs);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pad(mut v: Vec<u8>) -> Vec<u8> {
        v.resize(SNIFF_BYTES, 0);
        v
    }

    #[test]
    fn recognises_each_family() {
        assert_eq!(
            sniff(&pad(b"RIFF\0\0\0\0WAVEfmt ".to_vec())),
            Some(Container::Wav)
        );
        assert_eq!(
            sniff(&pad(b"FORM\0\0\0\0AIFFCOMM".to_vec())),
            Some(Container::Aiff)
        );
        assert_eq!(
            sniff(&pad(b"FORM\0\0\0\0AIFCCOMM".to_vec())),
            Some(Container::Aiff)
        );
        assert_eq!(sniff(&pad(b"fLaC".to_vec())), Some(Container::Flac));
        assert_eq!(sniff(&pad(b"OggS".to_vec())), Some(Container::Ogg));
        assert_eq!(
            sniff(&pad(vec![0x1A, 0x45, 0xDF, 0xA3])),
            Some(Container::Matroska)
        );
        let mut mp4 = vec![0, 0, 0, 0x18];
        mp4.extend(b"ftypisom");
        assert_eq!(sniff(&pad(mp4)), Some(Container::IsoBmff));
        let mut ts = vec![0u8; SNIFF_BYTES];
        for i in 0..4 {
            ts[i * 188] = 0x47;
        }
        assert_eq!(sniff(&ts), Some(Container::MpegTs));
    }

    #[test]
    fn rejects_everything_else_with_a_useful_name() {
        let avi = pad(b"RIFF\0\0\0\0AVI LIST".to_vec());
        assert_eq!(sniff(&avi), None);
        assert_eq!(describe_unreadable(&avi), "AVI container");
        let mxf = pad(vec![
            0x06, 0x0E, 0x2B, 0x34, 0x02, 0x05, 0x01, 0x01, 0x0D, 0x01, 0x02, 0x01, 0x01, 0x02,
            0x04, 0x00,
        ]);
        assert_eq!(sniff(&mxf), None);
        assert_eq!(describe_unreadable(&mxf), "MXF container");
        assert_eq!(sniff(b""), None);
        assert_eq!(sniff(&pad(b"hello world".to_vec())), None);
    }
}
