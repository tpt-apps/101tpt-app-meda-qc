//! Standalone audio files: WAV, AIFF/AIFC, FLAC and Ogg (Opus, Vorbis).
//!
//! These readers parse only headers and chunk/page structure with bounded
//! reads, so inspecting a multi-gigabyte file costs a few kilobytes of I/O.

use std::io::{Read, Seek, SeekFrom};

use tpt_app_media_qc_core::error::{Error, Result};
use tpt_app_media_qc_model::asset::{Stream, StreamId, StreamKind};
use tpt_app_media_qc_model::inspection::ContainerValidity;

use crate::codec::{pcm_codec_name, unsupported_error};
use crate::common::{ProbedContainer, ProbedStream, MAX_BUFFERED_BYTES};

fn audio_stream(
    codec: &str,
    channels: u64,
    sample_rate: u64,
    bit_depth: Option<u64>,
    bitrate: Option<u64>,
    duration_ms: Option<u64>,
) -> ProbedStream {
    let mut stream = Stream::primary_audio(0);
    stream.index = StreamId::new(0);
    stream.kind = StreamKind::Audio;
    stream.codec = Some(codec.to_string());
    stream.codec_profile = None;
    stream.channels = Some(channels);
    stream.channel_layout = channel_layout(channels);
    stream.sample_rate = Some(sample_rate);
    stream.bit_depth = bit_depth;
    stream.bitrate = bitrate;
    stream.duration = duration_ms.map(tpt_app_media_qc_model::time::DurationSeconds::from_millis);
    stream.time_base = (sample_rate > 0)
        .then(|| tpt_app_media_qc_model::time::Rational::new(1, sample_rate))
        .flatten();
    stream.language = None;
    stream.metadata.clear();
    ProbedStream {
        stream,
        native_id: 0,
        video: None,
        cues: Vec::new(),
        cues_truncated: false,
    }
}

/// Descriptive layout name for a bare channel count (no channel mask).
pub fn channel_layout(channels: u64) -> Option<String> {
    Some(
        match channels {
            1 => "mono",
            2 => "stereo",
            6 => "5.1",
            8 => "7.1",
            _ => return None,
        }
        .to_string(),
    )
}

fn read_exact<R: Read>(r: &mut R, buf: &mut [u8]) -> bool {
    r.read_exact(buf).is_ok()
}

