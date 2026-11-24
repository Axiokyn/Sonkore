//! CD images described by a .cue sheet, for the decoder and the library.
//!
//! A track is addressed as `"<sheet>.cue#<number>"`: no real file has that
//! name; the decoder and the library resolve it with [`split_track_path`].
//!
//! The parser keeps the behaviour earlier versions had, quirks included:
//! - a sheet over 1 MiB is not a sheet; a UTF-8 BOM is skipped;
//! - text that is not (loosely) valid UTF-8 is read as Windows-1252. Like
//!   the original check, only lead and continuation bytes are looked at:
//!   overlong forms and surrogates pass, and come out as U+FFFD (`String`);
//! - only the first word after a keyword counts (`REM GENRE Hard Rock` is
//!   "Hard"), unless it is quoted;
//! - TITLE / PERFORMER before the first TRACK of a FILE are the album's;
//! - tracks without a playable file or an INDEX 01 are dropped; a track ends
//!   where the next one in the same file starts (INDEX 01): the pregap
//!   (INDEX 00) stays with the track before it.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// What sits between the sheet and the track number in a track path.
pub const MARKER: &str = ".cue#";

/// CD frames per second.
pub const CD_FRAMES_PER_SECOND: u64 = 75;

/// The extensions a missing FILE is also looked for under (the image was
/// converted after ripping: `.wav` → `.flac`).
pub const AUDIO_EXTENSIONS: [&str; 14] =
    [".flac", ".wav", ".wv", ".ape", ".tta", ".m4a", ".aiff", ".aif", ".dsf", ".dff", ".mp3", ".ogg", ".opus", ".tak"];

/// One track of a sheet (`CueSheet::Track`).
#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub number: i32,
    pub title: String,
    pub performer: String,
    /// The audio file, resolved next to the sheet.
    pub file: PathBuf,
    /// INDEX 01, in CD frames (1/75 s).
    pub start: u64,
    /// The next track's INDEX 01 in the same file, in CD frames; `None`: the
    /// end of the file.
    pub end: Option<u64>,
    /// REM REPLAYGAIN_TRACK_GAIN in dB (`None` when absent or unreadable).
    pub gain: Option<f64>,
    /// REM REPLAYGAIN_TRACK_PEAK (0 when absent; NaN when unreadable).
    pub peak: f64,
}

impl Track {
    /// Where the track starts in the image, in sample frames.
    pub fn start_frame(&self, rate: u32) -> u64 {
        cd_to_frames(self.start, rate)
    }

    /// Where it ends in the image, in sample frames (`None`: the image's end).
    pub fn end_frame(&self, rate: u32) -> Option<u64> {
        self.end.map(|e| cd_to_frames(e, rate))
    }

    pub fn start_seconds(&self) -> f64 {
        self.start as f64 / 75.0
    }

    /// The track's length, given the image's.
    pub fn duration_seconds(&self, image_duration: f64) -> f64 {
        let end = match self.end {
            Some(e) => e as f64 / 75.0,
            None => image_duration,
        };
        (end - self.start_seconds()).max(0.0)
    }

    /// The stretch of the image it plays, as the engine's RangeDecoder sees it;
    /// `image_frames` is the image's length (`None` when unknown).
    pub fn range(&self, rate: u32, image_frames: Option<u64>) -> TrackRange {
        TrackRange::new(self.start_frame(rate), self.end_frame(rate), image_frames)
    }
}

/// A parsed sheet (`CueSheet`).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct CueSheet {
    /// The album.
    pub title: String,
    /// The album artist.
    pub performer: String,
    pub genre: String,
    pub date: String,
    /// REM REPLAYGAIN_ALBUM_GAIN in dB.
    pub album_gain: Option<f64>,
    /// REM REPLAYGAIN_ALBUM_PEAK (0 when absent; NaN when unreadable).
    pub album_peak: f64,
    pub tracks: Vec<Track>,
}

impl CueSheet {
    /// Reads a sheet; `None` when it cannot be read, is over 1 MiB, or has no
    /// playable track. FILE entries are looked up next to it.
    pub fn parse(sheet: impl AsRef<Path>) -> Option<CueSheet> {
        let sheet = sheet.as_ref();
        let text = fs::read(sheet).ok()?;
        let dir = match sheet.parent() {
            Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
            _ => PathBuf::from("."),
        };
        Self::parse_bytes(&text, &dir)
    }

