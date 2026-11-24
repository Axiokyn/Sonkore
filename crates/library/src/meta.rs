//! One track as the library keeps it, and its entry in library.json, with
//! the short keys of the stored format.

use std::sync::Arc;

use serde_json::{Map, Value};

use crate::cue;
use crate::json_out;

/// Shared text: the tracks of an album hold one copy of its artist, album,
/// genre and codec.
pub type Text = Arc<str>;

/// A track's tags, format and file facts.
#[derive(Clone, Debug, PartialEq)]
pub struct TrackMeta {
    /// The file, or a CD image track (`<sheet>.cue#<n>`).
    pub path: String,
    pub title: String,
    pub artist: Text,
    pub album: Text,
    pub album_artist: Text,
    pub genre: Text,
    /// "FLAC", "WAV", "MP3", "ALAC", "AAC", "Opus", "DSD64"…
    pub codec: Text,
    pub track_no: i32,
    pub disc_no: i32,
    pub year: i32,
    /// Seconds.
    pub duration: f64,
    pub sample_rate: i32,
    /// 0 for lossy and float sources.
    pub bits: i32,
    pub channels: i32,
    pub bitrate_kbps: i32,
    pub file_size: i64,
    /// ms since the epoch.
    pub modified: i64,
    /// ms since the epoch: when the library first saw the file.
    pub added: i64,
    pub has_embedded_cover: bool,
    /// ReplayGain 2.0: gains in dB towards -18 LUFS, peaks linear (1.0 =
    /// full scale). NaN = unknown. From the file's tags, or measured by
    /// Sonkore (`rg_analyzed`); files are never written.
    pub rg_track_gain: f64,
    pub rg_track_peak: f64,
    pub rg_album_gain: f64,
    pub rg_album_peak: f64,
    pub rg_analyzed: bool,
}

impl Default for TrackMeta {
    fn default() -> Self {
        let empty: Text = Arc::from("");
        TrackMeta {
            path: String::new(),
            title: String::new(),
            artist: empty.clone(),
            album: empty.clone(),
            album_artist: empty.clone(),
            genre: empty.clone(),
            codec: empty,
            track_no: 0,
            disc_no: 0,
            year: 0,
            duration: 0.0,
            sample_rate: 0,
            bits: 0,
            channels: 0,
            bitrate_kbps: 0,
            file_size: 0,
            modified: 0,
            added: 0,
            has_embedded_cover: false,
            rg_track_gain: f64::NAN,
            rg_track_peak: 0.0,
            rg_album_gain: f64::NAN,
            rg_album_peak: 0.0,
            rg_analyzed: false,
        }
    }
}

/// Lower case.
pub(crate) fn lower(s: &str) -> String {
    s.to_lowercase()
}

impl TrackMeta {
    /// The album's identity for grouping: "artist\x1falbum", lower case, the
    /// album artist when there is one.
    pub fn album_key(&self) -> String {
        let who = if self.album_artist.is_empty() { &self.artist } else { &self.album_artist };
        let mut key = lower(who);
        key.push('\u{1f}');
        key.push_str(&lower(&self.album));
        key
    }

    /// The artist the lists show: the track artist, else the album artist.
    pub fn display_artist(&self) -> &str {
        if self.artist.is_empty() { &self.album_artist } else { &self.artist }
    }

    /// The album artist, else the track artist (the browser's artist).
    pub fn album_or_track_artist(&self) -> &str {
        if self.album_artist.is_empty() { &self.artist } else { &self.album_artist }
    }

    /// "FLAC 24/96", "MP3 320k", "DSD64 5645k" or the codec alone.
    pub fn format_label(&self) -> String {
        if self.bits > 0 && self.sample_rate > 0 {
            let khz = self.sample_rate as f64 / 1000.0;
            return format!("{} {}/{}", self.codec, self.bits, format_g4(khz));
        }
        if self.bitrate_kbps > 0 {
            return format!("{} {}k", self.codec, self.bitrate_kbps);
        }
        self.codec.to_string()
    }

    /// kbps; for lossless files computed from the size and the length.
    pub fn average_bitrate(&self) -> i32 {
        if self.bitrate_kbps > 0 {
            return self.bitrate_kbps;
        }
        if self.duration > 0.0 { (self.file_size as f64 * 8.0 / self.duration / 1000.0 + 0.5) as i32 } else { 0 }
    }

    /// The file's name; a CD image track's title stands in for one.
    pub fn file_name(&self) -> String {
        if cue::is_track_path(&self.path) && !self.title.is_empty() {
            return self.title.clone();
        }
        self.path.rsplit('/').next().unwrap_or("").to_string()
    }

