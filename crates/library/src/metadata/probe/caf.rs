//! Apple CAF (libavformat/cafdec.c): the "desc" chunk, the packet table's
//! valid frames for the length, the "info" strings as tags.

use super::{Codec, Demuxed, Dict, Src, be};

pub(super) fn read(src: &mut Src, start: u64) -> Option<Demuxed> {
    let mut pos = start + 8;
    let mut d = Demuxed::new(Codec::Pcm, "caf");
    let mut have_desc = false;
    let (mut bytes_per_packet, mut frames_per_packet) = (0u64, 0u64);
    let mut data_size: Option<u64> = None;
    let mut valid_frames: Option<i64> = None;
    let mut meta = Dict::default();
    while pos + 12 <= src.size {
        let c = src.read_at(pos, 12);
        let raw = be(&c, 4, 8) as i64;
        let body = pos + 12;
        let size = if raw < 0 { src.size - body } else { raw as u64 };
        match &c[..4] {
            b"desc" => {
                let b = src.read_at(body, 32);
                if b.len() < 32 {
                    return None;
                }
                d.sample_rate = f64::from_bits(be(&b, 0, 8)) as u32;
                let format = &b[8..12];
                let flags = be(&b, 12, 4);
                bytes_per_packet = be(&b, 16, 4);
                frames_per_packet = be(&b, 20, 4);
                d.channels = be(&b, 24, 4) as u32;
                d.bits_coded = be(&b, 28, 4) as i32;
                d.codec = match format {
                    b"lpcm" => {
                        let _float = flags & 1 != 0;
                        Codec::Pcm
                    }
                    b"alac" => Codec::Alac,
                    b"aac " => Codec::Aac,
                    b"opus" => Codec::Opus,
                    b"flac" => Codec::Flac,
                    b".mp3" => Codec::Mp3,
                    b"ulaw" | b"alaw" => Codec::PcmLossy,
                    b"ac-3" => Codec::Ac3,
                    other => Codec::Other(String::from_utf8_lossy(other).trim().to_lowercase()),
                };
                have_desc = true;
            }
            b"data" => {
                data_size = Some(size.saturating_sub(4));
            }
            b"pakt" => {
                let b = src.read_at(body, 24);
                if b.len() >= 16 {
                    valid_frames = Some(be(&b, 8, 8) as i64);
                }
            }
            b"info" => {
                let b = src.read_at(body, size.min(1 << 20));
                let count = be(&b, 0, 4);
                let mut p = 4;
                for _ in 0..count {
                    let Some(k_end) = b[p.min(b.len())..].iter().position(|&x| x == 0) else {
                        break;
                    };
                    let key = String::from_utf8_lossy(&b[p..p + k_end]).into_owned();
                    p += k_end + 1;
                    let Some(v_end) = b[p.min(b.len())..].iter().position(|&x| x == 0) else {
                        break;
                    };
                    let value = String::from_utf8_lossy(&b[p..p + v_end]).into_owned();
                    p += v_end + 1;
                    meta.set(&key, &value);
                }
            }
            _ => {}
        }
        if raw < 0 {
            break;
        }
        pos = body + size;
    }
    if !have_desc {
        return None;
    }
    d.time_base = (1, d.sample_rate.max(1) as i64);
    if let Some(v) = valid_frames.filter(|&v| v > 0) {
        d.duration_ts = Some(v);
    } else if let (Some(size), true) = (data_size, bytes_per_packet > 0 && frames_per_packet > 0) {
        d.duration_ts = Some((size / bytes_per_packet * frames_per_packet) as i64);
    }
    if bytes_per_packet > 0 && frames_per_packet > 0 {
        d.stream_bit_rate = d.sample_rate as i64 * bytes_per_packet as i64 * 8 / frames_per_packet as i64;
    }
    d.format_tags = meta;
    Some(d)
}