    /// Parses a sheet's bytes, resolving its FILE entries in `dir`.
    pub fn parse_bytes(text: &[u8], dir: &Path) -> Option<CueSheet> {
        if text.len() > 1 << 20 {
            return None; // not a cue sheet
        }
        let text = text.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(text);
        let text: Vec<u8> = if valid_utf8(text) { text.to_vec() } else { from_cp1252(text) };

        let mut out = CueSheet { album_gain: None, ..CueSheet::default() };
        let mut file = PathBuf::new();
        let mut starts: Vec<Option<u64>> = Vec::new(); // INDEX 01 per track, until known
        let mut current: Option<usize> = None;
        for line in text.split(|&c| c == b'\n') {
            let w = words(line);
            if w.is_empty() {
                continue;
            }
            let key = w[0].to_ascii_lowercase();
            let arg: &[u8] = w.get(1).map(Vec::as_slice).unwrap_or(b"");
            match key.as_slice() {
                b"file" => {
                    file = resolve(dir, arg);
                    current = None;
                }
                b"track" => {
                    out.tracks.push(Track {
                        number: atoi(arg),
                        title: String::new(),
                        performer: String::new(),
                        file: file.clone(),
                        start: 0,
                        end: None,
                        gain: None,
                        peak: 0.0,
                    });
                    starts.push(None);
                    current = Some(out.tracks.len() - 1);
                }
                b"index" if current.is_some() && w.len() > 2 && atoi(arg) == 1 => {
                    if let Some(frames) = cd_time(&w[2]) {
                        starts[current.unwrap()] = Some(frames);
                    }
                }
                b"title" => {
                    let v = text_of(arg);
                    match current {
                        Some(i) => out.tracks[i].title = v,
                        None => out.title = v,
                    }
                }
                b"performer" => {
                    let v = text_of(arg);
                    match current {
                        Some(i) => out.tracks[i].performer = v,
                        None => out.performer = v,
                    }
                }
                b"rem" if w.len() > 2 => {
                    let what = arg.to_ascii_lowercase();
                    let v = number(&w[2]);
                    match what.as_slice() {
                        b"genre" => out.genre = text_of(&w[2]),
                        b"date" => out.date = text_of(&w[2]),
                        b"replaygain_album_gain" => out.album_gain = gain_of(v),
                        b"replaygain_album_peak" => out.album_peak = v,
                        b"replaygain_track_gain" => {
                            if let Some(i) = current {
                                out.tracks[i].gain = gain_of(v);
                            }
                        }
                        b"replaygain_track_peak" => {
                            if let Some(i) = current {
                                out.tracks[i].peak = v;
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }

        // Tracks without a playable file or an INDEX 01 cannot be played.
        let mut tracks = Vec::with_capacity(out.tracks.len());
        for (mut t, start) in out.tracks.into_iter().zip(starts) {
            if let Some(s) = start
                && !t.file.as_os_str().is_empty()
            {
                t.start = s;
                tracks.push(t);
            }
        }
        // A track ends where the next one in the same file begins (its
        // pregap, INDEX 00, stays with the track before it: playing them in
        // turn is the whole disc without a gap).
        for i in 0..tracks.len().saturating_sub(1) {
            if tracks[i + 1].file == tracks[i].file && tracks[i + 1].start > tracks[i].start {
                tracks[i].end = Some(tracks[i + 1].start);
            }
        }
        out.tracks = tracks;
        if out.tracks.is_empty() { None } else { Some(out) }
    }

    /// The first track with this number.
    pub fn track(&self, number: i32) -> Option<&Track> {
        self.tracks.iter().find(|t| t.number == number)
    }
}

// --- track paths -------------------------------------------------------------------

fn bytes(p: &Path) -> &[u8] {
    p.as_os_str().as_encoded_bytes()
}

/// Part of [`bytes`], cut at ASCII characters ('#', '/'), as a path again.
fn os(part: &[u8]) -> &OsStr {
    // SAFETY: `part` comes from `as_encoded_bytes` and was split only next
    // to ASCII characters, which is what from_encoded_bytes_unchecked allows.
    unsafe { OsStr::from_encoded_bytes_unchecked(part) }
}

/// A file name written inside a sheet, as the system names files: bytes as
/// they are on Unix; on Windows the text read as UTF-8.
fn sheet_name(name: &[u8]) -> OsString {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        OsStr::from_bytes(name).to_owned()
    }
    #[cfg(not(unix))]
    {
        OsString::from(String::from_utf8_lossy(name).into_owned())
    }
}

fn rfind_marker(path: &[u8]) -> Option<usize> {
    let m = MARKER.as_bytes();
    if path.len() < m.len() {
        return None;
    }
    (0..=path.len() - m.len()).rev().find(|&i| path[i..i + m.len()].eq_ignore_ascii_case(m))
}

/// Is this `"<sheet>.cue#<something>"`?
pub fn is_track_path(path: impl AsRef<Path>) -> bool {
    let p = bytes(path.as_ref());
    rfind_marker(p).is_some_and(|at| at + MARKER.len() < p.len())
}

/// `"<sheet>.cue#<n>"` → (sheet, n); `None` when it is not one.
pub fn split_track_path(path: impl AsRef<Path>) -> Option<(PathBuf, i32)> {
    let p = bytes(path.as_ref());
    let at = rfind_marker(p)?;
    let digits = &p[at + MARKER.len()..];
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    Some((PathBuf::from(os(&p[..at + 4])), atoi(digits)))
}

/// The address of a sheet's track: `"<sheet>#<n>"`.
pub fn track_path(sheet: impl AsRef<Path>, number: i32) -> PathBuf {
    let mut v = bytes(sheet.as_ref()).to_vec();
    v.push(b'#');
    v.extend_from_slice(number.to_string().as_bytes());
    // SAFETY: the path's own encoded bytes followed by ASCII.
    PathBuf::from(unsafe { OsString::from_encoded_bytes_unchecked(v) })
}

/// A `.cue` file (any case).
pub fn is_sheet(path: impl AsRef<Path>) -> bool {
    extension(bytes(path.as_ref())).eq_ignore_ascii_case(b".cue")
}

/// CD frames (1/75 s) to sample frames; exact at 44.1 kHz (588 per CD frame).
pub fn cd_to_frames(cd: u64, rate: u32) -> u64 {
    (cd.wrapping_mul(rate as u64).wrapping_add(37)) / 75
}

/// What the decoder opens for a path: `None` when it is not
/// a track path (open the file itself), else the track or the error
/// ("Track 3 not found in disc.cue").
pub fn open_track(path: impl AsRef<Path>) -> Option<Result<Track, String>> {
    let (sheet, number) = split_track_path(path)?;
    let track = CueSheet::parse(&sheet).and_then(|s| s.track(number).cloned());
    Some(track.ok_or_else(|| {
        let name = Path::new(os(filename(bytes(&sheet))));
        format!("Track {number} not found in {}", name.display())
    }))
}

/// One track as a file of its own: the stretch [start, end) of the image
/// (the engine's RangeDecoder). Consecutive tracks share their boundary, so
/// playing them in turn is the image without a missing or doubled sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrackRange {
    /// The first frame, in the image.
    pub start: u64,
    /// The track's length in frames (its total frames).
    pub frames: u64,
}

impl TrackRange {
    /// `end`: `None` for the image's end. `image_frames`: `None` when the
    /// image's length is unknown; then a track without an end
    /// has no frames.
    pub fn new(start: u64, end: Option<u64>, image_frames: Option<u64>) -> Self {
        let end = end.unwrap_or(0);
        let total = image_frames.unwrap_or(0);
        let last = if end > 0 && (total == 0 || end < total) { end } else { total };
        TrackRange { start, frames: last.saturating_sub(start) }
    }

    /// How many of `frames` can still be read at `position`.
    pub fn readable(&self, position: u64, frames: u64) -> u64 {
        frames.min(self.frames - self.frames.min(position))
    }

    /// A seek to `frame`: (the new position in the track, the frame to seek
    /// to in the image).
    pub fn seek(&self, frame: u64) -> (u64, u64) {
        let position = frame.min(self.frames);
        (position, self.start + position)
    }
}

// --- the library's use -------------------------------------------------------------

/// A sheet's tracks for the library: the track paths
/// whose image `is_audio` accepts, and the images (each once, in order),
/// which a folder scan then leaves out.
pub fn cue_tracks(sheet: impl AsRef<Path>, mut is_audio: impl FnMut(&Path) -> bool) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let sheet = sheet.as_ref();
    let mut paths = Vec::new();
    let mut images: Vec<PathBuf> = Vec::new();
    let Some(cue) = CueSheet::parse(sheet) else {
        return (paths, images);
    };
    for t in &cue.tracks {
        if !is_audio(&t.file) {
            continue;
        }
        paths.push(track_path(sheet, t.number));
        if !images.contains(&t.file) {
            images.push(t.file.clone());
        }
    }
    (paths, images)
}

/// What a CD image track's library entry was read from (its `modified`
/// stamp): the newer modification time, in ms
/// since the epoch, of its sheet and its image. `None` when the sheet cannot
/// be parsed. A missing image or track counts as 0.
pub fn track_stamp(path: impl AsRef<Path>) -> Option<i64> {
    let (sheet_path, number) = split_track_path(path)?;
    let sheet = CueSheet::parse(&sheet_path)?;
    let image = sheet.track(number).and_then(|t| modified_ms(&t.file)).unwrap_or(0);
    Some(image.max(modified_ms(&sheet_path).unwrap_or(0)))
}

/// A file's modification time in ms since the epoch.
pub fn modified_ms(path: &Path) -> Option<i64> {
    let t = fs::metadata(path).ok()?.modified().ok()?;
    Some(match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_millis() as i64,
        Err(e) => -(e.duration().as_millis() as i64),
    })
}

// --- parsing helpers ---------------------------------------------------------------

/// C's isspace in the "C" locale.
fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | 0x0B | 0x0C | b'\r')
}

