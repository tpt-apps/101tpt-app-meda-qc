//! ISO base media files: MP4, M4V and QuickTime MOV (brand-agnostic).
//!
//! The reader walks the top-level boxes with seeks (it never reads `mdat`),
//! buffers `moov` (bounded by [`MAX_BUFFERED_BYTES`]) and parses everything
//! else from that buffer. Sample payloads are only touched to extract subtitle
//! cues, with small bounded reads.
//!
//! Policy: every track is classified from its `hdlr` handler type and first
//! `stsd` sample-entry fourcc *before* anything inside a sample entry is
//! interpreted. A refused codec fails the whole file and its codec
//! configuration (`avcC`, `hvcC`, `esds`, ...) is never parsed.
//!
//! The box walk is iterative and follows a fixed path (`moov/trak/mdia/minf/
//! stbl/...`), so recursion depth is bounded by construction.
//!
//! Limitations: fragmented MP4 (`moof`) sample data is not read (only what
//! `moov` carries is reported); edit lists and `ctts` are ignored.

use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::panic::{catch_unwind, AssertUnwindSafe};

use tpt_app_media_qc_core::error::Result;
use tpt_app_media_qc_model::asset::{FieldOrder, Stream, StreamId, StreamKind};
use tpt_app_media_qc_model::finding::TimeRange;
use tpt_app_media_qc_model::inspection::{ContainerValidity, HdrMetadata, VideoMeasurements};
use tpt_app_media_qc_model::time::{DurationSeconds, Rational};

use crate::audio_files::channel_layout;
use crate::codec::{classify_iso, pcm_codec_name, unsupported_error, Class};
use crate::common::{ProbedContainer, ProbedStream, RawCue, MAX_BUFFERED_BYTES, MAX_CUES};
use crate::hdr::{mastering_from_fixed, non_empty, Cicp};

const FORMAT: &str = "mov,mp4";
const MAX_TRACKS: usize = 64;
const MAX_TABLE_ENTRIES: usize = 50_000_000;
const MAX_TOP_LEVEL_BOXES: usize = 100_000;
const MAX_GAPS: usize = 1000;
const MAX_NOTES: usize = 64;
const MAX_SUBTITLE_SAMPLE_BYTES: u64 = 64 * 1024;
const MAX_SUBTITLE_TOTAL_BYTES: u64 = 64 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Byte helpers
// ---------------------------------------------------------------------------

fn u16_at(d: &[u8], o: usize) -> Option<u16> {
    let b = d.get(o..o.checked_add(2)?)?;
    Some(u16::from_be_bytes([b[0], b[1]]))
}

fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    let b = d.get(o..o.checked_add(4)?)?;
    Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

fn u64_at(d: &[u8], o: usize) -> Option<u64> {
    let b = d.get(o..o.checked_add(8)?)?;
    Some(u64::from_be_bytes([
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
    ]))
}

fn fourcc_at(d: &[u8], o: usize) -> Option<[u8; 4]> {
    let b = d.get(o..o.checked_add(4)?)?;
    Some([b[0], b[1], b[2], b[3]])
}

fn fourcc_text(f: [u8; 4]) -> String {
    f.iter()
        .map(|b| {
            if b.is_ascii_graphic() || *b == b' ' {
                *b as char
            } else {
                '?'
            }
        })
        .collect()
}

/// Read up to `n` bytes at `pos`; short at EOF, empty on any I/O error.
fn read_at<R: Read + Seek>(r: &mut R, pos: u64, n: u64) -> Vec<u8> {
    let mut buf = Vec::new();
    if r.seek(SeekFrom::Start(pos)).is_err() {
        return buf;
    }
    if r.by_ref().take(n).read_to_end(&mut buf).is_err() {
        buf.clear();
    }
    buf
}

fn ticks_to_ms(ticks: u64, timescale: u32) -> u64 {
    if timescale == 0 {
        return 0;
    }
    let ms = u128::from(ticks) * 1000 / u128::from(timescale);
    u64::try_from(ms).unwrap_or(u64::MAX)
}

// ---------------------------------------------------------------------------
// In-memory box iteration
// ---------------------------------------------------------------------------

struct Bx<'a> {
    kind: [u8; 4],
    payload: &'a [u8],
}

struct Boxes<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Boxes<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }
}

impl<'a> Iterator for Boxes<'a> {
    type Item = Bx<'a>;

    fn next(&mut self) -> Option<Bx<'a>> {
        let remaining = self.data.len().checked_sub(self.pos)?;
        if remaining < 8 {
            return None;
        }
        let size32 = u64::from(u32_at(self.data, self.pos)?);
        let kind = fourcc_at(self.data, self.pos + 4)?;
        let (mut size, header) = (size32, 8usize);
        let mut header = header;
        if size32 == 1 {
            if remaining < 16 {
                return None;
            }
            size = u64_at(self.data, self.pos + 8)?;
            header = 16;
        } else if size32 == 0 {
            size = remaining as u64;
        }
        if size < header as u64 {
            return None;
        }
        let end = usize::try_from(size).unwrap_or(usize::MAX).min(remaining);
        let payload = self.data.get(self.pos + header..self.pos + end)?;
        self.pos += end;
        Some(Bx { kind, payload })
    }
}

fn find<'a>(data: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    Boxes::new(data)
        .find(|b| &b.kind == kind)
        .map(|b| b.payload)
}

// ---------------------------------------------------------------------------
// Header boxes
// ---------------------------------------------------------------------------

struct Mdhd {
    timescale: u32,
    duration: Option<u64>,
    language: u16,
}

fn parse_mdhd(d: &[u8]) -> Option<Mdhd> {
    match *d.first()? {
        0 => {
            let dur = u32_at(d, 16)?;
            Some(Mdhd {
                timescale: u32_at(d, 12)?,
                duration: (dur != u32::MAX).then_some(u64::from(dur)),
                language: u16_at(d, 20)?,
            })
        }
        1 => {
            let dur = u64_at(d, 24)?;
            Some(Mdhd {
                timescale: u32_at(d, 20)?,
                duration: (dur != u64::MAX).then_some(dur),
                language: u16_at(d, 32)?,
            })
        }
        _ => None,
    }
}

/// `(timescale, duration)` of `mvhd`.
fn parse_mvhd(d: &[u8]) -> Option<(u32, Option<u64>)> {
    match *d.first()? {
        0 => {
            let dur = u32_at(d, 16)?;
            Some((u32_at(d, 12)?, (dur != u32::MAX).then_some(u64::from(dur))))
        }
        1 => {
            let dur = u64_at(d, 24)?;
            Some((u32_at(d, 20)?, (dur != u64::MAX).then_some(dur)))
        }
        _ => None,
    }
}

fn parse_tkhd(d: &[u8]) -> Option<u32> {
    match *d.first()? {
        0 => u32_at(d, 12),
        1 => u32_at(d, 20),
        _ => None,
    }
}

/// Packed ISO-639-2/T language; `None` for `und`, QuickTime Mac codes and junk.
fn language_name(code: u16) -> Option<String> {
    if code < 0x400 {
        return None;
    }
    let mut s = String::with_capacity(3);
    for shift in [10u16, 5, 0] {
        let c = (code >> shift) & 0x1F;
        if !(1..=26).contains(&c) {
            return None;
        }
        s.push(char::from(0x60 + c as u8));
    }
    (s != "und").then_some(s)
}

fn handler_name(hdlr: &[u8]) -> Option<String> {
    let name = hdlr.get(24..)?;
    // QuickTime stores a Pascal string.
    let name = match name.first() {
        Some(n) if usize::from(*n) + 1 == name.len() => &name[1..],
        _ => name,
    };
    let name = String::from_utf8_lossy(name);
    let name = name.trim_matches(|c: char| c == '\0' || c.is_whitespace());
    (!name.is_empty()).then(|| name.to_string())
}

// ---------------------------------------------------------------------------
// Sample tables
// ---------------------------------------------------------------------------

struct Stsz<'a> {
    fixed: u32,
    count: usize,
    table: &'a [u8],
}

impl Stsz<'_> {
    fn get(&self, i: usize) -> Option<u32> {
        if i >= self.count {
            return None;
        }
        if self.fixed != 0 {
            return Some(self.fixed);
        }
        u32_at(self.table, i.checked_mul(4)?)
    }

    fn total_bytes(&self) -> u64 {
        if self.fixed != 0 {
            return u64::from(self.fixed) * self.count as u64;
        }
        (0..self.count)
            .filter_map(|i| u32_at(self.table, i * 4))
            .map(u64::from)
            .sum()
    }
}

