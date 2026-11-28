//! The rows of a track list, made when the list view asks for them (a
//! library of 50 000 tracks costs a vector of indices, not of rows), and the
//! columns' layout.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use slint::{Model, ModelNotify, ModelRc, ModelTracker, SharedString, VecModel};

use crate::i18n::tr;
use crate::lib_shim::{Library, Track};
use crate::{Cell, Column, TrackRow};

/// A column of the catalogue: key, title, width,
/// fixed (never stretches), badge.
pub struct ColumnDef {
    pub key: &'static str,
    pub title: &'static str,
    pub width: f32,
    pub fixed: bool,
    pub badge: bool,
}

pub const CATALOG: [ColumnDef; 21] = [
    ColumnDef { key: "title", title: "Title", width: 260.0, fixed: false, badge: false },
    ColumnDef { key: "artist", title: "Artist", width: 180.0, fixed: false, badge: false },
    ColumnDef { key: "album", title: "Album", width: 210.0, fixed: false, badge: false },
    ColumnDef { key: "albumArtist", title: "Album artist", width: 170.0, fixed: false, badge: false },
    ColumnDef { key: "genre", title: "Genre", width: 120.0, fixed: false, badge: false },
    ColumnDef { key: "year", title: "Year", width: 60.0, fixed: false, badge: false },
    ColumnDef { key: "trackNo", title: "Track", width: 60.0, fixed: false, badge: false },
    ColumnDef { key: "discNo", title: "Disc", width: 52.0, fixed: false, badge: false },
    // Time and Format start out as the user set them (measured on their
    // screenshots at 165 %, 2026-11-24 and 11-28): Format 113, never under
    // its widest badge, and Time 79.
    ColumnDef { key: "duration", title: "Time", width: 79.0, fixed: false, badge: false },
    ColumnDef { key: "format", title: "Format", width: 113.0, fixed: false, badge: true },
    ColumnDef { key: "codec", title: "Codec", width: 70.0, fixed: false, badge: false },
    ColumnDef { key: "sampleRate", title: "Sample rate", width: 96.0, fixed: false, badge: false },
    ColumnDef { key: "bits", title: "Bit depth", width: 80.0, fixed: false, badge: false },
    ColumnDef { key: "channels", title: "Channels", width: 80.0, fixed: false, badge: false },
    ColumnDef { key: "bitrate", title: "Bitrate", width: 121.0, fixed: false, badge: false },
    ColumnDef { key: "fileSize", title: "Size", width: 84.0, fixed: false, badge: false },
    ColumnDef { key: "fileName", title: "File name", width: 230.0, fixed: false, badge: false },
    ColumnDef { key: "folder", title: "Folder", width: 260.0, fixed: false, badge: false },
    ColumnDef { key: "added", title: "Date added", width: 110.0, fixed: false, badge: false },
    ColumnDef { key: "modified", title: "Modified", width: 110.0, fixed: false, badge: false },
    ColumnDef { key: "favorite", title: "Favorite", width: 44.0, fixed: true, badge: false },
];

pub const DEFAULT_KEYS: [&str; 4] = ["fileName", "format", "duration", "favorite"];

pub fn column(key: &str) -> Option<&'static ColumnDef> {
    CATALOG.iter().find(|c| c.key == key)
}

