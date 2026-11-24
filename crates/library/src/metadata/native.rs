//! Sonkore's own tag readers: FLAC, MP3
//! and WAV, with their ID3v2 parser. Only container headers are read, no
//! audio is decoded, so scanning thousands of files stays fast.
//! Ported line by line: same fields, same precedence, same quirks.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::sync::Arc;

use super::text::{decode_id3_text, leading_int, skip_id3_string, strip_genre_code};
use super::{Cover, apply_loudness_tag, apply_tag, be24, be32, le16, le32, syncsafe};
use crate::meta::TrackMeta;

/// File reading: what is there, short at the end.
pub(crate) struct Reader {
    file: File,
    size: u64,
}

impl Reader {
    pub fn open(path: &std::path::Path) -> Option<Reader> {
        let file = File::open(path).ok()?;
        let size = file.metadata().ok()?.len();
        Some(Reader { file, size })
    }
    pub fn size(&self) -> u64 {
        self.size
    }
    pub fn pos(&mut self) -> u64 {
        self.file.stream_position().unwrap_or(0)
    }
    pub fn seek(&mut self, pos: u64) -> bool {
        self.file.seek(SeekFrom::Start(pos)).is_ok()
    }
    pub fn at_end(&mut self) -> bool {
        self.pos() >= self.size
    }
    /// Up to `n` bytes (fewer at the end of the file).
    pub fn read(&mut self, n: u64) -> Vec<u8> {
        let n = n.min(self.size.saturating_sub(self.pos()));
        let mut buf = Vec::with_capacity(n as usize);
        let _ = (&mut self.file).take(n).read_to_end(&mut buf);
        buf
    }
    pub fn peek(&mut self, n: u64) -> Vec<u8> {
        let at = self.pos();
        let data = self.read(n);
        self.seek(at);
        data
    }
}

// --- ID3v2 ---------------------------------------------------------------------

/// A complete ID3v2 tag (header included). Fills `t` and, when asked, the
/// front cover (or the first picture).
pub(crate) fn parse_id3(tag: &[u8], t: &mut TrackMeta, mut cover: Option<&mut Cover>) {
    if tag.len() < 10 || !tag.starts_with(b"ID3") {
        return;
    }
    let ver = tag[3];
    let flags = tag[5];
    let mut pos: i64 = 10;
    if flags & 0x40 != 0 && ver >= 3 && tag.len() >= 14 {
        pos += if ver == 4 { syncsafe(&tag[10..]) as i64 } else { be32(&tag[10..]) as i64 + 4 };
    }
    let id_len: i64 = if ver == 2 { 3 } else { 4 };
    let hdr_len: i64 = if ver == 2 { 6 } else { 10 };
    let mut picture_type = -1;
    let len = tag.len() as i64;

    while pos >= 0 && pos + hdr_len <= len {
        let p = pos as usize;
        let id = &tag[p..p + id_len as usize];
        if id.is_empty() || id[0] == 0 {
            break;
        }
        let size = match ver {
            2 => be24(&tag[p + 3..]) as i64,
            4 => syncsafe(&tag[p + 4..]) as i64,
            _ => be32(&tag[p + 4..]) as i64,
        };
        let data_pos = pos + hdr_len;
        if size == 0 || data_pos + size > len {
            break;
        }
        let d = &tag[data_pos as usize..(data_pos + size) as usize];
        pos = data_pos + size;

        let is_pic = id == b"APIC" || id == b"PIC";
        if is_pic {
            t.has_embedded_cover = true;
            let Some(cover) = cover.as_deref_mut() else {
                continue;
            };
            if d.len() < 4 {
                continue;
            }
            let enc = d[0];
            let mut q: i64 = 1;
            if id == b"PIC" {
                q += 3; // 3-char image format
            } else {
                q = d[1..].iter().position(|&b| b == 0).map(|i| i as i64 + 1).unwrap_or(-1) + 1; // MIME type
            }
            if q <= 0 || q >= d.len() as i64 {
                continue;
            }
            let ptype = d[q as usize] as i32;
            let q = skip_id3_string(d, q as usize + 1, enc);
            if q < d.len() && picture_type != 3 && (cover.data.is_empty() || ptype == 3) {
                cover.data = d[q..].to_vec();
                picture_type = ptype;
            }
            continue;
        }
        if id[0] != b'T' || d.is_empty() {
            continue;
        }
        if id == b"TXXX" || id == b"TXX" {
            // User text, "description\0value": ReplayGain lives here.
            let enc = d[0];
            let value_pos = skip_id3_string(d, 1, enc);
            let key = decode_id3_text(&d[1..value_pos.max(1)], enc).to_uppercase();
            let value = decode_id3_text(&d[value_pos.min(d.len())..], enc);
            apply_loudness_tag(t, &key, &value);
            continue;
        }
        let text = decode_id3_text(&d[1..], d[0]);
        match id {
            b"TIT2" | b"TT2" => t.title = text,
            b"TPE1" | b"TP1" => t.artist = Arc::from(text),
            b"TPE2" | b"TP2" => t.album_artist = Arc::from(text),
            b"TALB" | b"TAL" => t.album = Arc::from(text),
            b"TCON" | b"TCO" => t.genre = Arc::from(strip_genre_code(&text)),
            b"TRCK" | b"TRK" => t.track_no = leading_int(&text),
            b"TPOS" | b"TPA" => t.disc_no = leading_int(&text),
            b"TDRC" | b"TYER" | b"TYE" if t.year == 0 => t.year = leading_int(&text),
            _ => {}
        }
    }
}

