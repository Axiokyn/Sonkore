//! ID3v2 as FFmpeg reads it (libavformat/id3v2.c), for the formats whose tags
//! earlier Sonkore versions got from FFmpeg (AIFF, DSF, TTA, raw AAC / MP2…):
//! text frames only (the first value), the first of a repeated frame,
//! TXXX under its description, genre numbers as names, TYER/TDAT merged
//! into a date, then FFmpeg's key names ("title", "album_artist"…).

use super::{Dict, ID3V1_GENRES, Picture};
use crate::metadata::text::{latin1, utf16};

/// ff_id3v2_34_metadata_conv.
const CONV_34: &[(&str, &str)] = &[
    ("TALB", "album"),
    ("TCOM", "composer"),
    ("TCON", "genre"),
    ("TCOP", "copyright"),
    ("TENC", "encoded_by"),
    ("TIT2", "title"),
    ("TLAN", "language"),
    ("TPE1", "artist"),
    ("TPE2", "album_artist"),
    ("TPE3", "performer"),
    ("TPOS", "disc"),
    ("TPUB", "publisher"),
    ("TRCK", "track"),
    ("TSSE", "encoder"),
    ("USLT", "lyrics"),
];

/// ff_id3v2_4_metadata_conv.
pub(crate) const CONV_24: &[(&str, &str)] = &[
    ("TCMP", "compilation"),
    ("TDRC", "date"),
    ("TDRL", "date"),
    ("TDEN", "creation_time"),
    ("TSOA", "album-sort"),
    ("TSOP", "artist-sort"),
    ("TSOT", "title-sort"),
    ("TIT1", "grouping"),
];

/// id3v2_2_metadata_conv.
const CONV_22: &[(&str, &str)] = &[
    ("TAL", "album"),
    ("TCO", "genre"),
    ("TCP", "compilation"),
    ("TT2", "title"),
    ("TEN", "encoded_by"),
    ("TP1", "artist"),
    ("TP2", "album_artist"),
    ("TP3", "performer"),
    ("TRK", "track"),
];

/// ff_id3v2_picture_types[3].
const FRONT: u8 = 3;

/// Undoes unsynchronisation (0xFF 0x00 -> 0xFF).
fn resync(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        out.push(data[i]);
        if data[i] == 0xff && i + 1 < data.len() && data[i + 1] == 0 {
            i += 1;
        }
        i += 1;
    }
    out
}

/// decode_str: one string in the frame's encoding, up to its terminator.
/// None for a UTF-16 string without a valid BOM (FFmpeg drops the frame).
fn decode_str(d: &[u8], pos: &mut usize, encoding: u8) -> Option<String> {
    let rest = &d[(*pos).min(d.len())..];
    match encoding {
        1 | 2 => {
            let mut body = rest;
            let mut big = encoding == 2;
            if encoding == 1 {
                if rest.len() < 2 {
                    *pos = d.len();
                    return None;
                }
                match (rest[0], rest[1]) {
                    (0xFF, 0xFE) => big = false,
                    (0xFE, 0xFF) => big = true,
                    _ => {
                        *pos = d.len();
                        return None;
                    }
                }
                body = &rest[2..];
                *pos += 2;
            }
            let mut units = Vec::new();
            let mut i = 0;
            while i + 1 < body.len() {
                let u = if big {
                    u16::from_be_bytes([body[i], body[i + 1]])
                } else {
                    u16::from_le_bytes([body[i], body[i + 1]])
                };
                i += 2;
                if u == 0 {
                    break;
                }
                units.push(u);
            }
            if i + 1 >= body.len() && body.len() % 2 == 1 {
                i = body.len();
            }
            *pos += i;
            Some(utf16(units.into_iter()))
        }
        0 | 3 => {
            let end = rest.iter().position(|&b| b == 0);
            let text = &rest[..end.unwrap_or(rest.len())];
            *pos += end.map(|e| e + 1).unwrap_or(rest.len());
            Some(if encoding == 0 { latin1(text) } else { String::from_utf8_lossy(text).into_owned() })
        }
        _ => {
            *pos = d.len();
            None
        }
    }
}