fn u16le(b: &[u8]) -> u16 {
    u16::from_le_bytes([b[0], b[1]])
}
fn u32le(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}
fn u32be(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

fn bitrate_from(size: u64, duration_ms: Option<u64>) -> Option<u64> {
    duration_ms
        .filter(|d| *d > 0)
        .map(|d| (size as u128 * 8 * 1000 / d as u128) as u64)
}

// ---------------------------------------------------------------------------
// WAV
// ---------------------------------------------------------------------------

pub fn probe_wav<R: Read + Seek>(r: &mut R, len: u64) -> Result<ProbedContainer> {
    r.seek(SeekFrom::Start(12))?;
    let mut fmt: Option<(u16, u16, u32, u16, Option<u16>)> = None; // tag, ch, rate, bits, valid
    let mut data: Option<(u64, u64)> = None; // offset, declared size
    let mut pos = 12u64;
    let mut notes: Vec<String> = Vec::new();

    while pos + 8 <= len {
        r.seek(SeekFrom::Start(pos))?;
        let mut header = [0u8; 8];
        if !read_exact(r, &mut header) {
            break;
        }
        let id = [header[0], header[1], header[2], header[3]];
        let size = u64::from(u32le(&header[4..8]));
        let body = pos + 8;
        match &id {
            b"fmt " => {
                if !(16..=MAX_BUFFERED_BYTES).contains(&size) {
                    return Ok(ProbedContainer::corrupt("wav", "invalid fmt chunk size"));
                }
                let mut buf = vec![0u8; size as usize];
                if !read_exact(r, &mut buf) {
                    return Ok(ProbedContainer::corrupt("wav", "truncated fmt chunk"));
                }
                let mut tag = u16le(&buf[0..2]);
                let valid = (size >= 26 && tag == 0xFFFE).then(|| u16le(&buf[18..20]));
                if size >= 26 && tag == 0xFFFE {
                    // WAVE_FORMAT_EXTENSIBLE: the real tag is the first two
                    // bytes of the sub-format GUID.
                    tag = u16le(&buf[24..26]);
                }
                fmt = Some((
                    tag,
                    u16le(&buf[2..4]),
                    u32le(&buf[4..8]),
                    u16le(&buf[14..16]),
                    valid,
                ));
            }
            b"data" => {
                data = Some((body, size));
                // The data chunk is last in practice; stop scanning here so a
                // multi-gigabyte payload is never walked.
                break;
            }
            _ => {}
        }
        // Chunks are word aligned.
        let next = body.saturating_add(size).saturating_add(size & 1);
        if next <= pos {
            return Ok(ProbedContainer::corrupt("wav", "chunk size overflow"));
        }
        pos = next;
    }

    let Some((tag, channels, rate, bits, valid)) = fmt else {
        return Ok(ProbedContainer::corrupt("wav", "no fmt chunk"));
    };
    let float = match tag {
        1 => false,
        3 => true,
        other => {
            return Err(unsupported_error(&format!(
                "audio codec (WAV format tag 0x{other:04X})"
            )))
        }
    };
    if channels == 0 || rate == 0 || bits == 0 {
        return Ok(ProbedContainer::corrupt(
            "wav",
            "fmt chunk has zero channels, rate or bits",
        ));
    }
    let Some((offset, declared)) = data else {
        return Ok(ProbedContainer::corrupt("wav", "no data chunk"));
    };

    let available = len.saturating_sub(offset);
    // 0xFFFFFFFF marks an unknown (streamed) size; otherwise a data chunk
    // larger than the file means the file is truncated.
    let (data_bytes, truncated) = if declared == 0xFFFF_FFFF || declared == 0 {
        (available, false)
    } else if declared > available {
        (available, true)
    } else {
        (declared, false)
    };
    let frame_bytes = u64::from(channels) * u64::from(bits).div_ceil(8);
    let frames = data_bytes / frame_bytes.max(1);
    let duration_ms = Some(frames * 1000 / u64::from(rate));
    let depth = valid.map_or(u64::from(bits), u64::from);

    let mut out = ProbedContainer::new("wav");
    out.duration_ms = duration_ms;
    out.timecode_present = Some(false);
    if truncated {
        out.validity = ContainerValidity::Corrupt(format!(
            "data chunk declares {declared} bytes but only {available} are present"
        ));
    }
    if data_bytes % frame_bytes.max(1) != 0 {
        notes.push("data chunk ends in a partial sample frame".into());
    }
    out.malformed_metadata = notes;
    out.streams.push(audio_stream(
        pcm_codec_name(float, u32::from(bits), false),
        u64::from(channels),
        u64::from(rate),
        Some(depth),
        Some(u64::from(rate) * u64::from(channels) * u64::from(bits)),
        duration_ms,
    ));
    Ok(out)
}

// ---------------------------------------------------------------------------
// AIFF / AIFC
// ---------------------------------------------------------------------------

/// Decode an 80-bit IEEE 754 extended-precision float (AIFF sample rate).
fn extended80(b: &[u8]) -> f64 {
    let exponent = i32::from(u16::from_be_bytes([b[0] & 0x7F, b[1]]));
    let mantissa = u64::from_be_bytes([b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9]]);
    if exponent == 0 && mantissa == 0 {
        return 0.0;
    }
    let value = mantissa as f64 * 2f64.powi(exponent - 16383 - 63);
    if b[0] & 0x80 != 0 {
        -value
    } else {
        value
    }
}

