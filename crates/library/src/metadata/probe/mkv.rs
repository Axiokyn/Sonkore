//! Matroska / WebM (libavformat/matroskadec.c): the first audio track, the
//! segment's duration, its tags (also those the SeekHead points to after
//! the clusters) and image attachments.

use super::{Codec, Demuxed, Dict, Picture, Src, be};

const SEGMENT: u64 = 0x18538067;
const SEEK_HEAD: u64 = 0x114D9B74;
const SEEK: u64 = 0x4DBB;
const SEEK_ID: u64 = 0x53AB;
const SEEK_POSITION: u64 = 0x53AC;
const INFO: u64 = 0x1549A966;
const TIMECODE_SCALE: u64 = 0x2AD7B1;
const DURATION: u64 = 0x4489;
const TITLE: u64 = 0x7BA9;
const TRACKS: u64 = 0x1654AE6B;
const TRACK_ENTRY: u64 = 0xAE;
const TRACK_UID: u64 = 0x73C5;
const TRACK_TYPE: u64 = 0x83;
const CODEC_ID: u64 = 0x86;
const CODEC_PRIVATE: u64 = 0x63A2;
const TRACK_NAME: u64 = 0x536E;
const AUDIO: u64 = 0xE1;
const SAMPLING_FREQUENCY: u64 = 0xB5;
const OUTPUT_SAMPLING_FREQUENCY: u64 = 0x78B5;
const CHANNELS: u64 = 0x9F;
const BIT_DEPTH: u64 = 0x6264;
const TAGS: u64 = 0x1254C367;
const TAG: u64 = 0x7373;
const TARGETS: u64 = 0x63C0;
const TAG_TRACK_UID: u64 = 0x63C5;
const TAG_ATTACHMENT_UID: u64 = 0x63C6;
const TAG_CHAPTER_UID: u64 = 0x63C4;
const SIMPLE_TAG: u64 = 0x67C8;
const TAG_NAME: u64 = 0x45A3;
const TAG_LANGUAGE: u64 = 0x447A;
const TAG_DEFAULT: u64 = 0x4484;
const TAG_STRING: u64 = 0x4487;
const ATTACHMENTS: u64 = 0x1941A469;
const ATTACHED_FILE: u64 = 0x61A7;
const FILE_MIME: u64 = 0x4660;
const FILE_DATA: u64 = 0x465C;
const CLUSTER: u64 = 0x1F43B675;

/// An EBML id (marker kept) or size (marker removed; None = unknown).
fn vint(d: &[u8], p: &mut usize, keep_marker: bool) -> Option<Option<u64>> {
    let first = *d.get(*p)?;
    let len = first.leading_zeros() as usize + 1;
    if len > 8 || *p + len > d.len() {
        return None;
    }
    let mut v = if keep_marker { first as u64 } else { (first as u64) & ((1u64 << (8 - len)) - 1) };
    let mut all_ones = v == (1u64 << (8 - len)) - 1;
    for i in 1..len {
        v = (v << 8) | d[*p + i] as u64;
        all_ones &= d[*p + i] == 0xff;
    }
    *p += len;
    Some(if !keep_marker && all_ones { None } else { Some(v) })
}

/// The children of `d[start..end]`: (id, body start, body end).
fn elements(d: &[u8], start: usize, end: usize) -> Vec<(u64, usize, usize)> {
    let mut out = Vec::new();
    let mut p = start;
    let end = end.min(d.len());
    while p < end {
        let Some(Some(id)) = vint(d, &mut p, true) else {
            break;
        };
        let Some(size) = vint(d, &mut p, false) else {
            break;
        };
        // A header that runs past its parent's end is damage (its size
        // borrowed the next element's bytes): stop there.
        if p > end {
            break;
        }
        let body_end = match size {
            Some(s) => (p as u64).saturating_add(s).min(end as u64) as usize,
            None => end,
        };
        out.push((id, p, body_end));
        p = body_end;
    }
    out
}

fn uint(d: &[u8]) -> u64 {
    be(d, 0, d.len().min(8))
}

fn float(d: &[u8]) -> f64 {
    match d.len() {
        4 => f32::from_bits(be(d, 0, 4) as u32) as f64,
        8 => f64::from_bits(be(d, 0, 8)),
        _ => 0.0,
    }
}

fn text(d: &[u8]) -> String {
    let end = d.iter().position(|&b| b == 0).unwrap_or(d.len());
    String::from_utf8_lossy(&d[..end]).into_owned()
}

#[derive(Default)]
struct State {
    timecode_scale: u64,
    duration: f64,
    title: Option<String>,
    track: Option<Demuxed>,
    track_uid: u64,
    format_tags: Dict,
    track_tags: Vec<(u64, Dict)>,
    pictures: Vec<Picture>,
    seen: Vec<u64>,
}

/// How deep SimpleTags nest at most: far more than files use (FFmpeg stops
/// a few levels sooner, its EBML_MAX_DEPTH counting the levels above them).
const MAX_TAG_DEPTH: usize = 16;