/// `sscanf(dst, "(%d)", &g) == 1 || sscanf(dst, "%d", &g) == 1`.
fn genre_number(s: &str) -> Option<u32> {
    let scan = |t: &str| -> Option<i64> {
        let t = t.trim_start();
        let (neg, digits) = match t.strip_prefix('-') {
            Some(r) => (true, r),
            None => (false, t.strip_prefix('+').unwrap_or(t)),
        };
        let n: String = digits.chars().take_while(|c| c.is_ascii_digit()).collect();
        if n.is_empty() {
            return None;
        }
        let v: i64 = n.parse().ok()?;
        Some(if neg { -v } else { v })
    };
    let v = match s.strip_prefix('(') {
        Some(r) => scan(r),
        None => scan(s),
    }?;
    u32::try_from(v).ok()
}

/// read_ttag.
fn read_ttag(d: &[u8], key: &str, meta: &mut Dict) {
    if d.is_empty() {
        return;
    }
    let encoding = d[0];
    let mut pos = 1;
    let Some(mut dst) = decode_str(d, &mut pos, encoding) else {
        return;
    };
    let mut key = key.to_string();
    if key == "TCON" || key == "TCO" {
        if let Some(g) = genre_number(&dst).filter(|&g| (g as usize) < ID3V1_GENRES.len()) {
            dst = ID3V1_GENRES[g as usize].to_string();
        }
    } else if key == "TXXX" || key == "TXX" {
        // dst holds the description: the value follows.
        key = dst;
        let Some(value) = decode_str(d, &mut pos, encoding) else {
            return;
        };
        dst = value;
        meta.set_new(&key, &dst);
        return;
    }
    if !dst.is_empty() {
        meta.set_new(&key, &dst);
    }
}

fn is_date(v: &str) -> bool {
    v.len() == 4 && v.bytes().all(|b| b.is_ascii_digit())
}

/// merge_date: TYER (+ TDAT, TIME) into "date".
fn merge_date(meta: &mut Dict) {
    let year = ["TYER", "TYE"].iter().find_map(|k| meta.get(k).filter(|v| is_date(v)).map(str::to_string));
    let Some(year) = year else { return };
    let mut date = year[..4].to_string();
    meta.remove("TYER");
    meta.remove("TYE");
    if let Some(dm) = ["TDAT", "TDA"].iter().find_map(|k| meta.get(k).filter(|v| is_date(v)).map(str::to_string)) {
        date.push_str(&format!("-{}-{}", &dm[2..4], &dm[0..2]));
        meta.remove("TDAT");
        meta.remove("TDA");
        if let Some(hm) = ["TIME", "TIM"].iter().find_map(|k| meta.get(k).filter(|v| is_date(v)).map(str::to_string)) {
            date.push_str(&format!(" {}:{}", &hm[0..2], &hm[2..4]));
            meta.remove("TIME");
            meta.remove("TIM");
        }
    }
    meta.set("date", &date);
}

fn known_image_mime(mime: &str) -> bool {
    const MIMES: [&str; 10] = [
        "image/gif",
        "image/jpeg",
        "image/jpg",
        "image/png",
        "image/tiff",
        "image/bmp",
        "image/webp",
        "JPG",
        "PNG",
        "GIF",
    ];
    MIMES.iter().any(|m| m.eq_ignore_ascii_case(mime))
}

