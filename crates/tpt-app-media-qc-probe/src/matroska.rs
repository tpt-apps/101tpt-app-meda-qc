//! Matroska / WebM reader.
//!
//! A small, seek-based EBML walker. Only the metadata a QC run needs is read:
//! the EBML header, Segment Info, Tracks, Tags (to look for a timecode tag)
//! and the *headers* of every Block in every Cluster. Block payloads are never
//! read except for text subtitle tracks, so a multi-gigabyte file costs a few
//! seeks and small reads per Block.
//!
//! Policy: every track is classified through [`crate::codec`] as soon as the
//! Tracks element is read. A refused codec fails the whole file before any
//! `CodecPrivate` is looked at. Passive tracks are listed but not interpreted.
//!
//! The reader never panics and always makes forward progress on arbitrary
//! bytes: every element advances the position by at least one byte, every
//! allocation is bounded by [`MAX_BUFFERED_BYTES`] and the remaining file
//! length, and the number of Blocks, tracks, cues and gap ranges is capped.

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Seek, SeekFrom};

use tpt_app_media_qc_core::error::Result;
use tpt_app_media_qc_model::asset::{FieldOrder, Stream, StreamId, StreamKind};
use tpt_app_media_qc_model::finding::TimeRange;
use tpt_app_media_qc_model::inspection::{
    ContainerValidity, HdrMetadata, MasteringDisplay, VideoMeasurements,
};
use tpt_app_media_qc_model::time::{DurationSeconds, Rational};

use crate::audio_files::channel_layout;
use crate::codec::{classify_matroska, pcm_codec_name, unsupported_error, Class};
use crate::common::{ProbedContainer, ProbedStream, RawCue, MAX_BUFFERED_BYTES, MAX_CUES};
use crate::hdr::{self, Cicp};

const FORMAT: &str = "matroska,webm";

// Element IDs (marker bit kept).
const ID_EBML: u32 = 0x1A45_DFA3;
const ID_DOCTYPE: u32 = 0x4282;
const ID_SEGMENT: u32 = 0x1853_8067;
const ID_SEEKHEAD: u32 = 0x114D_9B74;
const ID_INFO: u32 = 0x1549_A966;
const ID_TRACKS: u32 = 0x1654_AE6B;
const ID_CLUSTER: u32 = 0x1F43_B675;
const ID_CUES: u32 = 0x1C53_BB6B;
const ID_TAGS: u32 = 0x1254_C367;
const ID_CHAPTERS: u32 = 0x1043_A770;
const ID_ATTACHMENTS: u32 = 0x1941_A469;

const ID_TIMESTAMP_SCALE: u32 = 0x2A_D7B1;
const ID_DURATION: u32 = 0x4489;

const ID_TRACK_ENTRY: u32 = 0xAE;
const ID_TRACK_NUMBER: u32 = 0xD7;
const ID_TRACK_TYPE: u32 = 0x83;
const ID_FLAG_ENABLED: u32 = 0xB9;
const ID_FLAG_DEFAULT: u32 = 0x88;
const ID_FLAG_FORCED: u32 = 0x55AA;
const ID_FLAG_INTERLACED: u32 = 0x9A;
const ID_FIELD_ORDER: u32 = 0x9D;
const ID_DEFAULT_DURATION: u32 = 0x23_E383;
const ID_NAME: u32 = 0x536E;
const ID_LANGUAGE: u32 = 0x22_B59C;
const ID_LANGUAGE_BCP47: u32 = 0x22_B59D;
const ID_CODEC_ID: u32 = 0x86;
const ID_CODEC_PRIVATE: u32 = 0x63A2;
const ID_VIDEO: u32 = 0xE0;
const ID_AUDIO: u32 = 0xE1;
const ID_PIXEL_WIDTH: u32 = 0xB0;
const ID_PIXEL_HEIGHT: u32 = 0xBA;
const ID_SAMPLING_FREQ: u32 = 0xB5;
const ID_CHANNELS: u32 = 0x9F;
const ID_BIT_DEPTH: u32 = 0x6264;

const ID_COLOUR: u32 = 0x55B0;
const ID_MATRIX: u32 = 0x55B1;
const ID_RANGE: u32 = 0x55B9;
const ID_TRANSFER: u32 = 0x55BA;
const ID_PRIMARIES: u32 = 0x55BB;
const ID_MAX_CLL: u32 = 0x55BC;
const ID_MAX_FALL: u32 = 0x55BD;
const ID_MASTERING: u32 = 0x55D0;
const ID_R_X: u32 = 0x55D1;
const ID_R_Y: u32 = 0x55D2;
const ID_G_X: u32 = 0x55D3;
const ID_G_Y: u32 = 0x55D4;
const ID_B_X: u32 = 0x55D5;
const ID_B_Y: u32 = 0x55D6;
const ID_W_X: u32 = 0x55D7;
const ID_W_Y: u32 = 0x55D8;
const ID_L_MAX: u32 = 0x55D9;
const ID_L_MIN: u32 = 0x55DA;

const ID_C_TIMESTAMP: u32 = 0xE7;
const ID_SIMPLE_BLOCK: u32 = 0xA3;
const ID_BLOCK_GROUP: u32 = 0xA0;
const ID_BLOCK: u32 = 0xA1;
const ID_BLOCK_DURATION: u32 = 0x9B;

const ID_TAG: u32 = 0x7373;
const ID_SIMPLE_TAG: u32 = 0x67C8;
const ID_TAG_NAME: u32 = 0x45A3;

const DEFAULT_SCALE: u64 = 1_000_000;
const MAX_TRACKS: usize = 64;
const MAX_BLOCKS: u64 = 20_000_000;
const MAX_GAPS: usize = 1000;
const MAX_DEPTH: u32 = 12;
const MAX_TAGS_BYTES: u64 = 4 * 1024 * 1024;
const MAX_PRIVATE_BYTES: usize = 64 * 1024;
const MAX_CUE_PAYLOAD: u64 = 1024 * 1024;
const MAX_LACE_HEADER: u64 = 64 * 1024;
const MAX_HIST: usize = 4096;
const MAX_CANDIDATES: usize = 4096;
const MIN_GAP_NS: i128 = 20_000_000;
const CACHE_SIZE: usize = 8192;
/// Sanity clamp for any timestamp turned into milliseconds.
const MAX_MS: u64 = 1 << 48;

// ---------------------------------------------------------------------------
// Byte-level helpers
// ---------------------------------------------------------------------------

struct Vint {
    len: usize,
    /// Value with the length marker removed.
    value: u64,
    /// Value with the marker kept (an element ID).
    raw: u64,
    /// All value bits set: "unknown size".
    unknown: bool,
}

fn vint(b: &[u8]) -> Option<Vint> {
    let first = *b.first()?;
    if first == 0 {
        return None;
    }
    let len = first.leading_zeros() as usize + 1;
    let bytes = b.get(..len)?;
    let mut raw = 0u64;
    for &x in bytes {
        raw = (raw << 8) | u64::from(x);
    }
    let mask = (1u64 << (7 * len)) - 1;
    let value = raw & mask;
    Some(Vint {
        len,
        value,
        raw,
        unknown: value == mask,
    })
}

fn uint(b: &[u8]) -> u64 {
    b.iter()
        .rev()
        .take(8)
        .rev()
        .fold(0u64, |acc, &x| (acc << 8) | u64::from(x))
}

fn float(b: &[u8]) -> Option<f64> {
    let v = match b.len() {
        4 => f64::from(f32::from_be_bytes([b[0], b[1], b[2], b[3]])),
        8 => f64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]),
        _ => return None,
    };
    v.is_finite().then_some(v)
}

fn string(b: &[u8]) -> String {
    let end = b.iter().position(|&x| x == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).into_owned()
}

/// Children of an in-memory master element. Tolerant: stops at the first
/// malformed child; a child that overruns the buffer is clipped.
struct Elems<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Elems<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }
}

impl<'a> Iterator for Elems<'a> {
    type Item = (u32, &'a [u8]);

    fn next(&mut self) -> Option<Self::Item> {
        let rest = self.buf.get(self.pos..)?;
        if rest.is_empty() {
            return None;
        }
        let parsed = (|| {
            let id = vint(rest)?;
            if id.len > 4 {
                return None;
            }
            let size = vint(rest.get(id.len..)?)?;
            Some((id, size))
        })();
        let Some((id, size)) = parsed else {
            self.pos = self.buf.len();
            return None;
        };
        let body_start = id.len + size.len;
        let avail = rest.len().saturating_sub(body_start);
        let n = if size.unknown || size.value > avail as u64 {
            avail
        } else {
            size.value as usize
        };
        let body = rest.get(body_start..body_start + n)?;
        self.pos += body_start + n;
        Some((id.raw as u32, body))
    }
}

fn ticks_to_ms(ticks: i64, scale: u64) -> u64 {
    let v = (ticks.max(0) as u128) * u128::from(scale) / 1_000_000;
    u64::try_from(v).unwrap_or(MAX_MS).min(MAX_MS)
}

fn ratio(num: u128, den: u128) -> Option<Rational> {
    Rational::new(u64::try_from(num).ok()?, u64::try_from(den).ok()?)
}

// ---------------------------------------------------------------------------
// Buffered random-access reader
// ---------------------------------------------------------------------------

struct Rd<'a, R> {
    src: &'a mut R,
    cache: Vec<u8>,
    cache_pos: u64,
    len: u64,
}

