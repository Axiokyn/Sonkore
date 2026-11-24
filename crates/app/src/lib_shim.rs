//! The library as the interface reads it: the sonkore-library crate (scan,
//! tags, favourites, watching, loudness) mirrored into plain rows the pages
//! and lists share. `process()` brings in what the background work changed.

use std::collections::{HashMap, HashSet};

use serde::Deserialize;
use sonkore_library::Event;
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Track {
    #[serde(rename = "p", default)]
    pub path: String,
    #[serde(rename = "t", default)]
    pub title: String,
    #[serde(rename = "a", default)]
    pub artist: String,
    #[serde(rename = "al", default)]
    pub album: String,
    #[serde(rename = "aa", default)]
    pub album_artist: String,
    #[serde(rename = "g", default)]
    pub genre: String,
    #[serde(rename = "c", default)]
    pub codec: String,
    #[serde(rename = "tn", default)]
    pub track_no: i64,
    #[serde(rename = "dn", default)]
    pub disc_no: i64,
    #[serde(rename = "y", default)]
    pub year: i64,
    #[serde(rename = "d", default)]
    pub duration: f64,
    #[serde(rename = "sr", default)]
    pub sample_rate: i64,
    #[serde(rename = "b", default)]
    pub bits: i64,
    #[serde(rename = "ch", default)]
    pub channels: i64,
    #[serde(rename = "br", default)]
    pub bitrate: i64,
    #[serde(rename = "sz", default)]
    pub file_size: i64,
    #[serde(rename = "m", default)]
    pub modified: i64,
    #[serde(rename = "ad", default)]
    pub added: i64,
    #[serde(rename = "rtg", default)]
    pub rg_track_gain: Option<f64>,
    #[serde(rename = "rtp", default)]
    pub rg_track_peak: Option<f64>,
    #[serde(rename = "rag", default)]
    pub rg_album_gain: Option<f64>,
    #[serde(rename = "rap", default)]
    pub rg_album_peak: Option<f64>,
}

impl Track {
    /// The format label: "FLAC 24/96", "MP3 320k".
    pub fn format_label(&self) -> String {
        if self.bits > 0 && self.sample_rate > 0 {
            return format!("{} {}/{}", self.codec, self.bits, g4(self.sample_rate as f64 / 1000.0));
        }
        if self.bitrate > 0 {
            return format!("{} {}k", self.codec, self.bitrate);
        }
        self.codec.clone()
    }
    pub fn file_name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }
    pub fn folder(&self) -> &str {
        self.path.rsplit_once('/').map_or("", |(d, _)| d)
    }
}

/// The 'g' format at precision 4: 4 significant digits, no trailing zeros.
pub fn g4(v: f64) -> String {
    let s = format!("{:.*}", (3 - v.abs().log10().floor() as i32).max(0) as usize, v);
    if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_owned() } else { s }
}

impl Track {
    /// A file's own tags (one outside the library).
    /// What the search looks in, lower case ('\n' between the fields: a
    /// search never matches across two).
    pub fn search_key(&self) -> String {
        [&self.title, &self.artist, &self.album, &self.path].map(|f| f.to_lowercase()).join("\n")
    }

    pub fn read(path: &str) -> Option<Track> {
        sonkore_library::metadata::read(path).map(|m| Track::from_meta(&m))
    }

    fn from_meta(m: &sonkore_library::TrackMeta) -> Track {
        let known = |v: f64| (!v.is_nan()).then_some(v);
        Track {
            path: m.path.clone(),
            title: m.title.clone(),
            artist: m.artist.to_string(),
            album: m.album.to_string(),
            album_artist: m.album_artist.to_string(),
            genre: m.genre.to_string(),
            codec: m.codec.to_string(),
            track_no: m.track_no as i64,
            disc_no: m.disc_no as i64,
            year: m.year as i64,
            duration: m.duration,
            sample_rate: m.sample_rate as i64,
            bits: m.bits as i64,
            channels: m.channels as i64,
            bitrate: m.bitrate_kbps as i64,
            file_size: m.file_size,
            modified: m.modified,
            added: m.added,
            rg_track_gain: known(m.rg_track_gain),
            rg_track_peak: known(m.rg_track_peak),
            rg_album_gain: known(m.rg_album_gain),
            rg_album_peak: known(m.rg_album_peak),
        }
    }
}

pub struct Library {
    pub folders: Vec<String>,
    pub tracks: Vec<Track>,
    pub favorites: HashSet<String>,
    inner: sonkore_library::Library,
    revision: u64,
    /// Each track's row by path (gains measured patch only their rows).
    index: HashMap<String, usize>,
    /// Per track: title, artist, album and path in lower case, for the
    /// search (lower-casing 4 fields of every track on each key was 200,000
    /// allocations for 50,000 tracks).
    pub search_keys: Vec<String>,
    /// Bumped when the tracks or the folders change (what is derived from
    /// them, like the folder tree's counts, is computed again).
    pub generation: u64,
}

