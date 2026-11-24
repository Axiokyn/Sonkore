//! CD images described by a .cue sheet, as much as the
//! library needs: the tracks, their titles, performers and ReplayGain, where
//! each starts, and the `<sheet>.cue#<n>` address of each.
//!
//! This is a port of its own, not the engine's: sonkore-hdcd gets the full
//! CUE reader for playback; both follow the same reading line by line, so a
//! sheet lists the same tracks in either.

use std::fs;
use std::path::{Path, PathBuf};

/// A sheet's track.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CueTrack {
    pub number: i32,
    pub title: String,
    pub performer: String,
    /// The audio file, resolved next to the sheet.
    pub file: PathBuf,
    /// INDEX 01, in CD frames (1/75 s).
    pub start: u64,
    /// The next track's INDEX 01 in the same file; 0 = the end of the file.
    pub end: u64,
    /// REM REPLAYGAIN_TRACK_GAIN / _PEAK (NaN: none).
    pub gain: f64,
    pub peak: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CueSheet {
    pub title: String,
    pub performer: String,
    pub genre: String,
    pub date: String,
    pub album_gain: f64,
    pub album_peak: f64,
    pub tracks: Vec<CueTrack>,
}

impl Default for CueSheet {
    fn default() -> Self {
        CueSheet {
            title: String::new(),
            performer: String::new(),
            genre: String::new(),
            date: String::new(),
            album_gain: f64::NAN,
            album_peak: 0.0,
            tracks: Vec::new(),
        }
    }
}

const MARKER: &str = ".cue#";

#[cfg(test)]
thread_local! {
    /// Sheets read on this thread (the tests count them).
    pub(crate) static PARSES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The CUE reader's check: lead bytes and continuation bytes, nothing more.
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

/// Windows-1252 (Latin-1 with its 0x80–0x9F) to text.
fn from_cp1252(s: &[u8]) -> String {
    const HIGH: [u16; 32] = [
        0x20AC, 0x81, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039, 0x0152, 0x8D,
        0x017D, 0x8F, 0x90, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x02DC, 0x2122, 0x0161, 0x203A,
        0x0153, 0x9D, 0x017E, 0x0178,
    ];
    s.iter()
        .map(|&c| {
            let u = if (0x80..0xA0).contains(&c) { HIGH[(c - 0x80) as usize] } else { c as u16 };
            char::from_u32(u as u32).unwrap_or('\u{fffd}')
        })
        .collect()
}

fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\x0b' | '\x0c' | '\r')
}

/// A line's words; "quoted strings" stay whole.
fn words(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        while i < chars.len() && is_space(chars[i]) {
            i += 1;
        }
        if i >= chars.len() {
            break;
        }
        let mut w = String::new();
        if chars[i] == '"' {
            let mut j = i + 1;
            while j < chars.len() && chars[j] != '"' {
                w.push(chars[j]);
                j += 1;
            }
            i = if j < chars.len() { j + 1 } else { chars.len() };
        } else {
            while i < chars.len() && !is_space(chars[i]) {
                w.push(chars[i]);
                i += 1;
            }
        }
        out.push(w);
    }
    out
}

/// C's atoi: blanks, a sign, digits; anything else ends it.
pub(crate) fn atoi(s: &str) -> i32 {
    let s = s.trim_start_matches(is_space);
    let (neg, rest) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let mut v: i64 = 0;
    for b in rest.bytes() {
        if !b.is_ascii_digit() {
            break;
        }
        v = (v * 10 + (b - b'0') as i64).min(i64::from(i32::MAX) + 1);
    }
    let v = if neg { -v } else { v };
    v.clamp(i32::MIN as i64, i32::MAX as i64) as i32
}