fn fill<R: Read>(src: &mut R, out: &mut [u8]) -> usize {
    let mut done = 0;
    while done < out.len() {
        match src.read(&mut out[done..]) {
            Ok(0) => break,
            Ok(n) => done += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    done
}

impl<'a, R: Read + Seek> Rd<'a, R> {
    fn new(src: &'a mut R, len: u64) -> Self {
        Self {
            src,
            cache: Vec::new(),
            cache_pos: 0,
            len,
        }
    }

    /// Read as many bytes as available at `pos` (never past `len`).
    fn read_up_to(&mut self, pos: u64, out: &mut [u8]) -> usize {
        let mut done = 0usize;
        while done < out.len() {
            let p = pos.saturating_add(done as u64);
            if p >= self.len {
                break;
            }
            let cache_end = self.cache_pos + self.cache.len() as u64;
            if p >= self.cache_pos && p < cache_end {
                let off = (p - self.cache_pos) as usize;
                let n = (out.len() - done).min(self.cache.len() - off);
                out[done..done + n].copy_from_slice(&self.cache[off..off + n]);
                done += n;
                continue;
            }
            let want = out.len() - done;
            if self.src.seek(SeekFrom::Start(p)).is_err() {
                break;
            }
            if want >= CACHE_SIZE {
                let avail = (self.len - p).min(want as u64) as usize;
                let n = fill(self.src, &mut out[done..done + avail]);
                if n == 0 {
                    break;
                }
                done += n;
            } else {
                let n = (self.len - p).min(CACHE_SIZE as u64) as usize;
                self.cache.clear();
                self.cache.resize(n, 0);
                let got = fill(self.src, &mut self.cache);
                self.cache.truncate(got);
                self.cache_pos = p;
                if got == 0 {
                    break;
                }
            }
        }
        done
    }

    fn read_vec(&mut self, pos: u64, n: u64) -> Option<Vec<u8>> {
        if n > MAX_BUFFERED_BYTES || pos.checked_add(n)? > self.len {
            return None;
        }
        let mut v = vec![0u8; n as usize];
        (self.read_up_to(pos, &mut v) == v.len()).then_some(v)
    }
}

// ---------------------------------------------------------------------------
// Parsed structures
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Header {
    id: u32,
    data_start: u64,
    /// `None` = unknown size.
    size: Option<u64>,
}

/// End offset of `h`'s body clamped to `limit`, and whether it was clamped.
fn bound(h: &Header, limit: u64) -> (u64, bool) {
    match h.size {
        Some(s) => match h.data_start.checked_add(s) {
            Some(e) if e <= limit => (e, false),
            _ => (limit, true),
        },
        None => (limit, false),
    }
}

fn is_level1(id: u32) -> bool {
    matches!(
        id,
        ID_SEEKHEAD
            | ID_INFO
            | ID_TRACKS
            | ID_CLUSTER
            | ID_CUES
            | ID_TAGS
            | ID_CHAPTERS
            | ID_ATTACHMENTS
            | ID_SEGMENT
            | ID_EBML
    )
}

#[derive(Default)]
struct Mastering {
    rx: Option<f64>,
    ry: Option<f64>,
    gx: Option<f64>,
    gy: Option<f64>,
    bx: Option<f64>,
    by: Option<f64>,
    wx: Option<f64>,
    wy: Option<f64>,
    lmax: Option<f64>,
    lmin: Option<f64>,
}

impl Mastering {
    fn display(&self) -> Option<MasteringDisplay> {
        let m = MasteringDisplay {
            min_luminance_nits: self.lmin,
            max_luminance_nits: self.lmax,
            primaries_xy: match (self.rx, self.ry, self.gx, self.gy, self.bx, self.by) {
                (Some(rx), Some(ry), Some(gx), Some(gy), Some(bx), Some(by)) => {
                    Some([(rx, ry), (gx, gy), (bx, by)])
                }
                _ => None,
            },
            white_point_xy: self.wx.zip(self.wy),
        };
        (m != MasteringDisplay::default()).then_some(m)
    }
}

#[derive(Default)]
struct Colour {
    present: bool,
    matrix: Option<u64>,
    range: Option<u64>,
    transfer: Option<u64>,
    primaries: Option<u64>,
    max_cll: Option<u64>,
    max_fall: Option<u64>,
    mastering: Option<Mastering>,
}

#[derive(Default)]
struct RawTrack {
    number: u64,
    has_number: bool,
    ttype: u8,
    codec_id: Option<String>,
    enabled: Option<bool>,
    default: Option<bool>,
    forced: Option<bool>,
    interlaced: u64,
    field_order: Option<u64>,
    default_duration: Option<u64>,
    name: Option<String>,
    language: Option<String>,
    bcp47: Option<String>,
    private: Option<Vec<u8>>,
    width: Option<u64>,
    height: Option<u64>,
    sampling: Option<f64>,
    channels: Option<u64>,
    bit_depth: Option<u64>,
    colour: Colour,
}

fn parse_colour(body: &[u8]) -> Colour {
    let mut c = Colour {
        present: true,
        ..Default::default()
    };
    for (id, b) in Elems::new(body) {
        match id {
            ID_MATRIX => c.matrix = Some(uint(b)),
            ID_RANGE => c.range = Some(uint(b)),
            ID_TRANSFER => c.transfer = Some(uint(b)),
            ID_PRIMARIES => c.primaries = Some(uint(b)),
            ID_MAX_CLL => c.max_cll = Some(uint(b)),
            ID_MAX_FALL => c.max_fall = Some(uint(b)),
            ID_MASTERING => {
                let mut m = Mastering::default();
                for (mid, mb) in Elems::new(b) {
                    let slot = match mid {
                        ID_R_X => &mut m.rx,
                        ID_R_Y => &mut m.ry,
                        ID_G_X => &mut m.gx,
                        ID_G_Y => &mut m.gy,
                        ID_B_X => &mut m.bx,
                        ID_B_Y => &mut m.by,
                        ID_W_X => &mut m.wx,
                        ID_W_Y => &mut m.wy,
                        ID_L_MAX => &mut m.lmax,
                        ID_L_MIN => &mut m.lmin,
                        _ => continue,
                    };
                    *slot = float(mb);
                }
                c.mastering = Some(m);
            }
            _ => {}
        }
    }
    c
}

fn parse_track_entry(body: &[u8]) -> RawTrack {
    let mut t = RawTrack::default();
    for (id, b) in Elems::new(body) {
        match id {
            ID_TRACK_NUMBER => {
                t.number = uint(b);
                t.has_number = true;
            }
            ID_TRACK_TYPE => t.ttype = u8::try_from(uint(b)).unwrap_or(0),
            ID_FLAG_ENABLED => t.enabled = Some(uint(b) != 0),
            ID_FLAG_DEFAULT => t.default = Some(uint(b) != 0),
            ID_FLAG_FORCED => t.forced = Some(uint(b) != 0),
            ID_FLAG_INTERLACED => t.interlaced = uint(b),
            ID_FIELD_ORDER => t.field_order = Some(uint(b)),
            ID_DEFAULT_DURATION => t.default_duration = Some(uint(b)),
            ID_NAME => t.name = Some(string(b)),
            ID_LANGUAGE => t.language = Some(string(b)),
            ID_LANGUAGE_BCP47 => t.bcp47 = Some(string(b)),
            ID_CODEC_ID => t.codec_id = Some(string(b)),
            ID_CODEC_PRIVATE if b.len() <= MAX_PRIVATE_BYTES => t.private = Some(b.to_vec()),
            ID_VIDEO => {
                for (vid, vb) in Elems::new(b) {
                    match vid {
                        ID_PIXEL_WIDTH => t.width = Some(uint(vb)),
                        ID_PIXEL_HEIGHT => t.height = Some(uint(vb)),
                        ID_FLAG_INTERLACED => t.interlaced = uint(vb),
                        ID_FIELD_ORDER => t.field_order = Some(uint(vb)),
                        ID_COLOUR => t.colour = parse_colour(vb),
                        _ => {}
                    }
                }
            }
            ID_AUDIO => {
                for (aid, ab) in Elems::new(b) {
                    match aid {
                        ID_SAMPLING_FREQ => t.sampling = float(ab),
                        ID_CHANNELS => t.channels = Some(uint(ab)),
                        ID_BIT_DEPTH => t.bit_depth = Some(uint(ab)),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    t
}

fn has_timecode_tag(body: &[u8], depth: u32) -> bool {
    if depth > MAX_DEPTH {
        return false;
    }
    Elems::new(body).any(|(id, b)| match id {
        ID_TAG | ID_SIMPLE_TAG => has_timecode_tag(b, depth + 1),
        ID_TAG_NAME => string(b).eq_ignore_ascii_case("TIMECODE"),
        ID_TAGS => has_timecode_tag(b, depth + 1),
        _ => false,
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SubKind {
    Plain,
    Ass,
}

#[derive(Clone, Copy)]
struct TrackInfo {
    kind: StreamKind,
    /// Text subtitle track whose payloads are read.
    sub: Option<SubKind>,
}

struct CueRaw {
    start: i64,
    dur: Option<u64>,
    payload: Option<Vec<u8>>,
}

#[derive(Default)]
struct CueSet {
    cues: Vec<CueRaw>,
    truncated: bool,
}

#[derive(Default)]
struct Acc {
    blocks: u64,
    frames: u64,
    bytes: u64,
    min_ts: Option<i64>,
    max_ts: Option<i64>,
    last_ts: Option<i64>,
    hist: BTreeMap<i64, u64>,
    hist_total: u64,
    est: Option<i64>,
    candidates: Vec<(i64, i64)>,
}

fn median(hist: &BTreeMap<i64, u64>, total: u64) -> Option<i64> {
    if total == 0 {
        return None;
    }
    let target = total.div_ceil(2);
    let mut seen = 0u64;
    for (&k, &c) in hist {
        seen += c;
        if seen >= target {
            return Some(k);
        }
    }
    None
}

impl Acc {
    fn add(&mut self, ts: i64, frames: u64, bytes: u64) {
        self.blocks += 1;
        self.frames = self.frames.saturating_add(frames);
        self.bytes = self.bytes.saturating_add(bytes);
        self.min_ts = Some(self.min_ts.map_or(ts, |m| m.min(ts)));
        self.max_ts = Some(self.max_ts.map_or(ts, |m| m.max(ts)));
        if let Some(prev) = self.last_ts {
            let d = ts.saturating_sub(prev);
            if d > 0 {
                self.hist_add(d);
            }
            let candidate = d < 0
                || (d > 0
                    && self
                        .est
                        .is_none_or(|m| i128::from(d) * 2 > i128::from(m) * 3));
            if candidate && self.candidates.len() < MAX_CANDIDATES {
                self.candidates.push((prev, ts));
            }
        }
        self.last_ts = Some(ts);
    }

    fn hist_add(&mut self, d: i64) {
        if let Some(c) = self.hist.get_mut(&d) {
            *c += 1;
        } else if self.hist.len() < MAX_HIST {
            self.hist.insert(d, 1);
        } else {
            return;
        }
        self.hist_total += 1;
        if self.hist_total <= 64 || self.hist_total % 256 == 0 {
            self.est = median(&self.hist, self.hist_total);
        }
    }

    fn median(&self) -> Option<i64> {
        median(&self.hist, self.hist_total)
    }
}

struct BlockInfo {
    track: u64,
    rel: i16,
    flags: u8,
    body_pos: u64,
    body_size: u64,
}

// ---------------------------------------------------------------------------
// The walker
// ---------------------------------------------------------------------------

struct Probe<'a, R> {
    rd: Rd<'a, R>,
    len: u64,
    scale: u64,
    duration_raw: Option<f64>,
    info_seen: bool,
    tracks_seen: bool,
    tracks: Vec<(RawTrack, Class)>,
    by_number: HashMap<u64, TrackInfo>,
    acc: HashMap<u64, Acc>,
    cues: HashMap<u64, CueSet>,
    max_end_ticks: i64,
    blocks_scanned: u64,
    timecode_tag: bool,
    notes: Vec<String>,
    broken: Option<String>,
    truncated: Option<String>,
    stop: bool,
    capped: bool,
}

impl<'a, R: Read + Seek> Probe<'a, R> {
    fn new(src: &'a mut R, len: u64) -> Self {
        Self {
            rd: Rd::new(src, len),
            len,
            scale: DEFAULT_SCALE,
            duration_raw: None,
            info_seen: false,
            tracks_seen: false,
            tracks: Vec::new(),
            by_number: HashMap::new(),
            acc: HashMap::new(),
            cues: HashMap::new(),
            max_end_ticks: 0,
            blocks_scanned: 0,
            timecode_tag: false,
            notes: Vec::new(),
            broken: None,
            truncated: None,
            stop: false,
            capped: false,
        }
    }

    fn note(&mut self, s: impl Into<String>) {
        if self.notes.len() < 64 {
            self.notes.push(s.into());
        }
    }

    fn header(&mut self, pos: u64) -> Option<Header> {
        let mut buf = [0u8; 12];
        let want = self.len.saturating_sub(pos).min(12) as usize;
        let n = self.rd.read_up_to(pos, &mut buf[..want]);
        let id = vint(&buf[..n])?;
        if id.len > 4 {
            return None;
        }
        let size = vint(&buf[id.len..n])?;
        Some(Header {
            id: id.raw as u32,
            data_start: pos + (id.len + size.len) as u64,
            size: (!size.unknown).then_some(size.value),
        })
    }

    fn bad_header(&mut self, pos: u64) {
        if self.len.saturating_sub(pos) < 12 {
            self.truncated.get_or_insert_with(|| {
                format!("truncated: file ends inside an element at offset {pos}")
            });
        } else {
            self.broken
                .get_or_insert_with(|| format!("invalid element header at offset {pos}"));
        }
    }

    fn truncated_at(&mut self, h: &Header) {
        self.truncated.get_or_insert_with(|| {
            format!(
                "truncated: element 0x{:X} at offset {} declares more data than the file holds",
                h.id, h.data_start
            )
        });
    }

    fn read_body(&mut self, h: &Header, end: u64, cap: u64) -> Option<Vec<u8>> {
        let n = end.saturating_sub(h.data_start);
        if n > cap {
            self.note(format!("element 0x{:X} is too large to inspect", h.id));
            return None;
        }
        self.rd.read_vec(h.data_start, n)
    }

    fn read_uint(&mut self, h: &Header, end: u64) -> Option<u64> {
        let n = end.saturating_sub(h.data_start);
        if n > 8 {
            return None;
        }
        let v = self.rd.read_vec(h.data_start, n)?;
        Some(uint(&v))
    }

    // ---- top level -------------------------------------------------------

    fn run(&mut self) -> Result<ProbedContainer> {
        let Some(h) = self.header(0).filter(|h| h.id == ID_EBML) else {
            return Ok(ProbedContainer::corrupt(
                FORMAT,
                "not a valid EBML header (not a Matroska/WebM file)",
            ));
        };
        let Some(size) = h.size.filter(|s| *s <= 4096) else {
            return Ok(ProbedContainer::corrupt(FORMAT, "invalid EBML header size"));
        };
        let Some(body) = self.rd.read_vec(h.data_start, size) else {
            return Ok(ProbedContainer::corrupt(FORMAT, "truncated EBML header"));
        };
        let doctype = Elems::new(&body)
            .find(|(id, _)| *id == ID_DOCTYPE)
            .map(|(_, b)| string(b));
        match doctype.as_deref() {
            Some("webm" | "matroska") => {}
            Some(other) => self.note(format!("unrecognised DocType '{other}'")),
            None => self.note("EBML header has no DocType"),
        }

        // Find the Segment.
        let mut pos = h.data_start + size;
        let mut segment = None;
        while pos < self.len {
            let Some(c) = self.header(pos) else { break };
            if c.id == ID_SEGMENT {
                segment = Some(c);
                break;
            }
            let Some(next) = c.size.and_then(|s| c.data_start.checked_add(s)) else {
                break;
            };
            pos = next;
        }
        let Some(seg) = segment else {
            return Ok(ProbedContainer::corrupt(FORMAT, "no Segment element"));
        };

        self.walk_segment(&seg)?;

        if self.tracks.is_empty() {
            let reason = self
                .broken
                .clone()
                .or_else(|| self.truncated.clone())
                .unwrap_or_else(|| "no Tracks element".to_string());
            return Ok(ProbedContainer::corrupt(FORMAT, reason));
        }
        Ok(self.build(doctype))
    }

    fn walk_segment(&mut self, seg: &Header) -> Result<()> {
        let (end, clipped) = bound(seg, self.len);
        if clipped {
            self.truncated_at(seg);
        }
        let mut pos = seg.data_start;
        while pos < end && !self.stop {
            let Some(h) = self.header(pos) else {
                self.bad_header(pos);
                break;
            };
            if h.id == ID_CLUSTER {
                pos = self.scan_cluster(&h, end);
                continue;
            }
            if h.size.is_none() {
                self.broken.get_or_insert_with(|| {
                    format!("unknown-size element 0x{:X} at offset {pos}", h.id)
                });
                break;
            }
            let (next, clipped) = bound(&h, end);
            if clipped {
                self.truncated_at(&h);
            }
            match h.id {
                ID_INFO => self.handle_info(&h, next),
                ID_TRACKS => self.handle_tracks(&h, next)?,
                ID_TAGS => self.handle_tags(&h, next),
                _ => {}
            }
            pos = next;
        }
        Ok(())
    }

    fn handle_info(&mut self, h: &Header, end: u64) {
        if self.info_seen {
            return;
        }
        self.info_seen = true;
        let Some(body) = self.read_body(h, end, MAX_BUFFERED_BYTES) else {
            return;
        };
        for (id, b) in Elems::new(&body) {
            match id {
                ID_TIMESTAMP_SCALE => {
                    let s = uint(b);
                    if s == 0 {
                        self.note("TimestampScale is zero; assuming 1000000");
                    } else {
                        self.scale = s;
                    }
                }
                ID_DURATION => self.duration_raw = float(b),
                _ => {}
            }
        }
        if self.duration_raw.is_none() {
            self.note("Segment Info has no Duration");
        }
    }

    fn handle_tags(&mut self, h: &Header, end: u64) {
        let n = end.saturating_sub(h.data_start);
        if n > MAX_TAGS_BYTES {
            return;
        }
        if let Some(body) = self.rd.read_vec(h.data_start, n) {
            if has_timecode_tag(&body, 0) {
                self.timecode_tag = true;
            }
        }
    }

    fn handle_tracks(&mut self, h: &Header, end: u64) -> Result<()> {
        if self.tracks_seen {
            self.note("more than one Tracks element");
            return Ok(());
        }
        let Some(body) = self.read_body(h, end, MAX_BUFFERED_BYTES) else {
            return Ok(());
        };
        self.tracks_seen = true;
        for (id, b) in Elems::new(&body) {
            if id != ID_TRACK_ENTRY {
                continue;
            }
            let t = parse_track_entry(b);
            let class = match &t.codec_id {
                Some(c) => classify_matroska(c, t.ttype),
                None => {
                    self.note("TrackEntry without CodecID");
                    Class::Passive {
                        codec: String::new(),
                        kind: StreamKind::Data,
                    }
                }
            };
            // Policy gate: nothing of a refused track is interpreted.
            if let Class::Refused(what) = &class {
                return Err(unsupported_error(what));
            }
            if self.tracks.len() >= MAX_TRACKS {
                self.note("more than 64 tracks; the rest are ignored");
                continue;
            }
            if !t.has_number {
                self.note("TrackEntry without TrackNumber");
            }
            let info = match &class {
                Class::Supported {
                    codec,
                    kind: StreamKind::Subtitle,
                } => TrackInfo {
                    kind: StreamKind::Subtitle,
                    sub: Some(if matches!(*codec, "ass" | "ssa") {
                        SubKind::Ass
                    } else {
                        SubKind::Plain
                    }),
                },
                Class::Supported { kind, .. } | Class::Passive { kind, .. } => TrackInfo {
                    kind: *kind,
                    sub: None,
                },
                Class::Refused(_) => continue,
            };
            match self.by_number.entry(t.number) {
                std::collections::hash_map::Entry::Occupied(_) => {
                    self.note(format!("duplicate TrackNumber {}", t.number));
                }
                std::collections::hash_map::Entry::Vacant(v) => {
                    v.insert(info);
                }
            }
            self.tracks.push((t, class));
        }
        Ok(())
    }

    // ---- clusters --------------------------------------------------------

    /// Scan one Cluster; returns the offset where the walk should resume.
    fn scan_cluster(&mut self, h: &Header, limit: u64) -> u64 {
        let (end, clipped) = bound(h, limit);
        if clipped {
            self.truncated_at(h);
        }
        let unknown = h.size.is_none();
        let mut pos = h.data_start;
        let mut cts: i64 = 0;
        while pos < end && !self.stop {
            let Some(c) = self.header(pos) else {
                self.bad_header(pos);
                return self.len;
            };
            if unknown && is_level1(c.id) {
                return pos;
            }
            if c.size.is_none() {
                self.broken.get_or_insert_with(|| {
                    format!(
                        "unknown-size element 0x{:X} inside a Cluster at offset {pos}",
                        c.id
                    )
                });
                return self.len;
            }
            let (cend, clipped) = bound(&c, end);
            if clipped {
                self.truncated_at(&c);
            }
            match c.id {
                ID_C_TIMESTAMP => {
                    cts = self
                        .read_uint(&c, cend)
                        .map_or(0, |v| i64::try_from(v).unwrap_or(i64::MAX));
                }
                ID_SIMPLE_BLOCK => {
                    if let Some(b) = self.block_header(&c, cend) {
                        self.account(&b, cts, None);
                    }
                }
                ID_BLOCK_GROUP => self.block_group(&c, cend, cts),
                _ => {}
            }
            pos = cend;
        }
        if self.stop {
            self.len
        } else {
            end
        }
    }

    fn block_group(&mut self, h: &Header, end: u64, cts: i64) {
        let mut pos = h.data_start;
        let mut pending: Option<BlockInfo> = None;
        let mut dur: Option<u64> = None;
        while pos < end {
            let Some(c) = self.header(pos) else { break };
            if c.size.is_none() {
                break;
            }
            let (cend, _) = bound(&c, end);
            match c.id {
                ID_BLOCK => pending = self.block_header(&c, cend),
                ID_BLOCK_DURATION => dur = self.read_uint(&c, cend),
                _ => {}
            }
            pos = cend;
        }
        if let Some(b) = pending {
            self.account(&b, cts, dur);
        }
    }

    fn block_header(&mut self, h: &Header, end: u64) -> Option<BlockInfo> {
        let size = end.saturating_sub(h.data_start);
        let mut buf = [0u8; 11];
        let n = self
            .rd
            .read_up_to(h.data_start, &mut buf[..size.min(11) as usize]);
        let track = vint(&buf[..n]);
        let Some(track) = track.filter(|t| t.len as u64 + 3 <= size && t.len + 3 <= n) else {
            self.note("Block with an invalid header");
            return None;
        };
        let at = track.len;
        let header_len = (track.len + 3) as u64;
        Some(BlockInfo {
            track: track.value,
            rel: i16::from_be_bytes([buf[at], buf[at + 1]]),
            flags: buf[at + 2],
            body_pos: h.data_start + header_len,
            body_size: size - header_len,
        })
    }

    fn account(&mut self, b: &BlockInfo, cts: i64, dur: Option<u64>) {
        self.blocks_scanned += 1;
        if self.blocks_scanned > MAX_BLOCKS {
            self.stop = true;
            self.capped = true;
            return;
        }
        let ts = cts.saturating_add(i64::from(b.rel));
        let info = self.by_number.get(&b.track).copied();
        let lacing = (b.flags >> 1) & 3;
        let (frames, bytes) = if lacing != 0 && info.is_none_or(|i| i.kind != StreamKind::Subtitle)
        {
            self.lacing(b, lacing)
        } else {
            (1, b.body_size)
        };
        self.acc.entry(b.track).or_default().add(ts, frames, bytes);
        let d = dur.map_or(0, |d| i64::try_from(d).unwrap_or(i64::MAX));
        self.max_end_ticks = self.max_end_ticks.max(ts.saturating_add(d));

        if let Some(sub) = info.and_then(|i| i.sub) {
            let payload = (b.body_size <= MAX_CUE_PAYLOAD)
                .then(|| self.rd.read_vec(b.body_pos, b.body_size))
                .flatten()
                .map(|p| if sub == SubKind::Ass { ass_text(p) } else { p });
            let set = self.cues.entry(b.track).or_default();
            if set.cues.len() >= MAX_CUES {
                set.truncated = true;
            } else {
                set.cues.push(CueRaw {
                    start: ts,
                    dur,
                    payload,
                });
            }
        }
    }

    /// `(frames, payload bytes)` of a laced Block.
    fn lacing(&mut self, b: &BlockInfo, kind: u8) -> (u64, u64) {
        let fallback = (1, b.body_size);
        let end = b.body_pos + b.body_size;
        let mut one = [0u8; 1];
        if b.body_size == 0 || self.rd.read_up_to(b.body_pos, &mut one) == 0 {
            return fallback;
        }
        let n = u64::from(one[0]) + 1;
        let mut p = b.body_pos + 1;
        let limit = p.saturating_add(MAX_LACE_HEADER).min(end);
        match kind {
            // Xiph: each of the first n-1 sizes is a run of 255s plus a byte.
            1 => {
                for _ in 1..n {
                    loop {
                        if p >= limit {
                            return (n, b.body_size);
                        }
                        if self.rd.read_up_to(p, &mut one) == 0 {
                            return (n, b.body_size);
                        }
                        p += 1;
                        if one[0] != 255 {
                            break;
                        }
                    }
                }
            }
            // EBML: first size as a VINT, then n-2 signed-VINT deltas.
            3 => {
                for _ in 1..n {
                    let mut buf = [0u8; 8];
                    let got = self.rd.read_up_to(p, &mut buf);
                    let Some(v) = vint(&buf[..got]) else {
                        return (n, b.body_size);
                    };
                    p += v.len as u64;
                    if p > limit {
                        return (n, b.body_size);
                    }
                }
            }
            // Fixed-size: just the frame count byte.
            _ => {}
        }
        (n, end.saturating_sub(p))
    }

    // ---- result ----------------------------------------------------------

    fn build(&mut self, doctype: Option<String>) -> ProbedContainer {
        let scale = self.scale;
        let mut out = ProbedContainer::new(FORMAT);

        let duration_ms = self
            .duration_raw
            .filter(|d| *d >= 0.0)
            .map(|d| d * scale as f64 / 1_000_000.0)
            .filter(|ms| ms.is_finite() && *ms < MAX_MS as f64)
            .map(|ms| ms.round() as u64)
            .or_else(|| (self.max_end_ticks > 0).then(|| ticks_to_ms(self.max_end_ticks, scale)));
        out.duration_ms = duration_ms;

        let time_base = Rational::new(scale, 1_000_000_000);
        let tracks = std::mem::take(&mut self.tracks);
        let mut gaps: Vec<TimeRange> = Vec::new();
        let mut examined = false;

        for (i, (t, class)) in tracks.iter().enumerate() {
            let acc = self.acc.get(&t.number);
            let mut ps = build_stream(i as u64, t, class, scale, acc);
            ps.stream.time_base = time_base;
            ps.stream.duration = duration_ms.map(DurationSeconds::from_millis);
            if let (Some(acc), Some(ms)) = (acc, duration_ms) {
                if ms > 0
                    && acc.bytes > 0
                    && matches!(ps.stream.kind, StreamKind::Audio | StreamKind::Video)
                {
                    ps.stream.bitrate =
                        u64::try_from(u128::from(acc.bytes) * 8 * 1000 / u128::from(ms)).ok();
                }
            }
            if let Some(set) = self.cues.remove(&t.number) {
                // Duplicate track numbers share cues; the first stream wins.
                ps.cues = set
                    .cues
                    .into_iter()
                    .map(|c| {
                        let start_ms = ticks_to_ms(c.start, scale);
                        let end_ms = c.dur.map_or(start_ms, |d| {
                            ticks_to_ms(
                                c.start.saturating_add(i64::try_from(d).unwrap_or(i64::MAX)),
                                scale,
                            )
                        });
                        RawCue {
                            start_ms,
                            end_ms,
                            payload: c.payload,
                        }
                    })
                    .collect();
                ps.cues_truncated = set.truncated;
            }
            if let Some(acc) = acc {
                if matches!(ps.stream.kind, StreamKind::Audio | StreamKind::Video) {
                    if acc.blocks > 0 {
                        examined = true;
                    }
                    track_gaps(acc, scale, &mut gaps);
                }
            }
            out.streams.push(ps);
        }

        let mut unknown: Vec<u64> = self
            .acc
            .keys()
            .copied()
            .filter(|k| !self.by_number.contains_key(k))
            .collect();
        unknown.sort_unstable();
        for k in unknown.into_iter().take(8) {
            self.note(format!("Block for unknown track {k}"));
        }

        gaps.sort_by_key(|g| (g.start_ms, g.end_ms));
        gaps.dedup();
        gaps.truncate(MAX_GAPS);
        out.timestamp_gaps = gaps;
        out.timestamps_contiguous = examined.then_some(out.timestamp_gaps.is_empty());
        out.timecode_present = Some(self.timecode_tag);
        out.malformed_metadata = std::mem::take(&mut self.notes);
        if let Some(reason) = self.broken.take().or_else(|| self.truncated.take()) {
            out.validity = ContainerValidity::Corrupt(reason);
        }
        out.diagnostics
            .insert("doctype".into(), doctype.unwrap_or_default().into());
        out.diagnostics.insert(
            "blocks_scanned".into(),
            self.blocks_scanned.min(MAX_BLOCKS).into(),
        );
        if self.capped {
            out.diagnostics
                .insert("block_scan_capped".into(), true.into());
        }
        out
    }
}

/// Keep only the text part of an ASS/SSA Matroska block payload (everything
/// after the eighth comma).
fn ass_text(payload: Vec<u8>) -> Vec<u8> {
    let mut commas = 0;
    for (i, &b) in payload.iter().enumerate() {
        if b == b',' {
            commas += 1;
            if commas == 8 {
                return payload[i + 1..].to_vec();
            }
        }
    }
    payload
}

/// Timestamp gaps and backward jumps of one A/V track.
fn track_gaps(acc: &Acc, scale: u64, gaps: &mut Vec<TimeRange>) {
    let m = acc.median().filter(|m| *m > 0);
    let scale = i128::from(scale);
    for &(prev, next) in &acc.candidates {
        if gaps.len() >= MAX_GAPS {
            return;
        }
        let d = i128::from(next) - i128::from(prev);
        if d < 0 {
            let m_ns = m.map_or(0, i128::from) * scale;
            if -d * scale > m_ns {
                gaps.push(TimeRange::new(
                    ticks_to_ms(next, scale as u64),
                    ticks_to_ms(prev, scale as u64),
                ));
            }
        } else if let Some(m) = m {
            if d * scale >= MIN_GAP_NS && d * 2 > i128::from(m) * 3 {
                gaps.push(TimeRange::new(
                    ticks_to_ms(prev.saturating_add(m), scale as u64),
                    ticks_to_ms(next, scale as u64),
                ));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Stream construction
// ---------------------------------------------------------------------------

fn flag(v: Option<bool>, default: bool) -> String {
    if v.unwrap_or(default) { "1" } else { "0" }.to_string()
}

fn pix_fmt(chroma: &str, depth: u32) -> String {
    if depth > 8 {
        format!("{chroma}{depth}le")
    } else {
        chroma.to_string()
    }
}

/// `(profile, pixel_format)` from an `av1C` record.
fn av1_config(private: &[u8]) -> (Option<String>, Option<String>) {
    let (Some(&b0), Some(&b1), Some(&b2)) = (private.first(), private.get(1), private.get(2))
    else {
        return (None, None);
    };
    if b0 & 0x80 == 0 {
        return (None, None);
    }
    let profile = match b1 >> 5 {
        0 => Some("Main"),
        1 => Some("High"),
        2 => Some("Professional"),
        _ => None,
    };
    let depth = if b2 & 0x40 != 0 {
        if b2 & 0x20 != 0 {
            12
        } else {
            10
        }
    } else {
        8
    };
    let mono = b2 & 0x10 != 0;
    let chroma = match (mono, b2 & 0x08 != 0, b2 & 0x04 != 0) {
        (true, _, _) => Some("gray"),
        (false, true, true) => Some("yuv420p"),
        (false, true, false) => Some("yuv422p"),
        (false, false, false) => Some("yuv444p"),
        _ => None,
    };
    (
        profile.map(str::to_string),
        chroma.map(|c| pix_fmt(c, depth)),
    )
}

/// `(profile, pixel_format)` from the optional WebM "VP9 CodecPrivate" list.
fn vp9_config(private: &[u8]) -> (Option<String>, Option<String>) {
    let (mut profile, mut depth, mut chroma) = (None, None, None);
    let mut i = 0usize;
    while let (Some(&id), Some(&len)) = (private.get(i), private.get(i + 1)) {
        let Some(value) = private.get(i + 2..i + 2 + usize::from(len)) else {
            break;
        };
        let v = value.first().copied();
        match id {
            1 => profile = v,
            3 => depth = v,
            4 => chroma = v,
            _ => {}
        }
        i += 2 + usize::from(len);
    }
    let pixel = depth.zip(chroma).and_then(|(d, c)| {
        let c = match c {
            0 | 1 => "yuv420p",
            2 => "yuv422p",
            3 => "yuv444p",
            _ => return None,
        };
        Some(pix_fmt(c, u32::from(d)))
    });
    (profile.map(|p| format!("Profile {p}")), pixel)
}

fn field_order(t: &RawTrack) -> FieldOrder {
    if t.interlaced == 2 {
        return FieldOrder::Progressive;
    }
    match t.field_order {
        Some(1 | 14) => FieldOrder::TopFieldFirst,
        Some(6 | 9) => FieldOrder::BottomFieldFirst,
        _ if t.interlaced == 1 => FieldOrder::Unknown,
        // AV1 and VP9 are progressive unless the container says otherwise.
        _ => FieldOrder::Progressive,
    }
}

fn video_measurements(t: &RawTrack, acc: Option<&Acc>, scale: u64) -> VideoMeasurements {
    let c = &t.colour;
    let cicp = Cicp {
        primaries: c.primaries.and_then(|v| u8::try_from(v).ok()),
        transfer: c.transfer.and_then(|v| u8::try_from(v).ok()),
        matrix: c.matrix.and_then(|v| u8::try_from(v).ok()),
        full_range: match c.range {
            Some(1) => Some(false),
            Some(2) => Some(true),
            _ => None,
        },
    };
    let mut hdr_meta = HdrMetadata::default();
    if c.present {
        cicp.apply(&mut hdr_meta);
        hdr_meta.max_cll = c.max_cll.and_then(|v| u32::try_from(v).ok());
        hdr_meta.max_fall = c.max_fall.and_then(|v| u32::try_from(v).ok());
        hdr_meta.mastering_display = c.mastering.as_ref().and_then(Mastering::display);
    }
    let observed = acc.and_then(|a| {
        let span = i128::from(a.max_ts?) - i128::from(a.min_ts?);
        if a.blocks < 2 || span <= 0 {
            return None;
        }
        ratio(
            u128::from(a.blocks - 1) * 1_000_000_000,
            span as u128 * u128::from(scale),
        )
    });
    VideoMeasurements {
        frame_rate_observed: observed,
        colorspace: cicp.colorspace(),
        hdr: hdr::non_empty(hdr_meta),
        field_order: Some(field_order(t)),
        ..Default::default()
    }
}

fn build_stream(
    index: u64,
    t: &RawTrack,
    class: &Class,
    scale: u64,
    acc: Option<&Acc>,
) -> ProbedStream {
    let (codec, kind, supported) = match class {
        Class::Supported { codec, kind } => ((*codec).to_string(), *kind, true),
        Class::Passive { codec, kind } => (codec.clone(), *kind, false),
        // Refused tracks never get here (the file is refused first).
        Class::Refused(w) => (w.clone(), StreamKind::Unknown, false),
    };
    let codec_id = t.codec_id.as_deref().unwrap_or("");

    let mut metadata = BTreeMap::new();
    if let Some(name) = t.name.as_deref().filter(|n| !n.is_empty()) {
        metadata.insert("title".to_string(), name.to_string());
    }
    metadata.insert("default".into(), flag(t.default, true));
    metadata.insert("forced".into(), flag(t.forced, false));
    metadata.insert("enabled".into(), flag(t.enabled, true));
    let language = t
        .bcp47
        .as_deref()
        .filter(|l| !l.is_empty() && *l != "und")
        .or(t
            .language
            .as_deref()
            .filter(|l| !l.is_empty() && *l != "und"))
        .map(str::to_string);

    let mut stream = Stream {
        index: StreamId::new(index),
        kind,
        codec: Some(codec),
        codec_profile: None,
        width: None,
        height: None,
        pixel_format: None,
        field_order: None,
        frame_rate: None,
        time_base: None,
        bitrate: None,
        duration: None,
        language,
        channel_layout: None,
        channels: None,
        sample_rate: None,
        bit_depth: None,
        metadata,
    };
    let mut video = None;

    if supported && kind == StreamKind::Video {
        stream.width = t.width.filter(|w| *w > 0);
        stream.height = t.height.filter(|h| *h > 0);
        let (profile, pixel) = match (codec_id, t.private.as_deref()) {
            ("V_AV1", Some(p)) => av1_config(p),
            ("V_VP9", Some(p)) => vp9_config(p),
            _ => (None, None),
        };
        stream.codec_profile = profile;
        stream.pixel_format = pixel;
        stream.frame_rate = t
            .default_duration
            .filter(|d| *d > 0)
            .and_then(|d| Rational::new(1_000_000_000, d))
            .or_else(|| {
                let m = acc?.median().filter(|m| *m > 0)?;
                ratio(1_000_000_000, m as u128 * u128::from(scale))
            });
        let m = video_measurements(t, acc, scale);
        stream.field_order = m.field_order;
        video = Some(m);
    } else if supported && kind == StreamKind::Audio {
        let opus = codec_id == "A_OPUS";
        stream.channels = t.channels.filter(|c| *c > 0);
        stream.bit_depth = t.bit_depth.filter(|b| *b > 0);
        stream.sample_rate = if opus {
            Some(48_000)
        } else {
            t.sampling
                .filter(|s| *s >= 1.0 && *s < 1e9)
                .map(|s| s.round() as u64)
        };
        stream.channel_layout = stream.channels.and_then(channel_layout);
        if codec_id.starts_with("A_PCM/") {
            let float = codec_id == "A_PCM/FLOAT/IEEE";
            let big = codec_id == "A_PCM/INT/BIG";
            if let Some(bits) = stream.bit_depth {
                stream.codec =
                    Some(pcm_codec_name(float, u32::try_from(bits).unwrap_or(0), big).to_string());
            }
        }
    }

    ProbedStream {
        stream,
        native_id: t.number,
        video,
        cues: Vec::new(),
        cues_truncated: false,
    }
}

/// Probe a Matroska/WebM file of `len` bytes.
pub fn probe<R: Read + Seek>(source: &mut R, len: u64) -> Result<ProbedContainer> {
    Probe::new(source, len).run()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    // ---- synthetic builders ---------------------------------------------

    fn id_bytes(id: u32) -> Vec<u8> {
        let b = id.to_be_bytes();
        let skip = b.iter().take_while(|x| **x == 0).count();
        b[skip..].to_vec()
    }

    fn size_bytes(n: u64) -> Vec<u8> {
        for len in 1..=8usize {
            if n < (1u64 << (7 * len)) - 1 {
                let v = n | (1u64 << (7 * len));
                return v.to_be_bytes()[8 - len..].to_vec();
            }
        }
        unreachable!()
    }

    fn el(id: u32, body: &[u8]) -> Vec<u8> {
        let mut v = id_bytes(id);
        v.extend(size_bytes(body.len() as u64));
        v.extend(body);
        v
    }

    fn el_unknown(id: u32, body: &[u8]) -> Vec<u8> {
        let mut v = id_bytes(id);
        v.extend([0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
        v.extend(body);
        v
    }

    fn uint_el(id: u32, v: u64) -> Vec<u8> {
        let b = v.to_be_bytes();
        let skip = b.iter().take_while(|x| **x == 0).count().min(7);
        el(id, &b[skip..])
    }

    fn f64_el(id: u32, v: f64) -> Vec<u8> {
        el(id, &v.to_be_bytes())
    }

    fn f32_el(id: u32, v: f32) -> Vec<u8> {
        el(id, &v.to_be_bytes())
    }

    fn str_el(id: u32, s: &str) -> Vec<u8> {
        el(id, s.as_bytes())
    }

    fn cat(parts: &[Vec<u8>]) -> Vec<u8> {
        parts.concat()
    }

    fn track(num: u64, ttype: u64, codec: &str, extra: &[Vec<u8>]) -> Vec<u8> {
        el(
            ID_TRACK_ENTRY,
            &cat(&[
                uint_el(ID_TRACK_NUMBER, num),
                uint_el(ID_TRACK_TYPE, ttype),
                str_el(ID_CODEC_ID, codec),
                cat(extra),
            ]),
        )
    }

    fn video_el(w: u64, h: u64, more: &[Vec<u8>]) -> Vec<u8> {
        el(
            ID_VIDEO,
            &cat(&[
                uint_el(ID_PIXEL_WIDTH, w),
                uint_el(ID_PIXEL_HEIGHT, h),
                cat(more),
            ]),
        )
    }

    fn audio_el(rate: f64, ch: u64, depth: Option<u64>) -> Vec<u8> {
        let mut parts = vec![f64_el(ID_SAMPLING_FREQ, rate), uint_el(ID_CHANNELS, ch)];
        if let Some(d) = depth {
            parts.push(uint_el(ID_BIT_DEPTH, d));
        }
        el(ID_AUDIO, &cat(&parts))
    }

    fn simple_block(track: u8, rel: i16, flags: u8, payload: &[u8]) -> Vec<u8> {
        let mut b = vec![0x80 | track];
        b.extend(rel.to_be_bytes());
        b.push(flags);
        b.extend(payload);
        el(ID_SIMPLE_BLOCK, &b)
    }

    fn block_group(track: u8, rel: i16, payload: &[u8], dur: Option<u64>) -> Vec<u8> {
        let mut b = vec![0x80 | track];
        b.extend(rel.to_be_bytes());
        b.push(0);
        b.extend(payload);
        let mut parts = vec![el(ID_BLOCK, &b)];
        if let Some(d) = dur {
            parts.push(uint_el(ID_BLOCK_DURATION, d));
        }
        el(ID_BLOCK_GROUP, &cat(&parts))
    }

    fn cluster(ts: u64, blocks: &[Vec<u8>]) -> Vec<u8> {
        el(
            ID_CLUSTER,
            &cat(&[uint_el(ID_C_TIMESTAMP, ts), cat(blocks)]),
        )
    }

    fn cluster_unknown(ts: u64, blocks: &[Vec<u8>]) -> Vec<u8> {
        el_unknown(
            ID_CLUSTER,
            &cat(&[uint_el(ID_C_TIMESTAMP, ts), cat(blocks)]),
        )
    }

    fn info(duration_ms: Option<f64>) -> Vec<u8> {
        let mut parts = vec![uint_el(ID_TIMESTAMP_SCALE, 1_000_000)];
        if let Some(d) = duration_ms {
            parts.push(f64_el(ID_DURATION, d));
        }
        el(ID_INFO, &cat(&parts))
    }

    fn file_with(
        doctype: &str,
        info: Vec<u8>,
        tracks: &[Vec<u8>],
        clusters: &[Vec<u8>],
        unknown_segment: bool,
    ) -> Vec<u8> {
        let header = el(ID_EBML, &str_el(ID_DOCTYPE, doctype));
        let body = cat(&[info, el(ID_TRACKS, &cat(tracks)), cat(clusters)]);
        let seg = if unknown_segment {
            el_unknown(ID_SEGMENT, &body)
        } else {
            el(ID_SEGMENT, &body)
        };
        cat(&[header, seg])
    }

    fn file(tracks: &[Vec<u8>], clusters: &[Vec<u8>]) -> Vec<u8> {
        file_with("matroska", info(Some(1000.0)), tracks, clusters, false)
    }

    fn run(bytes: Vec<u8>) -> Result<ProbedContainer> {
        let len = bytes.len() as u64;
        probe(&mut Cursor::new(bytes), len)
    }

    fn ok(bytes: Vec<u8>) -> ProbedContainer {
        run(bytes).unwrap()
    }

    fn video_blocks(n: i16, step: i16) -> Vec<Vec<u8>> {
        (0..n)
            .map(|i| simple_block(1, i * step, 0x80, &[0u8; 10]))
            .collect()
    }

    // ---- tests -----------------------------------------------------------

    #[test]
    fn av1_hdr10_with_av1c() {
        let colour = el(
            ID_COLOUR,
            &cat(&[
                uint_el(ID_MATRIX, 9),
                uint_el(0x55B2, 10),
                uint_el(ID_RANGE, 1),
                uint_el(ID_TRANSFER, 16),
                uint_el(ID_PRIMARIES, 9),
                uint_el(ID_MAX_CLL, 1000),
                uint_el(ID_MAX_FALL, 400),
                el(
                    ID_MASTERING,
                    &cat(&[
                        f64_el(ID_R_X, 0.708),
                        f64_el(ID_R_Y, 0.292),
                        f64_el(ID_G_X, 0.170),
                        f64_el(ID_G_Y, 0.797),
                        f64_el(ID_B_X, 0.131),
                        f64_el(ID_B_Y, 0.046),
                        f32_el(ID_W_X, 0.3127),
                        f32_el(ID_W_Y, 0.3290),
                        f64_el(ID_L_MAX, 1000.0),
                        f64_el(ID_L_MIN, 0.005),
                    ]),
                ),
            ]),
        );
        let t = track(
            1,
            1,
            "V_AV1",
            &[
                video_el(3840, 2160, &[colour]),
                el(ID_CODEC_PRIVATE, &[0x81, 0x08, 0x4C, 0x00]),
                uint_el(ID_DEFAULT_DURATION, 40_000_000),
            ],
        );
        let c = ok(file(&[t], &[cluster(0, &video_blocks(5, 40))]));
        assert_eq!(c.format, "matroska,webm");
        assert_eq!(c.validity, ContainerValidity::Ok);
        assert_eq!(c.diagnostics["doctype"], "matroska");
        let s = &c.streams[0].stream;
        assert_eq!(s.codec.as_deref(), Some("av1"));
        assert_eq!((s.width, s.height), (Some(3840), Some(2160)));
        assert_eq!(s.codec_profile.as_deref(), Some("Main"));
        assert_eq!(s.pixel_format.as_deref(), Some("yuv420p10le"));
        assert_eq!(s.frame_rate, Rational::new(25, 1));
        let v = c.streams[0].video.as_ref().unwrap();
        assert_eq!(v.colorspace.as_deref(), Some("bt2020nc"));
        let h = v.hdr.as_ref().unwrap();
        assert_eq!(h.primaries.as_deref(), Some("bt2020"));
        assert_eq!(h.transfer.as_deref(), Some("smpte2084"));
        assert_eq!(h.matrix.as_deref(), Some("bt2020nc"));
        assert_eq!(h.range.as_deref(), Some("tv"));
        assert_eq!((h.max_cll, h.max_fall), (Some(1000), Some(400)));
        let m = h.mastering_display.unwrap();
        assert_eq!(m.max_luminance_nits, Some(1000.0));
        assert_eq!(m.min_luminance_nits, Some(0.005));
        assert!((m.primaries_xy.unwrap()[0].0 - 0.708).abs() < 1e-9);
        assert!((m.white_point_xy.unwrap().0 - 0.3127).abs() < 1e-4);
        assert_eq!(v.field_order, Some(FieldOrder::Progressive));
    }

    #[test]
    fn vp9_private_data() {
        let t = track(
            1,
            1,
            "V_VP9",
            &[
                video_el(1280, 720, &[]),
                el(ID_CODEC_PRIVATE, &[1, 1, 2, 2, 1, 31, 3, 1, 10, 4, 1, 0]),
            ],
        );
        let c = ok(file_with(
            "webm",
            info(Some(1000.0)),
            &[t],
            &[cluster(0, &video_blocks(3, 33))],
            false,
        ));
        let s = &c.streams[0].stream;
        assert_eq!(s.codec.as_deref(), Some("vp9"));
        assert_eq!(s.codec_profile.as_deref(), Some("Profile 2"));
        assert_eq!(s.pixel_format.as_deref(), Some("yuv420p10le"));
        assert_eq!(c.diagnostics["doctype"], "webm");
        assert!(c.streams[0].video.as_ref().unwrap().hdr.is_none());

        // No CodecPrivate: nothing invented.
        let t = track(1, 1, "V_VP9", &[video_el(64, 64, &[])]);
        let c = ok(file(&[t], &[]));
        assert_eq!(c.streams[0].stream.codec_profile, None);
        assert_eq!(c.streams[0].stream.pixel_format, None);
    }

    #[test]
    fn audio_parameters() {
        let tracks = [
            track(1, 2, "A_OPUS", &[audio_el(48000.0, 2, None)]),
            track(2, 2, "A_VORBIS", &[audio_el(44100.0, 6, None)]),
            track(3, 2, "A_FLAC", &[audio_el(96000.0, 1, Some(24))]),
            track(4, 2, "A_PCM/INT/LIT", &[audio_el(48000.0, 2, Some(16))]),
            track(5, 2, "A_PCM/INT/BIG", &[audio_el(48000.0, 2, Some(24))]),
            track(6, 2, "A_PCM/FLOAT/IEEE", &[audio_el(48000.0, 8, Some(32))]),
        ];
        let c = ok(file(&tracks, &[]));
        let s: Vec<&Stream> = c.streams.iter().map(|p| &p.stream).collect();
        assert_eq!(s[0].codec.as_deref(), Some("opus"));
        assert_eq!(s[0].sample_rate, Some(48_000));
        assert_eq!(s[0].channel_layout.as_deref(), Some("stereo"));
        assert_eq!(s[1].codec.as_deref(), Some("vorbis"));
        assert_eq!(s[1].channel_layout.as_deref(), Some("5.1"));
        assert_eq!(s[2].codec.as_deref(), Some("flac"));
        assert_eq!((s[2].sample_rate, s[2].bit_depth), (Some(96_000), Some(24)));
        assert_eq!(s[3].codec.as_deref(), Some("pcm_s16le"));
        assert_eq!(s[4].codec.as_deref(), Some("pcm_s24be"));
        assert_eq!(s[5].codec.as_deref(), Some("pcm_f32le"));
        assert_eq!(s[5].channel_layout.as_deref(), Some("7.1"));
        assert_eq!(c.streams[2].native_id, 3);
        assert_eq!(c.duration_ms, Some(1000));
        assert_eq!(s[0].metadata["default"], "1");
        assert_eq!(s[0].metadata["forced"], "0");
    }

    #[test]
    fn language_and_title() {
        let t = track(
            1,
            2,
            "A_OPUS",
            &[
                audio_el(48000.0, 2, None),
                str_el(ID_LANGUAGE, "fre"),
                str_el(ID_NAME, "Commentary"),
                uint_el(ID_FLAG_DEFAULT, 0),
            ],
        );
        let u = track(
            2,
            2,
            "A_OPUS",
            &[audio_el(48000.0, 2, None), str_el(ID_LANGUAGE, "und")],
        );
        let c = ok(file(&[t, u], &[]));
        assert_eq!(c.streams[0].stream.language.as_deref(), Some("fre"));
        assert_eq!(c.streams[0].stream.metadata["title"], "Commentary");
        assert_eq!(c.streams[0].stream.metadata["default"], "0");
        assert_eq!(c.streams[1].stream.language, None);
    }

    #[test]
    fn text_subtitle_cues() {
        let tracks = [
            track(1, 17, "S_TEXT/UTF8", &[]),
            track(2, 17, "S_TEXT/ASS", &[]),
            track(3, 17, "S_TEXT/WEBVTT", &[]),
        ];
        let ass = b"0,0,Default,,0,0,0,,Hello, world";
        let cl = cluster(
            1000,
            &[
                block_group(1, 0, b"First line", Some(1500)),
                block_group(1, 2000, b"No duration", None),
                block_group(2, 500, ass, Some(2000)),
                simple_block(3, 100, 0, b"vtt text"),
            ],
        );
        let c = ok(file(&tracks, &[cl]));
        let srt = &c.streams[0];
        assert_eq!(srt.stream.codec.as_deref(), Some("subrip"));
        assert_eq!(srt.stream.kind, StreamKind::Subtitle);
        assert_eq!(srt.cues.len(), 2);
        assert_eq!((srt.cues[0].start_ms, srt.cues[0].end_ms), (1000, 2500));
        assert_eq!(srt.cues[0].payload.as_deref(), Some(&b"First line"[..]));
        assert_eq!((srt.cues[1].start_ms, srt.cues[1].end_ms), (3000, 3000));
        let a = &c.streams[1].cues[0];
        assert_eq!((a.start_ms, a.end_ms), (1500, 3500));
        assert_eq!(a.payload.as_deref(), Some(&b"Hello, world"[..]));
        let w = &c.streams[2].cues[0];
        assert_eq!(w.start_ms, 1100);
        assert_eq!(w.payload.as_deref(), Some(&b"vtt text"[..]));
        assert!(!srt.cues_truncated);
    }

    #[test]
    fn default_duration_beats_measured_rate() {
        let with = track(
            1,
            1,
            "V_VP9",
            &[
                video_el(64, 64, &[]),
                uint_el(ID_DEFAULT_DURATION, 33_366_667),
            ],
        );
        let without = track(2, 1, "V_VP9", &[video_el(64, 64, &[])]);
        let blocks: Vec<Vec<u8>> = (0..11)
            .flat_map(|i| {
                [
                    simple_block(1, i * 40, 0x80, b"x"),
                    simple_block(2, i * 40, 0x80, b"x"),
                ]
            })
            .collect();
        let c = ok(file(&[with, without], &[cluster(0, &blocks)]));
        assert_eq!(
            c.streams[0].stream.frame_rate,
            Rational::new(1_000_000_000, 33_366_667)
        );
        assert_eq!(c.streams[1].stream.frame_rate, Rational::new(25, 1));
        let obs = c.streams[1].video.as_ref().unwrap().frame_rate_observed;
        assert_eq!(obs, Rational::new(25, 1));
        assert_eq!(c.timestamps_contiguous, Some(true));
        assert!(c.timestamp_gaps.is_empty());
    }

    #[test]
    fn gap_and_backward_jump() {
        let t = track(1, 1, "V_VP9", &[video_el(64, 64, &[])]);
        // 25 fps, a gap from 120 to 400, then a jump back to 200.
        let ts = [0i16, 40, 80, 120, 400, 440, 480, 200, 240];
        let blocks: Vec<Vec<u8>> = ts.iter().map(|t| simple_block(1, *t, 0x80, b"x")).collect();
        let c = ok(file(&[t], &[cluster(0, &blocks)]));
        assert_eq!(c.timestamps_contiguous, Some(false));
        assert!(
            c.timestamp_gaps.contains(&TimeRange::new(160, 400)),
            "{:?}",
            c.timestamp_gaps
        );
        assert!(
            c.timestamp_gaps.contains(&TimeRange::new(200, 480)),
            "{:?}",
            c.timestamp_gaps
        );
        assert_eq!(c.timecode_present, Some(false));
    }

    #[test]
    fn unknown_size_segment_and_clusters() {
        let t = track(1, 1, "V_VP9", &[video_el(64, 64, &[])]);
        let clusters = [
            cluster_unknown(0, &video_blocks(3, 40)),
            cluster_unknown(120, &video_blocks(3, 40)),
        ];
        let c = ok(file_with("webm", info(None), &[t], &clusters, true));
        assert_eq!(c.validity, ContainerValidity::Ok);
        // Duration falls back to the largest block timestamp seen: 120 + 80.
        assert_eq!(c.duration_ms, Some(200));
        assert!(c.malformed_metadata.iter().any(|m| m.contains("Duration")));
        let obs = c.streams[0].video.as_ref().unwrap().frame_rate_observed;
        assert_eq!(obs, Rational::new(25, 1));
        assert_eq!(c.timestamps_contiguous, Some(true));
    }

    #[test]
    fn lacing_accounts_for_every_frame_byte() {
        let t = track(1, 2, "A_OPUS", &[audio_el(48000.0, 2, None)]);
        let xiph = {
            // 3 frames: sizes 10, 300 (255+45), rest 20.
            let mut p = vec![2u8, 10, 255, 45];
            p.extend(vec![0u8; 330]);
            simple_block(1, 0, 0x02, &p)
        };
        let ebml_lace = {
            // 3 frames: first size 10 (0x8A), delta +5 (0xBF+... signed 1-byte: 63+5=68 -> 0xC4), rest 20.
            let mut p = vec![2u8, 0x8A, 0xC4];
            p.extend(vec![0u8; 45]);
            simple_block(1, 20, 0x06, &p)
        };
        let fixed = {
            let mut p = vec![2u8];
            p.extend(vec![0u8; 30]);
            simple_block(1, 40, 0x04, &p)
        };
        let c = ok(file(&[t], &[cluster(0, &[xiph, ebml_lace, fixed])]));
        // 330 + 45 + 30 = 405 payload bytes over 1000 ms.
        assert_eq!(c.streams[0].stream.bitrate, Some(405 * 8));
    }

    #[test]
    fn refused_codecs_fail_before_private_data() {
        for (codec, ttype) in [
            ("V_MPEG4/ISO/AVC", 1),
            ("A_AAC", 2),
            ("V_MPEG2", 1),
            ("V_MPEGH/ISO/HEVC", 1),
        ] {
            // Garbage CodecPrivate that must never be parsed.
            let t = track(1, ttype, codec, &[el(ID_CODEC_PRIVATE, &[0xFF; 16])]);
            let ok_track = track(2, 2, "A_OPUS", &[audio_el(48000.0, 2, None)]);
            let e = run(file(&[ok_track, t], &[])).unwrap_err().to_string();
            assert!(e.contains("royalty-free"), "{codec}: {e}");
        }
    }

    #[test]
    fn passive_tracks_are_listed_not_interpreted() {
        let pgs = track(2, 17, "S_HDMV/PGS", &[el(ID_CODEC_PRIVATE, &[1, 2, 3])]);
        let v = track(1, 1, "V_VP9", &[video_el(64, 64, &[])]);
        let c = ok(file(
            &[v, pgs],
            &[cluster(0, &[simple_block(2, 0, 0, b"image data")])],
        ));
        let s = &c.streams[1];
        assert_eq!(s.stream.kind, StreamKind::Subtitle);
        assert_eq!(s.stream.codec.as_deref(), Some("S_HDMV/PGS"));
        assert!(s.cues.is_empty());
        assert!(s.video.is_none());
    }

    #[test]
    fn timecode_tag_is_detected() {
        let tags = el(
            ID_TAGS,
            &el(
                ID_TAG,
                &el(
                    ID_SIMPLE_TAG,
                    &cat(&[
                        str_el(ID_TAG_NAME, "TIMECODE"),
                        str_el(0x4487, "01:00:00:00"),
                    ]),
                ),
            ),
        );
        let header = el(ID_EBML, &str_el(ID_DOCTYPE, "matroska"));
        let body = cat(&[
            info(Some(10.0)),
            el(ID_TRACKS, &track(1, 1, "V_VP9", &[video_el(8, 8, &[])])),
            tags,
        ]);
        let c = ok(cat(&[header, el(ID_SEGMENT, &body)]));
        assert_eq!(c.timecode_present, Some(true));
    }

    #[test]
    fn garbage_and_empty_files_are_corrupt() {
        let c = ok(vec![0x5A; 200]);
        assert!(matches!(c.validity, ContainerValidity::Corrupt(_)));

        let header = el(ID_EBML, &str_el(ID_DOCTYPE, "matroska"));
        let c = ok(cat(&[header.clone(), el(ID_SEGMENT, &[])]));
        assert!(matches!(c.validity, ContainerValidity::Corrupt(_)));

        let c = ok(header);
        assert!(matches!(c.validity, ContainerValidity::Corrupt(_)));
    }

    #[test]
    fn truncated_cluster_keeps_what_parsed() {
        let t = track(1, 1, "V_VP9", &[video_el(64, 64, &[])]);
        let mut bytes = file(&[t], &[cluster(0, &video_blocks(10, 40))]);
        bytes.truncate(bytes.len() - 30);
        let c = ok(bytes);
        assert_eq!(c.streams.len(), 1);
        match &c.validity {
            ContainerValidity::Corrupt(r) => assert!(r.contains("truncated"), "{r}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn oddities_are_reported() {
        let header = el(ID_EBML, &str_el(ID_DOCTYPE, "matroska"));
        let info = el(ID_INFO, &uint_el(ID_TIMESTAMP_SCALE, 0));
        let a = track(1, 1, "V_VP9", &[video_el(8, 8, &[])]);
        let nocodec = el(
            ID_TRACK_ENTRY,
            &cat(&[uint_el(ID_TRACK_NUMBER, 2), uint_el(ID_TRACK_TYPE, 1)]),
        );
        let dup = track(1, 2, "A_OPUS", &[]);
        let body = cat(&[
            info,
            el(ID_TRACKS, &cat(&[a, nocodec, dup])),
            cluster(0, &[simple_block(9, 0, 0, b"x")]),
        ]);
        let c = ok(cat(&[header, el(ID_SEGMENT, &body)]));
        let m = c.malformed_metadata.join("|");
        assert!(m.contains("TimestampScale"), "{m}");
        assert!(m.contains("without CodecID"), "{m}");
        assert!(m.contains("duplicate TrackNumber"), "{m}");
        assert!(m.contains("unknown track 9"), "{m}");
    }

    const FIXTURE: &[u8] = include_bytes!("../../../tests/fixtures/encoded/vp9-clip.webm");

    #[test]
    fn real_vp9_webm_fixture() {
        let c = ok(FIXTURE.to_vec());
        assert_eq!(c.validity, ContainerValidity::Ok, "{:?}", c.validity);
        let s = &c.streams[0].stream;
        assert_eq!(s.codec.as_deref(), Some("vp9"));
        assert!(s.width.unwrap() > 0 && s.height.unwrap() > 0);
        assert!(s.frame_rate.is_some());
        assert!(s.duration.is_some());
        assert!(c.duration_ms.is_some());
    }

    #[test]
    fn corrupted_fixture_never_panics() {
        let n = FIXTURE.len().min(4096);
        for cut in 0..n {
            let _ = run(FIXTURE[..cut].to_vec());
        }
        let mut cut = n;
        while cut < FIXTURE.len() {
            let _ = run(FIXTURE[..cut].to_vec());
            cut += 97;
        }
        for i in 0..FIXTURE.len().min(512) {
            for mask in [0xFFu8, 0x01, 0x80] {
                let mut b = FIXTURE.to_vec();
                b[i] ^= mask;
                let _ = run(b);
            }
        }
    }

    #[test]
    fn hostile_sizes_terminate() {
        // Block claiming a huge lace count and absurd element sizes.
        let t = track(1, 2, "A_OPUS", &[audio_el(48000.0, 2, None)]);
        let mut bytes = file(&[t], &[cluster(0, &[simple_block(1, 0, 0x02, &[255; 64])])]);
        bytes.extend([
            0x1F, 0x43, 0xB6, 0x75, 0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
        ]);
        let _ = run(bytes);
        for fill in [0x00u8, 0x01, 0xFF, 0x80] {
            let mut b = el(ID_EBML, &str_el(ID_DOCTYPE, "webm"));
            b.extend(vec![fill; 300]);
            let _ = run(b);
        }
    }
}
