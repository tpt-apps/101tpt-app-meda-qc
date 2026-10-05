//! MPEG transport stream (188-byte packets) reader.
//!
//! The file is streamed once through a bounded buffer (constant memory for any
//! file size). PAT and PMT tables are decoded (CRC-32/MPEG-2 checked), every
//! elementary stream is classified through the royalty-free allow-list *before*
//! any of its payload is looked at, and per-stream PES timestamps are tracked
//! to derive frame rate, duration and timestamp gaps. Transport-level
//! integrity counters (sync losses, continuity errors, transport-error
//! indicators, PSI CRC errors) are reported in the diagnostics.
//!
//! Only allowed codecs are ever interpreted: AV1 sequence headers are parsed
//! for geometry and colour; everything else is listed from PMT signalling.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::{ErrorKind, Read, Seek};
use std::panic::{catch_unwind, AssertUnwindSafe};

use tpt_app_media_qc_core::error::Result;
use tpt_app_media_qc_model::asset::{FieldOrder, Stream, StreamId, StreamKind};
use tpt_app_media_qc_model::finding::TimeRange;
use tpt_app_media_qc_model::inspection::{HdrMetadata, VideoMeasurements};
use tpt_app_media_qc_model::time::{DurationSeconds, Rational};

use crate::codec::{self, Class};
use crate::common::{ProbedContainer, ProbedStream};
use crate::hdr::{self, Cicp};

const PACKET: usize = 188;
const READ_CHUNK: usize = PACKET * 512;
/// Bytes that must be buffered (unless at end of file) before each decision.
const LOOKAHEAD: usize = 4096;
const MAX_PACKETS: u64 = 25_000_000;
const MAX_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_GAPS: usize = 1000;
const MAX_CANDIDATES: usize = 100_000;
const MAX_DISTINCT_DELTAS: usize = 4096;
/// PTS/PCR tick rate.
const TICKS: i64 = 90_000;
const WRAP: i64 = 1 << 33;
/// How many PES units of a video stream are searched for a sequence header.
const MAX_SEQ_PES: u32 = 8;
const MAX_PES_COLLECT: usize = 32 * 1024;
const MAX_REPORTED_PIDS: usize = 32;

// ---------------------------------------------------------------------------
// Timestamp tracking
// ---------------------------------------------------------------------------

/// Unwraps a stream of 33-bit timestamps and records the statistics needed for
/// frame rate, duration and gap detection in constant memory.
#[derive(Default)]
struct TsTrack {
    count: u64,
    last_raw: i64,
    cur: i64,
    min: i64,
    max: i64,
    /// Positive delta (ticks) -> occurrences.
    hist: BTreeMap<i64, u64>,
    /// Large forward steps: (previous, next) unwrapped ticks.
    candidates: Vec<(i64, i64)>,
    /// Backward jumps: (next, previous) unwrapped ticks.
    backward: Vec<(i64, i64)>,
}

impl TsTrack {
    fn push(&mut self, raw: u64) {
        let raw = (raw & ((1 << 33) - 1)) as i64;
        if self.count == 0 {
            self.cur = raw;
            self.min = raw;
            self.max = raw;
        } else {
            let mut d = raw - self.last_raw;
            if d < -(WRAP / 2) {
                d += WRAP;
            } else if d > WRAP / 2 {
                d -= WRAP;
            }
            let new = self.cur + d;
            if d < 0 {
                if self.backward.len() < MAX_GAPS {
                    self.backward.push((new, self.cur));
                }
            } else if d > 0 {
                if self.hist.len() < MAX_DISTINCT_DELTAS || self.hist.contains_key(&d) {
                    *self.hist.entry(d).or_insert(0) += 1;
                }
                // 20 ms.
                if d >= TICKS / 50 && self.candidates.len() < MAX_CANDIDATES {
                    self.candidates.push((self.cur, new));
                }
            }
            self.cur = new;
            self.min = self.min.min(new);
            self.max = self.max.max(new);
        }
        self.last_raw = raw;
        self.count += 1;
    }

    /// Lower median of the positive deltas.
    fn median_delta(&self) -> Option<i64> {
        let total: u64 = self.hist.values().sum();
        if total == 0 {
            return None;
        }
        let mut seen = 0u64;
        for (delta, n) in &self.hist {
            seen += n;
            if seen * 2 >= total {
                return Some(*delta);
            }
        }
        None
    }

    fn span_ticks(&self) -> Option<i64> {
        (self.count >= 2 && self.max > self.min).then_some(self.max - self.min)
    }

    fn gaps(&self) -> Vec<TimeRange> {
        let mut out = Vec::new();
        let median = self.median_delta().unwrap_or(0);
        for (prev, next) in &self.candidates {
            let d = next - prev;
            if d * 2 > median * 3 {
                out.push(range(*prev, *next));
            }
        }
        for (next, prev) in &self.backward {
            out.push(range(*next, *prev));
        }
        out
    }
}

fn range(a: i64, b: i64) -> TimeRange {
    TimeRange::new(ticks_to_ms(a), ticks_to_ms(b))
}

fn ticks_to_ms(t: i64) -> u64 {
    (t.max(0) as u64) / 90
}

// ---------------------------------------------------------------------------
// Tables
// ---------------------------------------------------------------------------

struct Es {
    stream_type: u8,
    tags: Vec<u8>,
    language: Option<String>,
    opus_channels: Option<u64>,
    class: Class,
}

#[derive(Default)]
struct Psi {
    buf: Vec<u8>,
    active: bool,
}

#[derive(Default)]
struct PidState {
    last_cc: Option<u8>,
    packets: u64,
    errors: u64,
    pcr: Option<Box<TsTrack>>,
}

struct Av1Info {
    width: u64,
    height: u64,
    profile: u8,
    bit_depth: u8,
    mono: bool,
    sub_x: bool,
    sub_y: bool,
    cicp: Cicp,
}