/// "m:ss", or "h:mm:ss" from an hour.
pub fn format_time(seconds: f64) -> String {
    let s = seconds.max(0.0) as i64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

fn human_size(bytes: i64) -> String {
    if bytes <= 0 {
        return String::new();
    }
    let units = ["B", "KB", "MB", "GB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < 3 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 { format!("{bytes} B") } else { format!("{:.1} {}", v, units[u]) }
}

/// The name to read: without its extension or a leading track number.
fn readable_file_name(name: &str) -> String {
    let mut n = name.to_owned();
    if let Some(dot) = n.rfind('.') {
        let ext = &n[dot + 1..];
        if (2..=5).contains(&ext.len()) && ext.chars().all(|c| c.is_ascii_alphanumeric()) {
            n.truncate(dot);
        }
    }
    // ^\d{1,3}\s*[.\-_)]\s*
    let digits = n.chars().take_while(|c| c.is_ascii_digit()).count();
    if (1..=3).contains(&digits) {
        let rest = n[digits..].trim_start();
        if let Some(c) = rest.chars().next() {
            if ".-_)".contains(c) {
                return rest[c.len_utf8()..].trim_start().to_owned();
            }
        }
    }
    n
}

/// What a cell shows.
pub fn cell_text(key: &str, t: &Track) -> String {
    match key {
        "title" => {
            if t.title.is_empty() {
                readable_file_name(t.file_name())
            } else {
                t.title.clone()
            }
        }
        "artist" => t.artist.clone(),
        "album" => t.album.clone(),
        "albumArtist" => t.album_artist.clone(),
        "genre" => t.genre.clone(),
        "year" => {
            if t.year > 0 {
                t.year.to_string()
            } else {
                String::new()
            }
        }
        "trackNo" => {
            if t.track_no > 0 {
                t.track_no.to_string()
            } else {
                String::new()
            }
        }
        "discNo" => {
            if t.disc_no > 0 {
                t.disc_no.to_string()
            } else {
                String::new()
            }
        }
        "duration" => format_time(t.duration),
        "format" => t.format_label(),
        "codec" => t.codec.clone(),
        "sampleRate" => {
            if t.sample_rate > 0 {
                format!("{} kHz", crate::lib_shim::g4(t.sample_rate as f64 / 1000.0))
            } else {
                String::new()
            }
        }
        "bits" => {
            if t.bits > 0 {
                format!("{}-bit", t.bits)
            } else {
                "—".into()
            }
        }
        "channels" => match t.channels {
            1 => tr("TrackColumns", "Mono"),
            2 => tr("TrackColumns", "Stereo"),
            c if c > 0 => format!("{c} ch"),
            _ => String::new(),
        },
        "bitrate" => {
            // The average bitrate: from the file's size when the tags have none.
            let kbps = if t.bitrate > 0 {
                t.bitrate as i64
            } else if t.duration > 0.0 {
                (t.file_size as f64 * 8.0 / t.duration / 1000.0 + 0.5) as i64
            } else {
                0
            };
            if kbps > 0 { format!("{kbps} kbps") } else { String::new() }
        }
        "fileSize" => human_size(t.file_size),
        "fileName" => readable_file_name(t.file_name()),
        "folder" => t.folder().to_owned(),
        _ => String::new(),
    }
}

/// What the rows read: the library, the list's track indices, the columns
/// shown, the playing path, the selection.
pub struct ListState {
    pub library: Rc<RefCell<Library>>,
    pub indices: Vec<usize>,
    pub keys: Vec<String>,
    pub playing: String,
    pub selected: HashSet<usize>,
    pub artist_line: bool,
    /// The covers' side and corner radius (logical px); 0: no covers.
    pub cover_side: f32,
    pub cover_radius: f32,
}

pub struct TrackListModel {
    pub state: RefCell<ListState>,
    notify: ModelNotify,
}

impl TrackListModel {
    pub fn new(library: Rc<RefCell<Library>>) -> Rc<Self> {
        Rc::new(TrackListModel {
            state: RefCell::new(ListState {
                library,
                indices: Vec::new(),
                keys: DEFAULT_KEYS.iter().map(|s| s.to_string()).collect(),
                playing: String::new(),
                selected: HashSet::new(),
                artist_line: false,
                cover_side: 0.0,
                cover_radius: 0.0,
            }),
            notify: ModelNotify::default(),
        })
    }

    /// The whole list changed (another page, order, filter, columns).
    pub fn reset(&self) {
        self.notify.reset();
    }

    /// Every row asked for again (covers arrived): the list keeps its place.
    pub fn refresh_all(&self) {
        let n = self.state.borrow().indices.len();
        for row in 0..n {
            self.notify.row_changed(row);
        }
    }

    pub fn changed(&self, row: usize) {
        self.notify.row_changed(row);
    }

    pub fn path_at(&self, row: usize) -> Option<String> {
        let s = self.state.borrow();
        let lib = s.library.borrow();
        s.indices.get(row).and_then(|&i| lib.tracks.get(i)).map(|t| t.path.clone())
    }

    /// The rows showing one of these paths.
    pub fn rows_of(&self, wanted: &[&str]) -> Vec<usize> {
        let s = self.state.borrow();
        let lib = s.library.borrow();
        s.indices
            .iter()
            .enumerate()
            .filter(|&(_, &i)| lib.tracks.get(i).is_some_and(|t| wanted.contains(&t.path.as_str())))
            .map(|(row, _)| row)
            .collect()
    }

    pub fn paths(&self) -> Vec<String> {
        let s = self.state.borrow();
        let lib = s.library.borrow();
        s.indices.iter().filter_map(|&i| lib.tracks.get(i)).map(|t| t.path.clone()).collect()
    }
}

impl Model for TrackListModel {
    type Data = TrackRow;

    fn row_count(&self) -> usize {
        self.state.borrow().indices.len()
    }

    fn row_data(&self, row: usize) -> Option<TrackRow> {
        let s = self.state.borrow();
        let lib = s.library.borrow();
        let t = lib.tracks.get(*s.indices.get(row)?)?;
        let special = t.bits > 16 || t.sample_rate > 48000 || t.codec.contains("DSD");
        let cells: Vec<Cell> = s
            .keys
            .iter()
            .map(|k| Cell { text: SharedString::from(cell_text(k, t)), special: k == "format" && special })
            .collect();
        let cover = if s.cover_side > 0.0 { crate::covers::get(&t.path, s.cover_side, s.cover_radius) } else { None };
        Some(TrackRow {
            number: SharedString::from((row + 1).to_string()),
            path: SharedString::from(t.path.as_str()),
            cover: cover.clone().unwrap_or_default(),
            has_cover: cover.is_some(),
            cells: ModelRc::new(VecModel::from(cells)),
            playing: !s.playing.is_empty() && t.path == s.playing,
            missing: false,
            favorite: lib.favorites.contains(&t.path),
            selected: s.selected.contains(&row),
            artist_line: if s.artist_line {
                SharedString::from(match (t.artist.is_empty(), t.album.is_empty()) {
                    (false, false) => format!("{} · {}", t.artist, t.album),
                    (false, true) => t.artist.clone(),
                    (true, false) => t.album.clone(),
                    _ => String::new(),
                })
            } else {
                SharedString::default()
            },
        })
    }

    fn model_tracker(&self) -> &dyn ModelTracker {
        &self.notify
    }
}

/// The layout of a list: the leading number
/// column, the cover column (0 when it does not fit), the columns' widths.
pub struct Layout {
    pub columns: Vec<Column>,
    pub cover: f32,
}

const LEADING: f64 = 36.0;
const CELL_PADDING: f64 = 12.0;

/// A column's least width: never under the header's title (with room for
/// the sort arrow) nor the widest badge.
pub fn min_width_of(key: &str, widest_badge: f64) -> f64 {
    if key == "favorite" {
        return 40.0;
    }
    let Some(c) = column(key) else { return 48.0 };
    let header =
        (crate::metrics::width(&tr("TrackColumns", c.title), 11.0, 500, 0.0) as f64).ceil() + 2.0 * CELL_PADDING + 15.0;
    let badge = if c.badge { widest_badge.ceil() + 12.0 + 2.0 * CELL_PADDING } else { 0.0 };
    48f64.max(header).max(badge)
}

/// A column's width: the saved width (never under the minimum), else
/// the catalogue's; the first column starts out wide.
fn width_of(key: &str, keys: &[String], saved: &serde_json::Map<String, serde_json::Value>, widest_badge: f64) -> f64 {
    let min = min_width_of(key, widest_badge);
    if let Some(w) = saved.get(key).and_then(|v| v.as_f64()).map(f64::round).filter(|w| *w > 0.0) {
        return w.max(min);
    }
    let base = column(key).map_or(100.0, |c| (c.width as f64).max(min));
    if keys.len() > 1 && keys.first().map(String::as_str) == Some(key) { base.max(829.0) } else { base }
}

/// The columns laid out for rows whose cells share `list_w` px (from the
/// row's left edge: the number, the cover, the columns; tracks.slint keeps
/// what is right of them).
pub fn layout_columns(
    keys: &[String],
    saved: &serde_json::Map<String, serde_json::Value>,
    list_w: f32,
    covers: bool,
    spacious: bool,
    widest_badge: f64,
) -> Layout {
    let list_w = list_w as f64;
    let flex = keys
        .iter()
        .rev()
        .find(|k| column(k).map_or(true, |c| !c.fixed))
        .cloned()
        .or_else(|| keys.last().cloned())
        .unwrap_or_default();
    let primary = if keys.len() > 1 { keys[0].clone() } else { String::new() };
    let wof = |k: &str| width_of(k, keys, saved, widest_badge);
    let mof = |k: &str| min_width_of(k, widest_badge);
    let cover_full = if spacious { 62.0 } else { 52.0 };
    let primary_full = if primary.is_empty() { 0.0 } else { wof(&primary) };
    let primary_min =
        if primary.is_empty() { 0.0 } else { primary_full.min(mof(&primary).max(200f64.min((list_w / 4.0).round()))) };
    let mut narrowest = LEADING + if flex.is_empty() { 0.0 } else { wof(&flex) };
    let (mut fixed, mut rest_slack) = (0.0, 0.0);
    for k in keys {
        if *k == flex {
            continue;
        }
        narrowest += if *k == primary { primary_min } else { mof(k) };
        fixed += wof(k);
        if *k != primary {
            rest_slack += wof(k) - mof(k);
        }
    }
    let cover = if covers && list_w >= narrowest + cover_full { cover_full } else { 0.0 };
    let flex_own = if flex.is_empty() { 0.0 } else { wof(&flex) };
    let available_flex = list_w - LEADING - cover - fixed;
    let shortfall = (flex_own - available_flex).max(0.0);
    let primary_width = primary_min.max(primary_full - shortfall);
    let primary_shrink = primary_full - primary_width;
    let rest_ratio = if rest_slack > 0.0 { ((shortfall - primary_shrink).max(0.0) / rest_slack).min(1.0) } else { 0.0 };
    let rest_shrink = rest_slack * rest_ratio;
    let mut flex_width = flex_own.max(available_flex + primary_shrink + rest_shrink);
    // Room to spare goes to the first column until the user sizes it (the
    // first launch: the name wide, Format and Time as they are); after that,
    // to the last one.
    let mut primary_width = primary_width;
    if !primary.is_empty() && !saved.contains_key(&primary) && flex_width > flex_own {
        primary_width += flex_width - flex_own;
        flex_width = flex_own;
    }
    let mut left = 0.0f32;
    let columns = keys
        .iter()
        .map(|k| {
            let w = if *k == flex {
                flex_width
            } else if *k == primary {
                primary_width
            } else {
                wof(k) - (wof(k) - mof(k)) * rest_ratio
            };
            let def = column(k);
            let width = w.floor() as f32;
            let x = left;
            left += width;
            Column {
                key: SharedString::from(k.as_str()),
                title: SharedString::from(def.map(|c| tr("TrackColumns", c.title)).unwrap_or_default()),
                x,
                width,
                badge: def.is_some_and(|c| c.badge),
                flex: *k == flex,
            }
        })
        .collect();
    Layout { columns, cover: cover as f32 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readable_names() {
        assert_eq!(readable_file_name("01 - Song One.flac"), "Song One");
        assert_eq!(readable_file_name("3_Intro.mp3"), "Intro");
        assert_eq!(readable_file_name("1999.flac"), "1999");
        assert_eq!(format_time(3725.0), "1:02:05");
        assert_eq!(format_time(65.0), "1:05");
    }

    /// The first launch: Format 113 px (wider only for a wider badge), Time
    /// 79 px, the name the rest.
    #[test]
    fn first_launch_columns() {
        let keys: Vec<String> = DEFAULT_KEYS.iter().map(|k| k.to_string()).collect();
        let badge = 69.0; // "FLAC 16/44.1": a minimum of 105
        let layout = layout_columns(&keys, &serde_json::Map::new(), 1300.0, true, true, badge);
        let width = |k: &str| layout.columns.iter().find(|c| c.key == k).unwrap().width;
        assert_eq!(width("format"), 113.0);
        let wide = layout_columns(&keys, &serde_json::Map::new(), 1300.0, true, true, 90.0);
        let wide_format = wide.columns.iter().find(|c| c.key == "format").unwrap().width;
        assert_eq!(wide_format as f64, min_width_of("format", 90.0));
        assert_eq!(width("duration"), 79.0);
        // The window's width left goes to the name.
        assert_eq!(width("fileName"), 1300.0 - 36.0 - 62.0 - 113.0 - 79.0 - 44.0);
        // Narrower than that, the name gives way and the rest stays.
        let narrow = layout_columns(&keys, &serde_json::Map::new(), 900.0, true, true, badge);
        let narrow_width = |k: &str| narrow.columns.iter().find(|c| c.key == k).unwrap().width;
        assert_eq!((narrow_width("format"), narrow_width("duration")), (113.0, 79.0));
        // Once the user sized the name, the last column takes what is left.
        let mut saved = serde_json::Map::new();
        saved.insert("fileName".into(), serde_json::json!(700));
        let sized = layout_columns(&keys, &saved, 1300.0, true, true, badge);
        let sized_width = |k: &str| sized.columns.iter().find(|c| c.key == k).unwrap().width;
        assert_eq!(sized_width("fileName"), 700.0);
        assert!(sized_width("duration") > 79.0);
    }
}
