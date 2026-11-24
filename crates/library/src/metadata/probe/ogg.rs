//! Ogg (Vorbis, Opus, FLAC, Speex) as FFmpeg's ogg demuxer reads it
//! (oggdec.c, oggparse*.c): the stream headers, the comments with
//! ff_vorbis_comment's rules (repeated keys joined with ";", pictures from
//! METADATA_BLOCK_PICTURE, then FFmpeg's names), and the length from the
//! last page's granule position (ogg_get_length).

use base64::Engine;

use super::{Codec, Demuxed, Dict, Picture, Src, le};
use crate::metadata::native::flac_picture;

/// ff_vorbiscomment_metadata_conv.
const VORBIS_CONV: &[(&str, &str)] =
    &[("ALBUMARTIST", "album_artist"), ("TRACKNUMBER", "track"), ("DISCNUMBER", "disc"), ("DESCRIPTION", "comment")];

/// ff_vorbis_comment: a comment block (vendor, count, entries).
pub(crate) fn vorbis_comment(d: &[u8], meta: &mut Dict, pictures: &mut Vec<Picture>) {
    if d.len() < 8 {
        return;
    }
    let mut p = 4 + le(d, 0, 4) as usize;
    if p + 4 > d.len() {
        return;
    }
    let count = le(d, p, 4);
    p += 4;
    let mut local = Dict::default();
    for _ in 0..count {
        if p + 4 > d.len() {
            break;
        }
        let len = le(d, p, 4) as usize;
        p += 4;
        if p + len > d.len() {
            break;
        }
        let entry = &d[p..p + len];
        p += len;
        let Some(eq) = entry.iter().position(|&b| b == b'=') else {
            continue;
        };
        let (key, value) = (&entry[..eq], &entry[eq + 1..]);
        if key.is_empty() || value.is_empty() {
            continue;
        }
        let key = String::from_utf8_lossy(key).into_owned();
        let value = String::from_utf8_lossy(value).into_owned();
        if key.eq_ignore_ascii_case("METADATA_BLOCK_PICTURE") {
            if let Ok(block) = base64::engine::general_purpose::STANDARD.decode(value.trim())
                && let Some((ptype, data)) = flac_picture(&block)
            {
                pictures.push(Picture { data: data.to_vec(), front: ptype == 3 });
            }
            continue;
        }
        // OGM chapters ("CHAPTER01=", "CHAPTER01NAME=") are not tags.
        let upper = key.to_ascii_uppercase();
        if upper.len() >= 9 && upper.starts_with("CHAPTER") && upper.as_bytes()[7..9].iter().all(u8::is_ascii_digit) {
            continue;
        }
        local.append(&key, &value);
    }
    local.convert(VORBIS_CONV);
    for (k, v) in local.0 {
        meta.set(&k, &v);
    }
}

/// An Opus packet's length in samples at 48 kHz (RFC 6716, the TOC byte).
fn opus_samples(p: &[u8]) -> i64 {
    let Some(&toc) = p.first() else { return 0 };
    let config = toc >> 3;
    let frame: i64 = match config {
        0..=11 => [480, 960, 1920, 2880][(config % 4) as usize],
        12..=15 => [480, 960][(config % 2) as usize],
        _ => [120, 240, 480, 960][(config % 4) as usize],
    };
    let frames = match toc & 3 {
        0 => 1,
        1 | 2 => 2,
        _ => p.get(1).map(|b| (b & 0x3f) as i64).unwrap_or(0),
    };
    frame * frames
}

struct Page {
    serial: u32,
    granule: i64,
    flags: u8,
    /// (packet bytes, complete)
    segments: Vec<(Vec<u8>, bool)>,
}

/// The page at `pos` and where the next one starts.
fn page(src: &mut Src, pos: u64) -> Option<(Page, u64)> {
    let h = src.read_at(pos, 27 + 255);
    if h.len() < 27 || &h[..4] != b"OggS" {
        return None;
    }
    let nseg = h[26] as usize;
    if h.len() < 27 + nseg {
        return None;
    }
    let lacing = &h[27..27 + nseg];
    let body_len: u64 = lacing.iter().map(|&l| l as u64).sum();
    let body = src.read_at(pos + 27 + nseg as u64, body_len);
    let mut segments = Vec::new();
    let mut cur = Vec::new();
    let mut at = 0usize;
    for &l in lacing {
        let end = (at + l as usize).min(body.len());
        cur.extend_from_slice(&body[at..end]);
        at = end;
        if l < 255 {
            segments.push((std::mem::take(&mut cur), true));
        }
    }
    if !cur.is_empty() || lacing.last() == Some(&255) {
        segments.push((cur, false));
    }
    let granule = le(&h, 6, 8) as i64;
    Some((Page { serial: le(&h, 14, 4) as u32, granule, flags: h[5], segments }, pos + 27 + nseg as u64 + body_len))
}

