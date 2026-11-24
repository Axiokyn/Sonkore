//! DSF and DSDIFF: FFmpeg's view (dsfdec.c, iff.c: rate / 8, the bit rate,
//! the ID3 tag of a DSF, DIIN title and artist of a DSDIFF) and Sonkore's
//! own reading of the exact length, which the probe
//! prefers because FFmpeg only estimates it.

use std::path::Path;

use super::{Codec, Demuxed, Dict, Src, be, ffid3, le};

/// A DSD file's format.
#[allow(dead_code)]
pub(crate) struct DsdFormat {
    pub rate: u32,
    pub channels: u32,
    pub bytes_per_channel: u64,
}

/// Sonkore's DSD probe for a .dsf / .dff file.
pub(crate) fn sonkore_dsd_probe(path: &Path) -> Option<DsdFormat> {
    let ext = path.extension()?.to_string_lossy().to_ascii_lowercase();
    if ext != "dsf" && ext != "dff" {
        return None;
    }
    let mut src = Src::open(path)?;
    parse_dsf(&mut src).or_else(|| parse_dff(&mut src))
}

fn parse_dsf(src: &mut Src) -> Option<DsdFormat> {
    let h = src.read_at(0, 28 + 52 + 12);
    if h.len() < 92 || &h[..4] != b"DSD " || &h[28..32] != b"fmt " {
        return None;
    }
    let fmt = &h[40..];
    let fmt_size = le(&h, 32, 8);
    if le(fmt, 4, 4) != 0 {
        return None;
    }
    let channels = le(fmt, 12, 4) as u32;
    let rate = le(fmt, 16, 4) as u32;
    let bits = le(fmt, 20, 4) as u32;
    let samples = le(fmt, 24, 8);
    let block = le(fmt, 32, 4) as u32;
    let data = src.read_at(28 + fmt_size, 12);
    if data.len() < 12 || &data[..4] != b"data" {
        return None;
    }
    if channels == 0 || channels > 8 || rate < 2822400 || block == 0 || (bits != 1 && bits != 8) {
        return None;
    }
    Some(DsdFormat { rate, channels, bytes_per_channel: samples / 8 })
}

fn parse_dff(src: &mut Src) -> Option<DsdFormat> {
    let h = src.read_at(0, 16);
    if h.len() < 16 || &h[..4] != b"FRM8" || &h[12..16] != b"DSD " {
        return None;
    }
    let end = be(&h, 4, 8).saturating_add(12);
    let (mut rate, mut channels, mut compressed) = (0u32, 0u32, false);
    let mut pos = 16u64;
    let mut dst: Option<DsdFormat> = None;
    while pos.saturating_add(12) <= end {
        let c = src.read_at(pos, 12);
        if c.len() < 12 {
            return dst;
        }
        let size = be(&c, 4, 8);
        let body = pos + 12;
        match &c[..4] {
            b"PROP" => {
                let mut sub = body + 4;
                while sub.saturating_add(12) <= body.saturating_add(size) {
                    let s = src.read_at(sub, 16);
                    if s.len() < 16 {
                        return None;
                    }
                    let sub_size = be(&s, 4, 8);
                    match &s[..4] {
                        b"FS  " => rate = be(&s, 12, 4) as u32,
                        b"CHNL" => channels = be(&s, 12, 2) as u32,
                        b"CMPR" => compressed = &s[12..16] != b"DSD ",
                        _ => {}
                    }
                    let Some(next) = after_chunk(sub, sub_size) else { break };
                    sub = next;
                }
            }
            b"DSD " => {
                if channels == 0 || channels > 8 || rate < 2822400 || compressed {
                    return None;
                }
                return Some(DsdFormat { rate, channels, bytes_per_channel: size / channels as u64 });
            }
            b"DST " => {
                let f = src.read_at(body, 18);
                if channels == 0
                    || channels > 6
                    || rate < 2822400
                    || rate % 44100 != 0
                    || f.len() < 18
                    || &f[..4] != b"FRTE"
                {
                    return None;
                }
                let frames = be(&f, 12, 4);
                let per_frame = 588 * (rate as u64 / 44100) / 8;
                dst = Some(DsdFormat { rate, channels, bytes_per_channel: frames * per_frame });
                return dst;
            }
            _ => {}
        }
        let Some(next) = after_chunk(pos, size) else { break };
        pos = next;
    }
    dst
}

/// Where the chunk at `pos` with a body of `size` bytes ends (its pad byte
/// included); None past the end of the address space.
fn after_chunk(pos: u64, size: u64) -> Option<u64> {
    pos.checked_add(12)?.checked_add(size)?.checked_add(size & 1)
}