fn parse_stsz(d: &[u8]) -> Option<(Stsz<'_>, u32)> {
    let fixed = u32_at(d, 4)?;
    let declared = u32_at(d, 8)?;
    let table = d.get(12..)?;
    let count = if fixed != 0 {
        declared as usize
    } else {
        (declared as usize).min(table.len() / 4)
    }
    .min(MAX_TABLE_ENTRIES);
    Some((
        Stsz {
            fixed,
            count,
            table,
        },
        declared,
    ))
}

fn parse_stts(d: &[u8]) -> Vec<(u32, u32)> {
    let Some(declared) = u32_at(d, 4) else {
        return Vec::new();
    };
    let body = d.get(8..).unwrap_or(&[]);
    let n = (declared as usize)
        .min(body.len() / 8)
        .min(MAX_TABLE_ENTRIES);
    (0..n)
        .filter_map(|i| Some((u32_at(body, i * 8)?, u32_at(body, i * 8 + 4)?)))
        .collect()
}

fn parse_stsc(d: &[u8]) -> Vec<(u32, u32)> {
    let Some(declared) = u32_at(d, 4) else {
        return Vec::new();
    };
    let body = d.get(8..).unwrap_or(&[]);
    let n = (declared as usize)
        .min(body.len() / 12)
        .min(MAX_TABLE_ENTRIES);
    (0..n)
        .filter_map(|i| Some((u32_at(body, i * 12)?, u32_at(body, i * 12 + 4)?)))
        .collect()
}

struct ChunkOffsets<'a> {
    data: &'a [u8],
    wide: bool,
    count: usize,
}

impl ChunkOffsets<'_> {
    fn new(d: &[u8], wide: bool) -> ChunkOffsets<'_> {
        let declared = u32_at(d, 4).unwrap_or(0) as usize;
        let data = d.get(8..).unwrap_or(&[]);
        let width = if wide { 8 } else { 4 };
        ChunkOffsets {
            data,
            wide,
            count: declared.min(data.len() / width).min(MAX_TABLE_ENTRIES),
        }
    }

    fn get(&self, i: usize) -> Option<u64> {
        if i >= self.count {
            return None;
        }
        if self.wide {
            u64_at(self.data, i * 8)
        } else {
            u32_at(self.data, i * 4).map(u64::from)
        }
    }
}

struct Tables<'a> {
    stts: Vec<(u32, u32)>,
    stsz: Option<(Stsz<'a>, u32)>,
    stsc: Vec<(u32, u32)>,
    offsets: Option<ChunkOffsets<'a>>,
}

impl<'a> Tables<'a> {
    fn new(stbl: &'a [u8]) -> Self {
        let offsets = find(stbl, b"stco")
            .map(|d| ChunkOffsets::new(d, false))
            .or_else(|| find(stbl, b"co64").map(|d| ChunkOffsets::new(d, true)));
        Self {
            stts: find(stbl, b"stts").map(parse_stts).unwrap_or_default(),
            stsz: find(stbl, b"stsz").and_then(parse_stsz),
            stsc: find(stbl, b"stsc").map(parse_stsc).unwrap_or_default(),
            offsets,
        }
    }
}

/// Timing facts derived from `stts`.
struct SttsStats {
    samples: u64,
    total_ticks: u64,
    /// Most common sample delta.
    mode_delta: u32,
    /// Gaps as `(start_ticks, end_ticks)`.
    gaps: Vec<(u64, u64)>,
}

fn stts_stats(entries: &[(u32, u32)], timescale: u32) -> Option<SttsStats> {
    let samples: u64 = entries.iter().map(|(c, _)| u64::from(*c)).sum();
    if samples == 0 || timescale == 0 {
        return None;
    }
    let total_ticks = entries.iter().fold(0u64, |a, (c, d)| {
        a.saturating_add(u64::from(*c) * u64::from(*d))
    });

    let mut by_delta: BTreeMap<u32, u64> = BTreeMap::new();
    for (c, d) in entries {
        *by_delta.entry(*d).or_insert(0) += u64::from(*c);
    }
    let mut mode_delta = 0u32;
    let mut mode_count = 0u64;
    for (d, c) in &by_delta {
        if *c > mode_count {
            mode_count = *c;
            mode_delta = *d;
        }
    }
    // Weighted median delta.
    let mut seen = 0u64;
    let mut median = mode_delta;
    for (d, c) in &by_delta {
        seen += c;
        if seen * 2 >= samples {
            median = *d;
            break;
        }
    }

    let mut gaps = Vec::new();
    if median > 0 {
        let min_gap_ticks = (u128::from(timescale) * 20).div_ceil(1000);
        let mut cursor = 0u64;
        let mut index = 0u64;
        'outer: for (count, delta) in entries {
            let big = u64::from(*delta) * 2 > u64::from(median) * 3
                && u128::from(*delta - median.min(*delta)) >= min_gap_ticks;
            if big {
                for k in 0..u64::from(*count) {
                    if index + k + 1 >= samples {
                        break; // the last sample's duration is not a gap
                    }
                    let start = cursor.saturating_add(k.saturating_mul(u64::from(*delta)));
                    gaps.push((
                        start.saturating_add(u64::from(median)),
                        start + u64::from(*delta),
                    ));
                    if gaps.len() >= MAX_GAPS {
                        break 'outer;
                    }
                }
            }
            cursor = cursor.saturating_add(u64::from(*count).saturating_mul(u64::from(*delta)));
            index += u64::from(*count);
        }
    }
    Some(SttsStats {
        samples,
        total_ticks,
        mode_delta,
        gaps,
    })
}

/// `(start_ticks, duration_ticks)` for the first `cap` samples.
fn sample_timing(entries: &[(u32, u32)], cap: usize) -> Vec<(u64, u32)> {
    let mut out = Vec::new();
    let mut cursor = 0u64;
    'outer: for (count, delta) in entries {
        for _ in 0..*count {
            if out.len() >= cap {
                break 'outer;
            }
            out.push((cursor, *delta));
            cursor = cursor.saturating_add(u64::from(*delta));
        }
    }
    out
}

