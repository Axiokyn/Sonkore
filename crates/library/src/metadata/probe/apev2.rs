//! APEv2 tags as FFmpeg reads them (libavformat/apetag.c ff_ape_parse_tag):
//! the footer must close the file; keys as stored ("Album Artist",
//! "Track"…), text values up to their first NUL; a binary item whose name
//! (its "filename") looks like an image becomes an attached picture.

use super::{Dict, Picture, Src, le};

const FOOTER: u64 = 32;
const FLAG_IS_HEADER: u64 = 1 << 29;
const FLAG_CONTAINS_HEADER: u64 = 1 << 31;
const FLAG_IS_BINARY: u64 = 1 << 1;

/// ff_guess_image2_codec: an image extension.
fn image_name(name: &str) -> bool {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    name.contains('.')
        && matches!(
            ext.as_str(),
            "jpg"
                | "jpeg"
                | "png"
                | "gif"
                | "bmp"
                | "tif"
                | "tiff"
                | "webp"
                | "jpe"
                | "jfif"
                | "pjpeg"
                | "ljpg"
                | "jls"
        )
}

/// Reads the tag into `meta` and `pictures`; returns where it starts (0: none).
pub(crate) fn read(src: &mut Src, meta: &mut Dict, pictures: &mut Vec<Picture>) -> u64 {
    if src.size < FOOTER {
        return 0;
    }
    let f = src.read_at(src.size - FOOTER, FOOTER);
    if f.len() < 32 || &f[..8] != b"APETAGEX" {
        return 0;
    }
    if le(&f, 8, 4) > 2000 {
        return 0;
    }
    let tag_bytes = le(&f, 12, 4);
    if tag_bytes.wrapping_sub(FOOTER) as u32 > 16 * 1024 * 1024 || tag_bytes > src.size - FOOTER {
        return 0;
    }
    let fields = le(&f, 16, 4);
    if fields > 65536 {
        return 0;
    }
    let flags = le(&f, 20, 4);
    if flags & FLAG_IS_HEADER != 0 {
        return 0;
    }
    let start = src.size - tag_bytes;
    let tag_start = if flags & FLAG_CONTAINS_HEADER != 0 { start.saturating_sub(32) } else { start };
    let body = src.read_at(start, tag_bytes);
    let mut p = 0usize;
    for _ in 0..fields {
        if p + 8 > body.len() {
            break;
        }
        let size = le(&body, p, 4) as usize;
        let item_flags = le(&body, p + 4, 4);
        p += 8;
        // The key: printable ASCII up to a NUL (at most 1024 bytes).
        let key_end = match body[p.min(body.len())..].iter().take(1025).position(|&b| b == 0) {
            Some(e) => p + e,
            None => break,
        };
        let key = &body[p..key_end];
        if key.is_empty() || !key.iter().all(|&c| (0x20..=0x7e).contains(&c)) {
            break;
        }
        let key = String::from_utf8_lossy(key).into_owned();
        p = key_end + 1;
        if size > 16 * 1024 * 1024 || p + size > body.len() {
            break;
        }
        let value = &body[p..p + size];
        p += size;
        if item_flags & FLAG_IS_BINARY != 0 {
            // "filename\0data"
            let name_end = value.iter().position(|&b| b == 0).unwrap_or(value.len());
            let name = String::from_utf8_lossy(&value[..name_end]).into_owned();
            let data = value.get(name_end + 1..).unwrap_or(&[]);
            if data.is_empty() {
                continue;
            }
            if image_name(&name) {
                pictures.push(Picture { data: data.to_vec(), front: false });
            }
        } else {
            let end = value.iter().position(|&b| b == 0).unwrap_or(value.len());
            meta.set(&key, &String::from_utf8_lossy(&value[..end]));
        }
    }
    tag_start
}