fn simple_tags(d: &[u8], start: usize, end: usize, prefix: Option<&str>, meta: &mut Dict, depth: usize) {
    if depth >= MAX_TAG_DEPTH {
        return;
    }
    for (id, s, e) in elements(d, start, end) {
        if id != SIMPLE_TAG {
            continue;
        }
        let (mut name, mut lang, mut default, mut value) = (String::new(), String::new(), true, None);
        for (cid, cs, ce) in elements(d, s, e) {
            match cid {
                TAG_NAME => name = text(&d[cs..ce]),
                TAG_LANGUAGE => lang = text(&d[cs..ce]),
                TAG_DEFAULT => default = uint(&d[cs..ce]) != 0,
                TAG_STRING => value = Some(text(&d[cs..ce])),
                _ => {}
            }
        }
        if name.is_empty() {
            continue;
        }
        let key = match prefix {
            Some(p) => format!("{p}/{name}"),
            None => name.clone(),
        };
        if let Some(v) = &value {
            if default || lang.is_empty() || lang == "und" {
                meta.set(&key, v);
            } else {
                meta.set(&format!("{key}-{lang}"), v);
            }
        }
        simple_tags(d, s, e, Some(&key), meta, depth + 1);
    }
}

fn level1(st: &mut State, id: u64, d: &[u8]) {
    match id {
        INFO => {
            for (cid, s, e) in elements(d, 0, d.len()) {
                match cid {
                    TIMECODE_SCALE => st.timecode_scale = uint(&d[s..e]),
                    DURATION => st.duration = float(&d[s..e]),
                    TITLE => st.title = Some(text(&d[s..e])),
                    _ => {}
                }
            }
        }
        TRACKS => {
            for (tid, ts, te) in elements(d, 0, d.len()) {
                if tid != TRACK_ENTRY || st.track.is_some() {
                    continue;
                }
                let (mut kind, mut codec, mut uid, mut name) = (0u64, String::new(), 0u64, None);
                let (mut rate, mut out_rate, mut channels, mut depth) = (0.0f64, 0.0f64, 1u64, 0u64);
                let mut private: &[u8] = &[];
                for (cid, s, e) in elements(d, ts, te) {
                    match cid {
                        TRACK_TYPE => kind = uint(&d[s..e]),
                        CODEC_ID => codec = text(&d[s..e]),
                        TRACK_UID => uid = uint(&d[s..e]),
                        TRACK_NAME => name = Some(text(&d[s..e])),
                        CODEC_PRIVATE => private = &d[s..e],
                        AUDIO => {
                            rate = 8000.0;
                            for (aid, as_, ae) in elements(d, s, e) {
                                match aid {
                                    SAMPLING_FREQUENCY => rate = float(&d[as_..ae]),
                                    OUTPUT_SAMPLING_FREQUENCY => out_rate = float(&d[as_..ae]),
                                    CHANNELS => channels = uint(&d[as_..ae]),
                                    BIT_DEPTH => depth = uint(&d[as_..ae]),
                                    _ => {}
                                }
                            }
                        }
                        _ => {}
                    }
                }
                if kind != 2 {
                    continue;
                }
                let c = match codec.as_str() {
                    "A_FLAC" => Codec::Flac,
                    "A_OPUS" => Codec::Opus,
                    "A_VORBIS" => Codec::Vorbis,
                    "A_AC3" => Codec::Ac3,
                    "A_EAC3" => Codec::Eac3,
                    "A_MPEG/L3" => Codec::Mp3,
                    "A_MPEG/L2" => Codec::Mp2,
                    "A_ALAC" => Codec::Alac,
                    "A_WAVPACK4" => Codec::WavPack,
                    "A_TTA1" => Codec::Tta,
                    "A_TRUEHD" => Codec::TrueHd,
                    c if c.starts_with("A_AAC") => Codec::Aac,
                    c if c.starts_with("A_DTS") => Codec::Dts,
                    c if c.starts_with("A_PCM/") => Codec::Pcm,
                    "A_MS/ACM" => {
                        let mut tmp = Demuxed::new(Codec::Pcm, "matroska,webm");
                        super::pcm::wave_format(private, &mut tmp);
                        tmp.codec
                    }
                    other => Codec::Other(other.trim_start_matches("A_").to_lowercase()),
                };
                let mut t = Demuxed::new(c, "matroska,webm");
                t.sample_rate = (if out_rate > 0.0 { out_rate } else { rate }) as u32;
                t.channels = channels as u32;
                t.bits_coded = depth as i32;
                if let Some(n) = name {
                    t.stream_tags.set("title", &n);
                }
                st.track_uid = uid;
                st.track = Some(t);
            }
        }
        TAGS => {
            for (tid, ts, te) in elements(d, 0, d.len()) {
                if tid != TAG {
                    continue;
                }
                let (mut track_uid, mut other_target) = (0u64, false);
                if let Some((_, s, e)) = elements(d, ts, te).into_iter().find(|x| x.0 == TARGETS) {
                    for (cid, cs, ce) in elements(d, s, e) {
                        match cid {
                            TAG_TRACK_UID => track_uid = uint(&d[cs..ce]),
                            TAG_ATTACHMENT_UID | TAG_CHAPTER_UID => other_target = uint(&d[cs..ce]) != 0,
                            _ => {}
                        }
                    }
                }
                if other_target {
                    continue;
                }
                if track_uid != 0 {
                    let mut m = Dict::default();
                    simple_tags(d, ts, te, None, &mut m, 0);
                    st.track_tags.push((track_uid, m));
                } else {
                    simple_tags(d, ts, te, None, &mut st.format_tags, 0);
                }
            }
        }
        ATTACHMENTS => {
            for (aid, s, e) in elements(d, 0, d.len()) {
                if aid != ATTACHED_FILE {
                    continue;
                }
                let (mut mime, mut data) = (String::new(), Vec::new());
                for (cid, cs, ce) in elements(d, s, e) {
                    match cid {
                        FILE_MIME => mime = text(&d[cs..ce]),
                        FILE_DATA => data = d[cs..ce].to_vec(),
                        _ => {}
                    }
                }
                let image = ["image/gif", "image/jpeg", "image/png", "image/tiff", "image/bmp", "image/webp"]
                    .iter()
                    .any(|m| mime.starts_with(m));
                if image && !data.is_empty() {
                    st.pictures.push(Picture { data, front: false });
                }
            }
        }
        _ => {}
    }
}