/// `(file_offset, size)` for the first `cap` samples.
fn sample_locations(t: &Tables<'_>, cap: usize) -> Vec<(u64, u32)> {
    let mut out = Vec::new();
    let (Some((stsz, _)), Some(offsets)) = (&t.stsz, &t.offsets) else {
        return out;
    };
    let mut run = 0usize;
    let mut sample = 0usize;
    for chunk in 0..offsets.count {
        let Some(base) = offsets.get(chunk) else {
            break;
        };
        let chunk_no = chunk as u64 + 1;
        while run + 1 < t.stsc.len() && u64::from(t.stsc[run + 1].0) <= chunk_no {
            run += 1;
        }
        let Some((first, per_chunk)) = t.stsc.get(run).copied() else {
            break;
        };
        if u64::from(first) > chunk_no {
            continue;
        }
        let mut offset = base;
        for _ in 0..per_chunk {
            if out.len() >= cap {
                return out;
            }
            let Some(size) = stsz.get(sample) else {
                return out;
            };
            out.push((offset, size));
            offset = offset.saturating_add(u64::from(size));
            sample += 1;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Track context
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Ctx {
    notes: Vec<String>,
    corrupt: Option<String>,
    gaps: Vec<TimeRange>,
    stts_read: bool,
    timecode: bool,
}

impl Ctx {
    fn note(&mut self, s: String) {
        if self.notes.len() < MAX_NOTES {
            self.notes.push(s);
        }
    }

    fn corrupt(&mut self, s: String) {
        if self.corrupt.is_none() {
            self.corrupt = Some(s);
        }
    }
}

#[derive(Default)]
struct TrakParts<'a> {
    tkhd: Option<&'a [u8]>,
    mdhd: Option<&'a [u8]>,
    hdlr: Option<&'a [u8]>,
    stbl: Option<&'a [u8]>,
}

fn split_trak(trak: &[u8]) -> TrakParts<'_> {
    let mdia = find(trak, b"mdia");
    let minf = mdia.and_then(|m| find(m, b"minf"));
    TrakParts {
        tkhd: find(trak, b"tkhd"),
        mdhd: mdia.and_then(|m| find(m, b"mdhd")),
        hdlr: mdia.and_then(|m| find(m, b"hdlr")),
        stbl: minf.and_then(|m| find(m, b"stbl")),
    }
}

/// First `stsd` entry: `(fourcc, payload after the 8-byte entry header)`.
fn first_sample_entry(stbl: Option<&[u8]>) -> Option<([u8; 4], &[u8])> {
    let stsd = find(stbl?, b"stsd")?;
    let body = stsd.get(8..)?;
    Boxes::new(body).next().map(|b| (b.kind, b.payload))
}

// ---------------------------------------------------------------------------
// Video
// ---------------------------------------------------------------------------

#[derive(Default)]
struct VideoInfo {
    width: u64,
    height: u64,
    profile: Option<String>,
    pixel_format: Option<String>,
    cicp: Cicp,
    mastering: Option<tpt_app_media_qc_model::inspection::MasteringDisplay>,
    max_cll: Option<u32>,
    max_fall: Option<u32>,
    field_order: Option<FieldOrder>,
}

fn pixel_format(bit_depth: u8, mono: bool, sub_x: bool, sub_y: bool) -> Option<String> {
    let suffix = match bit_depth {
        8 => "",
        10 => "10le",
        12 => "12le",
        _ => return None,
    };
    let base = if mono {
        "gray"
    } else {
        match (sub_x, sub_y) {
            (true, true) => "yuv420p",
            (true, false) => "yuv422p",
            (false, false) => "yuv444p",
            (false, true) => "yuv440p",
        }
    };
    if mono && !suffix.is_empty() {
        Some(format!("gray{suffix}"))
    } else {
        Some(format!("{base}{suffix}"))
    }
}

fn u8_code(v: u16) -> Option<u8> {
    u8::try_from(v).ok()
}

fn parse_video(codec: &str, entry: &[u8]) -> VideoInfo {
    let mut info = VideoInfo {
        width: u64::from(u16_at(entry, 24).unwrap_or(0)),
        height: u64::from(u16_at(entry, 26).unwrap_or(0)),
        field_order: Some(FieldOrder::Progressive),
        ..Default::default()
    };
    let children = entry.get(78..).unwrap_or(&[]);
    let mut nclx = Cicp::default();
    let mut codec_cicp = Cicp::default();

    for b in Boxes::new(children) {
        match &b.kind {
            b"colr" if b.payload.get(..4) == Some(b"nclx") => {
                let unspecified = |v: u16| u8_code(v).filter(|c| *c != 2);
                nclx = Cicp {
                    primaries: u16_at(b.payload, 4).and_then(unspecified),
                    transfer: u16_at(b.payload, 6).and_then(unspecified),
                    matrix: u16_at(b.payload, 8).and_then(unspecified),
                    full_range: b.payload.get(10).map(|f| f & 0x80 != 0),
                };
            }
            b"fiel" => {
                if let (Some(count), Some(detail)) = (b.payload.first(), b.payload.get(1)) {
                    info.field_order = Some(match (count, detail) {
                        (1, _) => FieldOrder::Progressive,
                        (2, 1) => FieldOrder::TopFieldFirst,
                        (2, 6) => FieldOrder::BottomFieldFirst,
                        _ => FieldOrder::Unknown,
                    });
                }
            }
            b"mdcv" if b.payload.len() >= 24 => {
                let p = b.payload;
                let pair = |o: usize| (u16_at(p, o).unwrap_or(0), u16_at(p, o + 2).unwrap_or(0));
                // Stored green, blue, red; the helper wants red, green, blue.
                info.mastering = Some(mastering_from_fixed(
                    [pair(8), pair(0), pair(4)],
                    pair(12),
                    u32_at(p, 16).unwrap_or(0),
                    u32_at(p, 20).unwrap_or(0),
                ));
            }
            b"clli" if b.payload.len() >= 4 => {
                info.max_cll = u16_at(b.payload, 0).map(u32::from);
                info.max_fall = u16_at(b.payload, 2).map(u32::from);
            }
            b"av1C" if codec == "av1" => parse_av1c(b.payload, &mut info, &mut codec_cicp),
            b"vpcC" if codec == "vp9" => parse_vpcc(b.payload, &mut info, &mut codec_cicp),
            _ => {}
        }
    }
    info.cicp = Cicp {
        primaries: nclx.primaries.or(codec_cicp.primaries),
        transfer: nclx.transfer.or(codec_cicp.transfer),
        matrix: nclx.matrix.or(codec_cicp.matrix),
        full_range: nclx.full_range.or(codec_cicp.full_range),
    };
    info
}

fn parse_av1c(d: &[u8], info: &mut VideoInfo, cicp: &mut Cicp) {
    let (Some(b1), Some(b2)) = (d.get(1), d.get(2)) else {
        return;
    };
    let profile = b1 >> 5;
    info.profile = Some(
        match profile {
            0 => "Main",
            1 => "High",
            2 => "Professional",
            _ => "Unknown",
        }
        .to_string(),
    );
    let high = b2 & 0x80 != 0;
    let twelve = b2 & 0x40 != 0;
    let mono = b2 & 0x20 != 0;
    let (mut sub_x, mut sub_y) = (b2 & 0x10 != 0, b2 & 0x08 != 0);
    let mut depth = match (high, twelve) {
        (true, true) => 12,
        (true, false) => 10,
        _ => 8,
    };
    let mut mono = mono;

    // The sequence header OBU (in configOBUs) carries the colour description.
    if let Some(obus) = d.get(4..) {
        let parsed = catch_unwind(AssertUnwindSafe(|| {
            let mut rest = obus;
            for _ in 0..8 {
                let (obu, used) = tpt_kinetix_av1::obu::Obu::parse(rest)?;
                if obu.obu_type == tpt_kinetix_av1::obu::ObuType::SequenceHeader {
                    return tpt_kinetix_av1::obu::SequenceHeaderObu::parse(&obu.payload).ok();
                }
                rest = rest.get(used..)?;
            }
            None
        }));
        if let Ok(Some(seq)) = parsed {
            let cc = seq.color_config;
            cicp.primaries = Some(cc.color_primaries).filter(|c| *c != 2);
            cicp.transfer = Some(cc.transfer_characteristics).filter(|c| *c != 2);
            cicp.matrix = Some(cc.matrix_coefficients).filter(|c| *c != 2);
            cicp.full_range = Some(cc.color_range);
            depth = cc.bit_depth;
            mono = cc.mono_chrome;
            sub_x = cc.subsampling_x;
            sub_y = cc.subsampling_y;
        }
    }
    info.pixel_format = pixel_format(depth, mono, sub_x, sub_y);
}

fn parse_vpcc(d: &[u8], info: &mut VideoInfo, cicp: &mut Cicp) {
    if d.len() < 12 || d[0] != 1 {
        return;
    }
    let profile = d[4];
    info.profile = Some(format!("Profile {profile}"));
    let packed = d[6];
    let depth = packed >> 4;
    let chroma = (packed >> 1) & 7;
    let full = packed & 1 != 0;
    let (mono, sub_x, sub_y) = match chroma {
        0 | 1 => (false, true, true),
        2 => (false, true, false),
        3 => (false, false, false),
        _ => (false, true, true),
    };
    info.pixel_format = pixel_format(depth, mono, sub_x, sub_y);
    cicp.primaries = Some(d[7]).filter(|c| *c != 2);
    cicp.transfer = Some(d[8]).filter(|c| *c != 2);
    cicp.matrix = Some(d[9]).filter(|c| *c != 2);
    cicp.full_range = Some(full);
}

// ---------------------------------------------------------------------------
// Audio
// ---------------------------------------------------------------------------

struct AudioEntry<'a> {
    channels: u64,
    bits: u64,
    rate: u64,
    flags: Option<u32>,
    children: &'a [u8],
}

