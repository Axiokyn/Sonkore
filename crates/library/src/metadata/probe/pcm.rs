//! AIFF / AIFF-C (aiffdec.c) and Sony Wave64 (wavdec.c's w64 demuxer).

use super::{Codec, Demuxed, Dict, Src, be, ffid3, le};

/// The 80-bit IEEE extended sample rate, as aiffdec.c turns it into an int.
fn extended(d: &[u8]) -> u32 {
    if d.len() < 10 {
        return 0;
    }
    let exp = (be(d, 0, 2) & 0x7fff) as i32;
    let mantissa = be(d, 2, 8);
    let v = (mantissa as f64) * 2f64.powi(exp - 16383 - 63);
    if v.is_finite() && v > 0.0 { v as u32 } else { 0 }
}

pub(super) fn read_aiff(src: &mut Src, start: u64) -> Option<Demuxed> {
    let h = src.read_at(start, 12);
    let aifc = &h[8..12] == b"AIFC";
    let end = (start + 8 + be(&h, 4, 4)).min(src.size);
    let mut pos = start + 12;
    let mut d = Demuxed::new(Codec::Pcm, "aiff");
    let mut have_comm = false;
    let mut meta = Dict::default();
    while pos + 8 <= end {
        let c = src.read_at(pos, 8);
        if c.len() < 8 {
            break;
        }
        let size = be(&c, 4, 4);
        let body = pos + 8;
        match &c[..4] {
            b"COMM" => {
                let b = src.read_at(body, size.min(64));
                if b.len() < 18 {
                    return None;
                }
                d.channels = be(&b, 0, 2) as u32;
                d.duration_ts = Some(be(&b, 2, 4) as i64);
                d.bits_coded = be(&b, 6, 2) as i32;
                d.sample_rate = extended(&b[8..18]);
                if aifc && b.len() >= 22 {
                    match &b[18..22] {
                        b"NONE" | b"twos" | b"sowt" | b"in24" | b"in32" | b"raw " => {}
                        b"fl32" | b"FL32" => d.bits_coded = 32,
                        b"fl64" | b"FL64" => d.bits_coded = 64,
                        b"alaw" | b"ulaw" | b"ALAW" | b"ULAW" => {
                            d.codec = Codec::PcmLossy;
                            d.bits_coded = 8;
                        }
                        other => d.codec = Codec::Other(String::from_utf8_lossy(other).trim().to_lowercase()),
                    }
                }
                have_comm = true;
            }
            b"NAME" | b"AUTH" | b"(c) " | b"ANNO" => {
                let key = match &c[..4] {
                    b"NAME" => "title",
                    b"AUTH" => "author",
                    b"(c) " => "copyright",
                    _ => "comment",
                };
                let text = src.read_at(body, size.min(1 << 16));
                let end = text.iter().position(|&b| b == 0).unwrap_or(text.len());
                meta.set(key, &String::from_utf8_lossy(&text[..end]));
            }
            b"ID3 " | b"id3 " => {
                let tag = src.read_at(body, size.min(64 << 20));
                ffid3::read_dict(&tag, &mut meta, Some(&mut d.pictures));
            }
            _ => {}
        }
        pos = body + size + (size & 1);
    }
    if !have_comm {
        return None;
    }
    d.time_base = (1, d.sample_rate.max(1) as i64);
    d.format_tags = meta;
    Some(d)
}

pub(super) const W64_RIFF: [u8; 16] =
    [b'r', b'i', b'f', b'f', 0x2E, 0x91, 0xCF, 0x11, 0xA5, 0xD6, 0x28, 0xDB, 0x04, 0xC1, 0x00, 0x00];
const W64_SUFFIX: [u8; 12] = [0xF3, 0xAC, 0xD3, 0x11, 0x8C, 0xD1, 0x00, 0xC0, 0x4F, 0x8E, 0xDB, 0x8A];

/// WAVEFORMAT(EX): the codec, channels, rate, block align and bits.
pub(super) fn wave_format(f: &[u8], d: &mut Demuxed) -> u32 {
    if f.len() < 16 {
        return 0;
    }
    let mut tag = le(f, 0, 2);
    d.channels = le(f, 2, 2) as u32;
    d.sample_rate = le(f, 4, 4) as u32;
    d.stream_bit_rate = le(f, 8, 4) as i64 * 8;
    let align = le(f, 12, 2) as u32;
    d.bits_coded = le(f, 14, 2) as i32;
    if tag == 0xFFFE && f.len() >= 40 {
        tag = le(f, 24, 2); // the sub-format GUID's first word
    }
    d.codec = match tag {
        1 | 3 => Codec::Pcm,
        6 | 7 => Codec::PcmLossy,
        0x55 => Codec::Mp3,
        0x50 => Codec::Mp2,
        0x2000 => Codec::Ac3,
        0x2001 => Codec::Dts,
        0x161 => Codec::Wma,
        0x162 => Codec::WmaPro,
        0x163 => Codec::WmaLossless,
        0xF1AC => Codec::Flac,
        other => Codec::Other(format!("wav_{other:04x}")),
    };
    align
}

pub(super) fn read_w64(src: &mut Src, start: u64) -> Option<Demuxed> {
    let mut pos = start + 40;
    let mut d = Demuxed::new(Codec::Pcm, "w64");
    let mut align = 0u32;
    let mut have_fmt = false;
    while pos.saturating_add(24) <= src.size {
        let c = src.read_at(pos, 24);
        let size = le(&c, 16, 8);
        if size < 24 || c[4..16] != W64_SUFFIX {
            break;
        }
        match &c[..4] {
            b"fmt " => {
                let f = src.read_at(pos + 24, (size - 24).min(256));
                align = wave_format(&f, &mut d);
                have_fmt = true;
            }
            b"data" if align > 0 => {
                d.duration_ts = Some(((size - 24) / align as u64) as i64);
            }
            _ => {}
        }
        // Padded to 8 bytes; a size past the end of the address space ends it.
        let Some(next) = size.checked_add(7).and_then(|s| pos.checked_add(s & !7)) else { break };
        pos = next;
    }
    if !have_fmt {
        return None;
    }
    d.time_base = (1, d.sample_rate.max(1) as i64);
    Some(d)
}