#[derive(Default)]
struct EsTrack {
    is_av1: bool,
    bytes: u64,
    pts: TsTrack,
    cur_pes: Vec<u8>,
    collecting: bool,
    pes_tried: u32,
    av1: Option<Av1Info>,
}

impl EsTrack {
    fn flush_pes(&mut self) {
        if self.collecting && self.av1.is_none() && self.pes_tried < MAX_SEQ_PES {
            self.pes_tried += 1;
            self.av1 = parse_av1(&self.cur_pes);
        }
        self.cur_pes.clear();
        self.collecting = false;
    }
}

#[derive(Default)]
struct State {
    packets: u64,
    transport_errors: u64,
    continuity_errors: u64,
    crc_errors: u64,
    pids: HashMap<u16, PidState>,
    psi: HashMap<u16, Psi>,
    /// program_number -> PMT PID.
    programs: BTreeMap<u16, u16>,
    pmt_done: BTreeSet<u16>,
    es: BTreeMap<u16, Es>,
    tracks: HashMap<u16, EsTrack>,
    saw_pat: bool,
}

fn mpeg_crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in bytes {
        crc ^= u32::from(*byte) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04C1_1DB7
            } else {
                crc << 1
            };
        }
    }
    crc
}

fn be16(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}

fn parse_av1(data: &[u8]) -> Option<Av1Info> {
    use tpt_kinetix_av1::obu::{parse_obu_sequence, ObuType, SequenceHeaderObu};
    catch_unwind(AssertUnwindSafe(|| {
        for obu in parse_obu_sequence(data) {
            if obu.obu_type != ObuType::SequenceHeader {
                continue;
            }
            let sh = SequenceHeaderObu::parse(&obu.payload).ok()?;
            let cc = &sh.color_config;
            return Some(Av1Info {
                width: u64::from(sh.frame_width()),
                height: u64::from(sh.frame_height()),
                profile: sh.seq_profile,
                bit_depth: cc.bit_depth,
                mono: cc.mono_chrome,
                sub_x: cc.subsampling_x,
                sub_y: cc.subsampling_y,
                cicp: Cicp {
                    primaries: Some(cc.color_primaries),
                    transfer: Some(cc.transfer_characteristics),
                    matrix: Some(cc.matrix_coefficients),
                    full_range: Some(cc.color_range),
                },
            });
        }
        None
    }))
    .ok()
    .flatten()
}

fn pixel_format(a: &Av1Info) -> Option<String> {
    let suffix = match a.bit_depth {
        8 => "",
        10 => "10le",
        12 => "12le",
        _ => return None,
    };
    if a.mono {
        return Some(format!("gray{suffix}"));
    }
    let layout = match (a.sub_x, a.sub_y) {
        (true, true) => "420",
        (true, false) => "422",
        (false, false) => "444",
        _ => return None,
    };
    Some(format!("yuv{layout}p{suffix}"))
}

// ---------------------------------------------------------------------------
// Section parsing
// ---------------------------------------------------------------------------

fn drain_sections(p: &mut Psi, out: &mut Vec<Vec<u8>>) {
    loop {
        if p.buf.is_empty() {
            break;
        }
        if p.buf[0] == 0xFF {
            p.buf.clear();
            break;
        }
        if p.buf.len() < 3 {
            break;
        }
        let section_length = usize::from(be16(&p.buf[1..3]) & 0x0FFF);
        if section_length > 1021 {
            p.buf.clear();
            break;
        }
        let total = 3 + section_length;
        if p.buf.len() < total {
            break;
        }
        out.push(p.buf[..total].to_vec());
        p.buf.drain(..total);
    }
    p.active = !p.buf.is_empty();
}

fn feed_psi(st: &mut State, pid: u16, pusi: bool, payload: &[u8]) -> Result<()> {
    let mut sections = Vec::new();
    {
        let p = st.psi.entry(pid).or_default();
        if pusi {
            let Some((&pointer, rest)) = payload.split_first() else {
                return Ok(());
            };
            let pointer = usize::from(pointer);
            if p.active {
                let n = pointer.min(rest.len());
                p.buf.extend_from_slice(&rest[..n]);
                drain_sections(p, &mut sections);
            }
            p.buf.clear();
            p.active = false;
            if pointer <= rest.len() {
                p.buf.extend_from_slice(&rest[pointer..]);
                drain_sections(p, &mut sections);
            }
        } else if p.active {
            if p.buf.len() + payload.len() > 8192 {
                p.buf.clear();
                p.active = false;
            } else {
                p.buf.extend_from_slice(payload);
                drain_sections(p, &mut sections);
            }
        }
    }
    for s in sections {
        handle_section(st, pid, &s)?;
    }
    Ok(())
}

fn handle_section(st: &mut State, pid: u16, s: &[u8]) -> Result<()> {
    if s.len() < 12 || s[1] & 0x80 == 0 {
        return Ok(());
    }
    if mpeg_crc32(s) != 0 {
        st.crc_errors += 1;
        return Ok(());
    }
    let end = s.len() - 4;
    match s[0] {
        0x00 if pid == 0 => {
            st.saw_pat = true;
            let mut i = 8;
            while i + 4 <= end {
                let program = be16(&s[i..]);
                let target = be16(&s[i + 2..]) & 0x1FFF;
                if program != 0 {
                    st.programs.entry(program).or_insert(target);
                }
                i += 4;
            }
        }
        0x02 => {
            let program = be16(&s[3..]);
            if st.programs.get(&program) != Some(&pid) || st.pmt_done.contains(&program) {
                return Ok(());
            }
            handle_pmt(st, program, s, end)?;
        }
        _ => {}
    }
    Ok(())
}

