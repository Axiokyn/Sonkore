//! MP4 / M4A / M4B as FFmpeg's mov demuxer reads them (libavformat/mov.c):
//! the first sound track's sample description (esds AudioSpecificConfig,
//! the ALAC cookie), its length after the edit list, the bit rate from the
//! sample sizes, the iTunes tags (ilst, "----" free-form and "keys"
//! indexed ones) and the cover atoms.

use super::{Codec, Demuxed, Dict, ID3V1_GENRES, Picture, Src, be, rescale};
use crate::metadata::text::utf16;

/// The atoms of `d[start..end]`: (body start, end, type).
fn atoms(d: &[u8], start: usize, end: usize) -> Vec<(usize, usize, [u8; 4])> {
    let mut out = Vec::new();
    let mut p = start;
    let end = end.min(d.len());
    while p + 8 <= end {
        let mut size = be(d, p, 4) as usize;
        let kind = [d[p + 4], d[p + 5], d[p + 6], d[p + 7]];
        let mut hdr = 8;
        if size == 1 {
            if p + 16 > end {
                break;
            }
            size = be(d, p + 8, 8) as usize;
            hdr = 16;
        } else if size == 0 {
            size = end - p;
        }
        if size < hdr || size > end - p {
            // A truncated last atom: what is there.
            out.push((p + hdr, end, kind));
            break;
        }
        out.push((p + hdr, p + size, kind));
        p += size;
    }
    out
}

fn child(d: &[u8], start: usize, end: usize, kind: &[u8; 4]) -> Option<(usize, usize)> {
    atoms(d, start, end).into_iter().find(|a| &a.2 == kind).map(|a| (a.0, a.1))
}

#[derive(Default)]
struct Track {
    sound: bool,
    timescale: i64,
    mdhd_duration: i64,
    stts_duration: i64,
    edits: Vec<(i64, i64)>, // (segment duration in movie units, media time)
    data_size: i64,
    codec: Option<Codec>,
    channels: u32,
    sample_size: i32,
    sample_rate: u32,
}

/// MPEG-4 AudioSpecificConfig: (sample rate, channels) as FFmpeg sets
/// them (the SBR output rate when it is signalled).
fn audio_specific_config(cfg: &[u8]) -> (u32, u32) {
    const RATES: [u32; 13] = [96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350];
    const CHANNELS: [u32; 15] = [0, 1, 2, 3, 4, 5, 6, 8, 0, 0, 0, 7, 8, 0, 24];
    let mut bit = 0usize;
    let get = |bit: &mut usize, n: usize| -> u32 {
        let mut v = 0u32;
        for _ in 0..n {
            let byte = cfg.get(*bit / 8).copied().unwrap_or(0);
            v = (v << 1) | ((byte >> (7 - *bit % 8)) & 1) as u32;
            *bit += 1;
        }
        v
    };
    let object_type = |bit: &mut usize| -> u32 {
        let t = get(bit, 5);
        if t == 31 { 32 + get(bit, 6) } else { t }
    };
    let rate = |bit: &mut usize| -> u32 {
        let i = get(bit, 4);
        if i == 15 { get(bit, 24) } else { RATES.get(i as usize).copied().unwrap_or(0) }
    };
    let mut aot = object_type(&mut bit);
    let sample_rate = rate(&mut bit);
    let chan_config = get(&mut bit, 4);
    let channels = CHANNELS.get(chan_config as usize).copied().unwrap_or(0);
    let mut ext_rate = 0;
    if aot == 5 || aot == 29 {
        ext_rate = rate(&mut bit);
        aot = object_type(&mut bit);
        let _ = aot;
    } else if matches!(aot, 1 | 2 | 3 | 4 | 6 | 7) && chan_config != 0 {
        // GASpecificConfig (frame length, core coder, extension), then the
        // backward compatible SBR signal (sync extension 0x2b7).
        get(&mut bit, 1);
        if get(&mut bit, 1) == 1 {
            get(&mut bit, 14);
        }
        get(&mut bit, 1);
        while cfg.len() * 8 >= bit + 16 {
            let save = bit;
            if get(&mut bit, 11) == 0x2b7 {
                let ext = object_type(&mut bit);
                if ext == 5 && get(&mut bit, 1) == 1 {
                    ext_rate = rate(&mut bit);
                    if ext_rate == sample_rate {
                        ext_rate = 0;
                    }
                }
                break;
            }
            bit = save + 1;
        }
    }
    (if ext_rate > 0 { ext_rate } else { sample_rate }, channels)
}

