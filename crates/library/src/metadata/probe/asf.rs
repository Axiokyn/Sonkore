//! WMA / ASF (libavformat/asfdec_f.c): file and stream properties, the
//! content description and extended content description (and the header
//! extension's metadata objects), with FFmpeg's ASF key names.

use super::{Codec, Demuxed, Dict, Picture, Src, le};
use crate::metadata::text::utf16;

pub(super) const HEADER_GUID: [u8; 16] =
    [0x30, 0x26, 0xB2, 0x75, 0x8E, 0x66, 0xCF, 0x11, 0xA6, 0xD9, 0x00, 0xAA, 0x00, 0x62, 0xCE, 0x6C];
const FILE_PROPERTIES: [u8; 16] =
    [0xA1, 0xDC, 0xAB, 0x8C, 0x47, 0xA9, 0xCF, 0x11, 0x8E, 0xE4, 0x00, 0xC0, 0x0C, 0x20, 0x53, 0x65];
const STREAM_PROPERTIES: [u8; 16] =
    [0x91, 0x07, 0xDC, 0xB7, 0xB7, 0xA9, 0xCF, 0x11, 0x8E, 0xE6, 0x00, 0xC0, 0x0C, 0x20, 0x53, 0x65];
const AUDIO_MEDIA: [u8; 16] =
    [0x40, 0x9E, 0x69, 0xF8, 0x4D, 0x5B, 0xCF, 0x11, 0xA8, 0xFD, 0x00, 0x80, 0x5F, 0x5C, 0x44, 0x2B];
const CONTENT_DESCRIPTION: [u8; 16] =
    [0x33, 0x26, 0xB2, 0x75, 0x8E, 0x66, 0xCF, 0x11, 0xA6, 0xD9, 0x00, 0xAA, 0x00, 0x62, 0xCE, 0x6C];
const EXTENDED_CONTENT: [u8; 16] =
    [0x40, 0xA4, 0xD0, 0xD2, 0x07, 0xE3, 0xD2, 0x11, 0x97, 0xF0, 0x00, 0xA0, 0xC9, 0x5E, 0xA8, 0x50];
const HEADER_EXTENSION: [u8; 16] =
    [0xB5, 0x03, 0xBF, 0x5F, 0x2E, 0xA9, 0xCF, 0x11, 0x8E, 0xE3, 0x00, 0xC0, 0x0C, 0x20, 0x53, 0x65];
const METADATA: [u8; 16] =
    [0xEA, 0xCB, 0xF8, 0xC5, 0xAF, 0x5B, 0x77, 0x48, 0x84, 0x67, 0xAA, 0x8C, 0x44, 0xFA, 0x4C, 0xCA];
const METADATA_LIBRARY: [u8; 16] =
    [0x94, 0x1C, 0x23, 0x44, 0x98, 0x94, 0xD1, 0x49, 0xA1, 0x41, 0x1D, 0x13, 0x4E, 0x45, 0x70, 0x54];

/// ff_asf_metadata_conv (and WM/Year, which FFmpeg reads as the date).
const ASF_CONV: &[(&str, &str)] = &[
    ("WM/AlbumArtist", "album_artist"),
    ("WM/AlbumTitle", "album"),
    ("Author", "artist"),
    ("Description", "comment"),
    ("WM/Composer", "composer"),
    ("WM/EncodedBy", "encoded_by"),
    ("WM/EncodingSettings", "encoder"),
    ("WM/Genre", "genre"),
    ("WM/Language", "language"),
    ("WM/OriginalFilename", "filename"),
    ("WM/PartOfSet", "disc"),
    ("WM/Publisher", "publisher"),
    ("WM/Tool", "encoder"),
    ("WM/TrackNumber", "track"),
    ("WM/MediaStationCallSign", "service_provider"),
    ("WM/MediaStationName", "service_name"),
    ("WM/Year", "date"),
];

fn utf16le(d: &[u8]) -> String {
    let s = utf16(d.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes([c[0], c[1]])));
    match s.find('\0') {
        Some(i) => s[..i].to_string(),
        None => s,
    }
}

/// get_tag: a value of the given type as FFmpeg's text.
fn tag_value(kind: u64, v: &[u8], bool_size: usize) -> Option<String> {
    match kind {
        0 => Some(utf16le(v)),
        2 => Some(le(v, 0, bool_size.min(v.len())).to_string()),
        3 => Some(le(v, 0, 4.min(v.len())).to_string()),
        4 => Some(le(v, 0, 8.min(v.len())).to_string()),
        5 => Some(le(v, 0, 2.min(v.len())).to_string()),
        _ => None,
    }
}

/// WM/Picture: type, MIME, description, data.
fn picture(v: &[u8], pictures: &mut Vec<Picture>) {
    if v.len() < 5 {
        return;
    }
    let ptype = v[0];
    let len = le(v, 1, 4) as usize;
    let mut p = 5;
    let skip_str = |p: &mut usize| {
        while *p + 1 < v.len() {
            let z = v[*p] == 0 && v[*p + 1] == 0;
            *p += 2;
            if z {
                break;
            }
        }
    };
    skip_str(&mut p); // MIME
    skip_str(&mut p); // description
    if p + len <= v.len() && len > 0 {
        pictures.push(Picture { data: v[p..p + len].to_vec(), front: ptype == 3 });
    }
}