/// "mm:ss:ff" (75 frames a second) to CD frames.
fn cd_time(s: &str) -> Option<u64> {
    let mut parts = s.trim_start_matches(is_space).splitn(3, ':');
    let num = |p: &str| -> Option<u64> {
        let digits: String = p.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() { None } else { digits.parse().ok() }
    };
    let m = num(parts.next()?)?;
    let sec_part = parts.next()?;
    let sec = num(sec_part)?;
    if sec_part.len() != sec_part.chars().take_while(|c| c.is_ascii_digit()).count() {
        return None; // "mm:ss" must be followed by ':'
    }
    let f = num(parts.next()?)?;
    if sec >= 60 || f >= 75 {
        return None;
    }
    Some((m * 60 + sec) * 75 + f)
}

/// "-4.20" whatever the locale; NaN when it is no number.
fn number(s: &str) -> f64 {
    let s = s.strip_prefix('+').unwrap_or(s);
    let b = s.as_bytes();
    let mut end = 0;
    if end < b.len() && b[end] == b'-' {
        end += 1;
    }
    let int_start = end;
    while end < b.len() && b[end].is_ascii_digit() {
        end += 1;
    }
    let mut digits = end - int_start;
    if end < b.len() && b[end] == b'.' {
        let mut e = end + 1;
        while e < b.len() && b[e].is_ascii_digit() {
            e += 1;
        }
        digits += e - end - 1;
        if digits > 0 {
            end = e;
        }
    }
    if digits == 0 {
        return f64::NAN;
    }
    if end < b.len() && (b[end] == b'e' || b[end] == b'E') {
        let mut e = end + 1;
        if e < b.len() && (b[e] == b'+' || b[e] == b'-') {
            e += 1;
        }
        let exp_start = e;
        while e < b.len() && b[e].is_ascii_digit() {
            e += 1;
        }
        if e > exp_start {
            end = e;
        }
    }
    s[..end].parse().unwrap_or(f64::NAN)
}

/// The FILE a sheet names, or the same image under another audio extension.
fn resolve(dir: &Path, name: &str) -> PathBuf {
    let file_name = Path::new(name).file_name().map(|n| n.to_owned()).unwrap_or_default();
    let direct = dir.join(&file_name);
    if !file_name.is_empty() && direct.is_file() {
        return direct;
    }
    let stem = Path::new(name).file_stem().map(|s| s.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    const AUDIO: [&str; 14] = [
        ".flac", ".wav", ".wv", ".ape", ".tta", ".m4a", ".aiff", ".aif", ".dsf", ".dff", ".mp3", ".ogg", ".opus",
        ".tak",
    ];
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if !p.is_file() {
                continue;
            }
            let s = p.file_stem().map(|s| s.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            if s != stem {
                continue;
            }
            let ext =
                p.extension().map(|e| format!(".{}", e.to_string_lossy().to_ascii_lowercase())).unwrap_or_default();
            if AUDIO.contains(&ext.as_str()) {
                return p;
            }
        }
    }
    PathBuf::new()
}