/// An MPEG-4 descriptor's tag and body (start, end).
fn descriptor(d: &[u8], p: usize) -> Option<(u8, usize, usize)> {
    let tag = *d.get(p)?;
    let mut len = 0usize;
    let mut q = p + 1;
    for _ in 0..4 {
        let b = *d.get(q)?;
        q += 1;
        len = (len << 7) | (b & 0x7f) as usize;
        if b & 0x80 == 0 {
            break;
        }
    }
    Some((tag, q, (q + len).min(d.len())))
}

/// esds: the codec from the object type, and AAC's rate and channels.
fn read_esds(d: &[u8], start: usize, end: usize, t: &mut Track) {
    let p = start + 4; // version / flags
    let Some((tag, mut q, es_end)) = descriptor(d, p) else {
        return;
    };
    if tag != 3 {
        return;
    }
    let flags = d.get(q + 2).copied().unwrap_or(0);
    q += 3;
    if flags & 0x80 != 0 {
        q += 2;
    }
    if flags & 0x40 != 0 {
        q += 1 + d.get(q).copied().unwrap_or(0) as usize;
    }
    if flags & 0x20 != 0 {
        q += 2;
    }
    let Some((tag, q, dc_end)) = descriptor(d, q) else {
        return;
    };
    if tag != 4 || q + 13 > end.min(es_end) {
        return;
    }
    let oti = d[q];
    t.codec = Some(match oti {
        0x40 | 0x66 | 0x67 | 0x68 => Codec::Aac,
        0x69 | 0x6B => Codec::Mp3,
        0xA5 => Codec::Ac3,
        0xA6 => Codec::Eac3,
        0xA9 => Codec::Dts,
        0xAD => Codec::Opus,
        0xDD => Codec::Vorbis,
        _ => Codec::Other(format!("mp4a_{oti:02x}")),
    });
    if let Some((5, s, e)) = descriptor(d, q + 13).filter(|x| x.2 <= dc_end)
        && t.codec == Some(Codec::Aac)
    {
        let (rate, channels) = audio_specific_config(&d[s..e]);
        if channels > 0 {
            t.channels = channels;
        }
        if rate > 0 {
            t.sample_rate = rate;
        }
    }
}

fn read_stsd(d: &[u8], start: usize, end: usize, t: &mut Track) {
    let entry = start + 8;
    if entry + 36 > end {
        return;
    }
    let size = be(d, entry, 4) as usize;
    let fourcc = &d[entry + 4..entry + 8];
    let entry_end = (entry + size).min(end);
    let version = be(d, entry + 16, 2);
    t.channels = be(d, entry + 24, 2) as u32;
    t.sample_size = be(d, entry + 26, 2) as i32;
    t.sample_rate = (be(d, entry + 32, 4) >> 16) as u32;
    let mut ext = entry + 36;
    match version {
        1 => ext += 16,
        2 => {
            // Version 2: a 64-bit float rate and 32-bit channels.
            t.sample_rate = f64::from_bits(be(d, entry + 40, 8)).round() as u32;
            t.channels = be(d, entry + 48, 4) as u32;
            t.sample_size = be(d, entry + 56, 4) as i32;
            ext += 36;
        }
        _ => {}
    }
    t.codec = Some(match fourcc {
        b"mp4a" => Codec::Aac,
        b"alac" => Codec::Alac,
        b"fLaC" => Codec::Flac,
        b"Opus" => Codec::Opus,
        b"ac-3" => Codec::Ac3,
        b"ec-3" => Codec::Eac3,
        b".mp3" => Codec::Mp3,
        b"dtsc" | b"dtsh" | b"dtsl" => Codec::Dts,
        b"lpcm" | b"sowt" | b"twos" | b"in24" | b"in32" | b"fl32" | b"fl64" | b"raw " | b"NONE" => Codec::Pcm,
        b"ulaw" | b"alaw" => Codec::PcmLossy,
        other => Codec::Other(String::from_utf8_lossy(other).trim().to_lowercase()),
    });
    for (s, e, kind) in atoms(d, ext, entry_end) {
        match &kind {
            b"esds" => read_esds(d, s, e, t),
            b"wave" => {
                if let Some((ws, we)) = child(d, s, e, b"esds") {
                    read_esds(d, ws, we, t);
                }
            }
            b"alac" if e >= s + 28 => {
                // The cookie after version / flags: channels at 9, rate at 20.
                t.channels = d[s + 4 + 9] as u32;
                t.sample_rate = be(d, s + 4 + 20, 4) as u32;
            }
            _ => {}
        }
    }
}