/// The ID3v2 tag at the start of the file, if any; returns its whole size
/// and leaves the file after it.
fn read_id3_at_start(f: &mut Reader, t: &mut TrackMeta, cover: Option<&mut Cover>) -> u64 {
    let hdr = f.peek(10);
    if hdr.len() < 10 || !hdr.starts_with(b"ID3") {
        return 0;
    }
    let mut size = 10 + syncsafe(&hdr[6..]) as u64;
    if hdr[5] & 0x10 != 0 {
        size += 10; // footer
    }
    let tag = f.read(size.min(64 * 1024 * 1024));
    parse_id3(&tag, t, cover);
    f.seek(size);
    size
}

// --- FLAC ------------------------------------------------------------------------

pub(crate) fn read_flac(f: &mut Reader, t: &mut TrackMeta, mut cover: Option<&mut Cover>) -> bool {
    read_id3_at_start(f, t, None); // some taggers prepend ID3 to FLAC
    if f.read(4) != b"fLaC" {
        return false;
    }
    t.codec = Arc::from("FLAC");
    let mut picture_type = -1;
    loop {
        let h = f.read(4);
        if h.len() < 4 {
            break;
        }
        let last = h[0] & 0x80 != 0;
        let kind = h[0] & 0x7f;
        let len = be24(&h[1..]) as u64;
        let next = f.pos() + len;

        if kind == 0 && len >= 18 {
            // STREAMINFO
            let d = f.read(len);
            if d.len() >= 18 {
                t.sample_rate = ((d[10] as i32) << 12) | ((d[11] as i32) << 4) | ((d[12] as i32) >> 4);
                t.channels = (((d[12] >> 1) & 7) + 1) as i32;
                t.bits = ((((d[12] & 1) as i32) << 4) | ((d[13] as i32) >> 4)) + 1;
                let total = (((d[13] & 0x0f) as u64) << 32) | be32(&d[14..]) as u64;
                if t.sample_rate > 0 {
                    t.duration = total as f64 / t.sample_rate as f64;
                }
            }
        } else if kind == 4 {
            // VORBIS_COMMENT (little endian)
            let d = f.read(len);
            read_vorbis_comments(&d, 0, t);
        } else if kind == 6 {
            // PICTURE (big endian)
            t.has_embedded_cover = true;
            if let Some(cover) = cover.as_deref_mut()
                && picture_type != 3
            {
                let d = f.read(len);
                if let Some((ptype, data)) = flac_picture(&d)
                    && (cover.data.is_empty() || ptype == 3)
                {
                    cover.data = data.to_vec();
                    picture_type = ptype;
                }
            }
        }
        if last || !f.seek(next) {
            break;
        }
    }
    t.sample_rate > 0
}

