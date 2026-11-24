//! Tags and stream facts: what the library knows of a file.
//!
//! FLAC, MP3 and WAV are read by Sonkore's own parsers (`native`); the
//! other formats earlier versions read through FFmpeg, whose view of them
//! `probe` rebuilds, so every field is filled the same way.

pub(crate) mod native;
pub(crate) mod probe;
pub(crate) mod text;

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::{Arc, RwLock};
use std::time::UNIX_EPOCH;

use crate::cue::{self, CueSheet};
use crate::meta::TrackMeta;
use text::{leading_int, leading_number};

/// A cover being looked for: the image bytes found so far.
#[derive(Default)]
pub(crate) struct Cover {
    pub data: Vec<u8>,
}

pub(crate) fn be32(p: &[u8]) -> u32 {
    if p.len() < 4 {
        return 0;
    }
    u32::from_be_bytes([p[0], p[1], p[2], p[3]])
}
pub(crate) fn be24(p: &[u8]) -> u32 {
    if p.len() < 3 {
        return 0;
    }
    ((p[0] as u32) << 16) | ((p[1] as u32) << 8) | p[2] as u32
}
pub(crate) fn le32(p: &[u8]) -> u32 {
    if p.len() < 4 {
        return 0;
    }
    u32::from_le_bytes([p[0], p[1], p[2], p[3]])
}
pub(crate) fn le16(p: &[u8]) -> u16 {
    if p.len() < 2 {
        return 0;
    }
    u16::from_le_bytes([p[0], p[1]])
}
pub(crate) fn syncsafe(p: &[u8]) -> u32 {
    if p.len() < 4 {
        return 0;
    }
    ((p[0] as u32 & 0x7f) << 21) | ((p[1] as u32 & 0x7f) << 14) | ((p[2] as u32 & 0x7f) << 7) | (p[3] as u32 & 0x7f)
}

/// ReplayGain (REPLAYGAIN_*: dB towards -18 LUFS) and Opus R128 (R128_*:
/// Q7.8 dB towards -23 LUFS, so 5 dB is added for the same reference).
/// True when the key is a loudness key (whether or not its value read).
pub(crate) fn apply_loudness_tag(t: &mut TrackMeta, key: &str, value: &str) -> bool {
    if !key.starts_with("REPLAYGAIN_") && !key.starts_with("R128_") {
        return false;
    }
    let v = leading_number(value);
    if v.is_nan() {
        return true;
    }
    match key {
        "REPLAYGAIN_TRACK_GAIN" => t.rg_track_gain = v,
        "REPLAYGAIN_TRACK_PEAK" => t.rg_track_peak = v,
        "REPLAYGAIN_ALBUM_GAIN" => t.rg_album_gain = v,
        "REPLAYGAIN_ALBUM_PEAK" => t.rg_album_peak = v,
        "R128_TRACK_GAIN" => t.rg_track_gain = v / 256.0 + 5.0,
        "R128_ALBUM_GAIN" => t.rg_album_gain = v / 256.0 + 5.0,
        _ => {}
    }
    true
}

/// One "KEY=value" tag (Vorbis comments, FFmpeg's dictionary).
pub(crate) fn apply_tag(t: &mut TrackMeta, key: &str, value: &str) {
    let key = key.to_uppercase();
    if value.is_empty() {
        return;
    }
    if apply_loudness_tag(t, &key, value) {
        return;
    }
    match key.as_str() {
        "TITLE" => t.title = value.to_string(),
        "ARTIST" => t.artist = Arc::from(value),
        "ALBUM" => t.album = Arc::from(value),
        "ALBUMARTIST" | "ALBUM ARTIST" | "ALBUM_ARTIST" => t.album_artist = Arc::from(value),
        "GENRE" => t.genre = Arc::from(value),
        "TRACKNUMBER" | "TRACK" => t.track_no = leading_int(value),
        "DISCNUMBER" | "DISC" => t.disc_no = leading_int(value),
        "DATE" | "YEAR" | "ORIGINALDATE" if t.year == 0 => t.year = leading_int(value),
        _ => {}
    }
}

