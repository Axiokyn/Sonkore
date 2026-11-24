//! User playlists: ordered lists of paths in playlists.json, with the
//! schema and layout of the stored format, and M3U / M3U8
//! import and export.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use sonkore_core::paths;

use crate::json_out;
use crate::library::{Library, PathMapper, clean_path, local_path, write_file};
use crate::meta::TrackMeta;
use crate::metadata;

/// What changed.
#[derive(Clone, Debug, PartialEq)]
pub enum PlaylistEvent {
    NamesChanged,
    PlaylistChanged(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Playlist {
    pub name: String,
    pub paths: Vec<String>,
}

pub struct Playlists {
    file: PathBuf,
    lists: Vec<Playlist>,
    new_name: String,
}

impl Playlists {
    /// $XDG_DATA_HOME/sonkore/playlists.json.
    pub fn open_standard() -> Playlists {
        Playlists::open(paths::data_file("playlists.json"), None)
    }

    /// Loads a playlists file; document-portal paths become the host's
    /// through `mapper` (and the file is saved again when any did).
    pub fn open(file: impl Into<PathBuf>, mapper: Option<PathMapper>) -> Playlists {
        let mut p = Playlists { file: file.into(), lists: Vec::new(), new_name: "New playlist".into() };
        let mut moved = false;
        if let Ok(bytes) = fs::read(&p.file)
            && let Ok(Value::Array(items)) = serde_json::from_slice::<Value>(&bytes)
        {
            for item in &items {
                let name = match item.get("name") {
                    Some(Value::String(s)) => s.clone(),
                    Some(Value::Number(n)) => n.to_string(),
                    Some(Value::Bool(b)) => b.to_string(),
                    _ => String::new(),
                };
                let stored = json_out::string_list(item.get("tracks"));
                let real: Vec<String> = match &mapper {
                    Some(m) => stored.iter().map(|s| m(s)).collect(),
                    None => stored.clone(),
                };
                moved = moved || real != stored;
                p.lists.push(Playlist { name, paths: real });
            }
        }
        if moved {
            p.save();
        }
        p
    }

    /// The name a playlist without one gets (earlier versions translated "New
    /// playlist"; the interface sets its translation).
    pub fn set_new_playlist_label(&mut self, label: &str) {
        self.new_name = label.to_string();
    }

    pub fn names(&self) -> Vec<String> {
        self.lists.iter().map(|p| p.name.clone()).collect()
    }

    pub fn lists(&self) -> &[Playlist] {
        &self.lists
    }

    fn find(&self, name: &str) -> Option<&Playlist> {
        self.lists.iter().find(|p| p.name == name)
    }

    fn find_mut(&mut self, name: &str) -> Option<&mut Playlist> {
        self.lists.iter_mut().find(|p| p.name == name)
    }

    fn unique_name(&self, wanted: &str) -> String {
        let base = if wanted.trim().is_empty() { self.new_name.clone() } else { wanted.trim().to_string() };
        let mut name = base.clone();
        let mut n = 2;
        while self.find(&name).is_some() {
            name = format!("{base} {n}");
            n += 1;
        }
        name
    }

    /// Creates a playlist; returns its name (made unique).
    pub fn create(&mut self, name: &str) -> (String, Vec<PlaylistEvent>) {
        let unique = self.unique_name(name);
        self.lists.push(Playlist { name: unique.clone(), paths: Vec::new() });
        self.save();
        (unique, vec![PlaylistEvent::NamesChanged])
    }

    pub fn remove(&mut self, name: &str) -> Vec<PlaylistEvent> {
        let Some(i) = self.lists.iter().position(|p| p.name == name) else {
            return Vec::new();
        };
        self.lists.remove(i);
        self.save();
        vec![PlaylistEvent::NamesChanged]
    }

    /// Renames; returns the name it now has (unchanged when `to` is blank).
    pub fn rename(&mut self, from: &str, to: &str) -> (String, Vec<PlaylistEvent>) {
        if self.find(from).is_none() || to.trim().is_empty() || to.trim() == from {
            return (from.to_string(), Vec::new());
        }
        let unique = self.unique_name(to);
        self.find_mut(from).unwrap().name = unique.clone();
        self.save();
        (unique, vec![PlaylistEvent::NamesChanged])
    }

    fn changed(&mut self, name: &str) -> Vec<PlaylistEvent> {
        self.save();
        vec![PlaylistEvent::PlaylistChanged(name.to_string())]
    }

    pub fn add_tracks(&mut self, name: &str, paths: &[String]) -> Vec<PlaylistEvent> {
        let Some(p) = self.find_mut(name) else {
            return Vec::new();
        };
        if paths.is_empty() {
            return Vec::new();
        }
        p.paths.extend_from_slice(paths);
        self.changed(name)
    }

    pub fn remove_at(&mut self, name: &str, index: usize) -> Vec<PlaylistEvent> {
        let Some(p) = self.find_mut(name) else {
            return Vec::new();
        };
        if index >= p.paths.len() {
            return Vec::new();
        }
        p.paths.remove(index);
        self.changed(name)
    }

    pub fn remove_rows(&mut self, name: &str, rows: &[usize]) -> Vec<PlaylistEvent> {
        let Some(p) = self.find_mut(name) else {
            return Vec::new();
        };
        if rows.is_empty() {
            return Vec::new();
        }
        let mut sorted = rows.to_vec();
        sorted.sort_unstable_by(|a, b| b.cmp(a));
        sorted.dedup();
        for row in sorted {
            if row < p.paths.len() {
                p.paths.remove(row);
            }
        }
        self.changed(name)
    }

    pub fn insert_tracks(&mut self, name: &str, index: usize, paths: &[String]) -> Vec<PlaylistEvent> {
        let Some(p) = self.find_mut(name) else {
            return Vec::new();
        };
        if paths.is_empty() {
            return Vec::new();
        }
        let at = index.min(p.paths.len());
        p.paths.splice(at..at, paths.iter().cloned());
        self.changed(name)
    }

    /// Moves the track at `from` to `to`.
    pub fn move_track(&mut self, name: &str, from: usize, to: usize) -> Vec<PlaylistEvent> {
        let Some(p) = self.find_mut(name) else {
            return Vec::new();
        };
        if from >= p.paths.len() || to >= p.paths.len() || from == to {
            return Vec::new();
        }
        let item = p.paths.remove(from);
        p.paths.insert(to, item);
        self.changed(name)
    }

    pub fn paths(&self, name: &str) -> Vec<String> {
        self.find(name).map(|p| p.paths.clone()).unwrap_or_default()
    }

    /// The tracks, with whether each file is missing.
    pub fn tracks(&self, name: &str, lib: &Library) -> Vec<(TrackMeta, bool)> {
        let Some(p) = self.find(name) else {
            return Vec::new();
        };
        p.paths.iter().map(|path| (lib.track(path), fs::metadata(path).is_err())).collect()
    }

    /// The length of the tracks the library knows.
    pub fn duration(&self, name: &str, lib: &Library) -> f64 {
        let Some(p) = self.find(name) else { return 0.0 };
        p.paths.iter().filter_map(|path| lib.find(path)).map(|t| t.duration).sum()
    }

    /// Imports an M3U / M3U8 (a path or file:// URL) as a new playlist
    /// named after the file; entries are relative to its folder. None when
    /// it cannot be read.
    pub fn import_m3u(&mut self, file: &str) -> Option<(String, Vec<PlaylistEvent>)> {
        let path = local_path(file);
        let bytes = fs::read(&path).ok()?;
        let text = String::from_utf8_lossy(&bytes);
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
        let base = Path::new(&path).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| ".".into());
        let mut entries = Vec::new();
        for line in text.split('\n') {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut entry = if line.starts_with("file:") { local_path(line) } else { line.to_string() };
            if !entry.starts_with('/') {
                entry = format!("{base}/{entry}");
            }
            entries.push(clean_path(&entry));
        }
        let (name, mut ev) = self.create(&metadata::complete_base_name(&path));
        ev.extend(self.add_tracks(&name, &entries));
        Some((name, ev))
    }

    /// Exports a playlist as extended M3U (UTF-8).
    pub fn export_m3u(&self, name: &str, file: &str, lib: &Library) -> bool {
        let Some(p) = self.find(name) else {
            return false;
        };
        let mut out = String::from("#EXTM3U\n");
        for path in &p.paths {
            if let Some(t) = lib.find(path) {
                out.push_str(&format!("#EXTINF:{},{} - {}\n", t.duration as i64, t.display_artist(), t.title));
            }
            out.push_str(path);
            out.push('\n');
        }
        fs::write(local_path(file), out).is_ok()
    }

    /// playlists.json in the stored format.
    pub fn to_json(&self) -> String {
        let list: Vec<Value> = self
            .lists
            .iter()
            .map(|p| {
                let mut m = Map::new();
                m.insert("name".into(), Value::String(p.name.clone()));
                m.insert("tracks".into(), json_out::array_of(&p.paths));
                Value::Object(m)
            })
            .collect();
        json_out::to_indented(&Value::Array(list))
    }

    fn save(&self) {
        let _ = write_file(&self.file, self.to_json().as_bytes());
    }
}