/// Loose UTF-8 validity: lead bytes and continuation bytes only (no check for
/// overlong forms or surrogates).
fn valid_utf8(s: &[u8]) -> bool {
    let mut i = 0;
    while i < s.len() {
        let c = s[i];
        let extra = if c < 0x80 {
            0
        } else if c >> 5 == 0x6 {
            1
        } else if c >> 4 == 0xE {
            2
        } else if c >> 3 == 0x1E {
            3
        } else {
            return false;
        };
        if i + extra >= s.len() && extra > 0 {
            return false;
        }
        for k in 1..=extra {
            if s[i + k] >> 6 != 0x2 {
                return false;
            }
        }
        i += extra + 1;
    }
    true
}

/// Windows-1252 (a superset of Latin-1 for text) to UTF-8.
fn from_cp1252(s: &[u8]) -> Vec<u8> {
    const HIGH: [u16; 32] = [
        0x20AC, 0x81, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039, 0x0152, 0x8D,
        0x017D, 0x8F, 0x90, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x02DC, 0x2122, 0x0161, 0x203A,
        0x0153, 0x9D, 0x017E, 0x0178,
    ];
    let mut out = Vec::with_capacity(s.len() + s.len() / 2);
    for &c in s {
        let u = if (0x80..0xA0).contains(&c) { HIGH[(c - 0x80) as usize] } else { c as u16 };
        if u < 0x80 {
            out.push(u as u8);
        } else if u < 0x800 {
            out.push(0xC0 | (u >> 6) as u8);
            out.push(0x80 | (u & 0x3F) as u8);
        } else {
            out.push(0xE0 | (u >> 12) as u8);
            out.push(0x80 | ((u >> 6) & 0x3F) as u8);
            out.push(0x80 | (u & 0x3F) as u8);
        }
    }
    out
}