/// Formats the library lists, lower case without the dot: those of earlier
/// versions (their own readers plus FFmpeg's), so all list the same files.
pub const AUDIO_EXTENSIONS: [&str; 29] = [
    "flac", "wav", "wave", "mp3", "m4a", "m4b", "mp4", "aac", "caf", "ogg", "oga", "opus", "aif", "aiff", "aifc",
    "w64", "ape", "wv", "tta", "tak", "mpc", "dsf", "dff", "wma", "mka", "webm", "ac3", "dts", "mp2",
];

/// The formats earlier versions read through FFmpeg.
fn uses_probe(ext: &str) -> bool {
    AUDIO_EXTENSIONS[4..].contains(&ext)
}

/// AUDIO_EXTENSIONS (Metadata::audioExtensions).
pub fn audio_extensions() -> &'static [&'static str] {
    &AUDIO_EXTENSIONS
}

/// What follows the last '.' of the whole path, lower case ("" without one).
pub(crate) fn extension_of(path: &str) -> String {
    match path.rfind('.') {
        Some(i) => path[i + 1..].to_lowercase(),
        None => String::new(),
    }
}

/// A file Sonkore plays (by its extension), or a CD image track.
pub fn is_audio_file(path: &str) -> bool {
    if cue::is_track_path(path) {
        return true;
    }
    let ext = extension_of(path);
    !ext.is_empty() && audio_extensions().contains(&ext.as_str())
}

fn read_probed(path: &str, t: &mut TrackMeta, cover: Option<&mut Cover>) -> bool {
    let want = cover.is_some();
    let Some(r) = probe::probe(Path::new(path), want) else {
        return false;
    };
    t.codec = Arc::from(r.codec.as_str());
    t.sample_rate = r.sample_rate as i32;
    t.channels = r.channels as i32;
    t.bits = if r.lossless { r.bits as i32 } else { 0 };
    t.duration = r.duration;
    t.bitrate_kbps = r.bitrate_kbps;
    t.has_embedded_cover = r.has_cover;
    for (key, value) in &r.tags {
        apply_tag(t, key, value.trim());
    }
    if let Some(c) = cover
        && !r.cover.is_empty()
    {
        c.data = r.cover;
    }
    true
}

/// A file's format and tags; false when it is not readable audio. A reader
/// that panics on a damaged file counts as that (the scan threads go on, and
/// the file is left out as any other unreadable one).
pub(crate) fn read_any(path: &str, t: &mut TrackMeta, cover: Option<&mut Cover>) -> bool {
    sonkore_core::panic_guard::catch(|| read_any_unguarded(path, t, cover)).unwrap_or(false)
}

fn read_any_unguarded(path: &str, t: &mut TrackMeta, cover: Option<&mut Cover>) -> bool {
    let ext = extension_of(path);
    if uses_probe(&ext) {
        return read_probed(path, t, cover);
    }
    let Some(mut f) = native::Reader::open(Path::new(path)) else {
        return false;
    };
    match ext.as_str() {
        "flac" => native::read_flac(&mut f, t, cover),
        "mp3" => native::read_mp3(&mut f, t, cover),
        "wav" | "wave" => native::read_wav(&mut f, t, cover),
        _ => false,
    }
}

/// A file's size and modification time (ms since the epoch), following
/// links; (0, 0) when it is not there.
pub(crate) fn file_stamp(path: &str) -> (i64, i64) {
    match fs::metadata(path) {
        Ok(m) => {
            let ms = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            (m.len() as i64, ms)
        }
        Err(_) => (0, 0),
    }
}

/// The name without its last suffix.
pub(crate) fn complete_base_name(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or("");
    match name.rfind('.') {
        Some(i) => name[..i].to_string(),
        None => name.to_string(),
    }
}

/// The suffix: after the last '.' of the name.
fn suffix(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or("");
    match name.rfind('.') {
        Some(i) => name[i + 1..].to_string(),
        None => String::new(),
    }
}

/// The parent folder's name.
fn dir_name(path: &str) -> String {
    let parent = match path.rfind('/') {
        Some(0) => return String::new(),
        Some(i) => &path[..i],
        None => return ".".into(),
    };
    parent.rsplit('/').next().unwrap_or("").to_string()
}