pub fn probe_aiff<R: Read + Seek>(r: &mut R, len: u64) -> Result<ProbedContainer> {
    r.seek(SeekFrom::Start(8))?;
    let mut form = [0u8; 4];
    if !read_exact(r, &mut form) {
        return Ok(ProbedContainer::corrupt("aiff", "truncated header"));
    }
    let aifc = &form == b"AIFC";

    let mut comm: Option<(u16, u32, u16, f64, [u8; 4])> = None;
    let mut have_ssnd = false;
    let mut pos = 12u64;
    while pos + 8 <= len {
        r.seek(SeekFrom::Start(pos))?;
        let mut header = [0u8; 8];
        if !read_exact(r, &mut header) {
            break;
        }
        let id = [header[0], header[1], header[2], header[3]];
        let size = u64::from(u32be(&header[4..8]));
        let body = pos + 8;
        match &id {
            b"COMM" => {
                if !(18..=MAX_BUFFERED_BYTES).contains(&size) {
                    return Ok(ProbedContainer::corrupt("aiff", "invalid COMM chunk size"));
                }
                let mut buf = vec![0u8; size as usize];
                if !read_exact(r, &mut buf) {
                    return Ok(ProbedContainer::corrupt("aiff", "truncated COMM chunk"));
                }
                let compression = if aifc && size >= 22 {
                    [buf[18], buf[19], buf[20], buf[21]]
                } else {
                    *b"NONE"
                };
                comm = Some((
                    u16::from_be_bytes([buf[0], buf[1]]),
                    u32be(&buf[2..6]),
                    u16::from_be_bytes([buf[6], buf[7]]),
                    extended80(&buf[8..18]),
                    compression,
                ));
            }
            b"SSND" => {
                have_ssnd = true;
                break;
            }
            _ => {}
        }
        let next = body.saturating_add(size).saturating_add(size & 1);
        if next <= pos {
            return Ok(ProbedContainer::corrupt("aiff", "chunk size overflow"));
        }
        pos = next;
    }
    let Some((channels, frames, bits, rate, compression)) = comm else {
        return Ok(ProbedContainer::corrupt("aiff", "no COMM chunk"));
    };
    let (float, big_endian) = match &compression {
        b"NONE" | b"twos" => (false, true),
        b"sowt" => (false, false),
        b"fl32" | b"FL32" => (true, true),
        b"fl64" | b"FL64" => (true, true),
        other => {
            return Err(unsupported_error(&format!(
                "audio compression '{}' in AIFF-C",
                String::from_utf8_lossy(other)
            )))
        }
    };
    if channels == 0 || bits == 0 || rate < 1.0 || !rate.is_finite() {
        return Ok(ProbedContainer::corrupt(
            "aiff",
            "COMM chunk has zero channels, bits or rate",
        ));
    }
    let rate = rate.round() as u64;
    let duration_ms = Some(u64::from(frames) * 1000 / rate);
    let mut out = ProbedContainer::new("aiff");
    out.duration_ms = duration_ms;
    out.timecode_present = Some(false);
    if !have_ssnd {
        out.validity = ContainerValidity::Corrupt("no SSND (sound data) chunk".into());
    }
    out.streams.push(audio_stream(
        pcm_codec_name(float, u32::from(bits), big_endian),
        u64::from(channels),
        rate,
        Some(u64::from(bits)),
        Some(rate * u64::from(channels) * u64::from(bits)),
        duration_ms,
    ));
    Ok(out)
}

// ---------------------------------------------------------------------------
// FLAC
// ---------------------------------------------------------------------------

pub fn probe_flac<R: Read + Seek>(r: &mut R, len: u64) -> Result<ProbedContainer> {
    r.seek(SeekFrom::Start(4))?;
    let mut header = [0u8; 4];
    if !read_exact(r, &mut header) || header[0] & 0x7F != 0 {
        return Ok(ProbedContainer::corrupt(
            "flac",
            "the first metadata block is not STREAMINFO",
        ));
    }
    let size = u32::from_be_bytes([0, header[1], header[2], header[3]]);
    if size != 34 {
        return Ok(ProbedContainer::corrupt(
            "flac",
            "STREAMINFO has the wrong size",
        ));
    }
    let mut si = [0u8; 34];
    if !read_exact(r, &mut si) {
        return Ok(ProbedContainer::corrupt("flac", "truncated STREAMINFO"));
    }
    // 20 bits sample rate, 3 bits channels-1, 5 bits bps-1, 36 bits samples.
    let rate = (u32::from(si[10]) << 12) | (u32::from(si[11]) << 4) | (u32::from(si[12]) >> 4);
    let channels = u64::from((si[12] >> 1) & 0x07) + 1;
    let bits = (u64::from(si[12] & 1) << 4 | u64::from(si[13] >> 4)) + 1;
    let total = (u64::from(si[13] & 0x0F) << 32)
        | (u64::from(si[14]) << 24)
        | (u64::from(si[15]) << 16)
        | (u64::from(si[16]) << 8)
        | u64::from(si[17]);
    if rate == 0 {
        return Ok(ProbedContainer::corrupt(
            "flac",
            "STREAMINFO sample rate is zero",
        ));
    }
    // total == 0 means "unknown".
    let duration_ms = (total > 0).then(|| total * 1000 / u64::from(rate));
    let mut out = ProbedContainer::new("flac");
    out.duration_ms = duration_ms;
    out.timecode_present = Some(false);
    out.streams.push(audio_stream(
        "flac",
        channels,
        u64::from(rate),
        Some(bits),
        bitrate_from(len, duration_ms),
        duration_ms,
    ));
    Ok(out)
}