pub(super) fn read(src: &mut Src) -> Option<Demuxed> {
    // The EBML header, then the segment.
    let head = src.read_at(0, 64);
    let mut p = 0;
    vint(&head, &mut p, true)?;
    let header_size = vint(&head, &mut p, false)??;
    let mut pos = p as u64 + header_size;
    let seg = src.read_at(pos, 16);
    let mut q = 0;
    if vint(&seg, &mut q, true)?? != SEGMENT {
        return None;
    }
    let seg_size = vint(&seg, &mut q, false)?;
    let seg_start = pos + q as u64;
    let seg_end = seg_size.map(|s| seg_start + s).unwrap_or(src.size).min(src.size);
    pos = seg_start;
    let mut st = State { timecode_scale: 1_000_000, ..Default::default() };
    let mut seeks: Vec<(u64, u64)> = Vec::new();
    // Level-1 elements up to the first cluster.
    while pos < seg_end {
        let h = src.read_at(pos, 16);
        let mut i = 0;
        let Some(Some(id)) = vint(&h, &mut i, true) else {
            break;
        };
        let Some(size) = vint(&h, &mut i, false) else {
            break;
        };
        let body = pos + i as u64;
        if id == CLUSTER {
            break;
        }
        let Some(size) = size else { break };
        if matches!(id, SEEK_HEAD | INFO | TRACKS | TAGS | ATTACHMENTS) && size < (64 << 20) {
            let data = src.read_at(body, size);
            if id == SEEK_HEAD {
                for (sid, s, e) in elements(&data, 0, data.len()) {
                    if sid != SEEK {
                        continue;
                    }
                    let (mut target, mut at) = (0u64, 0u64);
                    for (cid, cs, ce) in elements(&data, s, e) {
                        match cid {
                            SEEK_ID => target = uint(&data[cs..ce]),
                            SEEK_POSITION => at = uint(&data[cs..ce]),
                            _ => {}
                        }
                    }
                    seeks.push((target, at));
                }
            } else if !st.seen.contains(&body) {
                st.seen.push(body);
                level1(&mut st, id, &data);
            }
        }
        pos = body + size;
    }
    // What the SeekHead points to further on (tags written at the end).
    for (target, at) in seeks {
        if !matches!(target, INFO | TRACKS | TAGS | ATTACHMENTS) {
            continue;
        }
        let Some(at) = seg_start.checked_add(at) else {
            continue;
        };
        let h = src.read_at(at, 16);
        let mut i = 0;
        let Some(Some(id)) = vint(&h, &mut i, true) else {
            continue;
        };
        let Some(Some(size)) = vint(&h, &mut i, false) else {
            continue;
        };
        let body = at + i as u64;
        if id != target || st.seen.contains(&body) || size >= (64 << 20) {
            continue;
        }
        st.seen.push(body);
        let data = src.read_at(body, size);
        level1(&mut st, id, &data);
    }
    let mut d = st.track.take()?;
    let mut format = Dict::default();
    if let Some(t) = &st.title {
        format.set("title", t);
    }
    st.format_tags.copy_into(&mut format);
    // ff_mkv_metadata_conv
    let conv: &[(&str, &str)] = &[("LEAD_PERFORMER", "performer"), ("PART_NUMBER", "track")];
    format.convert(conv);
    for (uid, m) in &st.track_tags {
        if *uid == st.track_uid {
            m.copy_into(&mut d.stream_tags);
        }
    }
    d.stream_tags.convert(conv);
    d.format_tags = format;
    d.pictures = st.pictures;
    if st.duration > 0.0 {
        d.format_duration_us = Some((st.duration * st.timecode_scale as f64 * 1000.0 / 1_000_000.0) as i64);
    }
    Some(d)
}