fn parse_audio_entry(fourcc: [u8; 4], e: &[u8]) -> Option<AudioEntry<'_>> {
    let version = u16_at(e, 8)?;
    let qt_style = matches!(
        &fourcc,
        b"sowt" | b"twos" | b"in24" | b"in32" | b"fl32" | b"fl64" | b"raw " | b"lpcm"
    );
    if version == 2 {
        let rate = e.get(32..40).map_or(0.0, |b| {
            f64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
        });
        return Some(AudioEntry {
            channels: u64::from(u32_at(e, 40)?),
            bits: u64::from(u32_at(e, 48)?),
            rate: if rate.is_finite() && rate > 0.0 {
                rate as u64
            } else {
                0
            },
            flags: u32_at(e, 52),
            children: e.get(64..).unwrap_or(&[]),
        });
    }
    let offset = if version == 1 && qt_style { 44 } else { 28 };
    Some(AudioEntry {
        channels: u64::from(u16_at(e, 16)?),
        bits: u64::from(u16_at(e, 18)?),
        rate: u64::from(u32_at(e, 24)? >> 16),
        flags: None,
        children: e.get(offset..).unwrap_or(&[]),
    })
}

fn pcm_name(fourcc: [u8; 4], a: &AudioEntry<'_>) -> (&'static str, u64) {
    let raw_bits = u32::try_from(a.bits).unwrap_or(0);
    let (float, mut bits, mut big) = match &fourcc {
        b"sowt" => (false, raw_bits, false),
        b"twos" => (false, raw_bits, true),
        b"in24" => (false, 24, true),
        b"in32" => (false, 32, true),
        b"fl32" => (true, 32, true),
        b"fl64" => (true, 64, true),
        b"raw " => (false, 8, false),
        b"lpcm" => match a.flags {
            Some(f) => (f & 1 != 0, raw_bits, f & 2 != 0),
            None => (false, raw_bits, false),
        },
        _ => {
            let pcmc = find(a.children, b"pcmC");
            let little = pcmc.and_then(|p| p.get(4)).is_some_and(|f| f & 1 != 0);
            let size = pcmc
                .and_then(|p| p.get(5))
                .map_or(raw_bits, |s| u32::from(*s));
            (&fourcc == b"fpcm", size, !little)
        }
    };
    if bits == 0 && matches!(&fourcc, b"sowt" | b"twos") {
        bits = 16;
    }
    if !matches!(&fourcc, b"ipcm" | b"fpcm") {
        if let Some(enda) = find(a.children, b"enda").and_then(|e| u16_at(e, 0)) {
            big = enda == 0;
        }
    }
    (pcm_codec_name(float, bits, big), u64::from(bits))
}

/// `(sample_rate, channels, bits)` from a FLAC STREAMINFO block inside `dfLa`.
fn parse_dfla(d: &[u8]) -> Option<(u64, u64, u64)> {
    let mut pos = 4usize;
    for _ in 0..8 {
        let header = *d.get(pos)?;
        let block_len = (u32_at(d, pos)? & 0x00FF_FFFF) as usize;
        if header & 0x7F == 0 && block_len >= 18 {
            let s = d.get(pos + 4..pos + 4 + block_len)?;
            let rate = (u64::from(s[10]) << 12) | (u64::from(s[11]) << 4) | u64::from(s[12] >> 4);
            let channels = u64::from((s[12] >> 1) & 7) + 1;
            let bits = (u64::from(s[12] & 1) << 4 | u64::from(s[13] >> 4)) + 1;
            return Some((rate, channels, bits));
        }
        if header & 0x80 != 0 {
            return None;
        }
        pos = pos.checked_add(4)?.checked_add(block_len)?;
    }
    None
}

// ---------------------------------------------------------------------------
// Subtitles
// ---------------------------------------------------------------------------

fn read_cues<R: Read + Seek>(
    r: &mut R,
    len: u64,
    fourcc: [u8; 4],
    tables: &Tables<'_>,
    timescale: u32,
    ctx: &mut Ctx,
    track: usize,
) -> (Vec<RawCue>, bool) {
    let mut cues = Vec::new();
    let locations = sample_locations(tables, MAX_CUES + 1);
    let timing = sample_timing(&tables.stts, MAX_CUES + 1);
    let mut truncated = false;
    let mut n = locations.len().min(timing.len());
    if n > MAX_CUES {
        n = MAX_CUES;
        truncated = true;
    }
    let mut budget = MAX_SUBTITLE_TOTAL_BYTES;
    for i in 0..n {
        let (offset, size) = locations[i];
        let (start, dur) = timing[i];
        let size = u64::from(size);
        if size == 0 || size > MAX_SUBTITLE_SAMPLE_BYTES {
            continue;
        }
        if offset.checked_add(size).is_none_or(|end| end > len) {
            ctx.corrupt(format!(
                "track {track}: subtitle sample at offset {offset} extends beyond end of file"
            ));
            break;
        }
        if size > budget {
            truncated = true;
            break;
        }
        budget -= size;
        let data = read_at(r, offset, size);
        if (data.len() as u64) < size {
            ctx.corrupt(format!("track {track}: short read of a subtitle sample"));
            break;
        }
        let start_ms = ticks_to_ms(start, timescale);
        let end_ms = ticks_to_ms(start.saturating_add(u64::from(dur)), timescale);
        let push = |payload: Vec<u8>, cues: &mut Vec<RawCue>| {
            if cues.len() >= MAX_CUES {
                return false;
            }
            cues.push(RawCue {
                start_ms,
                end_ms,
                payload: Some(payload),
            });
            true
        };
        let ok = match &fourcc {
            b"tx3g" => {
                let text_len = usize::from(u16_at(&data, 0).unwrap_or(0));
                let end = (2 + text_len).min(data.len());
                push(data.get(2..end).unwrap_or(&[]).to_vec(), &mut cues)
            }
            _ => {
                let mut ok = true;
                for b in Boxes::new(&data) {
                    if &b.kind == b"vttc" {
                        if let Some(payl) = find(b.payload, b"payl") {
                            ok &= push(payl.to_vec(), &mut cues);
                        }
                    }
                }
                ok
            }
        };
        if !ok {
            truncated = true;
            break;
        }
    }
    (cues, truncated)
}

// ---------------------------------------------------------------------------
// Track assembly
// ---------------------------------------------------------------------------

fn blank_stream(kind: StreamKind, codec: String) -> Stream {
    let mut s = Stream::primary_video(0);
    s.index = StreamId::new(0);
    s.kind = kind;
    s.codec = Some(codec);
    s.codec_profile = None;
    s.width = None;
    s.height = None;
    s.pixel_format = None;
    s.field_order = None;
    s.frame_rate = None;
    s.time_base = None;
    s.bitrate = None;
    s.duration = None;
    s.language = None;
    s.channel_layout = None;
    s.channels = None;
    s.sample_rate = None;
    s.bit_depth = None;
    s.metadata.clear();
    s
}

fn bitrate(total_bytes: u64, ticks: Option<u64>, timescale: Option<u32>) -> Option<u64> {
    let (ticks, ts) = (ticks?, timescale?);
    if ticks == 0 || ts == 0 {
        return None;
    }
    let bps = u128::from(total_bytes) * 8 * u128::from(ts) / u128::from(ticks);
    u64::try_from(bps).ok()
}

struct BuiltTrack {
    probed: ProbedStream,
    duration_ms: Option<u64>,
}