static UNTITLED_TRACK: RwLock<Option<String>> = RwLock::new(None);

/// The title a CD image track without one gets: "{0}" is its number. The
/// interface sets the translation of "Track {0}".
pub fn set_untitled_track_label(pattern: &str) {
    *UNTITLED_TRACK.write().unwrap() = Some(pattern.to_string());
}

fn untitled_track(number: i32) -> String {
    let pattern = UNTITLED_TRACK.read().unwrap().clone().unwrap_or_else(|| "Track {0}".into());
    pattern.replace("{0}", &number.to_string())
}

/// One track of a CD image: its own tags from the sheet; format, cover and
/// the image's ReplayGain (the whole image is the album) from the file.
fn read_cue_track(path: &str, out: &mut TrackMeta) -> bool {
    let Some((sheet_path, number)) = cue::split_track_path(path) else {
        return false;
    };
    let Some(sheet) = CueSheet::parse(Path::new(&sheet_path)) else {
        return false;
    };
    let Some(track) = sheet.track(number) else {
        return false;
    };
    let mut image = TrackMeta::default();
    let audio = track.file.to_string_lossy().into_owned();
    read_any(&audio, &mut image, None);
    let (audio_size, audio_modified) = file_stamp(&audio);
    let (_, sheet_modified) = file_stamp(&sheet_path);

    out.path = path.to_string();
    out.title = if track.title.is_empty() { untitled_track(number) } else { track.title.clone() };
    out.artist =
        Arc::from(if track.performer.is_empty() { sheet.performer.as_str() } else { track.performer.as_str() });
    out.album = if sheet.title.is_empty() { image.album.clone() } else { Arc::from(sheet.title.as_str()) };
    out.album_artist =
        if sheet.performer.is_empty() { image.album_artist.clone() } else { Arc::from(sheet.performer.as_str()) };
    out.genre = if sheet.genre.is_empty() { image.genre.clone() } else { Arc::from(sheet.genre.as_str()) };
    out.year = if sheet.date.is_empty() { image.year } else { leading_int(&sheet.date) };
    out.track_no = number;
    out.disc_no = image.disc_no;
    out.codec = image.codec.clone();
    out.sample_rate = image.sample_rate;
    out.bits = image.bits;
    out.channels = image.channels;
    out.bitrate_kbps = image.bitrate_kbps;
    out.has_embedded_cover = image.has_embedded_cover;
    let start = track.start as f64 / 75.0;
    let end = if track.end > 0 { track.end as f64 / 75.0 } else { image.duration };
    out.duration = (end - start).max(0.0);
    // Its share of the image, so the average bit rate stays meaningful.
    out.file_size = if image.duration > 0.0 { (audio_size as f64 * (out.duration / image.duration)) as i64 } else { 0 };
    out.modified = audio_modified.max(sheet_modified);
    if !track.gain.is_nan() {
        out.rg_track_gain = track.gain;
        out.rg_track_peak = track.peak;
    }
    if !sheet.album_gain.is_nan() {
        out.rg_album_gain = sheet.album_gain;
        out.rg_album_peak = sheet.album_peak;
    } else if !image.rg_album_gain.is_nan() || !image.rg_track_gain.is_nan() {
        let use_track = image.rg_album_gain.is_nan();
        out.rg_album_gain = if use_track { image.rg_track_gain } else { image.rg_album_gain };
        out.rg_album_peak = if use_track { image.rg_track_peak } else { image.rg_album_peak };
    }
    true
}

/// The dates of a sheet's CD image tracks, the sheet read once: for each,
/// the newer of the sheet's and its image file's (what its entry was read
/// from).
pub(crate) struct CueStamps {
    tracks: HashMap<i32, i64>,
    sheet: i64,
}

impl CueStamps {
    pub(crate) fn of(&self, number: i32) -> i64 {
        // A number the sheet lacks: no image, the sheet's own date.
        self.tracks.get(&number).copied().unwrap_or(self.sheet.max(0))
    }
}