/// read_apic: an attached picture (FFmpeg keeps those of a known type).
fn read_apic(d: &[u8], v22: bool, pictures: &mut Vec<Picture>) {
    if d.is_empty() {
        return;
    }
    let encoding = d[0];
    let mut pos;
    let mime = if v22 {
        if d.len() < 4 {
            return;
        }
        pos = 4;
        latin1(&d[1..4])
    } else {
        let end = d[1..].iter().position(|&b| b == 0).map(|e| e + 1).unwrap_or(d.len());
        pos = end + 1;
        latin1(&d[1..end])
    };
    if !known_image_mime(&mime) || pos >= d.len() {
        return;
    }
    let ptype = d[pos];
    pos += 1;
    if decode_str(d, &mut pos, encoding).is_none() {
        return;
    }
    if pos >= d.len() {
        return;
    }
    pictures.push(Picture { data: d[pos..].to_vec(), front: ptype == FRONT });
}

/// Reads one complete ID3v2 tag into `meta` (FFmpeg's key names).
pub(crate) fn read_dict(tag: &[u8], meta: &mut Dict, mut pictures: Option<&mut Vec<Picture>>) {
    if tag.len() < 10 || !tag.starts_with(b"ID3") {
        return;
    }
    let version = tag[3];
    if !(2..=4).contains(&version) {
        return;
    }
    let flags = tag[5];
    let size = ((tag[6] as usize & 0x7f) << 21)
        | ((tag[7] as usize & 0x7f) << 14)
        | ((tag[8] as usize & 0x7f) << 7)
        | (tag[9] as usize & 0x7f);
    let end = (10 + size).min(tag.len());
    let unsync = flags & 0x80 != 0;
    let mut body = tag[10..end].to_vec();
    if unsync && version <= 3 {
        body = resync(&body);
    }
    let mut pos = 0usize;
    if flags & 0x40 != 0 {
        // extended header
        if version == 4 {
            let n = crate::metadata::syncsafe(&body[pos.min(body.len())..]) as usize;
            pos += n.max(4);
        } else if version == 3 {
            let n = crate::metadata::be32(&body[pos.min(body.len())..]) as usize;
            pos += n + 4;
        }
    }
    let v22 = version == 2;
    let (id_len, hdr_len) = if v22 { (3, 6) } else { (4, 10) };
    let mut local = Dict::default();
    while pos + hdr_len <= body.len() {
        let id = &body[pos..pos + id_len];
        if !id.iter().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) {
            break; // padding or garbage
        }
        let id = String::from_utf8_lossy(id).into_owned();
        let (fsize, fflags) = if v22 {
            (crate::metadata::be24(&body[pos + 3..]) as usize, 0u16)
        } else if version == 3 {
            (crate::metadata::be32(&body[pos + 4..]) as usize, u16::from_be_bytes([body[pos + 8], body[pos + 9]]))
        } else {
            (crate::metadata::syncsafe(&body[pos + 4..]) as usize, u16::from_be_bytes([body[pos + 8], body[pos + 9]]))
        };
        let data_pos = pos + hdr_len;
        if data_pos + fsize > body.len() {
            break;
        }
        let mut data = body[data_pos..data_pos + fsize].to_vec();
        pos = data_pos + fsize;
        // Compressed or encrypted frames are skipped (no zlib here).
        let (compressed, encrypted, frame_unsync, dli) = match version {
            3 => (fflags & 0x0080 != 0, fflags & 0x0040 != 0, false, false),
            4 => (fflags & 0x0008 != 0, fflags & 0x0004 != 0, fflags & 0x0002 != 0, fflags & 0x0001 != 0),
            _ => (false, false, false, false),
        };
        if compressed || encrypted {
            continue;
        }
        if dli && data.len() >= 4 {
            data.drain(..4);
        }
        if frame_unsync || (unsync && version == 4) {
            data = resync(&data);
        }
        if id == "APIC" || id == "PIC" {
            if let Some(p) = pictures.as_deref_mut() {
                read_apic(&data, v22, p);
            }
        } else if id.starts_with('T') {
            read_ttag(&data, &id, &mut local);
        }
    }
    if version <= 3 {
        merge_date(&mut local);
    }
    local.convert(CONV_34);
    local.convert(CONV_22);
    local.convert(CONV_24);
    for (k, v) in local.0 {
        meta.set_new(&k, &v);
    }
}