pub(super) fn read(src: &mut Src, start: u64) -> Option<Demuxed> {
    // The first packets of the first audio stream, and the first page that
    // carries audio (granule > 0): its packets give the first timestamp.
    let mut pos = start;
    let mut serial = None;
    let mut packets: Vec<Vec<u8>> = Vec::new();
    let mut partial: Vec<u8> = Vec::new();
    let mut first_audio: Option<(i64, Vec<Vec<u8>>)> = None;
    let mut pages = 0;
    while first_audio.is_none() && pages < 512 {
        let Some((pg, next)) = page(src, pos) else {
            break;
        };
        pos = next;
        pages += 1;
        if serial.is_none() && pg.flags & 2 != 0 {
            let first = pg.segments.first().map(|s| s.0.as_slice()).unwrap_or(&[]);
            let audio = first.starts_with(b"\x01vorbis")
                || first.starts_with(b"OpusHead")
                || first.starts_with(b"\x7fFLAC")
                || first.starts_with(b"Speex   ")
                || first.starts_with(b"fLaC");
            if audio {
                serial = Some(pg.serial);
            }
        }
        if Some(pg.serial) != serial {
            continue;
        }
        let mut done_here = Vec::new();
        for (data, complete) in pg.segments {
            partial.extend_from_slice(&data);
            if complete {
                let packet = std::mem::take(&mut partial);
                if packets.len() < 3 {
                    packets.push(packet.clone());
                }
                done_here.push(packet);
            }
        }
        if pg.granule > 0 {
            first_audio = Some((pg.granule, done_here));
        }
    }
    let serial = serial?;
    let head = packets.first()?;
    let mut d;
    let mut first_pts = 0i64;
    let mut pictures = Vec::new();
    let mut tags = Dict::default();
    if head.starts_with(b"\x01vorbis") && head.len() >= 30 {
        d = Demuxed::new(Codec::Vorbis, "ogg");
        d.channels = head[11] as u32;
        d.sample_rate = le(head, 12, 4) as u32;
        d.stream_bit_rate = le(head, 20, 4) as i64; // nominal
        if let Some(c) = packets.get(1).filter(|c| c.starts_with(b"\x03vorbis")) {
            vorbis_comment(&c[7..], &mut tags, &mut pictures);
        }
    } else if head.starts_with(b"OpusHead") && head.len() >= 19 {
        d = Demuxed::new(Codec::Opus, "ogg");
        d.channels = head[9] as u32;
        d.initial_padding = le(head, 10, 2) as i64;
        d.sample_rate = 48000;
        if let Some((granule, done)) = &first_audio {
            first_pts = granule - done.iter().map(|p| opus_samples(p)).sum::<i64>();
        }
        if let Some(c) = packets.get(1).filter(|c| c.starts_with(b"OpusTags")) {
            vorbis_comment(&c[8..], &mut tags, &mut pictures);
        }
    } else if (head.starts_with(b"\x7fFLAC") && head.len() >= 13 + 38) || head.starts_with(b"fLaC") {
        // The mapping header, then STREAMINFO; FFmpeg's decoder gives the
        // rate, channels and depth (find_stream_info).
        let info = if head.starts_with(b"\x7fFLAC") {
            &head[13 + 4..]
        } else {
            packets.get(1).map(|p| &p[4.min(p.len())..]).unwrap_or(&[])
        };
        if info.len() < 18 {
            return None;
        }
        d = Demuxed::new(Codec::Flac, "ogg");
        d.sample_rate = ((info[10] as u32) << 12) | ((info[11] as u32) << 4) | ((info[12] as u32) >> 4);
        d.channels = ((info[12] >> 1) & 7) as u32 + 1;
        d.bits_raw = ((((info[12] & 1) as i32) << 4) | ((info[13] as i32) >> 4)) + 1;
        let comment = if head.starts_with(b"\x7fFLAC") { packets.get(1) } else { packets.get(2) };
        if let Some(c) = comment.filter(|c| c.len() > 4 && c[0] & 0x7f == 4) {
            vorbis_comment(&c[4..], &mut tags, &mut pictures);
        }
    } else if head.starts_with(b"Speex   ") && head.len() >= 68 {
        d = Demuxed::new(Codec::Speex, "ogg");
        d.sample_rate = le(head, 36, 4) as u32;
        d.channels = le(head, 48, 4) as u32;
        // The first page's granule counts its packets' samples, less the
        // codec's look-ahead: the stream starts before 0.
        let per_packet = (le(head, 56, 4) as i64).saturating_mul(le(head, 64, 4).max(1) as i64);
        if let Some((granule, done)) = &first_audio {
            first_pts = granule.saturating_sub((done.len() as i64).saturating_mul(per_packet));
        }
        if let Some(c) = packets.get(1) {
            vorbis_comment(c, &mut tags, &mut pictures);
        }
    } else {
        return None;
    }
    // The comments go to the stream's metadata.
    d.stream_tags = tags;
    d.pictures = pictures;
    d.time_base = (1, d.sample_rate.max(1) as i64);
    // The length: the last granule position of the stream.
    let tail = 256 * 1024;
    let from = src.size.saturating_sub(tail);
    let buf = src.read_at(from, tail);
    let mut last = None;
    let mut i = buf.len().saturating_sub(27);
    loop {
        if buf.len() >= i + 27 && &buf[i..i + 4] == b"OggS" && le(&buf, i + 14, 4) as u32 == serial {
            let g = le(&buf, i + 6, 8) as i64;
            if g != -1 {
                last = Some(g);
                break;
            }
        }
        if i == 0 {
            break;
        }
        i -= 1;
    }
    if let Some(g) = last.filter(|&g| g > 0) {
        d.duration_ts = Some(g - first_pts);
    }
    Some(d)
}