/// Splits a line into words; "quoted strings" stay whole (an unclosed quote
/// runs to the end of the line).
fn words(line: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < line.len() {
        while i < line.len() && is_space(line[i]) {
            i += 1;
        }
        if i >= line.len() {
            break;
        }
        if line[i] == b'"' {
            match line[i + 1..].iter().position(|&c| c == b'"') {
                Some(n) => {
                    out.push(line[i + 1..i + 1 + n].to_vec());
                    i += n + 2;
                }
                None => {
                    out.push(line[i + 1..].to_vec());
                    i = line.len();
                }
            }
        } else {
            let from = i;
            while i < line.len() && !is_space(line[i]) {
                i += 1;
            }
            out.push(line[from..i].to_vec());
        }
    }
    out
}

fn text_of(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn gain_of(v: f64) -> Option<f64> {
    if v.is_nan() { None } else { Some(v) }
}

/// C's atoi (strtol, saturated to long, truncated to int).
fn atoi(s: &[u8]) -> i32 {
    let mut i = 0;
    while i < s.len() && is_space(s[i]) {
        i += 1;
    }
    let mut neg = false;
    if i < s.len() && (s[i] == b'+' || s[i] == b'-') {
        neg = s[i] == b'-';
        i += 1;
    }
    let mut v: i128 = 0;
    while i < s.len() && s[i].is_ascii_digit() {
        v = (v * 10 + (s[i] - b'0') as i128).min(i64::MAX as i128 + 1);
        i += 1;
    }
    let v = if neg { (-v).max(i64::MIN as i128) } else { v.min(i64::MAX as i128) };
    v as i64 as i32
}

/// `istream >> unsigned`: skips white space, takes a sign and digits; fails
/// without digits or past `u32::MAX` (a minus negates, like strtoul).
fn read_unsigned(s: &[u8], i: &mut usize) -> Option<u32> {
    while *i < s.len() && is_space(s[*i]) {
        *i += 1;
    }
    let mut neg = false;
    if *i < s.len() && (s[*i] == b'+' || s[*i] == b'-') {
        neg = s[*i] == b'-';
        *i += 1;
    }
    let from = *i;
    let mut v: u64 = 0;
    let mut overflow = false;
    while *i < s.len() && s[*i].is_ascii_digit() {
        v = v * 10 + (s[*i] - b'0') as u64;
        if v > u32::MAX as u64 {
            overflow = true;
            v = u32::MAX as u64;
        }
        *i += 1;
    }
    if *i == from || overflow {
        return None;
    }
    let v = v as u32;
    Some(if neg { v.wrapping_neg() } else { v })
}

/// `istream >> char`: skips white space.
fn read_char(s: &[u8], i: &mut usize) -> Option<u8> {
    while *i < s.len() && is_space(s[*i]) {
        *i += 1;
    }
    let c = *s.get(*i)?;
    *i += 1;
    Some(c)
}

/// "mm:ss:ff" (75 frames per second) to CD frames.
fn cd_time(s: &[u8]) -> Option<u64> {
    let mut i = 0;
    let m = read_unsigned(s, &mut i)?;
    let a = read_char(s, &mut i)?;
    let sec = read_unsigned(s, &mut i)?;
    let b = read_char(s, &mut i)?;
    let f = read_unsigned(s, &mut i)?;
    if a != b':' || b != b':' || sec >= 60 || f >= 75 {
        return None;
    }
    Some((m as u64 * 60 + sec as u64) * 75 + f as u64)
}

/// "-4.20" whatever the locale (std::from_chars after an optional '+'):
/// the longest number at the start, NaN when there is none or it is out of
/// range.
fn number(s: &[u8]) -> f64 {
    let s = s.strip_prefix(b"+").unwrap_or(s);
    let mut i = 0;
    let neg = s.first() == Some(&b'-');
    if neg {
        i += 1;
    }
    let rest = &s[i..];
    // inf / infinity / nan / nan(chars)
    let lower: Vec<u8> = rest.iter().take(8).map(u8::to_ascii_lowercase).collect();
    if lower.starts_with(b"inf") {
        return if neg { f64::NEG_INFINITY } else { f64::INFINITY };
    }
    if lower.starts_with(b"nan") {
        return f64::NAN;
    }
    let mut j = 0;
    let mut digits = 0;
    while j < rest.len() && rest[j].is_ascii_digit() {
        j += 1;
        digits += 1;
    }
    if j < rest.len() && rest[j] == b'.' {
        j += 1;
        while j < rest.len() && rest[j].is_ascii_digit() {
            j += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return f64::NAN;
    }
    if j < rest.len() && (rest[j] == b'e' || rest[j] == b'E') {
        let mut k = j + 1;
        if k < rest.len() && (rest[k] == b'+' || rest[k] == b'-') {
            k += 1;
        }
        let exp_from = k;
        while k < rest.len() && rest[k].is_ascii_digit() {
            k += 1;
        }
        if k > exp_from {
            j = k;
        }
    }
    let text = std::str::from_utf8(&s[..i + j]).unwrap_or("");
    match text.parse::<f64>() {
        // from_chars reports overflow (and underflow to zero) as out of range
        Ok(v) if v.is_infinite() => f64::NAN,
        Ok(v) if v == 0.0 && mantissa_nonzero(&rest[..j]) => f64::NAN,
        Ok(v) => v,
        Err(_) => f64::NAN,
    }
}

/// The mantissa (before any exponent) has a non-zero digit.
fn mantissa_nonzero(s: &[u8]) -> bool {
    s.iter().take_while(|&&c| c != b'e' && c != b'E').any(|c| (b'1'..=b'9').contains(c))
}

/// std::filesystem::path::filename: what follows the last '/'.
fn filename(p: &[u8]) -> &[u8] {
    // Windows also separates with '\\' (sheets written there use it).
    match p.iter().rposition(|&c| c == b'/' || (cfg!(windows) && c == b'\\')) {
        Some(i) => &p[i + 1..],
        None => p,
    }
}

/// std::filesystem::path::extension (".flac"; none for ".", "..", ".hidden").
fn extension(p: &[u8]) -> &[u8] {
    let name = filename(p);
    if name == b"." || name == b".." {
        return b"";
    }
    match name.iter().rposition(|&c| c == b'.') {
        Some(0) | None => b"",
        Some(i) => &name[i..],
    }
}

/// std::filesystem::path::stem
fn stem(p: &[u8]) -> &[u8] {
    let name = filename(p);
    &name[..name.len() - extension(p).len()]
}

/// The FILE a sheet names, or the same image under another audio extension;
/// empty when there is neither.
fn resolve(dir: &Path, name: &[u8]) -> PathBuf {
    let direct = dir.join(sheet_name(filename(name)));
    if fs::metadata(&direct).is_ok_and(|m| m.is_file()) {
        return direct;
    }
    let wanted = stem(name).to_ascii_lowercase();
    let Ok(entries) = fs::read_dir(dir) else {
        return PathBuf::new();
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if !fs::metadata(&p).is_ok_and(|m| m.is_file()) {
            continue;
        }
        let b = bytes(&p);
        if stem(b).to_ascii_lowercase() != wanted {
            continue;
        }
        let ext = extension(b).to_ascii_lowercase();
        if AUDIO_EXTENSIONS.iter().any(|e| e.as_bytes() == ext.as_slice()) {
            return p;
        }
    }
    PathBuf::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers_match_reference_semantics() {
        assert_eq!(extension(b"a/b.flac"), b".flac");
        assert_eq!(extension(b".cue"), b"");
        assert_eq!(stem(b".cue"), b".cue");
        assert_eq!(stem(b"x/imagen.wav"), b"imagen");
        assert_eq!(extension(b"a."), b".");
        assert_eq!(stem(b"..x"), b".");
        assert_eq!(atoi(b"01"), 1);
        assert_eq!(atoi(b" -7x"), -7);
        assert_eq!(atoi(b"99999999999"), 1215752191); // a long, truncated to int
        assert_eq!(atoi(b"99999999999999999999"), -1); // strtol's LONG_MAX, truncated
        assert_eq!(cd_time(b"00:04:00"), Some(300));
        assert_eq!(cd_time(b"01:00:74"), Some(4574));
        assert_eq!(cd_time(b"00:60:00"), None);
        assert_eq!(cd_time(b"00:00:75"), None);
        assert_eq!(cd_time(b"00-00-00"), None);
        assert_eq!(number(b"-4.20"), -4.20);
        assert_eq!(number(b"+3.5 dB"), 3.5);
        assert!(number(b"dB").is_nan());
        assert_eq!(cd_to_frames(300, 44100), 176400);
        assert_eq!(cd_to_frames(1, 48000), 640);
        assert_eq!(cd_to_frames(1, 96000), 1280);
        assert_eq!(cd_to_frames(1, 22050), 294);
    }
}
