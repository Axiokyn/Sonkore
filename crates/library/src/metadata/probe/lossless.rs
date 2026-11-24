//! Monkey's Audio, WavPack, TTA, Musepack and TAK, as FFmpeg's demuxers
//! read their headers (ape.c, wvdec.c, tta.c, mpc.c / mpc8.c, takdec.c),
//! with their APEv2 / ID3v1 tags.

use super::{Codec, Demuxed, Src, apev2, le, read_id3v1};

fn tags(src: &mut Src, d: &mut Demuxed, id3v1_when_empty: bool) {
    let mut meta = std::mem::take(&mut d.format_tags);
    apev2::read(src, &mut meta, &mut d.pictures);
    if id3v1_when_empty && meta.is_empty() {
        read_id3v1(src, &mut meta);
    }
    d.format_tags = meta;
}

pub(super) fn read_ape(src: &mut Src, start: u64) -> Option<Demuxed> {
    let h = src.read_at(start, 128);
    let version = le(&h, 4, 2);
    if !(3800..=3990).contains(&version) {
        return None;
    }
    let (bps, channels, rate, blocks_per_frame, final_blocks, total_frames);
    if version >= 3980 {
        let descriptor = le(&h, 8, 4) as usize;
        let at = descriptor.max(52); // the header follows the descriptor (which starts at "MAC ")
        let hdr = src.read_at(start + at as u64, 24);
        if hdr.len() < 24 {
            return None;
        }
        blocks_per_frame = le(&hdr, 4, 4);
        final_blocks = le(&hdr, 8, 4);
        total_frames = le(&hdr, 12, 4);
        bps = le(&hdr, 16, 2);
        channels = le(&hdr, 18, 2);
        rate = le(&hdr, 20, 4);
    } else {
        let compression = le(&h, 6, 2);
        let flags = le(&h, 8, 2);
        channels = le(&h, 10, 2);
        rate = le(&h, 12, 4);
        total_frames = le(&h, 24, 4);
        final_blocks = le(&h, 28, 4);
        bps = if flags & 1 != 0 {
            8
        } else if flags & 8 != 0 {
            24
        } else {
            16
        };
        blocks_per_frame = if version >= 3950 {
            73728 * 4
        } else if version >= 3900 || (version >= 3800 && compression >= 4000) {
            73728
        } else {
            9216
        };
    }
    if channels == 0 || rate == 0 {
        return None;
    }
    let total = if total_frames == 0 { 0 } else { (total_frames - 1) * blocks_per_frame + final_blocks };
    let mut d = Demuxed::new(Codec::Ape, "ape");
    d.sample_rate = rate as u32;
    d.channels = channels as u32;
    d.bits_coded = bps as i32;
    d.duration_ts = Some(total as i64);
    d.time_base = (1, rate as i64);
    tags(src, &mut d, false);
    Some(d)
}

const WV_RATES: [i64; 16] =
    [6000, 8000, 9600, 11025, 12000, 16000, 22050, 24000, 32000, 44100, 48000, 64000, 88200, 96000, 192000, -1];

pub(super) fn read_wv(src: &mut Src, start: u64) -> Option<Demuxed> {
    let h = src.read_at(start, 32);
    if h.len() < 32 {
        return None;
    }
    let block_size = le(&h, 4, 4);
    let total = le(&h, 12, 4);
    let flags = le(&h, 24, 4);
    let bpp = ((flags & 3) + 1) << 3;
    let mut channels = 1 + u64::from(flags & 4 == 0);
    let mut rate = WV_RATES[((flags >> 23) & 0xf) as usize];
    let single = flags & 0x1800 == 0x1800;
    if !single || rate == -1 {
        // The metadata sub-blocks: channel info (0x0d), the rate (0x27).
        let body = src.read_at(start + 32, block_size.saturating_sub(24).min(1 << 20));
        let mut p = 0usize;
        while p + 2 <= body.len() {
            let id = body[p];
            let mut size = (body[p + 1] as usize) << 1;
            p += 2;
            if id & 0x80 != 0 {
                if p + 2 > body.len() {
                    break;
                }
                size |= ((body[p] as usize) << 9) | ((body[p + 1] as usize) << 17);
                p += 2;
            }
            if p + size > body.len() {
                break;
            }
            let real = if id & 0x40 != 0 { size.saturating_sub(1) } else { size };
            let data = &body[p..p + real];
            match id & 0x3f {
                0x0d if !data.is_empty() => {
                    if real >= 6 && data.len() >= 6 {
                        channels = (le(data, 0, 2) & 0xfff) + 1;
                    } else {
                        channels = data[0] as u64;
                    }
                }
                0x27 if real >= 3 => {
                    rate = le(data, 0, 3) as i64;
                }
                _ => {}
            }
            p += size;
        }
    }
    if rate <= 0 || channels == 0 {
        return None;
    }
    let mut d = Demuxed::new(Codec::WavPack, "wv");
    d.sample_rate = rate as u32;
    d.channels = channels as u32;
    d.bits_coded = bpp as i32;
    d.time_base = (1, rate);
    if total != 0xFFFF_FFFF {
        d.duration_ts = Some(total as i64);
    }
    tags(src, &mut d, true);
    Some(d)
}