/// A Vorbis comment block from `start` (after a codec's packet header):
/// vendor, count, "KEY=value" entries, as the FLAC reader reads them.
pub(crate) fn read_vorbis_comments(d: &[u8], start: usize, t: &mut TrackMeta) {
    let d = &d[start.min(d.len())..];
    if d.len() < 8 {
        return;
    }
    let mut p = 4 + le32(d) as u64;
    if p + 4 > d.len() as u64 {
        return;
    }
    let count = le32(&d[p as usize..]);
    p += 4;
    let mut i = 0;
    while i < count && p + 4 <= d.len() as u64 {
        let l = le32(&d[p as usize..]) as u64;
        p += 4;
        if p + l > d.len() as u64 {
            break;
        }
        let kv = String::from_utf8_lossy(&d[p as usize..(p + l) as usize]);
        p += l;
        if let Some(eq) = kv.find('=')
            && eq > 0
        {
            apply_tag(t, &kv[..eq], kv[eq + 1..].trim());
        }
        i += 1;
    }
}

/// A FLAC PICTURE block's type and image bytes (with the FLAC reader's checks).
pub(crate) fn flac_picture(d: &[u8]) -> Option<(i32, &[u8])> {
    if d.len() < 32 {
        return None;
    }
    let ptype = be32(d) as i32;
    let mut p: u64 = 4;
    p += 4 + be32(&d[p as usize..]) as u64; // MIME
    if p + 4 <= d.len() as u64 {
        p += 4 + be32(&d[p as usize..]) as u64; // description
    }
    p += 16; // width / height / depth / colours
    if p + 4 <= d.len() as u64 {
        let data_len = be32(&d[p as usize..]) as u64;
        p += 4;
        if p + data_len <= d.len() as u64 {
            return Some((ptype, &d[p as usize..(p + data_len) as usize]));
        }
    }
    None
}

// --- MP3: ID3v2 + the first frame header (+ Xing/Info for VBR length) ------------

pub(crate) fn read_mp3(f: &mut Reader, t: &mut TrackMeta, cover: Option<&mut Cover>) -> bool {
    t.codec = Arc::from("MP3");
    let audio_start = read_id3_at_start(f, t, cover);
    let buf = f.read(64 * 1024);

    const BITRATE_V1: [i32; 16] = [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0];
    const BITRATE_V2: [i32; 16] = [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0];
    const RATES: [i32; 4] = [44100, 48000, 32000, 0];

    let mut i = 0usize;
    while i + 4 <= buf.len() {
        let u = &buf[i..];
        if u[0] != 0xff || (u[1] & 0xe0) != 0xe0 {
            i += 1;
            continue;
        }
        let version = (u[1] >> 3) & 3; // 3 = MPEG1, 2 = MPEG2, 0 = MPEG2.5
        let layer = (u[1] >> 1) & 3; // 1 = Layer III
        let br_index = (u[2] >> 4) as usize;
        let sr_index = ((u[2] >> 2) & 3) as usize;
        if version == 1 || layer != 1 || br_index == 0 || br_index == 15 || sr_index == 3 {
            i += 1;
            continue;
        }
        let mpeg1 = version == 3;
        let bitrate = if mpeg1 { BITRATE_V1[br_index] } else { BITRATE_V2[br_index] };
        let rate = RATES[sr_index]
            >> if mpeg1 {
                0
            } else if version == 2 {
                1
            } else {
                2
            };
        let mono = (u[3] >> 6) == 3;
        t.sample_rate = rate;
        t.channels = if mono { 1 } else { 2 };
        t.bitrate_kbps = bitrate;
        t.bits = 0;

        let samples_per_frame = if mpeg1 { 1152.0 } else { 576.0 };
        let side_info = if mpeg1 {
            if mono { 17 } else { 32 }
        } else if mono {
            9
        } else {
            17
        };
        let xing = i + 4 + side_info;
        if xing + 12 <= buf.len() {
            let magic = &buf[xing..xing + 4];
            if magic == b"Xing" || magic == b"Info" {
                let xflags = be32(&buf[xing + 4..]);
                if xflags & 1 != 0 {
                    let frames = be32(&buf[xing + 8..]);
                    t.duration = frames as f64 * samples_per_frame / rate as f64;
                    if xflags & 2 != 0 && xing + 16 <= buf.len() && t.duration > 0.0 {
                        let bytes = be32(&buf[xing + 12..]).wrapping_mul(8);
                        t.bitrate_kbps = (bytes as f64 / t.duration / 1000.0) as i32;
                    }
                }
            }
        }
        if t.duration <= 0.0 && bitrate > 0 {
            t.duration = (f.size() as f64 - audio_start as f64 - i as f64) * 8.0 / (bitrate as f64 * 1000.0);
        }
        return true;
    }
    false
}