fn handle_pmt(st: &mut State, program: u16, s: &[u8], end: usize) -> Result<()> {
    let program_info = usize::from(be16(&s[10..]) & 0x0FFF);
    let mut i = 12 + program_info;
    let mut found: Vec<(u16, Es)> = Vec::new();
    while i + 5 <= end {
        let stream_type = s[i];
        let pid = be16(&s[i + 1..]) & 0x1FFF;
        let info_len = usize::from(be16(&s[i + 3..]) & 0x0FFF);
        i += 5;
        let info_end = (i + info_len).min(end);
        let mut tags = Vec::new();
        let mut registration = None;
        let mut language = None;
        let mut opus_channels = None;
        let mut d = i;
        while d + 2 <= info_end {
            let tag = s[d];
            let len = usize::from(s[d + 1]);
            let body_end = d + 2 + len;
            if body_end > info_end {
                break;
            }
            let body = &s[d + 2..body_end];
            tags.push(tag);
            match tag {
                0x05 if body.len() >= 4 && registration.is_none() => {
                    registration = Some([body[0], body[1], body[2], body[3]]);
                }
                0x0A if body.len() >= 4 && language.is_none() => {
                    let code = &body[..3];
                    if code.iter().all(u8::is_ascii_alphabetic) {
                        let lang = String::from_utf8_lossy(code).to_ascii_lowercase();
                        if lang != "und" {
                            language = Some(lang);
                        }
                    }
                }
                0x7F if body.len() >= 2 && body[0] == 0x80 => {
                    opus_channels = match body[1] {
                        0 => Some(2),
                        n @ 1..=8 => Some(u64::from(n)),
                        _ => None,
                    };
                }
                _ => {}
            }
            d = body_end;
        }
        i = info_end;
        let class = codec::classify_ts(stream_type, registration, &tags);
        found.push((
            pid,
            Es {
                stream_type,
                tags,
                language,
                opus_channels,
                class,
            },
        ));
    }
    // Policy: refuse before any payload of any stream is interpreted.
    if let Some(what) = codec::first_refusal(found.iter().map(|(_, e)| &e.class)) {
        return Err(codec::unsupported_error(what));
    }
    st.pmt_done.insert(program);
    for (pid, es) in found {
        if st.es.contains_key(&pid) {
            continue;
        }
        if let Class::Supported { codec, kind } = &es.class {
            if matches!(kind, StreamKind::Video | StreamKind::Audio) {
                st.tracks.insert(
                    pid,
                    EsTrack {
                        is_av1: *codec == "av1",
                        ..Default::default()
                    },
                );
            }
        }
        st.es.insert(pid, es);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Packet processing
// ---------------------------------------------------------------------------

fn decode_pts(p: &[u8]) -> Option<u64> {
    if p.len() < 5 || p[0] & 0xE1 != 0x21 || p[2] & 1 == 0 || p[4] & 1 == 0 {
        return None;
    }
    Some(
        (u64::from(p[0] >> 1) & 7) << 30
            | u64::from(p[1]) << 22
            | (u64::from(p[2]) >> 1) << 15
            | u64::from(p[3]) << 7
            | u64::from(p[4]) >> 1,
    )
}

fn feed_pes(t: &mut EsTrack, pusi: bool, payload: &[u8]) {
    t.bytes += payload.len() as u64;
    let want_seq = t.is_av1 && t.av1.is_none() && t.pes_tried < MAX_SEQ_PES;
    if pusi {
        t.flush_pes();
        if payload.len() < 9 || payload[0] != 0 || payload[1] != 0 || payload[2] != 1 {
            return;
        }
        let sid = payload[3];
        // Stream ids without the optional PES header.
        if matches!(sid, 0xBC | 0xBE | 0xBF | 0xF0 | 0xF1 | 0xF2 | 0xF8 | 0xFF) {
            return;
        }
        let header_len = usize::from(payload[8]);
        if (payload[7] >> 6) & 2 != 0 && header_len >= 5 && payload.len() >= 14 {
            if let Some(v) = decode_pts(&payload[9..14]) {
                t.pts.push(v);
            }
        }
        if want_seq {
            if let Some(data) = payload.get(9 + header_len..) {
                t.cur_pes.extend_from_slice(data);
                t.collecting = true;
            }
        }
    } else if t.collecting && t.cur_pes.len() < MAX_PES_COLLECT {
        t.cur_pes.extend_from_slice(payload);
    }
}

fn process_packet(st: &mut State, p: &[u8]) -> Result<()> {
    st.packets += 1;
    if p[1] & 0x80 != 0 {
        st.transport_errors += 1;
        return Ok(());
    }
    let pusi = p[1] & 0x40 != 0;
    let pid = be16(&p[1..]) & 0x1FFF;
    let afc = (p[3] >> 4) & 3;
    let cc = p[3] & 0x0F;
    if pid == 0x1FFF || afc == 0 {
        return Ok(());
    }
    let mut payload_start = 4;
    let mut discontinuity = false;
    let mut pcr = None;
    if afc & 2 != 0 {
        let af_len = usize::from(p[4]);
        if af_len > 183 {
            return Ok(());
        }
        payload_start = 5 + af_len;
        if af_len >= 1 {
            let flags = p[5];
            discontinuity = flags & 0x80 != 0;
            if flags & 0x10 != 0 && af_len >= 7 {
                let b = &p[6..11];
                pcr = Some(
                    u64::from(b[0]) << 25
                        | u64::from(b[1]) << 17
                        | u64::from(b[2]) << 9
                        | u64::from(b[3]) << 1
                        | u64::from(b[4] >> 7),
                );
            }
        }
    }
    let has_payload = afc & 1 != 0 && payload_start < PACKET;

    let ps = st.pids.entry(pid).or_default();
    ps.packets += 1;
    if let Some(v) = pcr {
        ps.pcr.get_or_insert_with(Default::default).push(v);
    }
    if !has_payload {
        return Ok(());
    }
    if let Some(last) = ps.last_cc {
        if !discontinuity {
            if last == cc {
                // Duplicate packet: legal, carries no new data.
                return Ok(());
            }
            if cc != (last + 1) & 0x0F {
                ps.errors += 1;
                st.continuity_errors += 1;
            }
        }
    }
    ps.last_cc = Some(cc);

    let payload = &p[payload_start..];
    if pid == 0 || st.programs.values().any(|v| *v == pid) {
        feed_psi(st, pid, pusi, payload)?;
    } else if let Some(t) = st.tracks.get_mut(&pid) {
        feed_pes(t, pusi, payload);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Buffered scanner
// ---------------------------------------------------------------------------

struct Scanner<'a, R> {
    r: &'a mut R,
    buf: Vec<u8>,
    pos: usize,
    eof: bool,
}

impl<R: Read> Scanner<'_, R> {
    /// Make at least `want` bytes available past `pos`, unless at end of file.
    fn ensure(&mut self, want: usize) -> Result<()> {
        while self.buf.len() - self.pos < want && !self.eof {
            if self.pos > 0 {
                self.buf.drain(..self.pos);
                self.pos = 0;
            }
            let old = self.buf.len();
            self.buf.resize(old + READ_CHUNK, 0);
            let n = loop {
                match self.r.read(&mut self.buf[old..]) {
                    Ok(n) => break n,
                    Err(e) if e.kind() == ErrorKind::Interrupted => {}
                    Err(e) => return Err(e.into()),
                }
            };
            self.buf.truncate(old + n);
            if n == 0 {
                self.eof = true;
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Probe an MPEG transport stream of `len` bytes.
pub fn probe<R: Read + Seek>(source: &mut R, len: u64) -> Result<ProbedContainer> {
    source.rewind()?;
    let mut sc = Scanner {
        r: source,
        buf: Vec::new(),
        pos: 0,
        eof: false,
    };
    let mut st = State::default();
    let mut in_sync = false;
    let mut sync_losses = 0u64;
    let mut total = 0u64;
    let mut truncated = false;

    loop {
        sc.ensure(LOOKAHEAD)?;
        let avail = sc.buf.len() - sc.pos;
        if avail < PACKET {
            break;
        }
        if st.packets >= MAX_PACKETS || total >= MAX_BYTES {
            truncated = true;
            break;
        }
        let b = &sc.buf[sc.pos..];
        let synced = b[0] == 0x47 && (in_sync || avail == PACKET || b[PACKET] == 0x47);
        if synced {
            in_sync = true;
            process_packet(&mut st, &b[..PACKET])?;
            sc.pos += PACKET;
            total += PACKET as u64;
        } else {
            if in_sync {
                sync_losses += 1;
                in_sync = false;
            }
            sc.pos += 1;
            total += 1;
        }
    }

    for t in st.tracks.values_mut() {
        t.flush_pes();
    }

    let mut out = ProbedContainer::new("mpegts");
    out.timecode_present = None;
    let d = &mut out.diagnostics;
    d.insert("ts_packets".into(), st.packets.into());
    d.insert("ts_sync_losses".into(), sync_losses.into());
    d.insert("ts_continuity_errors".into(), st.continuity_errors.into());
    d.insert("ts_transport_errors".into(), st.transport_errors.into());
    d.insert("ts_psi_crc_errors".into(), st.crc_errors.into());
    d.insert("programs".into(), st.pmt_done.len().into());
    if truncated {
        d.insert("scan_truncated".into(), true.into());
    }
    if len % PACKET as u64 != 0 {
        d.insert("ts_length_not_multiple_of_188".into(), true.into());
    }

    // Integrity notes.
    let notes = &mut out.malformed_metadata;
    if sync_losses > 0 {
        notes.push(format!("{sync_losses} loss(es) of transport-stream sync"));
    }
    let mut cc_pids: Vec<_> = st.pids.iter().filter(|(_, s)| s.errors > 0).collect();
    cc_pids.sort_by_key(|(pid, _)| **pid);
    for (pid, s) in cc_pids.iter().take(MAX_REPORTED_PIDS) {
        notes.push(format!(
            "{} continuity-counter errors on PID 0x{:X}",
            s.errors, pid
        ));
    }
    if st.transport_errors > 0 {
        notes.push(format!(
            "{} packets with the transport-error indicator set",
            st.transport_errors
        ));
    }
    if st.crc_errors > 0 {
        notes.push(format!(
            "{} PSI sections failed their CRC-32",
            st.crc_errors
        ));
    }

    if st.pmt_done.is_empty() {
        out.validity = tpt_app_media_qc_model::inspection::ContainerValidity::Corrupt(
            "no program association/map table found".into(),
        );
        return Ok(out);
    }
    for (program, pmt_pid) in &st.programs {
        if !st.pids.contains_key(pmt_pid) {
            out.malformed_metadata.push(format!(
                "program {program} references PMT PID 0x{pmt_pid:X} which never appears"
            ));
        }
    }
    for pid in st.es.keys() {
        if !st.pids.contains_key(pid) {
            out.malformed_metadata.push(format!(
                "the PMT references elementary stream PID 0x{pid:X} which never appears"
            ));
        }
    }

    // Streams, ascending PID.
    let mut durations: Vec<u64> = Vec::new();
    let mut gaps: Vec<TimeRange> = Vec::new();
    let mut observed_any = false;
    for (position, (pid, es)) in st.es.iter().enumerate() {
        let track = st.tracks.get(pid);
        let mut stream = Stream::primary_audio(0);
        stream.index = StreamId::new(position as u64);
        stream.codec_profile = None;
        stream.width = None;
        stream.height = None;
        stream.pixel_format = None;
        stream.field_order = None;
        stream.frame_rate = None;
        stream.time_base = None;
        stream.bitrate = None;
        stream.duration = None;
        stream.language = es.language.clone();
        stream.channel_layout = None;
        stream.channels = None;
        stream.sample_rate = None;
        stream.bit_depth = None;
        stream.metadata.clear();
        stream
            .metadata
            .insert("pid".into(), u64::from(*pid).to_string());
        stream
            .metadata
            .insert("stream_type".into(), format!("0x{:02x}", es.stream_type));
        let _ = &es.tags;

        let mut video = None;
        match &es.class {
            Class::Passive { codec, kind } => {
                stream.kind = *kind;
                stream.codec = Some(codec.clone());
            }
            Class::Supported { codec, kind } => {
                stream.kind = *kind;
                stream.codec = Some((*codec).to_string());
                stream.time_base = Rational::new(1, 90_000);
                let mut median = None;
                let mut span_ms = None;
                if let Some(t) = track {
                    median = t.pts.median_delta();
                    if let Some(span) = t.pts.span_ticks() {
                        let ms = ticks_to_ms(span);
                        span_ms = Some(ms);
                        durations.push(ms);
                        stream.duration = Some(DurationSeconds::from_millis(ms));
                        if ms > 0 {
                            stream.bitrate =
                                Some((u128::from(t.bytes) * 8000 / u128::from(ms)) as u64);
                        }
                    }
                    if t.pts.count >= 2 {
                        observed_any = true;
                        gaps.extend(t.pts.gaps());
                    }
                }
                let _ = span_ms;
                match kind {
                    StreamKind::Video => {
                        let mut m = VideoMeasurements {
                            field_order: Some(FieldOrder::Progressive),
                            ..Default::default()
                        };
                        stream.field_order = Some(FieldOrder::Progressive);
                        if let Some(d) = median.filter(|d| *d > 0) {
                            stream.frame_rate = Rational::new(TICKS as u64, d as u64);
                        }
                        if let Some(t) = track {
                            if let Some(span) = t.pts.span_ticks() {
                                m.frame_rate_observed =
                                    Rational::new((t.pts.count - 1) * TICKS as u64, span as u64);
                            }
                            if let Some(a) = &t.av1 {
                                stream.width = Some(a.width);
                                stream.height = Some(a.height);
                                stream.pixel_format = pixel_format(a);
                                stream.bit_depth = Some(u64::from(a.bit_depth));
                                stream.codec_profile = match a.profile {
                                    0 => Some("Main"),
                                    1 => Some("High"),
                                    2 => Some("Professional"),
                                    _ => None,
                                }
                                .map(str::to_string);
                                let mut h = HdrMetadata::default();
                                a.cicp.apply(&mut h);
                                m.hdr = hdr::non_empty(h);
                                m.colorspace = a.cicp.colorspace();
                            }
                        }
                        video = Some(m);
                    }
                    _ => {
                        if *codec == "opus" {
                            stream.sample_rate = Some(48_000);
                            stream.channels = es.opus_channels;
                            stream.channel_layout = es
                                .opus_channels
                                .and_then(crate::audio_files::channel_layout);
                        }
                    }
                }
            }
            Class::Refused(_) => {}
        }
        out.streams.push(ProbedStream {
            stream,
            native_id: u64::from(*pid),
            video,
            cues: Vec::new(),
            cues_truncated: false,
        });
    }

    for (position, ps) in out.streams.iter_mut().enumerate() {
        if let Some(v) = ps.video.as_mut() {
            v.stream_idx = position as u64;
        }
    }

    out.duration_ms = durations.iter().copied().max().or_else(|| {
        st.pids
            .values()
            .filter_map(|p| p.pcr.as_ref()?.span_ticks())
            .max()
            .map(ticks_to_ms)
    });
    gaps.sort_by_key(|g| (g.start_ms, g.end_ms));
    gaps.truncate(MAX_GAPS);
    if observed_any {
        out.timestamps_contiguous = Some(gaps.is_empty());
    }
    out.timestamp_gaps = gaps;
    Ok(out)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use tpt_app_media_qc_model::inspection::ContainerValidity;

    const PMT_PID: u16 = 0x1000;

    fn section(table_id: u8, body: &[u8]) -> Vec<u8> {
        let mut s = vec![table_id];
        s.extend_from_slice(&(0xB000u16 | (body.len() + 4) as u16).to_be_bytes());
        s.extend_from_slice(body);
        s.extend_from_slice(&mpeg_crc32(&s).to_be_bytes());
        s
    }

    fn pat_section() -> Vec<u8> {
        let mut body = vec![0x00, 0x01, 0xC1, 0x00, 0x00];
        body.extend_from_slice(&1u16.to_be_bytes());
        body.extend_from_slice(&(0xE000 | PMT_PID).to_be_bytes());
        section(0x00, &body)
    }

    struct EsSpec {
        pid: u16,
        stream_type: u8,
        descriptors: Vec<u8>,
    }

    fn reg(fourcc: &[u8; 4]) -> Vec<u8> {
        let mut d = vec![0x05, 0x04];
        d.extend_from_slice(fourcc);
        d
    }

    fn pmt_section(streams: &[EsSpec]) -> Vec<u8> {
        let mut body = vec![0x00, 0x01, 0xC1, 0x00, 0x00];
        body.extend_from_slice(&0xE100u16.to_be_bytes());
        body.extend_from_slice(&0xF000u16.to_be_bytes());
        for s in streams {
            body.push(s.stream_type);
            body.extend_from_slice(&(0xE000 | s.pid).to_be_bytes());
            body.extend_from_slice(&(0xF000u16 | s.descriptors.len() as u16).to_be_bytes());
            body.extend_from_slice(&s.descriptors);
        }
        section(0x02, &body)
    }

    #[derive(Default)]
    struct Ts {
        out: Vec<u8>,
        cc: HashMap<u16, u8>,
    }

    impl Ts {
        fn next_cc(&mut self, pid: u16) -> u8 {
            let c = self.cc.entry(pid).or_insert(15);
            *c = (*c + 1) & 15;
            *c
        }

        fn psi(&mut self, pid: u16, section: &[u8]) {
            let cc = self.next_cc(pid);
            let mut p = vec![0x47, 0x40 | (pid >> 8) as u8, pid as u8, 0x10 | cc, 0x00];
            p.extend_from_slice(section);
            p.resize(PACKET, 0xFF);
            self.out.extend(p);
        }

        fn headers(&mut self, streams: &[EsSpec]) {
            self.psi(0, &pat_section());
            self.psi(PMT_PID, &pmt_section(streams));
        }

        fn pes(&mut self, pid: u16, pts: u64, payload: &[u8]) {
            let mut pes = vec![0, 0, 1, 0xE0, 0, 0, 0x80, 0x80, 5];
            let v = pts & 0x1_FFFF_FFFF;
            pes.push(0x21 | (((v >> 30) as u8 & 7) << 1));
            pes.push((v >> 22) as u8);
            pes.push(0x01 | (((v >> 15) as u8 & 0x7F) << 1));
            pes.push((v >> 7) as u8);
            pes.push(0x01 | ((v as u8 & 0x7F) << 1));
            pes.extend_from_slice(payload);
            assert!(pes.len() <= 184);
            let cc = self.next_cc(pid);
            let af_total = 184 - pes.len();
            let mut p = vec![0x47, 0x40 | (pid >> 8) as u8, pid as u8];
            if af_total == 0 {
                p.push(0x10 | cc);
            } else {
                p.push(0x30 | cc);
                p.push((af_total - 1) as u8);
                if af_total > 1 {
                    p.push(0);
                    p.resize(5 + af_total - 1, 0xFF);
                }
            }
            p.extend_from_slice(&pes);
            assert_eq!(p.len(), PACKET);
            self.out.extend(p);
        }

        fn pcr(&mut self, pid: u16, base: u64) {
            let cc = self.cc.get(&pid).copied().unwrap_or(0);
            let mut p = vec![0x47, (pid >> 8) as u8, pid as u8, 0x20 | cc, 183, 0x10];
            p.push((base >> 25) as u8);
            p.push((base >> 17) as u8);
            p.push((base >> 9) as u8);
            p.push((base >> 1) as u8);
            p.push(((base & 1) as u8) << 7 | 0x7E);
            p.push(0);
            p.resize(PACKET, 0xFF);
            self.out.extend(p);
        }
    }

    fn run(bytes: &[u8]) -> Result<ProbedContainer> {
        probe(&mut Cursor::new(bytes.to_vec()), bytes.len() as u64)
    }

    // --- AV1 sequence header builder -------------------------------------

    struct Bits {
        bytes: Vec<u8>,
        n: usize,
    }
    impl Bits {
        fn put(&mut self, v: u32, bits: usize) {
            for i in (0..bits).rev() {
                if self.n % 8 == 0 {
                    self.bytes.push(0);
                }
                let bit = ((v >> i) & 1) as u8;
                *self.bytes.last_mut().unwrap() |= bit << (7 - self.n % 8);
                self.n += 1;
            }
        }
    }

    fn av1_payload(hdr10: bool) -> Vec<u8> {
        let mut b = Bits {
            bytes: Vec::new(),
            n: 0,
        };
        b.put(0, 3); // profile
        b.put(0, 1); // still
        b.put(0, 1); // reduced
        b.put(0, 1); // timing info
        b.put(0, 1); // initial display delay
        b.put(0, 5); // op count - 1
        b.put(0, 12); // idc
        b.put(8, 5); // level
        b.put(0, 1); // tier
        b.put(11, 4);
        b.put(11, 4);
        b.put(1919, 12);
        b.put(1079, 12);
        b.put(0, 1); // frame id numbers
        b.put(0, 1); // 128 sb
        for _ in 0..7 {
            b.put(0, 1); // filter intra .. order hint
        }
        b.put(1, 1); // choose screen content tools
        b.put(1, 1); // choose integer mv
        b.put(0, 1); // superres
        b.put(0, 1); // cdef
        b.put(0, 1); // restoration
        b.put(u32::from(hdr10), 1); // high bitdepth
        b.put(0, 1); // mono
        b.put(1, 1); // colour description
        let (cp, tc, mc) = if hdr10 { (9, 16, 9) } else { (1, 1, 1) };
        b.put(cp, 8);
        b.put(tc, 8);
        b.put(mc, 8);
        b.put(0, 1); // range
        b.put(0, 2); // chroma sample position
        b.put(0, 1); // separate uv delta q
        b.put(0, 1); // film grain
        b.put(1, 1); // trailing bit
        b.bytes
    }

    fn av1_access_unit(hdr10: bool) -> Vec<u8> {
        let seq = av1_payload(hdr10);
        let mut v = vec![0x12, 0x00, 0x0A, seq.len() as u8];
        v.extend(seq);
        v
    }

    fn av1_spec(pid: u16) -> EsSpec {
        EsSpec {
            pid,
            stream_type: 0x06,
            descriptors: reg(b"AV01"),
        }
    }

    fn opus_spec(pid: u16, lang: Option<&[u8; 3]>) -> EsSpec {
        let mut d = reg(b"Opus");
        d.extend_from_slice(&[0x7F, 0x02, 0x80, 0x02]);
        if let Some(l) = lang {
            d.extend_from_slice(&[0x0A, 0x04, l[0], l[1], l[2], 0x00]);
        }
        EsSpec {
            pid,
            stream_type: 0x06,
            descriptors: d,
        }
    }

    fn video_file(pts: &[u64]) -> Vec<u8> {
        let mut ts = Ts::default();
        ts.headers(&[av1_spec(0x100)]);
        for (i, p) in pts.iter().enumerate() {
            let payload = if i == 0 {
                av1_access_unit(false)
            } else {
                vec![0x12, 0x00]
            };
            ts.pes(0x100, *p, &payload);
        }
        ts.out
    }

    fn steps(start: u64, step: u64, n: usize) -> Vec<u64> {
        (0..n as u64).map(|i| start + i * step).collect()
    }

    #[test]
    fn av1_and_opus_streams_in_pid_order() {
        let mut ts = Ts::default();
        // Declared out of order: audio PID 0x200 first, video 0x100 second.
        ts.headers(&[opus_spec(0x200, Some(b"eng")), av1_spec(0x100)]);
        for i in 0..50u64 {
            ts.pes(0x100, 90_000 + i * 3600, &av1_access_unit(false));
            ts.pes(0x200, 90_000 + i * 1800, &[1, 2, 3]);
        }
        let c = run(&ts.out).unwrap();
        assert_eq!(c.format, "mpegts");
        assert_eq!(c.validity, ContainerValidity::Ok);
        assert_eq!(c.streams.len(), 2);
        let v = &c.streams[0];
        let a = &c.streams[1];
        assert_eq!((v.native_id, a.native_id), (0x100, 0x200));
        assert_eq!(v.stream.codec.as_deref(), Some("av1"));
        assert_eq!(a.stream.codec.as_deref(), Some("opus"));
        assert_eq!(v.stream.kind, StreamKind::Video);
        assert_eq!(a.stream.kind, StreamKind::Audio);
        assert_eq!(a.stream.language.as_deref(), Some("eng"));
        assert_eq!(v.stream.language, None);
        assert_eq!(a.stream.channels, Some(2));
        assert_eq!(a.stream.sample_rate, Some(48_000));
        assert_eq!(v.stream.metadata["pid"], "256");
        assert_eq!(v.stream.metadata["stream_type"], "0x06");
        assert_eq!(v.stream.width, Some(1920));
        assert_eq!(v.stream.height, Some(1080));
        assert_eq!(v.stream.pixel_format.as_deref(), Some("yuv420p"));
        assert_eq!(v.stream.codec_profile.as_deref(), Some("Main"));
        assert_eq!(v.stream.field_order, Some(FieldOrder::Progressive));
        let m = v.video.as_ref().unwrap();
        assert_eq!(m.colorspace.as_deref(), Some("bt709"));
        // 50 video frames, 25 fps: 49 * 3600 ticks = 1.96 s.
        assert_eq!(c.duration_ms, Some(1960));
        assert_eq!(c.timecode_present, None);
        assert_eq!(c.timestamps_contiguous, Some(true));
        assert_eq!(c.diagnostics["ts_continuity_errors"], 0);
        assert_eq!(c.diagnostics["ts_sync_losses"], 0);
    }

    #[test]
    fn hdr10_colour_is_read_from_the_sequence_header() {
        let mut ts = Ts::default();
        ts.headers(&[av1_spec(0x100)]);
        ts.pes(0x100, 0, &av1_access_unit(true));
        ts.pes(0x100, 3600, &[0x12, 0]);
        let c = run(&ts.out).unwrap();
        let s = &c.streams[0];
        assert_eq!(s.stream.pixel_format.as_deref(), Some("yuv420p10le"));
        let h = s.video.as_ref().unwrap().hdr.as_ref().unwrap();
        assert_eq!(h.transfer.as_deref(), Some("smpte2084"));
        assert_eq!(h.primaries.as_deref(), Some("bt2020"));
        assert_eq!(
            s.video.as_ref().unwrap().colorspace.as_deref(),
            Some("bt2020nc")
        );
    }

    #[test]
    fn frame_rate_25_and_ntsc() {
        let c = run(&video_file(&steps(1000, 3600, 30))).unwrap();
        let s = &c.streams[0];
        assert_eq!(s.stream.frame_rate, Rational::new(25, 1));
        assert_eq!(
            s.video.as_ref().unwrap().frame_rate_observed,
            Rational::new(25, 1)
        );
        assert_eq!(c.duration_ms, Some(29 * 3600 / 90));

        let c = run(&video_file(&steps(1000, 3003, 60))).unwrap();
        let s = &c.streams[0];
        assert_eq!(s.stream.frame_rate, Rational::new(30_000, 1001));
        assert_eq!(
            s.video.as_ref().unwrap().frame_rate_observed,
            Rational::new(30_000, 1001)
        );
    }

    #[test]
    fn a_pts_gap_is_reported() {
        let mut pts = steps(90_000, 3600, 10);
        pts.extend(steps(90_000 + 9 * 3600 + 90_000, 3600, 10));
        let c = run(&video_file(&pts)).unwrap();
        assert_eq!(c.timestamps_contiguous, Some(false));
        assert_eq!(c.timestamp_gaps.len(), 1);
        let g = c.timestamp_gaps[0];
        assert_eq!(g.start_ms, (90_000 + 9 * 3600) / 90);
        assert_eq!(g.end_ms, g.start_ms + 1000);
    }

    #[test]
    fn a_backward_jump_is_reported() {
        let mut pts = steps(900_000, 3600, 10);
        pts.extend(steps(90_000, 3600, 10));
        let c = run(&video_file(&pts)).unwrap();
        assert_eq!(c.timestamps_contiguous, Some(false));
        assert_eq!(c.timestamp_gaps.len(), 1);
        let g = c.timestamp_gaps[0];
        assert_eq!(g.start_ms, 90_000 / 90 - 40 + 40);
        assert!(g.end_ms > g.start_ms);
    }

    #[test]
    fn the_33_bit_wrap_is_not_a_gap() {
        let start = (1u64 << 33) - 5 * 3600;
        let pts: Vec<u64> = (0..12u64)
            .map(|i| (start + i * 3600) & ((1 << 33) - 1))
            .collect();
        let c = run(&video_file(&pts)).unwrap();
        assert_eq!(c.timestamps_contiguous, Some(true));
        assert!(c.timestamp_gaps.is_empty());
        assert_eq!(c.duration_ms, Some(11 * 3600 / 90));
        assert_eq!(c.streams[0].stream.frame_rate, Rational::new(25, 1));
    }

    #[test]
    fn continuity_errors_are_counted() {
        let mut bytes = video_file(&steps(1000, 3600, 20));
        // Packets 0 and 1 are PAT/PMT; video starts at 2. Drop two video packets.
        let first = 4 * PACKET;
        bytes.drain(first..first + 2 * PACKET);
        let c = run(&bytes).unwrap();
        assert_eq!(c.diagnostics["ts_continuity_errors"], 1);
        assert!(c
            .malformed_metadata
            .iter()
            .any(|m| m.contains("continuity-counter errors on PID 0x100")));
    }

    #[test]
    fn transport_errors_are_counted() {
        let mut bytes = video_file(&steps(1000, 3600, 10));
        bytes[5 * PACKET + 1] |= 0x80;
        let c = run(&bytes).unwrap();
        assert_eq!(c.diagnostics["ts_transport_errors"], 1);
    }

    #[test]
    fn garbage_between_packets_resyncs() {
        let clean = video_file(&steps(1000, 3600, 20));
        let mut bytes = clean[..8 * PACKET].to_vec();
        bytes.extend_from_slice(&[0x12; 100]);
        bytes.extend_from_slice(&clean[8 * PACKET..]);
        let c = run(&bytes).unwrap();
        assert_eq!(c.diagnostics["ts_sync_losses"], 1);
        assert_eq!(c.validity, ContainerValidity::Ok);
        assert_eq!(c.streams.len(), 1);
        assert_eq!(c.streams[0].stream.codec.as_deref(), Some("av1"));
        assert!(c.diagnostics["ts_length_not_multiple_of_188"] == true);
    }

    #[test]
    fn a_corrupt_pmt_crc_is_counted_and_a_later_good_pmt_is_used() {
        let mut ts = Ts::default();
        ts.psi(0, &pat_section());
        let mut bad = pmt_section(&[av1_spec(0x100)]);
        let last = bad.len() - 1;
        bad[last] ^= 0xFF;
        ts.psi(PMT_PID, &bad);
        ts.psi(PMT_PID, &pmt_section(&[av1_spec(0x100)]));
        ts.pes(0x100, 0, &av1_access_unit(false));
        ts.pes(0x100, 3600, &[0x12, 0]);
        let c = run(&ts.out).unwrap();
        assert_eq!(c.diagnostics["ts_psi_crc_errors"], 1);
        assert_eq!(c.validity, ContainerValidity::Ok);
        assert_eq!(c.streams.len(), 1);

        // Only a corrupt PMT: no usable program map.
        let mut ts = Ts::default();
        ts.psi(0, &pat_section());
        ts.psi(PMT_PID, &bad);
        let c = run(&ts.out).unwrap();
        assert!(matches!(c.validity, ContainerValidity::Corrupt(_)));
        assert_eq!(c.diagnostics["ts_psi_crc_errors"], 1);
    }

    #[test]
    fn patented_codecs_are_refused() {
        for stream_type in [0x1B, 0x0F, 0x02] {
            let mut ts = Ts::default();
            ts.headers(&[
                EsSpec {
                    pid: 0x100,
                    stream_type,
                    descriptors: Vec::new(),
                },
                opus_spec(0x200, None),
            ]);
            ts.pes(0x100, 0, &[0, 0, 0, 1, 0x67]);
            let e = run(&ts.out).unwrap_err().to_string();
            assert!(e.contains("royalty-free"), "{e}");
        }
    }

    #[test]
    fn passive_streams_are_listed_but_not_interpreted() {
        let mut ts = Ts::default();
        ts.headers(&[
            av1_spec(0x100),
            EsSpec {
                pid: 0x300,
                stream_type: 0x86,
                descriptors: Vec::new(),
            },
        ]);
        ts.pes(0x100, 0, &av1_access_unit(false));
        ts.pes(0x100, 3600, &[0x12, 0]);
        ts.pes(0x300, 0, &[1, 2, 3]);
        let c = run(&ts.out).unwrap();
        assert_eq!(c.streams.len(), 2);
        let p = &c.streams[1];
        assert_eq!(p.native_id, 0x300);
        assert_eq!(p.stream.kind, StreamKind::Data);
        assert_eq!(p.stream.codec.as_deref(), Some("stream_type_0x86"));
        assert!(p.video.is_none());
    }

    #[test]
    fn duration_falls_back_to_pcr() {
        let mut ts = Ts::default();
        ts.headers(&[EsSpec {
            pid: 0x300,
            stream_type: 0x86,
            descriptors: Vec::new(),
        }]);
        ts.pcr(0x100, 90_000);
        ts.pcr(0x100, 90_000 * 4);
        let c = run(&ts.out).unwrap();
        assert_eq!(c.duration_ms, Some(3000));
        assert!(c
            .malformed_metadata
            .iter()
            .any(|m| m.contains("0x300") && m.contains("never appears")));
    }

    #[test]
    fn no_pat_is_corrupt() {
        let mut ts = Ts::default();
        for i in 0..5u64 {
            ts.pes(0x100, i * 3600, &[1, 2, 3]);
        }
        let c = run(&ts.out).unwrap();
        assert!(matches!(c.validity, ContainerValidity::Corrupt(_)));
    }

    #[test]
    fn truncation_garbage_and_bit_flips_never_panic() {
        let mut ts = Ts::default();
        ts.headers(&[av1_spec(0x100), opus_spec(0x200, Some(b"fra"))]);
        for i in 0..8u64 {
            ts.pes(0x100, i * 3600, &av1_access_unit(i % 2 == 0));
            ts.pes(0x200, i * 1800, &[9; 40]);
        }
        let file = ts.out;
        for n in 0..file.len().min(4096) {
            let _ = run(&file[..n]);
        }
        // Pseudo-random garbage.
        let mut x = 0x1234_5678u32;
        for size in [0usize, 1, 187, 188, 189, 376, 1000, 5000, 20_000] {
            let mut g = Vec::with_capacity(size);
            for _ in 0..size {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                g.push((x >> 8) as u8);
            }
            let _ = run(&g);
            // Garbage with valid sync bytes.
            for chunk in g.chunks_mut(PACKET) {
                chunk[0] = 0x47;
            }
            let _ = run(&g);
        }
        // Byte flips.
        for i in 0..file.len() {
            let mut f = file.clone();
            f[i] ^= 0xA5;
            let _ = run(&f);
        }
    }
}
