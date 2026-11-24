//! The music library: every audio file under the watched folders plus files
//! added one by one, kept in library.json with the schema earlier Sonkore
//! versions wrote. Scans run in the background, incrementally (unchanged
//! files keep their entries), and again when the folders change.
//!
//! Threads: a `Library` lives on one thread (the interface's). Background
//! work (scans, loudness analysis, folder watching) reports through a
//! channel; whenever it has something, the notifier set with
//! `set_notifier` is called (from any thread), and the owner then calls
//! `process()`, which applies the results and returns what changed as
//! `Event`s.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::{Map, Value};
use sonkore_core::paths;
use sonkore_core::settings::Settings;

use crate::collate::{Collator, utf16_cmp};
use crate::json_out;
use crate::loudness;
use crate::meta::TrackMeta;
use crate::metadata;
use crate::scan::{self, ScanInput, ScanResult};
use crate::watch::FolderWatcher;
use crate::{Text, cue};

/// library.json's format version (kCacheVersion).
/// 2: ReplayGain / R128 tags are read. 3: DSF / DSDIFF lengths;
/// unreadable files dropped. 4: loudness measured with mono, 7.1 sides
/// and lossy peaks done right.
pub const CACHE_VERSION: i64 = 4;

/// The browser's key for an empty tag (no genre, no artist…).
pub const UNKNOWN: &str = "\u{1}";

/// Where the library keeps its files.
#[derive(Clone, Debug)]
pub struct LibraryFiles {
    pub library_json: PathBuf,
    pub favorites_json: PathBuf,
    pub settings: PathBuf,
}

impl LibraryFiles {
    /// The standard ones: $XDG_DATA_HOME/sonkore/library.json & co.
    pub fn standard() -> LibraryFiles {
        LibraryFiles {
            library_json: paths::data_file("library.json"),
            favorites_json: paths::data_file("favorites.json"),
            settings: paths::settings_file(),
        }
    }

    /// The same names in another data folder (tests, tools).
    pub fn in_dir(data_dir: &Path, settings: &Path) -> LibraryFiles {
        LibraryFiles {
            library_json: data_dir.join("library.json"),
            favorites_json: data_dir.join("favorites.json"),
            settings: settings.to_path_buf(),
        }
    }
}

/// What changed.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The watched folders.
    FoldersChanged,
    /// The tracks were replaced (a model reset): count, total duration,
    /// format labels and every derived list.
    TracksReset,
    CountChanged,
    ScanningChanged,
    ScanProgress,
    PreferencesChanged,
    LoudnessProgress,
    FavoritesChanged,
    /// One track's row changed (its favourite flag).
    TrackChanged(String),
    /// Gains measured for these tracks (the player re-applies them).
    LoudnessUpdated(Vec<String>),
}

/// A browser entry.
#[derive(Clone, Debug, PartialEq)]
pub struct BrowseEntry {
    /// The value, or UNKNOWN for an empty tag.
    pub key: String,
    pub name: String,
    pub count: usize,
    pub albums: usize,
    /// The first album's key.
    pub album_key: String,
}

/// A folder that holds tracks directly.
#[derive(Clone, Debug, PartialEq)]
pub struct TrackFolder {
    pub path: String,
    /// Relative to its library folder ("Rock/Alt").
    pub name: String,
    pub count: usize,
}

/// Turns a Flatpak document-portal path into the host's. The library
/// applies it where earlier versions did; without one, paths are kept as
/// they are.
pub type PathMapper = Arc<dyn Fn(&str) -> String + Send + Sync>;

type Notifier = Arc<Mutex<Option<Box<dyn Fn() + Send + Sync>>>>;

/// Gains measured for one album.
struct AlbumGains {
    paths: Vec<String>,
    modified: Vec<i64>,
    track_gain: Vec<f64>,
    track_peak: Vec<f64>,
    album_gain: f64,
    album_peak: f64,
    album_ok: bool,
}

enum Msg {
    Progress(usize),
    ScanDone(Box<ScanResult>),
    Rescan,
    Loudness(Box<AlbumGains>),
    LoudnessDone,
}

/// The library.
pub struct Library {
    files: LibraryFiles,
    tracks: Vec<TrackMeta>,
    index: HashMap<String, usize>,
    search_text: Vec<String>,
    folders: Vec<String>,
    extra_files: Vec<String>,
    hidden: Vec<String>,
    favorites: HashSet<String>,
    favorites_revision: u64,
    revision: u64,

    watch_folders: bool,
    rescan_on_startup: bool,
    excluded_folders: Vec<String>,
    min_duration: i32,

    scanning: bool,
    rescan_pending: bool,
    scanned_files: usize,
    cancel: Arc<AtomicBool>,
    scan_threads: usize,

    analyzing: bool,
    analyze_pending: bool,
    auto_analyze: bool,
    loudness_done: usize,
    loudness_total: usize,
    unsaved_albums: usize,

    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    notifier: Notifier,
    workers: Vec<JoinHandle<()>>,
    watcher: Option<FolderWatcher>,
    scanned_dirs: Vec<String>,
    path_mapper: Option<PathMapper>,
    /// `cache_fingerprint` of what library.json holds (None: unknown).
    saved: Cell<Option<u64>>,
}

fn notify(n: &Notifier) {
    if let Some(f) = n.lock().unwrap().as_ref() {
        f();
    }
}

/// A clean path: no "//", "." or "..", no trailing '/'. On Windows with '/'
/// (the stored form: "C:\Music" is "C:/Music"), keeping the root: "C:/", or
/// the "//" of a UNC path.
pub fn clean_path(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }
    let path = paths::stored_path(path);
    let root = path_root(&path);
    let absolute = root.ends_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for part in path[root.len()..].split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|p| *p != "..") {
                    parts.pop();
                } else if !absolute {
                    parts.push("..");
                }
            }
            p => parts.push(p),
        }
    }
    let joined = parts.join("/");
    if !root.is_empty() {
        format!("{root}{joined}")
    } else if joined.is_empty() {
        ".".into()
    } else {
        joined
    }
}