    /// The folder: everything before the last '/'.
    pub fn folder(&self) -> String {
        match self.path.rfind('/') {
            Some(i) => self.path[..i].to_string(),
            None => String::new(),
        }
    }

    /// "m:ss", rounded to the second.
    pub fn duration_text(&self) -> String {
        let s = (self.duration + 0.5) as i64;
        format!("{}:{:02}", s / 60, s % 60)
    }

    pub fn has_replay_gain(&self) -> bool {
        !self.rg_track_gain.is_nan()
    }

    /// The lower-case text a search looks in: title, artists, album, genre
    /// and codec.
    pub fn search_text(&self) -> String {
        lower(&format!(
            "{} {} {} {} {} {}",
            self.title, self.artist, self.album_artist, self.album, self.genre, self.codec
        ))
    }

    /// The library.json entry.
    pub fn to_json(&self) -> Value {
        let mut o = Map::new();
        let s = |v: &str| Value::String(v.to_string());
        o.insert("p".into(), s(&self.path));
        o.insert("t".into(), s(&self.title));
        o.insert("a".into(), s(&self.artist));
        o.insert("al".into(), s(&self.album));
        o.insert("aa".into(), s(&self.album_artist));
        o.insert("g".into(), s(&self.genre));
        o.insert("c".into(), s(&self.codec));
        o.insert("tn".into(), Value::from(self.track_no));
        o.insert("dn".into(), Value::from(self.disc_no));
        o.insert("y".into(), Value::from(self.year));
        o.insert("d".into(), json_out::number(self.duration));
        o.insert("sr".into(), Value::from(self.sample_rate));
        o.insert("b".into(), Value::from(self.bits));
        o.insert("ch".into(), Value::from(self.channels));
        o.insert("br".into(), Value::from(self.bitrate_kbps));
        o.insert("sz".into(), Value::from(self.file_size));
        o.insert("m".into(), Value::from(self.modified));
        o.insert("ad".into(), Value::from(self.added));
        o.insert("cv".into(), Value::Bool(self.has_embedded_cover));
        if !self.rg_track_gain.is_nan() {
            o.insert("rtg".into(), json_out::number(self.rg_track_gain));
            o.insert("rtp".into(), json_out::number(self.rg_track_peak));
        }
        if !self.rg_album_gain.is_nan() {
            o.insert("rag".into(), json_out::number(self.rg_album_gain));
            o.insert("rap".into(), json_out::number(self.rg_album_peak));
        }
        if self.rg_analyzed {
            o.insert("rga".into(), Value::Bool(true));
        }
        Value::Object(o)
    }

    /// An entry of library.json, read as earlier Sonkore versions read it.
    pub fn from_json(o: &Value) -> TrackMeta {
        let g = |k: &str| o.get(k);
        let text = |k: &str| -> Text { Arc::from(json_out::as_string(g(k))) };
        let mut t = TrackMeta {
            path: json_out::as_string(g("p")),
            title: json_out::as_string(g("t")),
            artist: text("a"),
            album: text("al"),
            album_artist: text("aa"),
            genre: text("g"),
            codec: text("c"),
            track_no: json_out::as_i32(g("tn")),
            disc_no: json_out::as_i32(g("dn")),
            year: json_out::as_i32(g("y")),
            duration: json_out::as_f64_or(g("d"), 0.0),
            sample_rate: json_out::as_i32(g("sr")),
            bits: json_out::as_i32(g("b")),
            channels: json_out::as_i32(g("ch")),
            bitrate_kbps: json_out::as_i32(g("br")),
            file_size: json_out::as_i64_or(g("sz"), 0),
            modified: json_out::as_i64_or(g("m"), 0),
            added: 0,
            has_embedded_cover: json_out::as_bool(g("cv")),
            rg_track_gain: json_out::as_f64_or(g("rtg"), f64::NAN),
            rg_track_peak: json_out::as_f64_or(g("rtp"), 0.0),
            rg_album_gain: json_out::as_f64_or(g("rag"), f64::NAN),
            rg_album_peak: json_out::as_f64_or(g("rap"), 0.0),
            rg_analyzed: json_out::as_bool(g("rga")),
        };
        // Caches written by early builds stored the English fallback.
        if &*t.artist == "Unknown artist" {
            t.artist = Arc::from("");
        }
        t.added = json_out::as_i64_or(g("ad"), t.modified);
        t
    }