fn metadata_object(o: &[u8], meta: &mut Dict, library: bool) {
    let count = le(o, 24, 2) as usize;
    let mut p = 26;
    for _ in 0..count {
        if p + 12 > o.len() {
            break;
        }
        let name_len = le(o, p + 4, 2) as usize;
        let kind = le(o, p + 6, 2);
        let data_len = le(o, p + 8, 4) as usize;
        p += 12;
        if p + name_len + data_len > o.len() {
            break;
        }
        let name = utf16le(&o[p..p + name_len]);
        let value = &o[p + name_len..p + name_len + data_len];
        p += name_len + data_len;
        let _ = library;
        if let Some(v) = tag_value(kind, value, 2)
            && !v.is_empty()
        {
            meta.set(&name, &v);
        }
    }
}

pub(super) fn read(src: &mut Src) -> Option<Demuxed> {
    let h = src.read_at(0, 30);
    let header_size = le(&h, 16, 8);
    let header = src.read_at(0, header_size.min(64 << 20));
    let mut d = Demuxed::new(Codec::Other("asf".into()), "asf");
    let mut meta = Dict::default();
    let mut pictures = Vec::new();
    let (mut play_time, mut preroll) = (0u64, 0u64);
    let mut have_audio = false;
    let mut objects: Vec<(usize, usize)> = Vec::new();
    let mut p = 30usize;
    while p + 24 <= header.len() {
        let size = le(&header, p + 16, 8) as usize;
        if size < 24 || size > header.len() - p {
            break;
        }
        objects.push((p, p + size));
        p += size;
    }
    let mut i = 0;
    while i < objects.len() {
        let (s, e) = objects[i];
        i += 1;
        let o = &header[s..e];
        let guid = &o[..16];
        if guid == FILE_PROPERTIES {
            play_time = le(o, 64, 8);
            preroll = le(o, 80, 4);
        } else if guid == STREAM_PROPERTIES && !have_audio && o.get(24..40) == Some(&AUDIO_MEDIA[..]) {
            let type_len = le(o, 64, 4) as usize;
            let f = &o[78.min(o.len())..(78 + type_len).min(o.len())];
            if f.len() >= 16 {
                let tag = le(f, 0, 2);
                d.channels = le(f, 2, 2) as u32;
                d.sample_rate = le(f, 4, 4) as u32;
                d.stream_bit_rate = le(f, 8, 4) as i64 * 8;
                d.bits_coded = le(f, 14, 2) as i32;
                d.codec = match tag {
                    0x160 | 0x161 => Codec::Wma,
                    0x162 => Codec::WmaPro,
                    0x163 => Codec::WmaLossless,
                    0x55 => Codec::Mp3,
                    0x50 => Codec::Mp2,
                    1 | 3 => Codec::Pcm,
                    other => Codec::Other(format!("asf_{other:04x}")),
                };
                have_audio = true;
            }
        } else if guid == CONTENT_DESCRIPTION && o.len() >= 34 {
            let lens: Vec<usize> = (0..5).map(|k| le(o, 24 + 2 * k, 2) as usize).collect();
            let mut q = 34;
            for (k, key) in ["title", "author", "copyright", "comment"].iter().enumerate() {
                let v = &o[q.min(o.len())..(q + lens[k]).min(o.len())];
                q += lens[k];
                let text = utf16le(v);
                if !text.is_empty() {
                    meta.set(key, &text);
                }
            }
        } else if guid == EXTENDED_CONTENT && o.len() >= 26 {
            let count = le(o, 24, 2) as usize;
            let mut q = 26;
            for _ in 0..count {
                if q + 2 > o.len() {
                    break;
                }
                let name_len = le(o, q, 2) as usize;
                q += 2;
                if q + name_len + 4 > o.len() {
                    break;
                }
                let name = utf16le(&o[q..q + name_len]);
                q += name_len;
                let kind = le(o, q, 2);
                let len = le(o, q + 2, 2) as usize;
                q += 4;
                if q + len > o.len() {
                    break;
                }
                let value = &o[q..q + len];
                q += len;
                if name == "WM/Picture" {
                    picture(value, &mut pictures);
                } else if let Some(v) = tag_value(kind, value, 4)
                    && !v.is_empty()
                {
                    meta.set(&name, &v);
                }
            }
        } else if guid == HEADER_EXTENSION && o.len() >= 46 {
            let mut q = 46;
            while q + 24 <= o.len() {
                let size = le(o, q + 16, 8) as usize;
                if size < 24 || size > o.len() - q {
                    break;
                }
                let sub = &o[q..q + size];
                if sub[..16] == METADATA {
                    metadata_object(sub, &mut meta, false);
                } else if sub[..16] == METADATA_LIBRARY {
                    metadata_object(sub, &mut meta, true);
                }
                q += size;
            }
        }
    }
    if !have_audio {
        return None;
    }
    meta.convert(ASF_CONV);
    d.format_tags = meta;
    d.pictures = pictures;
    d.time_base = (1, 1000);
    let ms = (play_time / 10_000) as i64 - preroll as i64;
    if ms > 0 {
        d.duration_ts = Some(ms);
    }
    Some(d)
}