fn read_trak(d: &[u8], start: usize, end: usize) -> Track {
    let mut t = Track::default();
    if let Some((es, ee)) = child(d, start, end, b"edts")
        && let Some((s, e)) = child(d, es, ee, b"elst")
    {
        let version = d.get(s).copied().unwrap_or(0);
        let count = be(d, s + 4, 4) as usize;
        let mut p = s + 8;
        for _ in 0..count.min(1024) {
            let (dur, time, step) = if version == 1 {
                (be(d, p, 8) as i64, be(d, p + 8, 8) as i64, 20)
            } else {
                (be(d, p, 4) as i64, be(d, p + 4, 4) as u32 as i32 as i64, 12)
            };
            if p + step > e {
                break;
            }
            t.edits.push((dur, time));
            p += step;
        }
    }
    let Some((ms, me)) = child(d, start, end, b"mdia") else {
        return t;
    };
    if let Some((s, _)) = child(d, ms, me, b"mdhd") {
        if d.get(s) == Some(&1) {
            t.timescale = be(d, s + 20, 4) as i64;
            t.mdhd_duration = be(d, s + 24, 8) as i64;
        } else {
            t.timescale = be(d, s + 12, 4) as i64;
            t.mdhd_duration = be(d, s + 16, 4) as i64;
        }
    }
    if let Some((s, _)) = child(d, ms, me, b"hdlr") {
        t.sound = d.get(s + 8..s + 12) == Some(b"soun");
    }
    let Some((ns, ne)) = child(d, ms, me, b"minf") else {
        return t;
    };
    let Some((bs, be_)) = child(d, ns, ne, b"stbl") else {
        return t;
    };
    for (s, e, kind) in atoms(d, bs, be_) {
        match &kind {
            b"stsd" => read_stsd(d, s, e, &mut t),
            b"stts" => {
                let n = be(d, s + 4, 4) as usize;
                let mut total = 0i64;
                for i in 0..n.min((e - s).saturating_sub(8) / 8) {
                    total =
                        total.saturating_add(
                            (be(d, s + 8 + 8 * i, 4) as i64).saturating_mul(be(d, s + 12 + 8 * i, 4) as i64),
                        );
                }
                t.stts_duration = total;
            }
            b"stsz" => {
                let size = be(d, s + 4, 4) as i64;
                let n = be(d, s + 8, 4) as usize;
                t.data_size = if size > 0 {
                    size.saturating_mul(n as i64)
                } else {
                    (0..n.min((e - s).saturating_sub(12) / 4)).map(|i| be(d, s + 12 + 4 * i, 4) as i64).sum()
                };
            }
            b"stz2" => {
                let field = d.get(s + 7).copied().unwrap_or(0) as usize;
                // No more entries than the atom holds.
                let room = (e - s).saturating_sub(12);
                let n = (be(d, s + 8, 4) as usize).min(match field {
                    4 => room * 2,
                    8 => room,
                    16 => room / 2,
                    _ => 0,
                });
                let mut total = 0i64;
                for i in 0..n {
                    let v = match field {
                        4 => {
                            let b = d.get(s + 12 + i / 2).copied().unwrap_or(0);
                            (if i % 2 == 0 { b >> 4 } else { b & 15 }) as i64
                        }
                        8 => d.get(s + 12 + i).copied().unwrap_or(0) as i64,
                        16 => be(d, s + 12 + 2 * i, 2) as i64,
                        _ => 0,
                    };
                    total += v;
                }
                t.data_size = total;
            }
            _ => {}
        }
    }
    t
}