/// What `clean_path` keeps before the first part: "/" on Unix; on Windows
/// also "C:/" ("C:" alone is relative to that drive) and "//" (UNC).
fn path_root(path: &str) -> &str {
    let b = path.as_bytes();
    if cfg!(windows) {
        if path.starts_with("//") {
            return "//";
        }
        if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
            return if b.get(2) == Some(&b'/') { &path[..3] } else { &path[..2] };
        }
    }
    if path.starts_with('/') { "/" } else { "" }
}

/// A local path from a path or a file:// URL.
pub fn local_path(url_or_path: &str) -> String {
    if let Some(rest) = url_or_path.strip_prefix("file://") {
        let rest = rest.strip_prefix("localhost").unwrap_or(rest);
        return percent_encoding::percent_decode_str(rest).decode_utf8_lossy().into_owned();
    }
    url_or_path.to_string()
}

fn inside_folders(folders: &[String], path: &str) -> bool {
    folders.iter().any(|f| path == f || path.starts_with(&format!("{f}/")))
}

fn restore_under(hidden: &mut Vec<String>, folder: &str) -> bool {
    let prefix = format!("{folder}/");
    let before = hidden.len();
    hidden.retain(|p| !p.starts_with(&prefix));
    hidden.len() != before
}

/// Writes a file whole: to a temporary name, on the disk, then renamed
/// over it (a crash or power cut leaves the old file or the new one).
pub(crate) fn write_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    let written = fs::File::create(&tmp).and_then(|mut f| {
        f.write_all(bytes)?;
        f.sync_all()
    });
    written.and_then(|()| fs::rename(&tmp, path)).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

/// A track's gains and peaks, bit for bit (NaN is "none").
fn gain_bits(t: &TrackMeta) -> [u64; 4] {
    [t.rg_track_gain, t.rg_track_peak, t.rg_album_gain, t.rg_album_peak].map(f64::to_bits)
}

/// The same entry, as far as a scan changes one: it reads an entry again
/// only when the file's date or size changed, and carries over the loudness
/// measured meanwhile.
fn same_entry(a: &TrackMeta, b: &TrackMeta) -> bool {
    a.path == b.path
        && (a.modified, a.file_size, a.added, a.rg_analyzed) == (b.modified, b.file_size, b.added, b.rg_analyzed)
        && gain_bits(a) == gain_bits(b)
}

/// Tracks of one album share their artist, album, genre and codec: one
/// copy of each distinct text.
fn share_repeated_strings(tracks: &mut [TrackMeta]) {
    let mut pool: HashSet<Text> = HashSet::new();
    let mut intern = |s: &mut Text| {
        if s.is_empty() {
            return;
        }
        match pool.get(s) {
            Some(shared) => *s = shared.clone(),
            None => {
                pool.insert(s.clone());
            }
        }
    };
    for t in tracks {
        intern(&mut t.artist);
        intern(&mut t.album_artist);
        intern(&mut t.album);
        intern(&mut t.genre);
        intern(&mut t.codec);
    }
}

impl Library {
    /// Opens the library at the standard places.
    pub fn open_standard() -> Library {
        Library::open(LibraryFiles::standard())
    }

    /// Loads library.json, favorites.json and the library settings; when
    /// rescanOnStartup is on and there is something to scan, a scan starts
    /// half a second later (to pick up what changed while closed).
    pub fn open(files: LibraryFiles) -> Library {
        Library::open_with(files, None)
    }