/// What a change to the library means for the interface.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Changed {
    /// The rows (tracks, folders, favourites) are new: lists, sidebar, albums.
    pub rows: bool,
    /// Gains were measured for these tracks (their rows are patched).
    pub loudness: Vec<String>,
    /// Favourites only: hearts and counts, the rows stay.
    pub favorites: bool,
    pub scanning: bool,
}

impl Library {
    pub fn load() -> Library {
        let inner = sonkore_library::Library::open_standard();
        let mut lib = Library {
            folders: Vec::new(),
            tracks: Vec::new(),
            favorites: HashSet::new(),
            inner,
            revision: u64::MAX,
            index: HashMap::new(),
            search_keys: Vec::new(),
            generation: 0,
        };
        lib.sync();
        lib
    }

    /// Rows without a library behind them (tests): nothing is read or written.
    #[cfg(test)]
    pub fn with_tracks(tracks: Vec<Track>) -> Library {
        let dir = std::env::temp_dir().join(format!("sonkore-test-{}", std::process::id()));
        let files = sonkore_library::LibraryFiles {
            library_json: dir.join("library.json"),
            favorites_json: dir.join("favorites.json"),
            settings: dir.join("sonkore.conf"),
        };
        let inner = sonkore_library::Library::open(files);
        let search_keys = tracks.iter().map(Track::search_key).collect();
        Library {
            folders: Vec::new(),
            tracks,
            favorites: HashSet::new(),
            inner,
            revision: 0,
            index: HashMap::new(),
            search_keys,
            generation: 0,
        }
    }

    /// A track's row by its path.
    pub fn index_of(&self, path: &str) -> Option<usize> {
        self.index.get(path).copied()
    }

    /// The library itself (browse, playlists, covers…).
    pub fn inner(&self) -> &sonkore_library::Library {
        &self.inner
    }

    fn sync(&mut self) {
        if self.folders != self.inner.folders() {
            self.generation += 1;
        }
        if self.revision != self.inner.revision() {
            self.generation += 1;
            self.revision = self.inner.revision();
            self.tracks = self.inner.tracks().iter().map(Track::from_meta).collect();
            self.index = self.tracks.iter().enumerate().map(|(i, t)| (t.path.clone(), i)).collect();
            self.search_keys = self.tracks.iter().map(Track::search_key).collect();
        }
        self.folders = self.inner.folders().to_vec();
        self.favorites = self.inner.favorites().clone();
    }

    fn apply(&mut self, events: Vec<Event>) -> Changed {
        let mut changed = Changed::default();
        for e in events {
            match e {
                Event::LoudnessUpdated(paths) => changed.loudness.extend(paths),
                Event::FavoritesChanged | Event::TrackChanged(_) => changed.favorites = true,
                Event::ScanningChanged | Event::ScanProgress | Event::LoudnessProgress => changed.scanning = true,
                Event::PreferencesChanged => {}
                _ => changed.rows = true,
            }
        }
        // Rebuilding every row (and the lists, the selection with them) for a
        // heart clicked or an album measured froze big libraries and lost the
        // selection every few seconds during the analysis.
        if changed.rows || self.revision != self.inner.revision() {
            changed.rows = true;
            self.revision = u64::MAX;
            self.sync();
        } else {
            if changed.favorites {
                self.favorites = self.inner.favorites().clone();
            }
            // Gains change rows without a new revision: those rows only.
            for path in &changed.loudness {
                if let Some(&i) = self.index.get(path)
                    && let Some(m) = self.inner.tracks().get(i)
                {
                    self.tracks[i] = Track::from_meta(m);
                }
            }
        }
        changed
    }

    /// What the scan, the watcher and the analysis did since the last call.
    pub fn process(&mut self) -> Changed {
        let events = self.inner.process();
        self.apply(events)
    }

    pub fn toggle_favorite(&mut self, path: &str) -> Changed {
        let events = self.inner.toggle_favorite(path);
        self.apply(events)
    }

    /// Several favourites off (or on) with one write.
    pub fn set_favorites(&mut self, paths: &[String], on: bool) -> Changed {
        let events = self.inner.set_favorites(paths, on);
        self.apply(events)
    }

    pub fn remove_tracks(&mut self, paths: &[String]) -> Changed {
        let events = self.inner.remove_tracks(paths);
        self.apply(events)
    }

    pub fn add_folder(&mut self, folder: &str) -> Changed {
        let events = self.inner.add_folder(folder);
        self.apply(events)
    }

    pub fn rescan(&mut self) -> Changed {
        let events = self.inner.rescan();
        self.apply(events)
    }

    /// Anything else the library does (settings, loudness…), its events applied.
    pub fn with(&mut self, f: impl FnOnce(&mut sonkore_library::Library) -> Vec<Event>) -> Changed {
        let events = f(&mut self.inner);
        self.apply(events)
    }
}