// --- WAV: RIFF fmt/data + LIST/INFO + id3 chunk ------------------------------------

pub(crate) fn read_wav(f: &mut Reader, t: &mut TrackMeta, mut cover: Option<&mut Cover>) -> bool {
    t.codec = Arc::from("WAV");
    let riff = f.read(12);
    if riff.len() < 12 || !riff.starts_with(b"RIFF") || &riff[8..12] != b"WAVE" {
        return false;
    }
    let mut block_align = 0i32;
    let mut is_float = false;
    while !f.at_end() {
        let ch = f.read(8);
        if ch.len() < 8 {
            break;
        }
        let id = &ch[..4];
        let size = le32(&ch[4..]) as u64;
        let next = f.pos() + size + (size & 1);
        if id == b"fmt " && size >= 16 {
            let d = f.read(size);
            if d.len() >= 16 {
                let format = le16(&d);
                t.channels = le16(&d[2..]) as i32;
                t.sample_rate = le32(&d[4..]) as i32;
                block_align = le16(&d[12..]) as i32;
                t.bits = le16(&d[14..]) as i32;
                is_float = format == 3;
            }
        } else if id == b"data" {
            if block_align > 0 && t.sample_rate > 0 {
                t.duration = (size / block_align as u64) as f64 / t.sample_rate as f64;
            }
        } else if id == b"LIST" {
            let d = f.read(size);
            if d.starts_with(b"INFO") {
                let mut p = 4usize;
                while p + 8 <= d.len() {
                    let sub = &d[p..p + 4];
                    let l = le32(&d[p + 4..]) as usize;
                    let end = (p + 8).saturating_add(l).min(d.len());
                    let v: String =
                        String::from_utf8_lossy(&d[p + 8..end]).chars().filter(|&c| c != '\0').collect::<String>();
                    let v = v.trim().to_string();
                    match sub {
                        b"INAM" => t.title = v,
                        b"IART" => t.artist = Arc::from(v),
                        b"IPRD" => t.album = Arc::from(v),
                        b"IGNR" => t.genre = Arc::from(v),
                        b"ICRD" => t.year = leading_int(&v),
                        b"ITRK" | b"IPRT" => t.track_no = leading_int(&v),
                        _ => {}
                    }
                    p = p.saturating_add(8 + l + (l & 1));
                }
            }
        } else if id == b"id3 " || id == b"ID3 " {
            let d = f.read(size);
            parse_id3(&d, t, cover.as_deref_mut());
        }
        if !f.seek(next) {
            break;
        }
    }
    if is_float {
        t.bits = 0;
    }
    t.sample_rate > 0
}