/// The value of an ilst item's "data" atom as FFmpeg turns it into text.
fn data_text(key: &str, kind: &[u8; 4], dtype: u32, v: &[u8]) -> Option<String> {
    match kind {
        b"trkn" | b"disk" => {
            if v.len() < 6 {
                return None;
            }
            let current = be(v, 2, 2);
            let total = be(v, 4, 2);
            return Some(if total == 0 { current.to_string() } else { format!("{current}/{total}") });
        }
        b"gnre" => {
            if v.len() < 2 {
                return None;
            }
            let g = be(v, 0, 2) as usize;
            return (1..=ID3V1_GENRES.len()).contains(&g).then(|| ID3V1_GENRES[g - 1].to_string());
        }
        b"egid" | b"hdvd" | b"pcst" | b"pgap" | b"rtng" | b"stik" | b"tves" | b"tvsn" | b"cpil" => {
            return v.first().map(|b| b.to_string());
        }
        _ => {}
    }
    let _ = key;
    match dtype {
        2 => Some(utf16(v.as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes([c[0], c[1]])))),
        21 | 22 => {
            let n = v.len().min(8);
            if n == 0 {
                return None;
            }
            let raw = be(v, 0, n);
            if dtype == 21 {
                let shift = 64 - 8 * n as u32;
                Some((((raw << shift) as i64) >> shift).to_string())
            } else {
                Some(raw.to_string())
            }
        }
        _ => Some(String::from_utf8_lossy(v).trim_end_matches('\0').to_string()),
    }
}

fn ilst_key(kind: &[u8; 4]) -> Option<&'static str> {
    Some(match kind {
        b"aART" => "album_artist",
        b"cprt" => "copyright",
        b"desc" => "description",
        b"disk" => "disc",
        b"gnre" => "genre",
        b"keyw" => "keywords",
        b"ldes" => "synopsis",
        b"purd" => "purchase_date",
        b"rtng" => "rating",
        b"soaa" => "sort_album_artist",
        b"soal" => "sort_album",
        b"soar" => "sort_artist",
        b"soco" => "sort_composer",
        b"sonm" => "sort_name",
        b"sosn" => "sort_show",
        b"stik" => "media_type",
        b"trkn" => "track",
        b"tven" => "episode_id",
        b"tvnn" => "network",
        b"tvsh" => "show",
        b"cpil" => "compilation",
        b"pgap" => "gapless_playback",
        b"tmpo" => "tmpo",
        _ if kind[0] == 0xa9 => match &kind[1..] {
            b"ART" | b"aut" => "artist",
            b"PRD" | b"prd" => "producer",
            b"alb" => "album",
            b"cmt" | b"inf" => "comment",
            b"com" | b"wrt" => "composer",
            b"cpy" => "copyright",
            b"day" => "date",
            b"dir" => "director",
            b"enc" | b"swr" | b"too" => "encoder",
            b"gen" => "genre",
            b"grp" => "grouping",
            b"lyr" => "lyrics",
            b"nam" => "title",
            b"ope" => "original_artist",
            b"prf" => "performers",
            b"trk" => "track",
            b"xyz" => "location",
            _ => return None,
        },
        _ => return None,
    })
}