// ---------------------------------------------------------------------------
// Ogg (Opus / Vorbis)
// ---------------------------------------------------------------------------

/// First packet of the first Ogg page and that page's total length.
fn first_ogg_packet<R: Read + Seek>(r: &mut R) -> Option<Vec<u8>> {
    r.seek(SeekFrom::Start(0)).ok()?;
    let mut header = [0u8; 27];
    if !read_exact(r, &mut header) || &header[..4] != b"OggS" {
        return None;
    }
    let segments = usize::from(header[26]);
    let mut table = vec![0u8; segments];
    if !read_exact(r, &mut table) {
        return None;
    }
    // The BOS packet of a logical stream is a single, short packet.
    let mut packet_len = 0usize;
    for s in &table {
        packet_len += usize::from(*s);
        if *s < 255 {
            break;
        }
    }
    if packet_len == 0 || packet_len > 4096 {
        return None;
    }
    let mut packet = vec![0u8; packet_len];
    read_exact(r, &mut packet).then_some(packet)
}

/// Granule position of the last Ogg page, found by scanning the final 64 KiB.
fn last_granule<R: Read + Seek>(r: &mut R, len: u64) -> Option<i64> {
    let window = len.min(64 * 1024);
    r.seek(SeekFrom::Start(len - window)).ok()?;
    let mut tail = vec![0u8; window as usize];
    if !read_exact(r, &mut tail) {
        return None;
    }
    let mut best = None;
    let mut i = 0;
    while i + 14 <= tail.len() {
        if &tail[i..i + 4] == b"OggS" {
            let granule = i64::from_le_bytes([
                tail[i + 6],
                tail[i + 7],
                tail[i + 8],
                tail[i + 9],
                tail[i + 10],
                tail[i + 11],
                tail[i + 12],
                tail[i + 13],
            ]);
            if granule >= 0 {
                best = Some(granule);
            }
        }
        i += 1;
    }
    best
}

pub fn probe_ogg<R: Read + Seek>(r: &mut R, len: u64) -> Result<ProbedContainer> {
    let Some(packet) = first_ogg_packet(r) else {
        return Ok(ProbedContainer::corrupt(
            "ogg",
            "no readable first Ogg page",
        ));
    };
    let granule = last_granule(r, len);

    if packet.starts_with(b"OpusHead") && packet.len() >= 19 {
        let channels = u64::from(packet[9]);
        let pre_skip = i64::from(u16::from_le_bytes([packet[10], packet[11]]));
        if channels == 0 {
            return Ok(ProbedContainer::corrupt(
                "ogg",
                "OpusHead declares zero channels",
            ));
        }
        // Opus always plays at 48 kHz; the header rate is informational.
        let duration_ms = granule.map(|g| ((g - pre_skip).max(0) as u64) * 1000 / 48_000);
        let mut out = ProbedContainer::new("ogg");
        out.duration_ms = duration_ms;
        out.timecode_present = Some(false);
        out.streams.push(audio_stream(
            "opus",
            channels,
            48_000,
            None,
            bitrate_from(len, duration_ms),
            duration_ms,
        ));
        return Ok(out);
    }

    if packet.len() >= 30 && packet[0] == 1 && &packet[1..7] == b"vorbis" {
        let channels = u64::from(packet[11]);
        let rate = u64::from(u32le(&packet[12..16]));
        let nominal = i32::from_le_bytes([packet[20], packet[21], packet[22], packet[23]]);
        if channels == 0 || rate == 0 {
            return Ok(ProbedContainer::corrupt(
                "ogg",
                "Vorbis header has zero channels or rate",
            ));
        }
        let duration_ms = granule.map(|g| (g.max(0) as u64) * 1000 / rate);
        let bitrate = (nominal > 0)
            .then_some(nominal as u64)
            .or_else(|| bitrate_from(len, duration_ms));
        let mut out = ProbedContainer::new("ogg");
        out.duration_ms = duration_ms;
        out.timecode_present = Some(false);
        out.streams.push(audio_stream(
            "vorbis",
            channels,
            rate,
            None,
            bitrate,
            duration_ms,
        ));
        return Ok(out);
    }

    let what = if packet.starts_with(b"\x7fFLAC") {
        "Ogg FLAC".to_string()
    } else if packet.starts_with(b"\x80theora") {
        "Theora video in Ogg".to_string()
    } else if packet.starts_with(b"Speex") {
        "Speex audio in Ogg".to_string()
    } else {
        "unknown codec in Ogg".to_string()
    };
    Err(unsupported_error(&what))
}