#[allow(clippy::too_many_arguments)]
fn build_track<R: Read + Seek>(
    r: &mut R,
    len: u64,
    index: usize,
    parts: &TrakParts<'_>,
    class: &Class,
    fourcc: [u8; 4],
    entry: &[u8],
    ctx: &mut Ctx,
) -> BuiltTrack {
    let (codec, kind, supported) = match class {
        Class::Supported { codec, kind } => ((*codec).to_string(), *kind, true),
        Class::Passive { codec, kind } => (codec.clone(), *kind, false),
        Class::Refused(_) => ("unknown".to_string(), StreamKind::Unknown, false),
    };

    let native_id = parts.tkhd.and_then(parse_tkhd);
    if native_id.is_none() {
        ctx.note(format!("track {index}: missing or invalid tkhd box"));
    }
    let mdhd = parts.mdhd.and_then(parse_mdhd);
    match &mdhd {
        None => ctx.note(format!("track {index}: missing or invalid mdhd box")),
        Some(m) if m.timescale == 0 => ctx.note(format!("track {index}: mdhd timescale is 0")),
        _ => {}
    }
    let timescale = mdhd.as_ref().map(|m| m.timescale).filter(|t| *t > 0);
    let duration_ticks = mdhd.as_ref().and_then(|m| m.duration);
    let duration_ms = timescale
        .zip(duration_ticks)
        .map(|(ts, d)| ticks_to_ms(d, ts));

    let mut stream = blank_stream(kind, codec.clone());
    stream.time_base = timescale.and_then(|ts| Rational::new(1, u64::from(ts)));
    stream.duration = duration_ms.map(DurationSeconds::from_millis);
    stream.language = mdhd.as_ref().and_then(|m| language_name(m.language));
    if let Some(name) = parts.hdlr.and_then(handler_name) {
        stream.metadata.insert("handler_name".into(), name);
    }

    let mut video = None;
    let mut cues = Vec::new();
    let mut cues_truncated = false;

    if supported {
        let tables = Tables::new(parts.stbl.unwrap_or(&[]));

        // Sample-table consistency and EOF checks.
        if let Some(offsets) = &tables.offsets {
            if (0..offsets.count)
                .filter_map(|i| offsets.get(i))
                .any(|o| o >= len)
            {
                ctx.corrupt(format!(
                    "track {index}: chunk offset points beyond end of file"
                ));
            }
        }
        let stats = timescale.and_then(|ts| stts_stats(&tables.stts, ts));
        if let (Some(stats), Some((_, declared))) = (&stats, &tables.stsz) {
            if u64::from(*declared) != stats.samples {
                ctx.note(format!(
                    "track {index}: stsz sample count {declared} differs from stts sample count {}",
                    stats.samples
                ));
            }
        }
        let total_bytes = tables.stsz.as_ref().map_or(0, |(s, _)| s.total_bytes());
        let ticks_for_rate = duration_ticks
            .filter(|d| *d > 0)
            .or_else(|| stats.as_ref().map(|s| s.total_ticks));

        match kind {
            StreamKind::Video | StreamKind::Audio => {
                if let (Some(stats), Some(ts)) = (&stats, timescale) {
                    ctx.stts_read = true;
                    for (s, e) in &stats.gaps {
                        if ctx.gaps.len() < MAX_GAPS {
                            ctx.gaps
                                .push(TimeRange::new(ticks_to_ms(*s, ts), ticks_to_ms(*e, ts)));
                        }
                    }
                }
                stream.bitrate = bitrate(total_bytes, ticks_for_rate, timescale);
            }
            _ => {}
        }

        match kind {
            StreamKind::Video => {
                let info = parse_video(&codec, entry);
                stream.width = Some(info.width).filter(|w| *w > 0);
                stream.height = Some(info.height).filter(|h| *h > 0);
                stream.codec_profile = info.profile.clone();
                stream.pixel_format = info.pixel_format.clone();
                stream.field_order = info.field_order;
                let mut observed = None;
                if let (Some(stats), Some(ts)) = (&stats, timescale) {
                    if stats.mode_delta > 0 {
                        stream.frame_rate =
                            Rational::new(u64::from(ts), u64::from(stats.mode_delta));
                    }
                    if stats.total_ticks > 0 {
                        observed = stats
                            .samples
                            .checked_mul(u64::from(ts))
                            .and_then(|n| Rational::new(n, stats.total_ticks));
                    }
                }
                let mut hdr = HdrMetadata::default();
                info.cicp.apply(&mut hdr);
                hdr.mastering_display = info.mastering;
                hdr.max_cll = info.max_cll;
                hdr.max_fall = info.max_fall;
                video = Some(VideoMeasurements {
                    frame_rate_observed: observed,
                    colorspace: info.cicp.colorspace(),
                    hdr: non_empty(hdr),
                    field_order: info.field_order,
                    ..Default::default()
                });
            }
            StreamKind::Audio => {
                if let Some(a) = parse_audio_entry(fourcc, entry) {
                    let mut channels = a.channels;
                    let mut rate = a.rate;
                    let mut bits = Some(a.bits).filter(|b| *b > 0);
                    match &fourcc {
                        b"Opus" => {
                            if let Some(c) = find(a.children, b"dOps").and_then(|d| d.get(1)) {
                                channels = u64::from(*c);
                            }
                            rate = 48_000;
                            bits = None;
                        }
                        b"fLaC" => {
                            if let Some((r2, c2, b2)) =
                                find(a.children, b"dfLa").and_then(parse_dfla)
                            {
                                rate = r2;
                                channels = c2;
                                bits = Some(b2);
                            }
                        }
                        _ => {
                            let (name, b) = pcm_name(fourcc, &a);
                            stream.codec = Some(name.to_string());
                            bits = Some(b).filter(|b| *b > 0);
                        }
                    }
                    if rate == 0 {
                        rate = timescale.map_or(0, u64::from);
                    }
                    stream.channels = Some(channels).filter(|c| *c > 0);
                    stream.channel_layout = channel_layout(channels);
                    stream.sample_rate = Some(rate).filter(|r| *r > 0);
                    stream.bit_depth = bits;
                }
            }
            StreamKind::Subtitle => {
                if let Some(ts) = timescale {
                    let (c, t) = read_cues(r, len, fourcc, &tables, ts, ctx, index);
                    cues = c;
                    cues_truncated = t;
                }
            }
            _ => {}
        }
    }

    BuiltTrack {
        probed: ProbedStream {
            stream,
            native_id: u64::from(native_id.unwrap_or(0)),
            video,
            cues,
            cues_truncated,
        },
        duration_ms,
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

struct TopLevel {
    moov: Option<Vec<u8>>,
    has_moof: bool,
    corrupt: Option<String>,
    brands: Option<serde_json::Value>,
}

fn walk_top_level<R: Read + Seek>(r: &mut R, len: u64) -> std::result::Result<TopLevel, String> {
    let mut top = TopLevel {
        moov: None,
        has_moof: false,
        corrupt: None,
        brands: None,
    };
    let mut pos = 0u64;
    let mut count = 0usize;
    while pos < len && count < MAX_TOP_LEVEL_BOXES {
        count += 1;
        let head = read_at(r, pos, 16);
        if head.len() < 8 {
            break;
        }
        let (Some(size32), Some(kind)) = (u32_at(&head, 0), fourcc_at(&head, 4)) else {
            break;
        };
        let (mut size, mut header) = (u64::from(size32), 8u64);
        if size32 == 1 {
            let Some(large) = u64_at(&head, 8) else {
                break;
            };
            size = large;
            header = 16;
        } else if size32 == 0 {
            size = len - pos;
        }
        if size < header {
            top.corrupt.get_or_insert(format!(
                "invalid size {size} for '{}' box at offset {pos}",
                fourcc_text(kind)
            ));
            break;
        }
        let Some(declared_end) = pos.checked_add(size) else {
            top.corrupt.get_or_insert(format!(
                "box '{}' at offset {pos} has an impossible size",
                fourcc_text(kind)
            ));
            break;
        };
        let end = declared_end.min(len);
        if declared_end > len {
            top.corrupt.get_or_insert(format!(
                "box '{}' at offset {pos} declares {size} bytes but only {} remain in the file",
                fourcc_text(kind),
                len - pos
            ));
        }
        let payload_start = pos + header;
        let payload_len = end.saturating_sub(payload_start);
        match &kind {
            b"ftyp" => {
                let d = read_at(r, payload_start, payload_len.min(512));
                if let Some(major) = fourcc_at(&d, 0) {
                    let compat: Vec<String> = d
                        .get(8..)
                        .unwrap_or(&[])
                        .chunks_exact(4)
                        .map(|c| fourcc_text([c[0], c[1], c[2], c[3]]))
                        .collect();
                    top.brands = Some(serde_json::json!({
                        "major": fourcc_text(major),
                        "compatible": compat,
                    }));
                }
            }
            b"moov" if top.moov.is_none() => {
                if payload_len > MAX_BUFFERED_BYTES {
                    return Err(format!(
                        "moov box is {payload_len} bytes, larger than the {MAX_BUFFERED_BYTES} byte limit"
                    ));
                }
                top.moov = Some(read_at(r, payload_start, payload_len));
            }
            b"moof" => top.has_moof = true,
            _ => {}
        }
        pos = end;
    }
    Ok(top)
}

type TrakEntry<'a> = (TrakParts<'a>, [u8; 4], Option<([u8; 4], &'a [u8])>);

/// Probe an ISO base media file of `len` bytes.
pub fn probe<R: Read + Seek>(source: &mut R, len: u64) -> Result<ProbedContainer> {
    let top = match walk_top_level(source, len) {
        Ok(t) => t,
        Err(reason) => return Ok(ProbedContainer::corrupt(FORMAT, reason)),
    };
    let Some(moov) = top.moov.as_deref() else {
        let mut c =
            ProbedContainer::corrupt(FORMAT, "no moov box (truncated file or fragmented stream)");
        if top.has_moof {
            c.diagnostics.insert("fragmented".into(), true.into());
        }
        if let Some(b) = top.brands {
            c.diagnostics.insert("brands".into(), b);
        }
        return Ok(c);
    };

    // Pass 1: classify every track; refuse before parsing any sample entry.
    let mut traks: Vec<TrakEntry<'_>> = Vec::new();
    let mut ctx = Ctx::default();
    let mut mvhd = None;
    let mut has_mvex = false;
    let mut track_count = 0usize;
    for b in Boxes::new(moov) {
        match &b.kind {
            b"mvhd" => mvhd = parse_mvhd(b.payload),
            b"mvex" => has_mvex = true,
            b"trak" => {
                track_count += 1;
                if traks.len() >= MAX_TRACKS {
                    continue;
                }
                let parts = split_trak(b.payload);
                let handler = parts.hdlr.and_then(|h| fourcc_at(h, 8)).unwrap_or([0; 4]);
                let entry = first_sample_entry(parts.stbl);
                let fourcc = entry.map_or([0; 4], |(f, _)| f);
                if let Class::Refused(what) = classify_iso(handler, fourcc) {
                    return Err(unsupported_error(&what));
                }
                traks.push((parts, handler, entry));
            }
            _ => {}
        }
    }
    if track_count > MAX_TRACKS {
        ctx.note(format!(
            "{track_count} tracks present; only the first {MAX_TRACKS} were inspected"
        ));
    }

    // Pass 2: build streams.
    let mut out = ProbedContainer::new(FORMAT);
    let mut longest_ms: Option<u64> = None;
    let mut timecode = false;
    for (i, (parts, handler, entry)) in traks.iter().enumerate() {
        let fourcc = entry.map_or([0; 4], |(f, _)| f);
        if entry.is_none() {
            ctx.note(format!("track {i}: missing or empty stsd box"));
        }
        if handler == b"tmcd" || &fourcc == b"tmcd" {
            timecode = true;
        }
        let class = classify_iso(*handler, fourcc);
        let built = build_track(
            source,
            len,
            i,
            parts,
            &class,
            fourcc,
            entry.map_or(&[], |(_, p)| p),
            &mut ctx,
        );
        longest_ms = longest_ms.max(built.duration_ms);
        out.streams.push(built.probed);
    }
    ctx.timecode = timecode;

    let fragmented = has_mvex || top.has_moof;
    let movie_ms = mvhd
        .and_then(|(ts, d)| d.map(|d| ticks_to_ms(d, ts)))
        .filter(|ms| *ms > 0);
    out.duration_ms = movie_ms.or(longest_ms.filter(|ms| *ms > 0));
    out.timecode_present = Some(ctx.timecode);
    if ctx.stts_read {
        ctx.gaps.sort_by_key(|g| (g.start_ms, g.end_ms));
        out.timestamps_contiguous = Some(ctx.gaps.is_empty());
        out.timestamp_gaps = std::mem::take(&mut ctx.gaps);
    }
    if mvhd.is_none() {
        ctx.note("missing or invalid mvhd box".into());
    }
    out.malformed_metadata = std::mem::take(&mut ctx.notes);
    if fragmented {
        out.diagnostics.insert("fragmented".into(), true.into());
    }
    if let Some(b) = top.brands {
        out.diagnostics.insert("brands".into(), b);
    }
    if let Some(reason) = top.corrupt.or(ctx.corrupt) {
        out.validity = ContainerValidity::Corrupt(reason);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use tpt_app_media_qc_model::inspection::DynamicRange;

    const FIXTURE: &[u8] = include_bytes!("../../../tests/fixtures/encoded/vp9-clip.mp4");

    fn bx(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut v = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend(kind);
        v.extend(payload);
        v
    }

    fn full(kind: &[u8; 4], version: u8, payload: &[u8]) -> Vec<u8> {
        let mut p = vec![version, 0, 0, 0];
        p.extend(payload);
        bx(kind, &p)
    }

    fn cat(parts: &[Vec<u8>]) -> Vec<u8> {
        parts.concat()
    }

    fn visual(fourcc: &[u8; 4], w: u16, h: u16, children: &[Vec<u8>]) -> Vec<u8> {
        let mut p = vec![0u8; 6];
        p.extend([0, 1]);
        p.extend([0u8; 16]);
        p.extend(w.to_be_bytes());
        p.extend(h.to_be_bytes());
        p.extend([0, 0x48, 0, 0, 0, 0x48, 0, 0]);
        p.extend([0u8; 4]);
        p.extend([0, 1]);
        p.extend([0u8; 32]);
        p.extend([0, 0x18, 0xFF, 0xFF]);
        p.extend(cat(children));
        bx(fourcc, &p)
    }

    fn audio(
        fourcc: &[u8; 4],
        channels: u16,
        bits: u16,
        rate: u32,
        children: &[Vec<u8>],
    ) -> Vec<u8> {
        let mut p = vec![0u8; 6];
        p.extend([0, 1]);
        p.extend([0u8; 8]);
        p.extend(channels.to_be_bytes());
        p.extend(bits.to_be_bytes());
        p.extend([0u8; 4]);
        p.extend((rate << 16).to_be_bytes());
        p.extend(cat(children));
        bx(fourcc, &p)
    }

    fn text_entry(fourcc: &[u8; 4]) -> Vec<u8> {
        let mut p = vec![0u8; 6];
        p.extend([0, 1]);
        p.extend([0u8; 16]);
        bx(fourcc, &p)
    }

    struct Track {
        id: u32,
        handler: &'static [u8; 4],
        entry: Vec<u8>,
        timescale: u32,
        deltas: Vec<(u32, u32)>,
        sizes: Vec<u32>,
        data: Option<Vec<u8>>,
    }

    impl Track {
        fn new(id: u32, handler: &'static [u8; 4], entry: Vec<u8>, timescale: u32) -> Self {
            Self {
                id,
                handler,
                entry,
                timescale,
                deltas: vec![(10, timescale / 10)],
                sizes: vec![100; 10],
                data: None,
            }
        }
    }

    fn be32(v: u32) -> [u8; 4] {
        v.to_be_bytes()
    }

    fn mp4(tracks: &[Track], moov_extra: &[Vec<u8>]) -> Vec<u8> {
        let ftyp = bx(b"ftyp", b"isom\0\0\x02\0isomiso2");
        let mut zeros = vec![0u8; 4096];
        let base = (ftyp.len() + 8) as u32;
        let mut offsets = Vec::new();
        for t in tracks {
            match &t.data {
                Some(d) => {
                    offsets.push(base + zeros.len() as u32);
                    zeros.extend(d);
                }
                None => offsets.push(base),
            }
        }
        let mdat = bx(b"mdat", &zeros);
        let mut moov = vec![full(
            b"mvhd",
            0,
            &cat(&[
                be32(0).to_vec(),
                be32(0).to_vec(),
                be32(1000).to_vec(),
                be32(10_000).to_vec(),
                vec![0u8; 80],
            ]),
        )];
        for (t, off) in tracks.iter().zip(offsets) {
            let dur: u32 = t.deltas.iter().map(|(c, d)| c * d).sum();
            let tkhd = full(
                b"tkhd",
                0,
                &cat(&[
                    be32(0).to_vec(),
                    be32(0).to_vec(),
                    be32(t.id).to_vec(),
                    vec![0u8; 68],
                ]),
            );
            let mdhd = full(
                b"mdhd",
                0,
                &cat(&[
                    be32(0).to_vec(),
                    be32(0).to_vec(),
                    be32(t.timescale).to_vec(),
                    be32(dur).to_vec(),
                    vec![0x15, 0xC7, 0, 0], // "eng"
                ]),
            );
            let mut hd = vec![0u8; 4];
            hd.extend(t.handler);
            hd.extend([0u8; 12]);
            hd.extend(b"Handler\0");
            let hdlr = full(b"hdlr", 0, &hd);
            let stsd = full(b"stsd", 0, &cat(&[be32(1).to_vec(), t.entry.clone()]));
            let mut stts = be32(t.deltas.len() as u32).to_vec();
            for (c, d) in &t.deltas {
                stts.extend(be32(*c));
                stts.extend(be32(*d));
            }
            let mut stsz = cat(&[be32(0).to_vec(), be32(t.sizes.len() as u32).to_vec()]);
            for s in &t.sizes {
                stsz.extend(be32(*s));
            }
            let stsc = cat(&[
                be32(1).to_vec(),
                be32(1).to_vec(),
                be32(t.sizes.len() as u32).to_vec(),
                be32(1).to_vec(),
            ]);
            let stco = cat(&[be32(1).to_vec(), be32(off).to_vec()]);
            let stbl = bx(
                b"stbl",
                &cat(&[
                    stsd,
                    full(b"stts", 0, &stts),
                    full(b"stsc", 0, &stsc),
                    full(b"stsz", 0, &stsz),
                    full(b"stco", 0, &stco),
                ]),
            );
            let minf = bx(b"minf", &stbl);
            let mdia = bx(b"mdia", &cat(&[mdhd, hdlr, minf]));
            moov.push(bx(b"trak", &cat(&[tkhd, mdia])));
        }
        moov.extend(moov_extra.iter().cloned());
        cat(&[ftyp, mdat, bx(b"moov", &cat(&moov))])
    }

    fn run(bytes: Vec<u8>) -> Result<ProbedContainer> {
        let len = bytes.len() as u64;
        probe(&mut Cursor::new(bytes), len)
    }

    fn nclx_hdr10() -> Vec<u8> {
        let mut p = b"nclx".to_vec();
        p.extend(9u16.to_be_bytes());
        p.extend(16u16.to_be_bytes());
        p.extend(9u16.to_be_bytes());
        p.push(0);
        bx(b"colr", &p)
    }

    fn mdcv() -> Vec<u8> {
        let mut p = Vec::new();
        // G, B, R
        for v in [13250u16, 34500, 7500, 3000, 34000, 16000, 15635, 16450] {
            p.extend(v.to_be_bytes());
        }
        p.extend(10_000_000u32.to_be_bytes());
        p.extend(50u32.to_be_bytes());
        bx(b"mdcv", &p)
    }

    fn clli() -> Vec<u8> {
        let mut p = 1000u16.to_be_bytes().to_vec();
        p.extend(400u16.to_be_bytes());
        bx(b"clli", &p)
    }

    #[test]
    fn av1_hdr10() {
        // profile 2 (Professional), high_bitdepth, 4:2:0
        let av1c = bx(b"av1C", &[0x81, 0x40, 0x98, 0x00]);
        let entry = visual(b"av01", 3840, 2160, &[av1c, nclx_hdr10(), mdcv(), clli()]);
        let mut t = Track::new(1, b"vide", entry, 25_000);
        t.deltas = vec![(10, 1000)];
        let c = run(mp4(&[t], &[])).unwrap();
        assert_eq!(c.validity, ContainerValidity::Ok);
        assert_eq!(c.format, "mov,mp4");
        let s = &c.streams[0];
        assert_eq!(s.native_id, 1);
        assert_eq!(s.stream.codec.as_deref(), Some("av1"));
        assert_eq!(s.stream.width, Some(3840));
        assert_eq!(s.stream.height, Some(2160));
        assert_eq!(s.stream.codec_profile.as_deref(), Some("Professional"));
        assert_eq!(s.stream.pixel_format.as_deref(), Some("yuv420p10le"));
        assert_eq!(s.stream.field_order, Some(FieldOrder::Progressive));
        assert_eq!(s.stream.frame_rate, Rational::new(25, 1));
        assert_eq!(s.stream.language.as_deref(), Some("eng"));
        assert_eq!(
            s.stream.metadata.get("handler_name").map(String::as_str),
            Some("Handler")
        );
        let v = s.video.as_ref().unwrap();
        assert_eq!(v.frame_rate_observed, Rational::new(25, 1));
        assert_eq!(v.colorspace.as_deref(), Some("bt2020nc"));
        let hdr = v.hdr.as_ref().unwrap();
        assert_eq!(hdr.primaries.as_deref(), Some("bt2020"));
        assert_eq!(hdr.transfer.as_deref(), Some("smpte2084"));
        assert_eq!(hdr.matrix.as_deref(), Some("bt2020nc"));
        assert_eq!(hdr.range.as_deref(), Some("tv"));
        assert_eq!(hdr.dynamic_range(), Some(DynamicRange::Pq));
        assert_eq!(hdr.max_cll, Some(1000));
        assert_eq!(hdr.max_fall, Some(400));
        let m = hdr.mastering_display.unwrap();
        assert_eq!(m.max_luminance_nits, Some(1000.0));
        let prim = m.primaries_xy.unwrap();
        assert!((prim[0].0 - 0.68).abs() < 1e-9, "red first: {prim:?}");
        assert!((prim[1].1 - 0.69).abs() < 1e-9, "green second: {prim:?}");
        assert_eq!(c.timecode_present, Some(false));
        assert_eq!(c.timestamps_contiguous, Some(true));
        assert_eq!(c.duration_ms, Some(10_000));
        assert_eq!(c.diagnostics["brands"]["major"], "isom");
    }

    #[test]
    fn vp9_with_vpcc_and_ntsc_rate() {
        let mut vp = vec![1u8, 0, 0, 0, 2, 31, (10 << 4) | (3 << 1) | 1, 1, 1, 1, 0, 0];
        vp[4] = 2; // profile
        let entry = visual(b"vp09", 1280, 720, &[bx(b"vpcC", &vp)]);
        let mut t = Track::new(2, b"vide", entry, 30_000);
        t.deltas = vec![(30, 1001)];
        t.sizes = vec![50; 30];
        let c = run(mp4(&[t], &[])).unwrap();
        let s = &c.streams[0].stream;
        assert_eq!(s.codec.as_deref(), Some("vp9"));
        assert_eq!(s.codec_profile.as_deref(), Some("Profile 2"));
        assert_eq!(s.pixel_format.as_deref(), Some("yuv444p10le"));
        assert_eq!(s.frame_rate, Rational::new(30_000, 1001));
        let v = c.streams[0].video.as_ref().unwrap();
        assert_eq!(v.frame_rate_observed, Rational::new(30_000, 1001));
        let hdr = v.hdr.as_ref().unwrap();
        assert_eq!(hdr.primaries.as_deref(), Some("bt709"));
        assert_eq!(hdr.range.as_deref(), Some("pc"));
        assert!(s.bitrate.unwrap() > 0);
    }

    #[test]
    fn opus_flac_pcm_audio() {
        let dops = bx(b"dOps", &[0, 6, 0, 0, 0, 0, 0xBB, 0x80, 0, 0, 0]);
        let opus = Track::new(1, b"soun", audio(b"Opus", 2, 16, 48_000, &[dops]), 48_000);

        let mut si = vec![0u8; 34];
        let (rate, ch, bits) = (44_100u64, 2u64, 24u64);
        si[10] = (rate >> 12) as u8;
        si[11] = (rate >> 4) as u8;
        si[12] = (((rate & 0xF) << 4) | ((ch - 1) << 1) | ((bits - 1) >> 4)) as u8;
        si[13] = (((bits - 1) & 0xF) << 4) as u8;
        let mut dfla = vec![0x80, 0, 0, 34];
        dfla.extend(si);
        let flac = Track::new(
            2,
            b"soun",
            audio(b"fLaC", 2, 16, 44_100, &[full(b"dfLa", 0, &dfla)]),
            44_100,
        );
        let pcm = Track::new(3, b"soun", audio(b"sowt", 1, 16, 48_000, &[]), 48_000);
        let c = run(mp4(&[opus, flac, pcm], &[])).unwrap();
        assert_eq!(c.validity, ContainerValidity::Ok);
        let a = &c.streams[0].stream;
        assert_eq!(a.codec.as_deref(), Some("opus"));
        assert_eq!(a.channels, Some(6));
        assert_eq!(a.channel_layout.as_deref(), Some("5.1"));
        assert_eq!(a.sample_rate, Some(48_000));
        let f = &c.streams[1].stream;
        assert_eq!(f.codec.as_deref(), Some("flac"));
        assert_eq!(f.sample_rate, Some(44_100));
        assert_eq!(f.channels, Some(2));
        assert_eq!(f.bit_depth, Some(24));
        let p = &c.streams[2].stream;
        assert_eq!(p.codec.as_deref(), Some("pcm_s16le"));
        assert_eq!(p.channels, Some(1));
        assert_eq!(p.channel_layout.as_deref(), Some("mono"));
        assert_eq!(p.bit_depth, Some(16));
        assert_eq!(p.time_base, Rational::new(1, 48_000));
    }

    #[test]
    fn subtitles_tx3g_and_wvtt() {
        let mut tx = Vec::new();
        let mut sizes = Vec::new();
        for text in ["Hello", "", "World"] {
            let mut s = (text.len() as u16).to_be_bytes().to_vec();
            s.extend(text.as_bytes());
            sizes.push(s.len() as u32);
            tx.extend(s);
        }
        let mut t1 = Track::new(1, b"sbtl", text_entry(b"tx3g"), 1000);
        t1.deltas = vec![(1, 1000), (1, 500), (1, 2000)];
        t1.sizes = sizes;
        t1.data = Some(tx);

        let vttc = bx(b"vttc", &bx(b"payl", b"Line one"));
        let vtte = bx(b"vtte", &[]);
        let vttc2 = bx(
            b"vttc",
            &cat(&[bx(b"sttg", b"x"), bx(b"payl", b"Line two")]),
        );
        let mut t2 = Track::new(2, b"text", text_entry(b"wvtt"), 1000);
        t2.deltas = vec![(3, 1000)];
        t2.sizes = vec![vttc.len() as u32, vtte.len() as u32, vttc2.len() as u32];
        t2.data = Some(cat(&[vttc, vtte, vttc2]));

        let c = run(mp4(&[t1, t2], &[])).unwrap();
        assert_eq!(c.validity, ContainerValidity::Ok, "{:?}", c.validity);
        let s1 = &c.streams[0];
        assert_eq!(s1.stream.codec.as_deref(), Some("tx3g"));
        assert_eq!(s1.stream.kind, StreamKind::Subtitle);
        assert_eq!(s1.cues.len(), 3);
        assert_eq!(s1.cues[0].start_ms, 0);
        assert_eq!(s1.cues[0].end_ms, 1000);
        assert_eq!(s1.cues[0].payload.as_deref(), Some(&b"Hello"[..]));
        assert_eq!(s1.cues[1].payload.as_deref(), Some(&b""[..]));
        assert_eq!(s1.cues[2].start_ms, 1500);
        assert_eq!(s1.cues[2].end_ms, 3500);
        assert_eq!(s1.cues[2].payload.as_deref(), Some(&b"World"[..]));
        assert!(!s1.cues_truncated);
        let s2 = &c.streams[1];
        assert_eq!(s2.stream.codec.as_deref(), Some("webvtt"));
        assert_eq!(s2.cues.len(), 2);
        assert_eq!((s2.cues[0].start_ms, s2.cues[0].end_ms), (0, 1000));
        assert_eq!(s2.cues[0].payload.as_deref(), Some(&b"Line one"[..]));
        assert_eq!((s2.cues[1].start_ms, s2.cues[1].end_ms), (2000, 3000));
        assert_eq!(s2.cues[1].payload.as_deref(), Some(&b"Line two"[..]));
    }

    #[test]
    fn timecode_track_sets_flag() {
        let v = Track::new(1, b"vide", visual(b"vp09", 64, 64, &[]), 1000);
        let tc = Track::new(2, b"tmcd", text_entry(b"tmcd"), 1000);
        let c = run(mp4(&[v, tc], &[])).unwrap();
        assert_eq!(c.timecode_present, Some(true));
        assert_eq!(c.streams[1].stream.kind, StreamKind::Data);
        assert_eq!(c.streams[1].stream.codec.as_deref(), Some("tmcd"));
    }

    #[test]
    fn gap_in_stts_is_reported() {
        let mut t = Track::new(1, b"vide", visual(b"vp09", 64, 64, &[]), 1000);
        // 5 frames of 40 ms, one 240 ms jump, 5 more frames of 40 ms.
        t.deltas = vec![(5, 40), (1, 240), (5, 40)];
        t.sizes = vec![10; 11];
        let c = run(mp4(&[t], &[])).unwrap();
        assert_eq!(c.timestamps_contiguous, Some(false));
        assert_eq!(c.timestamp_gaps.len(), 1);
        // Sample 5 starts at 200 ms and nominally ends at 240 ms; next starts at 440.
        assert_eq!(c.timestamp_gaps[0], TimeRange::new(240, 440));
    }

    #[test]
    fn missing_moov_is_corrupt() {
        let mut bytes = bx(b"ftyp", b"isom\0\0\0\0");
        bytes.extend(bx(b"mdat", &[0u8; 64]));
        let c = run(bytes).unwrap();
        assert!(matches!(c.validity, ContainerValidity::Corrupt(ref r) if r.contains("no moov")));
    }

    #[test]
    fn refuses_non_royalty_free_codecs() {
        for (handler, entry, needle) in [
            (
                b"vide",
                visual(b"avc1", 64, 64, &[bx(b"avcC", &[1, 2, 3])]),
                "H.264",
            ),
            (b"soun", audio(b"mp4a", 2, 16, 48_000, &[]), "AAC"),
            (b"vide", visual(b"apch", 64, 64, &[]), "ProRes"),
        ] {
            let ts = 1000;
            let t = Track::new(1, handler, entry, ts);
            let e = run(mp4(&[t], &[])).unwrap_err().to_string();
            assert!(e.contains("royalty-free") && e.contains(needle), "{e}");
        }
    }

    #[test]
    fn fragmented_is_flagged_not_corrupt() {
        let t = Track::new(1, b"vide", visual(b"vp09", 64, 64, &[]), 1000);
        let c = run(mp4(&[t], &[bx(b"mvex", &[])])).unwrap();
        assert_eq!(c.validity, ContainerValidity::Ok);
        assert_eq!(c.diagnostics["fragmented"], true);
    }

    #[test]
    fn chunk_offset_past_eof_is_corrupt() {
        let t = Track::new(1, b"vide", visual(b"vp09", 64, 64, &[]), 1000);
        let mut bytes = mp4(&[t], &[]);
        // stco is the last box in the moov: patch its offset to something huge.
        let pos = bytes.len() - 4;
        bytes[pos..].copy_from_slice(&u32::MAX.to_be_bytes());
        let c = run(bytes).unwrap();
        assert!(
            matches!(c.validity, ContainerValidity::Corrupt(ref r) if r.contains("chunk offset")),
            "{:?}",
            c.validity
        );
    }

    #[test]
    fn real_vp9_fixture() {
        let c = run(FIXTURE.to_vec()).unwrap();
        assert_eq!(
            c.validity,
            ContainerValidity::Ok,
            "{:?}",
            c.malformed_metadata
        );
        assert!(!c.streams.is_empty());
        let s = &c.streams[0].stream;
        assert_eq!(s.codec.as_deref(), Some("vp9"));
        assert!(s.width.unwrap() > 0 && s.height.unwrap() > 0);
        assert!(s.frame_rate.is_some());
        assert!(s.duration.is_some() || c.duration_ms.is_some());
        assert!(c.duration_ms.is_some());
    }

    #[test]
    fn truncation_and_corruption_never_panic() {
        let len = FIXTURE.len();
        let stride = (len / 256).max(1);
        let lengths = (0..len.min(4096)).chain((4096..len).step_by(stride));
        for n in lengths {
            let bytes = FIXTURE[..n].to_vec();
            let _ = run(bytes);
        }
        for i in 0..512.min(len) {
            for flip in [0xFFu8, 0x55, 0x01] {
                let mut bytes = FIXTURE.to_vec();
                bytes[i] ^= flip;
                let _ = run(bytes);
            }
        }
        // Flip bytes inside the moov of the fixture as well.
        let moov_start = len.saturating_sub(1200);
        for i in moov_start..len {
            let mut bytes = FIXTURE.to_vec();
            bytes[i] ^= 0xFF;
            let _ = run(bytes);
        }
    }

    #[test]
    fn hostile_sizes_terminate() {
        // size 1 with a huge largesize, size 0, and tiny sizes.
        let mut a = vec![0, 0, 0, 1];
        a.extend(b"moov");
        a.extend(u64::MAX.to_be_bytes());
        let _ = run(a);
        let _ = run([&[0, 0, 0, 0][..], b"moov", &[0u8; 64]].concat());
        let _ = run([&[0, 0, 0, 4][..], b"moov"].concat());
        let _ = run(bx(b"moov", &bx(b"trak", &[0xFF; 40])));
    }
}