/// ilst: iTunes items into `meta`, covers into `pictures`.
fn read_ilst(d: &[u8], start: usize, end: usize, keys: &[String], meta: &mut Dict, pictures: &mut Vec<Picture>) {
    for (s, e, kind) in atoms(d, start, end) {
        if &kind == b"----" {
            let mut name = String::new();
            let mut value = None;
            for (cs, ce, ck) in atoms(d, s, e) {
                match &ck {
                    b"name" if ce >= cs + 4 => name = String::from_utf8_lossy(&d[cs + 4..ce]).into_owned(),
                    b"data" if ce >= cs + 8 => value = Some(String::from_utf8_lossy(&d[cs + 8..ce]).into_owned()),
                    _ => {}
                }
            }
            if let (false, Some(v)) = (name.is_empty(), value) {
                meta.set(&name, &v);
            }
            continue;
        }
        if &kind == b"covr" {
            for (cs, ce, ck) in atoms(d, s, e) {
                if &ck == b"data" && ce > cs + 8 {
                    let dtype = be(d, cs, 4) & 0xffffff;
                    if matches!(dtype, 13 | 14 | 27) {
                        pictures.push(Picture { data: d[cs + 8..ce].to_vec(), front: false });
                    }
                }
            }
            continue;
        }
        // "keys"-indexed items (mdta): the atom type is a 1-based index.
        let index = u32::from_be_bytes(kind) as usize;
        let key: String = if !keys.is_empty() && index >= 1 && index <= keys.len() {
            keys[index - 1].clone()
        } else {
            match ilst_key(&kind) {
                Some(k) => k.to_string(),
                None => continue,
            }
        };
        for (cs, ce, ck) in atoms(d, s, e) {
            if &ck == b"data" && ce >= cs + 8 {
                let dtype = (be(d, cs, 4) & 0xffffff) as u32;
                if let Some(text) = data_text(&key, &kind, dtype, &d[cs + 8..ce]) {
                    meta.set(&key, &text);
                }
                break;
            }
        }
    }
}

/// meta: its handler, the "keys" atom and the ilst.
fn read_meta(d: &[u8], start: usize, end: usize, meta: &mut Dict, pictures: &mut Vec<Picture>) {
    // A QuickTime meta has no version / flags before its children.
    let body = if d.get(start + 4..start + 8) == Some(b"hdlr") { start } else { start + 4 };
    let mut keys = Vec::new();
    if let Some((s, e)) = child(d, body, end, b"keys") {
        let n = be(d, s + 4, 4) as usize;
        let mut p = s + 8;
        for _ in 0..n.min(4096) {
            let size = be(d, p, 4) as usize;
            if size < 8 || p + size > e {
                break;
            }
            keys.push(String::from_utf8_lossy(&d[p + 8..p + size]).into_owned());
            p += size;
        }
    }
    if let Some((s, e)) = child(d, body, end, b"ilst") {
        read_ilst(d, s, e, &keys, meta, pictures);
    }
}

/// udta: its meta, and QuickTime "©xxx" strings (size, language, text).
fn read_udta(d: &[u8], start: usize, end: usize, meta: &mut Dict, pictures: &mut Vec<Picture>) {
    for (s, e, kind) in atoms(d, start, end) {
        if &kind == b"meta" {
            read_meta(d, s, e, meta, pictures);
        } else if kind[0] == 0xa9 && e >= s + 4 {
            let Some(key) = ilst_key(&kind) else { continue };
            let len = be(d, s, 2) as usize;
            if s + 4 + len <= e {
                meta.set(key, String::from_utf8_lossy(&d[s + 4..s + 4 + len]).trim_end_matches('\0'));
            }
        }
    }
}