#[allow(dead_code)]
fn _assert_error_is_used(e: Error) -> Error {
    e
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn wav(tag: u16, channels: u16, rate: u32, bits: u16, frames: u32) -> Vec<u8> {
        let block = u32::from(channels) * u32::from(bits) / 8;
        let data = frames * block;
        let mut v = b"RIFF".to_vec();
        v.extend((36 + data).to_le_bytes());
        v.extend(b"WAVEfmt ");
        v.extend(16u32.to_le_bytes());
        v.extend(tag.to_le_bytes());
        v.extend(channels.to_le_bytes());
        v.extend(rate.to_le_bytes());
        v.extend((rate * block).to_le_bytes());
        v.extend((block as u16).to_le_bytes());
        v.extend(bits.to_le_bytes());
        v.extend(b"data");
        v.extend(data.to_le_bytes());
        v.extend(vec![0u8; data as usize]);
        v
    }

    fn run_wav(bytes: Vec<u8>) -> Result<ProbedContainer> {
        let len = bytes.len() as u64;
        probe_wav(&mut Cursor::new(bytes), len)
    }

    #[test]
    fn wav_pcm_and_float() {
        let c = run_wav(wav(1, 2, 48_000, 24, 96_000)).unwrap();
        let s = &c.streams[0].stream;
        assert_eq!(s.codec.as_deref(), Some("pcm_s24le"));
        assert_eq!(
            (s.channels, s.sample_rate, s.bit_depth),
            (Some(2), Some(48_000), Some(24))
        );
        assert_eq!(s.channel_layout.as_deref(), Some("stereo"));
        assert_eq!(c.duration_ms, Some(2000));
        assert_eq!(c.validity, ContainerValidity::Ok);
        assert_eq!(c.timecode_present, Some(false));

        let f = run_wav(wav(3, 1, 44_100, 32, 44_100)).unwrap();
        assert_eq!(f.streams[0].stream.codec.as_deref(), Some("pcm_f32le"));
    }

    #[test]
    fn wav_truncated_data_is_corrupt_not_an_error() {
        let mut bytes = wav(1, 1, 8000, 16, 8000);
        bytes.truncate(bytes.len() - 4000);
        let c = run_wav(bytes).unwrap();
        assert!(
            matches!(c.validity, ContainerValidity::Corrupt(_)),
            "{:?}",
            c.validity
        );
        assert!(c.duration_ms.unwrap() < 1000);
    }

    #[test]
    fn wav_with_a_patented_format_tag_is_refused() {
        // 0x0055 = MPEG Layer 3
        let e = run_wav(wav(0x55, 2, 44_100, 16, 10))
            .unwrap_err()
            .to_string();
        assert!(e.contains("royalty-free"), "{e}");
    }

    #[test]
    fn wav_garbage_is_corrupt() {
        let c = run_wav(b"RIFF\0\0\0\0WAVEjunkjunkjunkjunk".to_vec()).unwrap();
        assert!(matches!(c.validity, ContainerValidity::Corrupt(_)));
    }

    #[test]
    fn aiff_sample_rate_and_duration() {
        let mut v = b"FORM".to_vec();
        v.extend(0u32.to_be_bytes());
        v.extend(b"AIFFCOMM");
        v.extend(18u32.to_be_bytes());
        v.extend(2u16.to_be_bytes());
        v.extend(44_100u32.to_be_bytes());
        v.extend(16u16.to_be_bytes());
        // 44100 as 80-bit extended: exponent 0x400E, mantissa 0xAC44 << 48
        v.extend([0x40, 0x0E, 0xAC, 0x44, 0, 0, 0, 0, 0, 0]);
        v.extend(b"SSND");
        v.extend(8u32.to_be_bytes());
        let len = v.len() as u64;
        let c = probe_aiff(&mut Cursor::new(v), len).unwrap();
        let s = &c.streams[0].stream;
        assert_eq!(s.sample_rate, Some(44_100));
        assert_eq!(s.codec.as_deref(), Some("pcm_s16be"));
        assert_eq!(c.duration_ms, Some(1000));
    }

    #[test]
    fn flac_streaminfo() {
        // 48000 Hz, 2 ch, 16 bit, 96000 samples
        let mut si = [0u8; 34];
        let rate: u32 = 48_000;
        si[10] = (rate >> 12) as u8;
        si[11] = (rate >> 4) as u8;
        si[12] = ((rate & 0xF) << 4) as u8 | (1 << 1); // channels-1 = 1
        si[13] = 15 << 4; // bps-1 = 15 (low bit in si[12] is 0)
        let total: u64 = 96_000;
        si[13] |= ((total >> 32) & 0x0F) as u8;
        si[14] = (total >> 24) as u8;
        si[15] = (total >> 16) as u8;
        si[16] = (total >> 8) as u8;
        si[17] = total as u8;
        let mut v = b"fLaC".to_vec();
        v.extend([0x00, 0, 0, 34]);
        v.extend(si);
        let len = v.len() as u64;
        let c = probe_flac(&mut Cursor::new(v), len).unwrap();
        let s = &c.streams[0].stream;
        assert_eq!(s.codec.as_deref(), Some("flac"));
        assert_eq!(
            (s.channels, s.sample_rate, s.bit_depth),
            (Some(2), Some(48_000), Some(16))
        );
        assert_eq!(c.duration_ms, Some(2000));
    }

    fn ogg_page(packet: &[u8], granule: i64, bos: bool) -> Vec<u8> {
        let mut v = b"OggS".to_vec();
        v.push(0);
        v.push(if bos { 2 } else { 0 });
        v.extend(granule.to_le_bytes());
        v.extend(1u32.to_le_bytes());
        v.extend(0u32.to_le_bytes());
        v.extend(0u32.to_le_bytes());
        v.push(1);
        v.push(packet.len() as u8);
        v.extend(packet);
        v
    }

    #[test]
    fn ogg_opus_duration_uses_pre_skip() {
        let mut head = b"OpusHead".to_vec();
        head.push(1);
        head.push(2);
        head.extend(312u16.to_le_bytes());
        head.extend(48_000u32.to_le_bytes());
        head.extend(0i16.to_le_bytes());
        head.push(0);
        let mut v = ogg_page(&head, 0, true);
        v.extend(ogg_page(b"payload", 96_312, false));
        let len = v.len() as u64;
        let c = probe_ogg(&mut Cursor::new(v), len).unwrap();
        assert_eq!(c.streams[0].stream.codec.as_deref(), Some("opus"));
        assert_eq!(c.duration_ms, Some(2000));
    }

    #[test]
    fn ogg_vorbis_header() {
        let mut head = vec![1u8];
        head.extend(b"vorbis");
        head.extend(0u32.to_le_bytes());
        head.push(2);
        head.extend(44_100u32.to_le_bytes());
        head.extend(0i32.to_le_bytes());
        head.extend(128_000i32.to_le_bytes());
        head.extend(0i32.to_le_bytes());
        head.push(0xB8);
        head.push(1);
        let mut v = ogg_page(&head, 0, true);
        v.extend(ogg_page(b"x", 88_200, false));
        let len = v.len() as u64;
        let c = probe_ogg(&mut Cursor::new(v), len).unwrap();
        let s = &c.streams[0].stream;
        assert_eq!(s.codec.as_deref(), Some("vorbis"));
        assert_eq!(s.bitrate, Some(128_000));
        assert_eq!(c.duration_ms, Some(2000));
    }

    #[test]
    fn ogg_with_other_codecs_is_refused() {
        let v = ogg_page(b"\x80theora012345678901234567890", 0, true);
        let len = v.len() as u64;
        assert!(probe_ogg(&mut Cursor::new(v), len).is_err());
    }
}