pub(super) fn read_dsf(src: &mut Src) -> Option<Demuxed> {
    let h = src.read_at(0, 28 + 52 + 12);
    if h.len() < 92 || le(&h, 4, 8) != 28 || &h[28..32] != b"fmt " || le(&h, 32, 8) != 52 {
        return None;
    }
    let fmt = &h[40..];
    if le(fmt, 0, 4) != 1 || le(fmt, 4, 4) != 0 {
        return None;
    }
    let channels = le(fmt, 12, 4) as u32;
    let rate = le(fmt, 16, 4) as u32 / 8;
    let codec_ok = matches!(le(fmt, 20, 4), 1 | 8);
    if rate == 0 || channels == 0 || !codec_ok {
        return None;
    }
    let mut d = Demuxed::new(Codec::Dsd, "dsf");
    d.sample_rate = rate;
    d.channels = channels;
    d.stream_bit_rate = channels as i64 * 8 * rate as i64;
    d.time_base = (1, rate as i64);
    let id3_pos = le(&h, 20, 8);
    if id3_pos > 0 && id3_pos < src.size {
        let head = src.read_at(id3_pos, 10);
        let size = super::id3v2_size(&head);
        if size > 0 {
            let tag = src.read_at(id3_pos, size);
            let mut meta = Dict::default();
            ffid3::read_dict(&tag, &mut meta, Some(&mut d.pictures));
            d.format_tags = meta;
        }
    }
    // FFmpeg's own estimate, used when Sonkore's reading of the length fails.
    let data_size = le(&h, 80 + 4, 8).saturating_sub(12);
    if d.stream_bit_rate > 0 {
        d.format_duration_us = Some(super::rescale(data_size as i64, 8_000_000, d.stream_bit_rate));
    }
    Some(d)
}

pub(super) fn read_dff(src: &mut Src) -> Option<Demuxed> {
    let h = src.read_at(0, 16);
    if h.get(12..16) != Some(b"DSD ") {
        return None;
    }
    let end = be(&h, 4, 8).saturating_add(12).min(src.size);
    let mut d = Demuxed::new(Codec::Dsd, "iff");
    let mut pos = 16u64;
    let mut meta = Dict::default();
    let mut data_size = 0u64;
    while pos.saturating_add(12) <= end {
        let c = src.read_at(pos, 12);
        if c.len() < 12 {
            break;
        }
        let size = be(&c, 4, 8);
        let body = pos + 12;
        match &c[..4] {
            b"PROP" => {
                let mut sub = body + 4;
                while sub.saturating_add(12) <= body.saturating_add(size) {
                    let s = src.read_at(sub, 20);
                    if s.len() < 12 {
                        break;
                    }
                    let sub_size = be(&s, 4, 8);
                    match &s[..4] {
                        b"FS  " => d.sample_rate = be(&s, 12, 4) as u32 / 8,
                        b"CHNL" => d.channels = be(&s, 12, 2) as u32,
                        b"CMPR" if s.get(12..16) == Some(b"DST ") => {
                            d.codec = Codec::Other("dst".into());
                        }
                        _ => {}
                    }
                    let Some(next) = after_chunk(sub, sub_size) else { break };
                    sub = next;
                }
            }
            b"DIIN" => {
                let mut sub = body;
                while sub.saturating_add(12) <= body.saturating_add(size) {
                    let s = src.read_at(sub, 16);
                    if s.len() < 12 {
                        break;
                    }
                    let sub_size = be(&s, 4, 8);
                    let key = match &s[..4] {
                        b"DIAR" => Some("artist"),
                        b"DITI" => Some("title"),
                        _ => None,
                    };
                    if let (Some(key), true) = (key, sub_size > 4) {
                        let len = be(&s, 12, 4).min(sub_size - 4);
                        let text = src.read_at(sub + 16, len);
                        meta.set(key, &String::from_utf8_lossy(&text));
                    }
                    let Some(next) = after_chunk(sub, sub_size) else { break };
                    sub = next;
                }
            }
            b"DSD " | b"DST " => {
                data_size = size;
                break;
            }
            _ => {}
        }
        let Some(next) = after_chunk(pos, size) else { break };
        pos = next;
    }
    if d.sample_rate == 0 || d.channels == 0 {
        return None;
    }
    d.bits_coded = 8;
    d.stream_bit_rate = d.channels as i64 * d.sample_rate as i64 * 8;
    d.time_base = (1, d.sample_rate as i64);
    if d.stream_bit_rate > 0 {
        d.format_duration_us = Some(super::rescale(data_size as i64, 8_000_000, d.stream_bit_rate));
    }
    d.format_tags = meta;
    Some(d)
}