pub(super) fn read_tta(src: &mut Src, start: u64) -> Option<Demuxed> {
    let h = src.read_at(start, 22);
    if h.len() < 22 {
        return None;
    }
    let channels = le(&h, 6, 2);
    let bps = le(&h, 8, 2);
    let rate = le(&h, 10, 4);
    let samples = le(&h, 14, 4);
    if channels == 0 || rate == 0 {
        return None;
    }
    let mut d = Demuxed::new(Codec::Tta, "tta");
    d.sample_rate = rate as u32;
    d.channels = channels as u32;
    d.bits_coded = bps as i32;
    d.duration_ts = Some(samples as i64);
    d.time_base = (1, rate as i64);
    // ID3v1 first, then the APE tag over it.
    let mut meta = super::Dict::default();
    read_id3v1(src, &mut meta);
    apev2::read(src, &mut meta, &mut d.pictures);
    d.format_tags = meta;
    Some(d)
}

/// ffio_read_varlen.
fn varlen(d: &[u8], p: &mut usize) -> u64 {
    let mut v = 0u64;
    while *p < d.len() {
        let b = d[*p];
        *p += 1;
        v = (v << 7) | (b & 0x7f) as u64;
        if b & 0x80 == 0 {
            break;
        }
    }
    v
}

pub(super) fn read_mpc(src: &mut Src, start: u64) -> Option<Demuxed> {
    let h = src.read_at(start, 64);
    let mut d = Demuxed::new(Codec::Musepack, "mpc");
    d.channels = 2;
    d.bits_coded = 16;
    if h.starts_with(b"MP+") {
        let ver = h.get(3).copied()? & 0x0f;
        if ver != 7 {
            return None;
        }
        let frames = le(&h, 4, 4);
        const RATES: [u32; 4] = [44100, 48000, 37800, 32000];
        d.sample_rate = RATES[(h.get(10).copied()? & 3) as usize];
        d.time_base = (1152, d.sample_rate as i64);
        d.duration_ts = Some(frames as i64);
        tags(src, &mut d, true);
        return Some(d);
    }
    // SV8: packets "XX" + varlen size; the stream header "SH".
    let mut pos = start + 4;
    for _ in 0..64 {
        let chunk = src.read_at(pos, 64);
        if chunk.len() < 3 {
            return None;
        }
        let mut p = 2;
        let size = varlen(&chunk, &mut p);
        if &chunk[..2] == b"SH" {
            let mut q = p + 5; // CRC, version
            let samples = varlen(&chunk, &mut q);
            let _silence = varlen(&chunk, &mut q);
            if q + 2 > chunk.len() {
                return None;
            }
            const RATES: [i64; 8] = [44100, 48000, 37800, 32000, -1, -1, -1, -1];
            let rate = RATES[(chunk[q] >> 5) as usize];
            if rate <= 0 {
                return None;
            }
            d.sample_rate = rate as u32;
            d.channels = (chunk[q + 1] >> 4) as u32 + 1;
            let per = 1152i64 << ((chunk[q + 1] & 3) * 2);
            d.time_base = (per, rate);
            d.duration_ts = Some(samples as i64 / per);
            tags(src, &mut d, false);
            return Some(d);
        }
        if size == 0 {
            return None;
        }
        pos = pos.checked_add(size)?;
    }
    None
}

/// Little-endian bit reader (TAK's).
struct BitsLe<'a> {
    d: &'a [u8],
    at: usize,
}

impl BitsLe<'_> {
    fn get(&mut self, n: usize) -> u64 {
        let mut v = 0u64;
        for i in 0..n {
            let byte = self.d.get(self.at / 8).copied().unwrap_or(0);
            let bit = (byte >> (self.at % 8)) & 1;
            v |= (bit as u64) << i;
            self.at += 1;
        }
        v
    }
}

pub(super) fn read_tak(src: &mut Src, start: u64) -> Option<Demuxed> {
    let mut pos = start + 4;
    loop {
        let h = src.read_at(pos, 4);
        if h.len() < 4 {
            return None;
        }
        let kind = h[0] & 0x7f;
        let size = le(&h, 1, 3);
        if kind == 0 {
            return None; // the end, without a stream info block
        }
        if kind == 1 {
            let data = src.read_at(pos + 4, size.min(64));
            let mut b = BitsLe { d: &data, at: 0 };
            b.get(6); // codec
            b.get(4); // profile
            b.get(4); // frame duration
            let samples = b.get(35);
            b.get(3); // data type
            let rate = b.get(18) + 6000;
            let bps = b.get(5) + 8;
            let channels = b.get(4) + 1;
            let mut d = Demuxed::new(Codec::Tak, "tak");
            d.sample_rate = rate as u32;
            d.channels = channels as u32;
            d.bits_coded = bps as i32;
            d.time_base = (1, rate as i64);
            d.duration_ts = Some(samples as i64);
            tags(src, &mut d, false);
            return Some(d);
        }
        pos += 4 + size;
    }
}