/// None when the sheet does not read (its tracks' date is then -1).
pub(crate) fn cue_stamps(sheet_path: &str) -> Option<CueStamps> {
    let sheet = CueSheet::parse(Path::new(sheet_path))?;
    let sheet_stamp = file_stamp(sheet_path).1;
    let mut images: HashMap<&Path, i64> = HashMap::new();
    let mut tracks = HashMap::new();
    for t in &sheet.tracks {
        let image = *images.entry(t.file.as_path()).or_insert_with(|| file_stamp(&t.file.to_string_lossy()).1);
        tracks.entry(t.number).or_insert(image.max(sheet_stamp)); // the first of a number, as `track` finds it
    }
    Some(CueStamps { tracks, sheet: sheet_stamp })
}

/// The playable tracks of a .cue sheet (`<sheet>#<n>`); `images` gets the
/// audio files it splits (listed through the sheet, not on their own).
pub fn cue_tracks(sheet: &str, mut images: Option<&mut Vec<String>>) -> Vec<String> {
    let mut paths = Vec::new();
    let Some(cue) = CueSheet::parse(Path::new(sheet)) else {
        return paths;
    };
    for t in &cue.tracks {
        let file = t.file.to_string_lossy().into_owned();
        if !is_audio_file(&file) {
            continue;
        }
        paths.push(cue::track_path(sheet, t.number));
        if let Some(images) = images.as_deref_mut()
            && !images.contains(&file)
        {
            images.push(file);
        }
    }
    paths
}

/// Reads a file's tags and format into `out` (Metadata::read): false when
/// the file could not be read as audio. `out` is filled either way, with the
/// fallbacks (title from the file name, album from the folder's).
pub fn read_into(path: &str, out: &mut TrackMeta) -> bool {
    *out = TrackMeta::default();
    if cue::is_track_path(path) {
        return read_cue_track(path, out);
    }
    out.path = path.to_string();
    let (size, modified) = file_stamp(path);
    out.file_size = size;
    out.modified = modified;
    let ok = read_any(path, out, None);
    if out.title.is_empty() {
        out.title = complete_base_name(path);
    }
    if out.album.is_empty() {
        out.album = Arc::from(dir_name(path));
    }
    if out.codec.is_empty() {
        out.codec = Arc::from(suffix(path).to_uppercase());
    }
    ok
}

/// A file's track entry; None when it is not readable audio (no rate or
/// no channels: what the library leaves out).
pub fn read(path: &str) -> Option<TrackMeta> {
    let mut t = TrackMeta::default();
    (read_into(path, &mut t) && t.sample_rate > 0 && t.channels > 0).then_some(t)
}

/// The embedded cover's raw image bytes (front cover, else the first
/// picture); empty when there is none.
pub fn embedded_cover(path: &str) -> Vec<u8> {
    let mut t = TrackMeta::default();
    let mut cover = Cover::default();
    if let Some((sheet_path, number)) = cue::split_track_path(path) {
        // The image file's cover.
        if let Some(sheet) = CueSheet::parse(Path::new(&sheet_path))
            && let Some(track) = sheet.track(number)
        {
            read_any(&track.file.to_string_lossy(), &mut t, Some(&mut cover));
        }
        return cover.data;
    }
    read_any(path, &mut t, Some(&mut cover));
    cover.data
}

/// A folder's cover image: cover / folder / front / album / albumart /
/// artwork (any case; .jpg .jpeg .png .webp), else the first image there.
pub fn folder_cover(directory: &str) -> Option<String> {
    const NAMES: [&str; 6] = ["cover", "folder", "front", "album", "albumart", "artwork"];
    let mut images: Vec<String> = fs::read_dir(directory)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_string();
            if name.starts_with('.') {
                return None; // hidden
            }
            let lower = name.to_lowercase();
            let image = [".jpg", ".jpeg", ".png", ".webp"].iter().any(|x| lower.ends_with(x));
            (image && e.path().is_file()).then_some(name)
        })
        .collect();
    // By name, case aside.
    images.sort_by(|a, b| a.to_lowercase().encode_utf16().cmp(b.to_lowercase().encode_utf16()));
    let join = |f: &str| format!("{}/{}", directory.trim_end_matches('/'), f);
    for wanted in NAMES {
        for file in &images {
            if complete_base_name(file).to_lowercase() == wanted {
                return Some(join(file));
            }
        }
    }
    images.first().map(|f| join(f))
}