/// The first sound track of an MP4 file: what its edit list plays, in
/// the track's own units: (frames skipped, frames played, timescale).
pub(crate) fn edit_range(path: &std::path::Path) -> Option<(u64, u64, u64)> {
    let mut src = Src::open(path)?;
    let d = moov(&mut src, 0)?;
    let body_start = if be(&d, 0, 4) == 1 { 16 } else { 8 };
    let mut movie_timescale = 0i64;
    for (s, e, kind) in atoms(&d, body_start, d.len()) {
        match &kind {
            b"mvhd" => {
                movie_timescale = if d.get(s) == Some(&1) { be(&d, s + 20, 4) as i64 } else { be(&d, s + 12, 4) as i64 }
            }
            b"trak" => {
                let t = read_trak(&d, s, e);
                if !t.sound || t.codec.is_none() {
                    continue;
                }
                let media = if t.mdhd_duration > 0 { t.mdhd_duration } else { t.stts_duration };
                let edit = t.edits.iter().find(|e| e.1 >= 0);
                let (skip, played) = match edit {
                    Some(&(dur, time)) if movie_timescale > 0 => (time, rescale(dur, t.timescale, movie_timescale)),
                    _ => (0, media),
                };
                return Some((skip.max(0) as u64, played.max(0) as u64, t.timescale.max(1) as u64));
            }
            _ => {}
        }
    }
    None
}

fn moov(src: &mut Src, start: u64) -> Option<Vec<u8>> {
    let mut pos = start;
    while pos + 8 <= src.size {
        let h = src.read_at(pos, 16);
        let mut size = be(&h, 0, 4);
        let mut hdr = 8;
        if size == 1 {
            size = be(&h, 8, 8);
            hdr = 16;
        } else if size == 0 {
            size = src.size - pos;
        }
        if size < hdr {
            return None;
        }
        if &h[4..8] == b"moov" {
            return Some(src.read_at(pos, size.min(256 << 20)));
        }
        pos = pos.checked_add(size)?;
    }
    None
}

pub(super) fn read(src: &mut Src, start: u64) -> Option<Demuxed> {
    // Only the moov atom is needed: find it among the top-level atoms.
    let d = moov(src, start)?;
    let body_start = if be(&d, 0, 4) == 1 { 16 } else { 8 };
    let mut movie_timescale = 0i64;
    let mut sound: Option<Track> = None;
    let mut meta = Dict::default();
    let mut pictures = Vec::new();
    for (s, e, kind) in atoms(&d, body_start, d.len()) {
        match &kind {
            b"mvhd" => {
                movie_timescale =
                    if d.get(s) == Some(&1) { be(&d, s + 20, 4) as i64 } else { be(&d, s + 12, 4) as i64 };
            }
            b"trak" => {
                let t = read_trak(&d, s, e);
                if t.sound && sound.is_none() && t.codec.is_some() {
                    sound = Some(t);
                }
            }
            b"udta" => read_udta(&d, s, e, &mut meta, &mut pictures),
            b"meta" => read_meta(&d, s, e, &mut meta, &mut pictures),
            _ => {}
        }
    }
    let t = sound?;
    let mut out = Demuxed::new(t.codec.clone()?, "mov,mp4,m4a,3gp,3g2,mj2");
    out.sample_rate = t.sample_rate;
    out.channels = t.channels;
    out.bits_coded = t.sample_size;
    out.time_base = (1, t.timescale.max(1));
    let media = if t.mdhd_duration > 0 { t.mdhd_duration } else { t.stts_duration };
    // The bit rate counts the whole media (priming included).
    if media > 0 {
        out.stream_bit_rate = rescale(t.data_size, t.timescale * 8, media);
    }
    // The length the edit list plays.
    let mut duration = media;
    let played: Vec<i64> = t.edits.iter().filter(|e| e.1 >= 0).map(|e| e.0).collect();
    if !played.is_empty() && movie_timescale > 0 {
        let total = played.iter().fold(0i64, |sum, &p| sum.saturating_add(rescale(p, t.timescale, movie_timescale)));
        if total > 0 {
            duration = duration.min(total);
        }
    }
    out.duration_ts = (duration > 0).then_some(duration);
    out.format_tags = meta;
    out.pictures = pictures;
    Some(out)
}