impl CueSheet {
    /// Reads a sheet (UTF-8, or Latin-1 / Windows-1252 as older rippers
    /// wrote them). None when it lists no playable track.
    pub fn parse(sheet: &Path) -> Option<CueSheet> {
        #[cfg(test)]
        PARSES.with(|n| n.set(n.get() + 1));
        let mut bytes = fs::read(sheet).ok()?;
        if bytes.len() > 1 << 20 {
            return None; // not a cue sheet
        }
        if bytes.starts_with(b"\xEF\xBB\xBF") {
            bytes.drain(..3);
        }
        let text = if valid_utf8(&bytes) { String::from_utf8_lossy(&bytes).into_owned() } else { from_cp1252(&bytes) };

        let mut out = CueSheet::default();
        let dir = match sheet.parent() {
            Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
            _ => PathBuf::from("."),
        };
        let mut file = PathBuf::new();
        let mut current: Option<usize> = None;
        for line in text.split('\n') {
            let w = words(line);
            if w.is_empty() {
                continue;
            }
            let key = w[0].to_ascii_lowercase();
            let arg = w.get(1).cloned().unwrap_or_default();
            match key.as_str() {
                "file" => {
                    file = resolve(&dir, &arg);
                    current = None;
                }
                "track" => {
                    out.tracks.push(CueTrack {
                        number: atoi(&arg),
                        file: file.clone(),
                        start: u64::MAX, // until its INDEX 01
                        gain: f64::NAN,
                        ..Default::default()
                    });
                    current = Some(out.tracks.len() - 1);
                }
                "index" if current.is_some() && w.len() > 2 && atoi(&arg) == 1 => {
                    if let Some(frames) = cd_time(&w[2]) {
                        out.tracks[current.unwrap()].start = frames;
                    }
                }
                "title" => match current {
                    Some(i) => out.tracks[i].title = arg,
                    None => out.title = arg,
                },
                "performer" => match current {
                    Some(i) => out.tracks[i].performer = arg,
                    None => out.performer = arg,
                },
                "rem" if w.len() > 2 => {
                    let what = arg.to_ascii_lowercase();
                    let v = number(&w[2]);
                    match what.as_str() {
                        "genre" => out.genre = w[2].clone(),
                        "date" => out.date = w[2].clone(),
                        "replaygain_album_gain" => out.album_gain = v,
                        "replaygain_album_peak" => out.album_peak = v,
                        "replaygain_track_gain" => {
                            if let Some(i) = current {
                                out.tracks[i].gain = v;
                            }
                        }
                        "replaygain_track_peak" => {
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
        out.tracks.retain(|t| !t.file.as_os_str().is_empty() && t.start != u64::MAX);
        // A track ends where the next one in the same file begins.
        for i in 0..out.tracks.len().saturating_sub(1) {
            if out.tracks[i + 1].file == out.tracks[i].file && out.tracks[i + 1].start > out.tracks[i].start {
                out.tracks[i].end = out.tracks[i + 1].start;
            }
        }
        if out.tracks.is_empty() { None } else { Some(out) }
    }

    pub fn track(&self, number: i32) -> Option<&CueTrack> {
        self.tracks.iter().find(|t| t.number == number)
    }
}

/// A CD image track's address (`<sheet>.cue#<n>`)?
pub fn is_track_path(path: &str) -> bool {
    match path.to_ascii_lowercase().rfind(MARKER) {
        Some(at) => at + MARKER.len() < path.len(),
        None => false,
    }
}

/// `<sheet>.cue#<n>` into the sheet and the number.
pub fn split_track_path(path: &str) -> Option<(String, i32)> {
    let at = path.to_ascii_lowercase().rfind(MARKER)?;
    let digits = &path[at + MARKER.len()..];
    if digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some((path[..at + 4].to_string(), atoi(digits)))
}

pub fn track_path(sheet: &str, number: i32) -> String {
    format!("{sheet}#{number}")
}

/// A ".cue" file (by its extension, any case).
pub fn is_sheet(path: &str) -> bool {
    Path::new(path).extension().is_some_and(|e| e.eq_ignore_ascii_case("cue"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses() {
        assert!(is_track_path("/m/Disc.CUE#3"));
        assert!(!is_track_path("/m/disc.cue#"));
        assert_eq!(split_track_path("/m/Disc.CUE#12"), Some(("/m/Disc.CUE".into(), 12)));
        assert_eq!(split_track_path("/m/disc.cue#1a"), None);
        assert_eq!(track_path("/m/d.cue", 4), "/m/d.cue#4");
    }

    #[test]
    fn pieces() {
        assert_eq!(cd_time("01:02:03"), Some((62 * 75) + 3));
        assert_eq!(cd_time("00:60:00"), None);
        assert_eq!(number("-3.50"), -3.5);
        assert_eq!(number("+0.5"), 0.5);
        assert!(number("dB").is_nan());
        assert_eq!(atoi("07"), 7);
        assert_eq!(words("TITLE \"A b\" c"), vec!["TITLE", "A b", "c"]);
    }
}