    /// As `open`, with the document-portal path mapper.
    pub fn open_with(files: LibraryFiles, path_mapper: Option<PathMapper>) -> Library {
        let (tx, rx) = mpsc::channel();
        let settings = Settings::open(&files.settings);
        let map = |l: Vec<String>| -> Vec<String> {
            match &path_mapper {
                Some(m) => l.iter().map(|p| m(p)).collect(),
                None => l,
            }
        };
        let mut lib = Library {
            watch_folders: settings.flag("library/watchFolders", true),
            rescan_on_startup: settings.flag("library/rescanOnStartup", true),
            excluded_folders: map(settings.list("library/excludedFolders")),
            min_duration: settings.int("library/minDuration", 0).clamp(0, 600) as i32,
            files,
            tracks: Vec::new(),
            index: HashMap::new(),
            search_text: Vec::new(),
            folders: Vec::new(),
            extra_files: Vec::new(),
            hidden: Vec::new(),
            favorites: HashSet::new(),
            favorites_revision: 0,
            revision: 0,
            scanning: false,
            rescan_pending: false,
            scanned_files: 0,
            cancel: Arc::new(AtomicBool::new(false)),
            scan_threads: thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(8),
            analyzing: false,
            analyze_pending: false,
            auto_analyze: false,
            loudness_done: 0,
            loudness_total: 0,
            unsaved_albums: 0,
            tx,
            rx,
            notifier: Arc::new(Mutex::new(None)),
            workers: Vec::new(),
            watcher: None,
            scanned_dirs: Vec::new(),
            path_mapper,
            saved: Cell::new(None),
        };
        lib.load_cache();
        lib.load_favorites();
        if lib.rescan_on_startup && (!lib.folders.is_empty() || !lib.extra_files.is_empty()) {
            let tx = lib.tx.clone();
            let notifier = lib.notifier.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(500));
                if tx.send(Msg::Rescan).is_ok() {
                    notify(&notifier);
                }
            });
        }
        lib
    }

    /// Called from any thread when background work has results: the owner
    /// should then call `process()` on the library's thread.
    pub fn set_notifier(&mut self, f: impl Fn() + Send + Sync + 'static) {
        *self.notifier.lock().unwrap() = Some(Box::new(f));
    }

    /// Threads reading tags during a scan (default: the cores, up to 8).
    pub fn set_scan_threads(&mut self, n: usize) {
        self.scan_threads = n.max(1);
    }

    fn map_path(&self, p: &str) -> String {
        match &self.path_mapper {
            Some(m) => m(p),
            None => p.to_string(),
        }
    }

    // --- Queries --------------------------------------------------------------

    pub fn files(&self) -> &LibraryFiles {
        &self.files
    }
    /// Library order: album artist, year, album, disc, track, path.
    pub fn tracks(&self) -> &[TrackMeta] {
        &self.tracks
    }
    pub fn count(&self) -> usize {
        self.tracks.len()
    }
    /// Bumped whenever the tracks are replaced (views rebuild then).
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn folders(&self) -> &[String] {
        &self.folders
    }
    pub fn extra_files(&self) -> &[String] {
        &self.extra_files
    }
    pub fn total_duration(&self) -> f64 {
        self.tracks.iter().map(|t| t.duration).sum()
    }
    /// Every format label in the library, once each (sorted).
    pub fn format_labels(&self) -> Vec<String> {
        let set: HashSet<String> = self.tracks.iter().map(|t| t.format_label()).collect();
        let mut v: Vec<String> = set.into_iter().collect();
        v.sort();
        v
    }
    pub fn scanning(&self) -> bool {
        self.scanning
    }
    pub fn scanned_files(&self) -> usize {
        self.scanned_files
    }
    pub fn find(&self, path: &str) -> Option<&TrackMeta> {
        self.index.get(path).map(|&i| &self.tracks[i])
    }
    /// The row of a path in `tracks()`.
    pub fn row_of(&self, path: &str) -> Option<usize> {
        self.index.get(path).copied()
    }
    pub fn contains(&self, path: &str) -> bool {
        self.index.contains_key(path)
    }
    /// The lower-case text a search looks in (a library track's).
    pub fn search_text(&self, path: &str) -> Option<&str> {
        self.index.get(path).map(|&i| self.search_text[i].as_str())
    }
    /// The search text of row `row`.
    pub fn search_text_at(&self, row: usize) -> &str {
        &self.search_text[row]
    }
    /// A track's facts: the library's entry, else read from the file.
    pub fn track(&self, path: &str) -> TrackMeta {
        if let Some(t) = self.find(path) {
            return t.clone();
        }
        let mut t = TrackMeta::default();
        metadata::read_into(path, &mut t);
        t
    }
    /// "*.flac *.wav *.mp3 …" for file dialogs.
    pub fn audio_name_filter(&self) -> String {
        metadata::audio_extensions().iter().map(|e| format!("*.{e}")).collect::<Vec<_>>().join(" ")
    }
    pub fn hidden_count(&self) -> usize {
        self.hidden.len()
    }
    pub fn hidden(&self) -> &[String] {
        &self.hidden
    }
    pub fn watch_folders(&self) -> bool {
        self.watch_folders
    }
    pub fn rescan_on_startup(&self) -> bool {
        self.rescan_on_startup
    }
    pub fn excluded_folders(&self) -> &[String] {
        &self.excluded_folders
    }
    pub fn min_duration(&self) -> i32 {
        self.min_duration
    }
    /// The folders being watched for changes now.
    pub fn watched_dirs(&self) -> Vec<String> {
        self.watcher.as_ref().map(|w| w.watched().to_vec()).unwrap_or_default()
    }

    /// The tracks of an album (library order: by disc and track).
    pub fn album_paths(&self, album_key: &str) -> Vec<String> {
        self.tracks.iter().filter(|t| t.album_key() == album_key).map(|t| t.path.clone()).collect()
    }

    /// The tracks inside a folder, recursively, in library order.
    pub fn folder_paths(&self, folder: &str) -> Vec<String> {
        let prefix = if folder.ends_with('/') { folder.to_string() } else { format!("{folder}/") };
        self.tracks.iter().filter(|t| t.path.starts_with(&prefix)).map(|t| t.path.clone()).collect()
    }

    /// Every folder that holds tracks directly, in natural order, named from
    /// the library folder that holds it.
    pub fn track_folders(&self) -> Vec<TrackFolder> {
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for t in &self.tracks {
            if let Some(slash) = t.path.rfind('/')
                && slash > 0
            {
                *counts.entry(&t.path[..slash]).or_default() += 1;
            }
        }
        let collator = Collator::natural();
        let mut dirs: Vec<(&str, Vec<u32>)> = counts.keys().map(|d| (*d, collator.sort_key(d))).collect();
        dirs.sort_by(|a, b| a.1.cmp(&b.1));
        dirs.into_iter()
            .map(|(dir, _)| {
                let mut name = dir.rsplit('/').next().unwrap_or("").to_string();
                for root in &self.folders {
                    if dir == root {
                        break;
                    }
                    if dir.starts_with(&format!("{root}/")) {
                        name = dir[root.len() + 1..].to_string();
                        break;
                    }
                }
                TrackFolder { path: dir.to_string(), name, count: counts[dir] }
            })
            .collect()
    }

    /// The values of `field` ("genre", "artist" or "album") among the tracks
    /// that match the given genre / artist ("" = any), by name; an empty tag
    /// is listed once, last, with key UNKNOWN. The artist is the album
    /// artist, else the track artist.
    pub fn browse(&self, field: &str, genre: &str, artist: &str) -> Vec<BrowseEntry> {
        struct E {
            name: String,
            count: usize,
            albums: HashSet<String>,
            album_key: String,
        }
        let key_of = |v: &str| {
            if v.is_empty() { UNKNOWN.to_string() } else { v.to_string() }
        };
        let mut entries: HashMap<String, E> = HashMap::new();
        for t in &self.tracks {
            let track_artist = t.album_or_track_artist();
            if !genre.is_empty() && key_of(&t.genre) != genre {
                continue;
            }
            if !artist.is_empty() && key_of(track_artist) != artist && &*t.artist != artist {
                continue;
            }
            let value: &str = match field {
                "genre" => &t.genre,
                "album" => &t.album,
                _ => track_artist,
            };
            let album_key = t.album_key();
            let e = entries.entry(key_of(value)).or_insert_with(|| E {
                name: String::new(),
                count: 0,
                albums: HashSet::new(),
                album_key: String::new(),
            });
            e.name = value.to_string();
            e.count += 1;
            if e.album_key.is_empty() {
                e.album_key = album_key.clone();
            }
            e.albums.insert(album_key);
        }
        let collator = Collator::locale();
        let mut keys: Vec<(String, Vec<u32>)> =
            entries.iter().map(|(k, e)| (k.clone(), collator.sort_key(&e.name))).collect();
        keys.sort_by(|a, b| {
            let (ua, ub) = (a.0 == UNKNOWN, b.0 == UNKNOWN);
            if ua != ub {
                return ua.cmp(&ub);
            }
            a.1.cmp(&b.1)
        });
        keys.into_iter()
            .map(|(k, _)| {
                let e = &entries[&k];
                BrowseEntry {
                    key: k.clone(),
                    name: e.name.clone(),
                    count: e.count,
                    albums: e.albums.len(),
                    album_key: e.album_key.clone(),
                }
            })
            .collect()
    }

    /// The audio files among `urls` (paths or file:// URLs): folders and
    /// other files left out, a .cue sheet as its tracks.
    pub fn audio_paths(&self, urls: &[String]) -> Vec<String> {
        let mut out = Vec::new();
        for url in urls {
            let path = clean_path(&self.map_path(&local_path(url)));
            let is_file = fs::metadata(&path).map(|m| m.is_file()).unwrap_or(false);
            if is_file && metadata::is_audio_file(&path) {
                out.push(path);
            } else if cue::is_sheet(&path) {
                out.extend(metadata::cue_tracks(&path, None));
            }
        }
        out
    }

    /// As `audio_paths`, with folders expanded (recursively, following
    /// links, in natural order).
    pub fn expand_audio_paths(&self, urls: &[String]) -> Vec<String> {
        let mut out = Vec::new();
        for url in urls {
            let path = clean_path(&self.map_path(&local_path(url)));
            let Ok(meta) = fs::metadata(&path) else {
                continue;
            };
            if meta.is_file() && metadata::is_audio_file(&path) {
                out.push(path);
            } else if meta.is_file() && cue::is_sheet(&path) {
                out.extend(metadata::cue_tracks(&path, None));
            } else if meta.is_dir() {
                let mut found = Vec::new();
                let mut images = Vec::new();
                let mut visited = HashSet::new();
                let mut stack = vec![path.clone()];
                while let Some(dir) = stack.pop() {
                    if !visited.insert(scan::canonical(&dir)) {
                        continue;
                    }
                    for e in scan::list_dir(&dir, false) {
                        if e.is_dir {
                            stack.push(e.path);
                        } else if e.is_file && cue::is_sheet(&e.path) {
                            found.extend(metadata::cue_tracks(&e.path, Some(&mut images)));
                        } else if e.is_file && metadata::is_audio_file(&e.path) {
                            found.push(e.path);
                        }
                    }
                }
                found.retain(|f| !images.contains(f));
                let collator = Collator::natural();
                let mut keyed: Vec<(Vec<u32>, String)> =
                    found.into_iter().map(|f| (collator.sort_key(&f), f)).collect();
                keyed.sort_by(|a, b| a.0.cmp(&b.0));
                out.extend(keyed.into_iter().map(|(_, f)| f));
            }
        }
        out
    }

    // --- Preferences ------------------------------------------------------------

    fn settings(&self) -> Settings {
        Settings::open(&self.files.settings)
    }

    fn save_settings(&self, f: impl FnOnce(&mut Settings)) {
        let mut s = self.settings();
        f(&mut s);
        let _ = s.save();
    }

    pub fn set_watch_folders(&mut self, on: bool) -> Vec<Event> {
        if on == self.watch_folders {
            return Vec::new();
        }
        self.watch_folders = on;
        self.save_settings(|s| s.set_flag("library/watchFolders", on));
        let dirs = self.scanned_dirs.clone();
        self.watch_directories(dirs);
        vec![Event::PreferencesChanged]
    }

    pub fn set_rescan_on_startup(&mut self, on: bool) -> Vec<Event> {
        if on == self.rescan_on_startup {
            return Vec::new();
        }
        self.rescan_on_startup = on;
        self.save_settings(|s| s.set_flag("library/rescanOnStartup", on));
        vec![Event::PreferencesChanged]
    }

    fn save_excluded_folders(&self) {
        let list = self.excluded_folders.clone();
        self.save_settings(|s| s.set_list("library/excludedFolders", &list));
    }

    /// Skips a folder (and what is inside it) from now on (a path or URL).
    pub fn exclude_folder(&mut self, folder: &str) -> Vec<Event> {
        let mut path = self.map_path(&local_path(folder));
        while path.len() > 1 && path.ends_with('/') {
            path.pop();
        }
        if path.is_empty() || self.excluded_folders.contains(&path) {
            return Vec::new();
        }
        self.excluded_folders.push(path.clone());
        self.excluded_folders.sort_by(|a, b| utf16_cmp(a, b));
        self.save_excluded_folders();
        let mut ev = vec![Event::PreferencesChanged];
        ev.extend(self.forget_under(&path));
        ev.extend(self.rescan());
        ev
    }

    /// The tracks under `folder` (not the files added one by one) out now,
    /// and the cache saved: quitting before the scan is through must not
    /// bring them back on the next start.
    fn forget_under(&mut self, folder: &str) -> Vec<Event> {
        let prefix = format!("{folder}/");
        let before = self.tracks.len();
        let extras: HashSet<&str> = self.extra_files.iter().map(String::as_str).collect();
        let kept: Vec<TrackMeta> = self
            .tracks
            .iter()
            .filter(|t| !(t.path == folder || t.path.starts_with(&prefix)) || extras.contains(t.path.as_str()))
            .cloned()
            .collect();
        if kept.len() == before {
            return Vec::new();
        }
        let ev = self.set_tracks(kept);
        self.save_cache();
        ev
    }

    pub fn remove_excluded_folder(&mut self, folder: &str) -> Vec<Event> {
        let Some(i) = self.excluded_folders.iter().position(|f| f == folder) else {
            return Vec::new();
        };
        self.excluded_folders.remove(i);
        self.save_excluded_folders();
        let mut ev = vec![Event::PreferencesChanged];
        ev.extend(self.rescan());
        ev
    }

    /// Files shorter than this (seconds, 0–600) are left out; 0 = all.
    pub fn set_min_duration(&mut self, seconds: i32) -> Vec<Event> {
        let seconds = seconds.clamp(0, 600);
        if seconds == self.min_duration {
            return Vec::new();
        }
        self.min_duration = seconds;
        self.save_settings(|s| s.set_int("library/minDuration", seconds as i64));
        let mut ev = vec![Event::PreferencesChanged];
        ev.extend(self.rescan());
        ev
    }

    // --- Editing ------------------------------------------------------------------

    /// Brings every hand-removed track back (on the next scan).
    pub fn restore_hidden(&mut self) -> Vec<Event> {
        if self.hidden.is_empty() {
            return Vec::new();
        }
        self.hidden.clear();
        let mut ev = vec![Event::CountChanged];
        ev.extend(self.rescan());
        ev
    }

    /// Adds `path` (a folder) to the watched folders; true when the library
    /// has to be scanned again.
    fn watch_folder(&mut self, path: &str, ev: &mut Vec<Event>) -> bool {
        // Adding a folder brings back its skipped sub-folders (and itself).
        let prefix = format!("{path}/");
        let before = self.excluded_folders.len();
        self.excluded_folders.retain(|f| !(f == path || f.starts_with(&prefix)));
        let unskipped = self.excluded_folders.len() != before;
        if unskipped {
            self.save_excluded_folders();
            ev.push(Event::PreferencesChanged);
        }
        // Already in the library: adding it again brings back what was
        // removed from it by hand.
        if inside_folders(&self.folders, path) {
            return restore_under(&mut self.hidden, path) || unskipped;
        }
        // A parent of watched folders takes their place: one root, no overlap.
        self.folders.retain(|f| !f.starts_with(&prefix));
        self.folders.push(path.to_string());
        ev.push(Event::FoldersChanged);
        true
    }

    /// Adds a folder (a path or file:// URL) to the library.
    pub fn add_folder(&mut self, folder: &str) -> Vec<Event> {
        let path = clean_path(&self.map_path(&local_path(folder)));
        if path.is_empty() || !fs::metadata(&path).map(|m| m.is_dir()).unwrap_or(false) {
            return Vec::new();
        }
        let mut ev = Vec::new();
        if self.watch_folder(&path, &mut ev) {
            ev.extend(self.rescan());
        }
        ev
    }

    /// Stops watching a folder: its tracks leave (with the next scan), and
    /// its hand-removed tracks are forgotten.
    pub fn remove_folder(&mut self, folder: &str) -> Vec<Event> {
        let Some(i) = self.folders.iter().position(|f| f == folder) else {
            return Vec::new();
        };
        self.folders.remove(i);
        restore_under(&mut self.hidden, folder);
        let mut ev = vec![Event::FoldersChanged];
        ev.extend(self.forget_under(folder));
        // (Saved even with nothing in it: the folder must not come back.)
        self.save_cache();
        ev.extend(self.rescan());
        ev
    }

    /// Files and / or folders (paths or file:// URLs): folders are watched,
    /// files outside them added one by one, hidden ones inside them shown.
    pub fn add_urls(&mut self, urls: &[String]) -> Vec<Event> {
        let mut ev = Vec::new();
        let mut changed = false;
        let mut extras: HashSet<String> = self.extra_files.iter().cloned().collect();
        let mut shown: HashSet<String> = HashSet::new();
        for url in urls {
            let path = clean_path(&self.map_path(&local_path(url)));
            let Ok(meta) = fs::metadata(&path) else {
                continue;
            };
            if meta.is_dir() {
                changed = self.watch_folder(&path, &mut ev) || changed;
            } else if meta.is_file() && metadata::is_audio_file(&path) {
                if inside_folders(&self.folders, &path) {
                    shown.insert(path);
                } else if extras.insert(path.clone()) {
                    self.extra_files.push(path);
                    changed = true;
                }
            }
        }
        if !shown.is_empty() {
            let before = self.hidden.len();
            self.hidden.retain(|p| !shown.contains(p));
            changed = self.hidden.len() != before || changed;
        }
        if changed {
            ev.extend(self.rescan());
        }
        ev
    }

    /// Removes tracks: those inside watched folders stay hidden even though
    /// their folder is watched (until restored or added again).
    pub fn remove_tracks(&mut self, paths: &[String]) -> Vec<Event> {
        let gone: HashSet<&str> = paths.iter().map(String::as_str).filter(|p| self.index.contains_key(*p)).collect();
        if gone.is_empty() {
            return Vec::new();
        }
        self.extra_files.retain(|p| !gone.contains(p.as_str()));
        let mut hidden: HashSet<String> = self.hidden.iter().cloned().collect();
        for path in paths {
            if gone.contains(path.as_str()) && inside_folders(&self.folders, path) && hidden.insert(path.clone()) {
                self.hidden.push(path.clone());
            }
        }
        let tracks: Vec<TrackMeta> = self.tracks.iter().filter(|t| !gone.contains(t.path.as_str())).cloned().collect();
        let ev = self.set_tracks(tracks);
        self.save_cache();
        ev
    }

    // --- Favourites ------------------------------------------------------------------

    pub fn favorites_revision(&self) -> u64 {
        self.favorites_revision
    }
    pub fn favorite_count(&self) -> usize {
        self.favorites.len()
    }
    pub fn is_favorite(&self, path: &str) -> bool {
        self.favorites.contains(path)
    }
    pub fn favorites(&self) -> &HashSet<String> {
        &self.favorites
    }

    pub fn set_favorite(&mut self, path: &str, on: bool) -> Vec<Event> {
        if path.is_empty() || on == self.favorites.contains(path) {
            return Vec::new();
        }
        if on {
            self.favorites.insert(path.to_string());
        } else {
            self.favorites.remove(path);
        }
        self.save_favorites();
        self.favorites_revision += 1;
        let mut ev = Vec::new();
        if self.index.contains_key(path) {
            ev.push(Event::TrackChanged(path.to_string()));
        }
        ev.push(Event::FavoritesChanged);
        ev
    }

    pub fn toggle_favorite(&mut self, path: &str) -> Vec<Event> {
        let on = !self.is_favorite(path);
        self.set_favorite(path, on)
    }

    /// Several at once (Delete on the Favorites page): favorites.json is
    /// written once, not once per track.
    pub fn set_favorites(&mut self, paths: &[String], on: bool) -> Vec<Event> {
        let mut ev = Vec::new();
        let mut changed = false;
        for path in paths {
            if path.is_empty() || on == self.favorites.contains(path.as_str()) {
                continue;
            }
            if on {
                self.favorites.insert(path.clone());
            } else {
                self.favorites.remove(path.as_str());
            }
            changed = true;
            if self.index.contains_key(path.as_str()) {
                ev.push(Event::TrackChanged(path.clone()));
            }
        }
        if changed {
            self.save_favorites();
            self.favorites_revision += 1;
            ev.push(Event::FavoritesChanged);
        }
        ev
    }

    // --- Scanning --------------------------------------------------------------------

    /// Scans the folders again (in the background). While a scan runs, one
    /// more is queued.
    pub fn rescan(&mut self) -> Vec<Event> {
        if self.scanning {
            self.rescan_pending = true;
            return Vec::new();
        }
        self.scanning = true;
        self.scanned_files = 0;
        let known: HashMap<String, TrackMeta> = self.tracks.iter().map(|t| (t.path.clone(), t.clone())).collect();
        let input = ScanInput {
            folders: self.folders.clone(),
            extras: self.extra_files.clone(),
            hidden: self.hidden.iter().cloned().collect(),
            excluded_folders: self.excluded_folders.clone(),
            min_duration: self.min_duration as f64,
            known,
            threads: self.scan_threads,
        };
        let cancel = self.cancel.clone();
        let tx = self.tx.clone();
        let notifier = self.notifier.clone();
        let handle = thread::Builder::new()
            .name("sonkore-library-scan".into())
            .spawn(move || {
                let progress = |n: usize| {
                    if tx.send(Msg::Progress(n)).is_ok() {
                        notify(&notifier);
                    }
                };
                if let Some(result) = scan::run(input, &cancel, &progress)
                    && tx.send(Msg::ScanDone(Box::new(result))).is_ok()
                {
                    notify(&notifier);
                }
            })
            .expect("scan thread");
        self.workers.retain(|h| !h.is_finished());
        self.workers.push(handle);
        vec![Event::ScanningChanged, Event::ScanProgress]
    }

    fn apply_scan(&mut self, mut result: ScanResult) -> Vec<Event> {
        // The scan started from a copy of the folders, files and hidden
        // tracks: what was removed or skipped since stays out (a scan is
        // queued after it for what was added).
        {
            // (A drive's root, "D:/", ends with its '/' already.)
            fn with_slash(f: &String) -> (&str, String) {
                (f, if f.ends_with('/') { f.clone() } else { format!("{f}/") })
            }
            let roots: Vec<(&str, String)> = self.folders.iter().map(with_slash).collect();
            let skipped: Vec<(&str, String)> = self.excluded_folders.iter().map(with_slash).collect();
            let under =
                |list: &[(&str, String)], p: &str| list.iter().any(|(f, prefix)| p == *f || p.starts_with(prefix));
            let extras: HashSet<&str> = self.extra_files.iter().map(String::as_str).collect();
            let hidden: HashSet<&str> = self.hidden.iter().map(String::as_str).collect();
            result.tracks.retain(|t| {
                let p = t.path.as_str();
                !hidden.contains(p) && (extras.contains(p) || (under(&roots, p) && !under(&skipped, p)))
            });
            result.directories.retain(|d| under(&roots, d) && !under(&skipped, d));
        }
        // Loudness measured meanwhile is carried over to the files that did
        // not change.
        for t in &mut result.tracks {
            let Some(&i) = self.index.get(&t.path) else {
                continue;
            };
            let current = &self.tracks[i];
            if current.rg_analyzed
                && !t.rg_analyzed
                && current.modified == t.modified
                && current.file_size == t.file_size
            {
                t.rg_track_gain = current.rg_track_gain;
                t.rg_track_peak = current.rg_track_peak;
                t.rg_album_gain = current.rg_album_gain;
                t.rg_album_peak = current.rg_album_peak;
                t.rg_analyzed = true;
            }
        }
        // Nothing new (the usual rescan): no model reset, no file written,
        // the same folders watched.
        let same = self.tracks.len() == result.tracks.len()
            && self.tracks.iter().zip(&result.tracks).all(|(a, b)| same_entry(a, b));
        let mut ev = if same { Vec::new() } else { self.set_tracks(result.tracks) };
        // Watched again only when that changes something (thousands of
        // folders take a while).
        if !self.watch_folders || self.watcher.as_ref().is_none_or(|w| !w.watching(&result.directories)) {
            self.watch_directories(result.directories);
        } else {
            self.scanned_dirs = result.directories;
        }
        self.save_cache();
        self.scanning = false;
        self.scanned_files = self.count();
        ev.extend([Event::ScanProgress, Event::ScanningChanged, Event::LoudnessProgress]);
        if self.rescan_pending {
            self.rescan_pending = false;
            ev.extend(self.rescan());
        } else if self.analyze_pending || self.auto_analyze {
            self.analyze_pending = false;
            ev.extend(self.analyze_loudness());
        }
        ev
    }

    fn set_tracks(&mut self, mut tracks: Vec<TrackMeta>) -> Vec<Event> {
        share_repeated_strings(&mut tracks);
        self.tracks = tracks;
        self.index = self.tracks.iter().enumerate().map(|(i, t)| (t.path.clone(), i)).collect();
        self.search_text = self.tracks.iter().map(TrackMeta::search_text).collect();
        self.revision += 1;
        vec![Event::TracksReset, Event::CountChanged]
    }

    fn watch_directories(&mut self, dirs: Vec<String>) {
        self.scanned_dirs = dirs;
        if !self.watch_folders {
            if let Some(w) = &mut self.watcher {
                w.set(&[]);
            }
            return;
        }
        if self.watcher.is_none() {
            let tx = self.tx.clone();
            let notifier = self.notifier.clone();
            self.watcher = FolderWatcher::new(move || {
                if tx.send(Msg::Rescan).is_ok() {
                    notify(&notifier);
                }
            });
        }
        if let Some(w) = &mut self.watcher {
            w.set(&self.scanned_dirs);
        }
    }

    fn handle(&mut self, msg: Msg) -> Vec<Event> {
        match msg {
            Msg::Progress(n) => {
                if self.scanning {
                    self.scanned_files = n;
                    return vec![Event::ScanProgress];
                }
                Vec::new()
            }
            Msg::ScanDone(result) => self.apply_scan(*result),
            Msg::Rescan => self.rescan(),
            Msg::Loudness(gains) => self.apply_loudness(&gains),
            Msg::LoudnessDone => {
                self.save_cache();
                self.analyzing = false;
                vec![Event::LoudnessProgress]
            }
        }
    }

    /// Applies what the background work sent; returns what changed.
    pub fn process(&mut self) -> Vec<Event> {
        let mut ev = Vec::new();
        while let Ok(msg) = self.rx.try_recv() {
            ev.extend(self.handle(msg));
        }
        ev
    }

    /// For tools and tests without an event loop: handles what background
    /// work sends until no scan or analysis runs (or `timeout` passes), and
    /// returns the events. A startup scan comes 500 ms after `open`: call
    /// `rescan()` first to wait for one now.
    pub fn wait_idle(&mut self, timeout: Duration) -> Vec<Event> {
        let end = Instant::now() + timeout;
        let mut ev = Vec::new();
        while self.scanning || self.analyzing {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            if let Ok(msg) = self.rx.recv_timeout(left.min(Duration::from_millis(50))) {
                ev.extend(self.handle(msg));
                ev.extend(self.process());
            }
        }
        ev
    }

    /// True while a scan or an analysis runs.
    pub fn busy(&self) -> bool {
        self.scanning || self.analyzing
    }

    // --- ReplayGain analysis ------------------------------------------------------------

    pub fn analyzing_loudness(&self) -> bool {
        self.analyzing
    }
    pub fn loudness_done(&self) -> usize {
        self.loudness_done
    }
    pub fn loudness_total(&self) -> usize {
        self.loudness_total
    }
    pub fn tracks_without_loudness(&self) -> usize {
        self.tracks.iter().filter(|t| !t.has_replay_gain() || t.rg_album_gain.is_nan()).count()
    }

    /// While on, new and changed files are measured after every scan.
    pub fn set_auto_analyze_loudness(&mut self, on: bool) -> Vec<Event> {
        self.auto_analyze = on;
        if on { self.analyze_loudness() } else { Vec::new() }
    }

    /// Measures, album by album, every album with a track that lacks track
    /// or album gain. Results go to the library; files are never written.
    pub fn analyze_loudness(&mut self) -> Vec<Event> {
        if self.analyzing {
            return Vec::new();
        }
        if self.scanning {
            self.analyze_pending = true;
            return Vec::new();
        }
        // Whole albums (the album gain needs every track); tracks without
        // an album stand alone.
        let mut albums: HashMap<String, Vec<usize>> = HashMap::new();
        let mut order: Vec<String> = Vec::new();
        for (i, t) in self.tracks.iter().enumerate() {
            let key = if t.album.is_empty() { t.path.clone() } else { t.album_key() };
            albums
                .entry(key.clone())
                .or_insert_with(|| {
                    order.push(key);
                    Vec::new()
                })
                .push(i);
        }
        let mut work: Vec<(Vec<String>, Vec<i64>)> = Vec::new();
        let mut total = 0;
        for key in &order {
            let rows = &albums[key];
            let needed =
                rows.iter().any(|&i| !self.tracks[i].has_replay_gain() || self.tracks[i].rg_album_gain.is_nan());
            if !needed {
                continue;
            }
            total += rows.len();
            work.push((
                rows.iter().map(|&i| self.tracks[i].path.clone()).collect(),
                rows.iter().map(|&i| self.tracks[i].modified).collect(),
            ));
        }
        if work.is_empty() {
            return Vec::new();
        }
        self.analyzing = true;
        self.loudness_done = 0;
        self.loudness_total = total;
        self.unsaved_albums = 0;
        let cancel = self.cancel.clone();
        let tx = self.tx.clone();
        let notifier = self.notifier.clone();
        let handle = thread::Builder::new()
            .name("sonkore-loudness".into())
            .spawn(move || {
                for (paths, modified) in work {
                    let m = loudness::measure_album(&paths, &cancel);
                    if cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    let gains = AlbumGains {
                        track_gain: m.tracks.iter().map(|t| if t.ok { t.gain_db() } else { f64::NAN }).collect(),
                        track_peak: m.tracks.iter().map(|t| t.peak).collect(),
                        album_ok: m.album.ok,
                        album_gain: m.album.gain_db(),
                        album_peak: m.album.peak,
                        paths,
                        modified,
                    };
                    if tx.send(Msg::Loudness(Box::new(gains))).is_ok() {
                        notify(&notifier);
                    }
                }
                if tx.send(Msg::LoudnessDone).is_ok() {
                    notify(&notifier);
                }
            })
            .expect("loudness thread");
        self.workers.retain(|h| !h.is_finished());
        self.workers.push(handle);
        vec![Event::LoudnessProgress]
    }

    /// Values read from tags stay; only the missing ones are filled in.
    fn apply_loudness(&mut self, g: &AlbumGains) -> Vec<Event> {
        let mut updated = Vec::new();
        for (k, path) in g.paths.iter().enumerate() {
            let Some(&i) = self.index.get(path) else {
                continue;
            };
            let t = &mut self.tracks[i];
            if t.modified != g.modified[k] {
                continue; // rescanned meanwhile: the measurement may be stale
            }
            let mut changed = false;
            if !t.has_replay_gain() && !g.track_gain[k].is_nan() {
                t.rg_track_gain = g.track_gain[k];
                t.rg_track_peak = g.track_peak[k];
                changed = true;
            }
            if t.rg_album_gain.is_nan() {
                // Silent or unreadable album: the track gain stands in for it.
                t.rg_album_gain = if g.album_ok { g.album_gain } else { t.rg_track_gain };
                t.rg_album_peak = if g.album_ok { g.album_peak } else { t.rg_track_peak };
                changed = changed || !t.rg_album_gain.is_nan();
            }
            if changed {
                t.rg_analyzed = true;
                updated.push(t.path.clone());
            }
        }
        self.loudness_done += g.paths.len();
        self.unsaved_albums += 1;
        if self.unsaved_albums >= 25 {
            // Keep the work if the app is closed midway.
            self.save_cache();
            self.unsaved_albums = 0;
        }
        let mut ev = vec![Event::LoudnessProgress];
        if !updated.is_empty() {
            ev.push(Event::LoudnessUpdated(updated));
        }
        ev
    }

    // --- Persistence ---------------------------------------------------------------------

    fn load_cache(&mut self) {
        let Ok(bytes) = fs::read(&self.files.library_json) else {
            return;
        };
        let Ok(root) = serde_json::from_slice::<Value>(&bytes) else {
            // Damaged: kept aside (the last one) before the next save
            // replaces it, the library starting empty.
            let mut bad = self.files.library_json.clone().into_os_string();
            bad.push(".bad");
            let _ = fs::rename(&self.files.library_json, bad);
            return;
        };
        drop(bytes);
        let stored_folders = json_out::string_list(root.get("folders"));
        let mut folders: Vec<String> = stored_folders.iter().map(|f| self.map_path(f)).collect();
        let mut moved = folders != stored_folders;
        let mut seen = HashSet::new();
        let listed = folders.len();
        folders.retain(|f| seen.insert(f.clone()));
        let as_stored = folders.len() == listed;
        self.folders = folders;
        self.extra_files = json_out::string_list(root.get("files")).iter().map(|f| self.map_path(f)).collect();
        self.hidden = json_out::string_list(root.get("excluded")).iter().map(|f| self.map_path(f)).collect();
        // Version 1 did not read loudness tags: its entries count as
        // changed, so the next scan reads every file again (dates added kept).
        let version = json_out::as_i64_or(root.get("version"), 0);
        let mut tracks = Vec::new();
        if let Some(Value::Array(list)) = root.get("tracks") {
            tracks.reserve(list.len());
            for v in list {
                let mut t = TrackMeta::from_json(v);
                if t.path.starts_with("/run/") {
                    let real = self.map_path(&t.path);
                    moved = moved || real != t.path;
                    t.path = real;
                }
                // Version 2 read DSD without its length and kept files with
                // no audio: only those are read again.
                if version < 2 || (version < 3 && (t.duration <= 0.0 || t.sample_rate <= 0)) {
                    t.modified = 0;
                }
                // Version 3 measured mono 3 dB low, ignored the sides of 7.1
                // and clipped lossy peaks: those measurements are taken again.
                if version < 4 && t.rg_analyzed && (t.channels == 1 || t.channels == 8 || t.bits == 0) {
                    t.rg_track_gain = f64::NAN;
                    t.rg_album_gain = f64::NAN;
                    t.rg_track_peak = 0.0;
                    t.rg_album_peak = 0.0;
                    t.rg_analyzed = false;
                }
                tracks.push(t);
            }
        }
        drop(root);
        self.set_tracks(tracks);
        if moved {
            self.save_cache(); // kept for when the portal's paths are gone
        } else if version == CACHE_VERSION && as_stored {
            self.saved.set(Some(self.cache_fingerprint()));
        }
    }

    /// What library.json holds, in short. The tracks change through scans
    /// (an entry is read again only when its file's date or size changed),
    /// removals and the loudness measured: their paths, dates, sizes and
    /// gains stand for them.
    fn cache_fingerprint(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        CACHE_VERSION.hash(&mut h);
        self.folders.hash(&mut h);
        self.extra_files.hash(&mut h);
        self.hidden.hash(&mut h);
        for t in &self.tracks {
            t.path.hash(&mut h);
            (t.modified, t.file_size, t.added, t.rg_analyzed).hash(&mut h);
            gain_bits(t).hash(&mut h);
        }
        h.finish()
    }

    /// library.json in the stored format.
    pub fn cache_json(&self) -> String {
        let mut root = Map::new();
        root.insert("version".into(), Value::from(CACHE_VERSION));
        root.insert("folders".into(), json_out::array_of(&self.folders));
        root.insert("files".into(), json_out::array_of(&self.extra_files));
        root.insert("excluded".into(), json_out::array_of(&self.hidden));
        root.insert("tracks".into(), Value::Array(self.tracks.iter().map(TrackMeta::to_json).collect()));
        json_out::to_compact(&Value::Object(root))
    }

    /// Writes library.json, unless it holds this already.
    pub fn save_cache(&self) {
        let now = self.cache_fingerprint();
        if self.saved.get() == Some(now) {
            return;
        }
        let written = write_file(&self.files.library_json, self.cache_json().as_bytes()).is_ok();
        self.saved.set(written.then_some(now));
    }

    fn load_favorites(&mut self) {
        let Ok(bytes) = fs::read(&self.files.favorites_json) else {
            return;
        };
        let Ok(root) = serde_json::from_slice::<Value>(&bytes) else {
            return;
        };
        let stored = json_out::string_list(root.get("tracks"));
        let list: Vec<String> = stored.iter().map(|p| self.map_path(p)).collect();
        self.favorites = list.iter().cloned().collect();
        if list != stored {
            self.save_favorites();
        }
    }

    /// favorites.json in the stored format.
    pub fn favorites_json(&self) -> String {
        let mut list: Vec<String> = self.favorites.iter().cloned().collect();
        list.sort_by(|a, b| utf16_cmp(a, b));
        let mut root = Map::new();
        root.insert("tracks".into(), json_out::array_of(&list));
        json_out::to_indented(&Value::Object(root))
    }

    fn save_favorites(&self) {
        let _ = write_file(&self.files.favorites_json, self.favorites_json().as_bytes());
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        for h in self.workers.drain(..) {
            let _ = h.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::clean_path;

    #[test]
    fn clean_path_like_qdir() {
        assert_eq!(clean_path("/a//b/./c/../d/"), "/a/b/d");
        assert_eq!(clean_path("/../a"), "/a");
        assert_eq!(clean_path("a/../../b"), "../b");
        assert_eq!(clean_path("/"), "/");
        assert_eq!(clean_path("./"), ".");
    }

    #[test]
    #[cfg(windows)]
    fn clean_path_on_windows_keeps_the_root() {
        assert_eq!(clean_path(r"C:\Music\.\Band\..\a.flac"), "C:/Music/a.flac");
        assert_eq!(clean_path(r"C:\..\Music\"), "C:/Music");
        assert_eq!(clean_path("C:/"), "C:/");
        assert_eq!(clean_path(r"\\nas\music\a"), "//nas/music/a");
        assert_eq!(clean_path(r"\\?\D:\x"), "D:/x");
    }
}