    /// One field by its role name (the names of the lists'
    /// roles: "title", "trackNo", "fileSize"…). `None` for an unknown name.
    pub fn value(&self, role: &str) -> Option<FieldValue> {
        use FieldValue::*;
        Some(match role {
            "path" => Text(self.path.clone()),
            "title" | "display" => Text(self.title.clone()),
            "artist" => Text(self.display_artist().to_string()),
            "album" => Text(self.album.to_string()),
            "albumArtist" => Text(self.album_artist.to_string()),
            "genre" => Text(self.genre.to_string()),
            "trackNo" => Int(self.track_no as i64),
            "discNo" => Int(self.disc_no as i64),
            "year" => Int(self.year as i64),
            "duration" => Float(self.duration),
            "durationText" => Text(self.duration_text()),
            "format" => Text(self.format_label()),
            "albumKey" => Text(self.album_key()),
            "searchText" => Text(self.search_text()),
            "codec" => Text(self.codec.to_string()),
            "sampleRate" => Int(self.sample_rate as i64),
            "bits" => Int(self.bits as i64),
            "channels" => Int(self.channels as i64),
            "bitrate" => Int(self.average_bitrate() as i64),
            "fileSize" => Int(self.file_size),
            "fileName" => Text(self.file_name()),
            "folder" => Text(self.folder()),
            "modified" => Int(self.modified),
            "added" => Int(self.added),
            "rgTrackGain" => Float(self.rg_track_gain),
            "rgTrackPeak" => Float(self.rg_track_peak),
            "rgAlbumGain" => Float(self.rg_album_gain),
            "rgAlbumPeak" => Float(self.rg_album_peak),
            _ => return None,
        })
    }
}

/// A field's value, typed as the lists hand it to their views.
#[derive(Clone, Debug, PartialEq)]
pub enum FieldValue {
    Text(String),
    Int(i64),
    Float(f64),
    Bool(bool),
}

impl FieldValue {
    pub fn is_text(&self) -> bool {
        matches!(self, FieldValue::Text(_))
    }
    /// As text.
    pub fn to_text(&self) -> String {
        match self {
            FieldValue::Text(s) => s.clone(),
            FieldValue::Int(i) => i.to_string(),
            FieldValue::Float(f) => json_out::format_double(*f),
            FieldValue::Bool(b) => b.to_string(),
        }
    }
    /// As a number (text that is no number gives 0).
    pub fn to_f64(&self) -> f64 {
        match self {
            FieldValue::Text(s) => s.trim().parse().unwrap_or(0.0),
            FieldValue::Int(i) => *i as f64,
            FieldValue::Float(f) => *f,
            FieldValue::Bool(b) => f64::from(u8::from(*b)),
        }
    }
}

/// 'g' with 4 digits: at most four significant digits, no
/// trailing zeros ("44.1", "96", "176.4", "2822").
pub(crate) fn format_g4(v: f64) -> String {
    if v == 0.0 {
        return "0".into();
    }
    let exp = v.abs().log10().floor() as i32;
    if !(-5..4).contains(&exp) {
        // %g switches to exponent form; sample rates never get there.
        let s = format!("{:.3e}", v);
        return s;
    }
    let decimals = (3 - exp).max(0) as usize;
    let mut s = format!("{:.*}", decimals, v);
    // Rounding may have added a digit (999.95 -> "1000.0"): %g would then
    // have one decimal less, which trimming the zeros gives anyway.
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        let mut t = TrackMeta { codec: Arc::from("FLAC"), bits: 24, sample_rate: 96000, ..Default::default() };
        assert_eq!(t.format_label(), "FLAC 24/96");
        t.sample_rate = 44100;
        t.bits = 16;
        assert_eq!(t.format_label(), "FLAC 16/44.1");
        t.sample_rate = 176400;
        assert_eq!(t.format_label(), "FLAC 16/176.4");
        t.sample_rate = 2822400;
        assert_eq!(t.format_label(), "FLAC 16/2822");
        t.sample_rate = 11025;
        assert_eq!(t.format_label(), "FLAC 16/11.03"); // 11.025 is 11.02500000000000035… in binary
        t.bits = 0;
        t.bitrate_kbps = 320;
        t.codec = Arc::from("MP3");
        assert_eq!(t.format_label(), "MP3 320k");
        t.bitrate_kbps = 0;
        assert_eq!(t.format_label(), "MP3");
    }

    #[test]
    fn json_round_trip() {
        let t = TrackMeta {
            path: "/m/a.flac".into(),
            title: "T".into(),
            duration: 3.030204081632653,
            rg_track_gain: -6.54,
            rg_track_peak: 0.988,
            ..Default::default()
        };
        let v = t.to_json();
        assert!(v.get("rag").is_none());
        let back = TrackMeta::from_json(&v);
        assert_eq!(back.duration, t.duration);
        assert_eq!(back.rg_track_gain, -6.54);
        assert!(back.rg_album_gain.is_nan());
    }
}
