//! Diagnostic: drive the Kinetix decoder directly on an MP4/MKV file and print
//! what each packet produces. `cargo run -p tpt-app-media-qc-decode --example frame_diag -- <file>`

use tpt_kinetix_av1::Av1Decoder;
use tpt_kinetix_core::codec::CodecId;
use tpt_kinetix_core::packet::Packet;
use tpt_kinetix_demux::{Demuxer, MkvDemuxer, Mp4Demuxer};

fn obu_types(data: &[u8]) -> Vec<u8> {
    // Walk low-overhead-format OBUs: header, optional ext, leb128 size.
    let mut out = Vec::new();
    let mut i = 0;
    while i < data.len() && out.len() < 12 {
        let h = data[i];
        let ty = (h >> 3) & 0xF;
        let ext = (h >> 2) & 1;
        let has_size = (h >> 1) & 1;
        i += 1 + usize::from(ext);
        let mut size = 0usize;
        if has_size == 1 {
            let mut shift = 0;
            loop {
                let Some(b) = data.get(i) else { return out };
                i += 1;
                size |= usize::from(b & 0x7F) << shift;
                shift += 7;
                if b & 0x80 == 0 || shift > 35 {
                    break;
                }
            }
        } else {
            size = data.len() - i;
        }
        out.push(ty);
        i += size;
    }
    out
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: frame_diag <file>");
    let bytes = std::fs::read(&path).unwrap();
    let mut demuxer: Box<dyn Demuxer>;
    let mut stream = 0u32;
    if bytes.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        let d = MkvDemuxer::new(bytes).unwrap();
        for t in d.tracks() {
            println!("mkv track {} codec {} type {:?}", t.track_number, t.codec_id, t.track_type);
            if t.codec_id == "V_AV1" {
                stream = t.track_number as u32;
            }
        }
        demuxer = Box::new(d);
    } else {
        let d = Mp4Demuxer::new(bytes).unwrap();
        for (i, t) in d.tracks().iter().enumerate() {
            println!("mp4 track {i} id {} codec {:?} samples {}", t.track_id, t.codec, t.sample_count());
            if t.codec == Some(CodecId::Av1) {
                stream = i as u32;
            }
        }
        demuxer = Box::new(d);
    }
    let mut decoder = Av1Decoder::new().with_strict(true);
    let mut n = 0;
    let mut prev: Option<u64> = None;
    while let Some(p) = demuxer.read_packet().unwrap() {
        let p: Packet = p;
        if p.stream_index != stream {
            continue;
        }
        let obus = obu_types(&p.data);
        match decoder.decode(&p) {
            Ok(Some(f)) => {
                let sum: u64 = f.data.iter().take(f.width as usize * f.height as usize).map(|b| u64::from(*b)).sum();
                let hash = f.data.iter().fold(0xcbf29ce484222325u64, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x100000001b3));
                println!(
                    "pkt {n:3} len {:6} key {} obus {:?} -> frame {}x{} {:?} meanY {:.1} hash {hash:016x} {}",
                    p.data.len(), p.is_key_frame, obus, f.width, f.height, f.pixel_format,
                    sum as f64 / (f.width as f64 * f.height as f64),
                    if prev == Some(hash) { "IDENTICAL-TO-PREV" } else { "" }
                );
                prev = Some(hash);
            }
            Ok(None) => println!("pkt {n:3} len {:6} obus {:?} -> no frame", p.data.len(), obus),
            Err(e) => println!("pkt {n:3} len {:6} obus {:?} -> ERROR {e}", p.data.len(), obus),
        }
        n += 1;
        if n >= 8 {
            break;
        }
    }
    println!("sequence header known: {}", decoder.sequence_header().is_some());
}
