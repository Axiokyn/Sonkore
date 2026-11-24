//! The playlists: playlists.json in the data folder, the format earlier
//! Sonkore versions wrote ([{name, tracks}]), saved after every change.

use sonkore_core::paths;

use crate::i18n::tr;

#[derive(Clone, Debug, Default)]
pub struct Playlist {
    pub name: String,
    pub paths: Vec<String>,
}

#[derive(Default)]
pub struct Playlists {
    pub list: Vec<Playlist>,
}

impl Playlists {
    pub fn load() -> Playlists {
        let Ok(bytes) = std::fs::read(paths::data_file("playlists.json")) else { return Playlists::default() };
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) else { return Playlists::default() };
        let items = v
            .get("playlists")
            .and_then(|p| p.as_array())
            .cloned()
            .or_else(|| v.as_array().cloned())
            .unwrap_or_default();
        let list = items
            .iter()
            .filter_map(|p| {
                Some(Playlist {
                    name: p.get("name")?.as_str()?.to_owned(),
                    paths: p
                        .get("tracks")
                        .and_then(|t| t.as_array())
                        .map(|t| t.iter().filter_map(|s| s.as_str().map(str::to_owned)).collect())
                        .unwrap_or_default(),
                })
            })
            .collect();
        Playlists { list }
    }

    fn save(&self) {
        let list: Vec<serde_json::Value> =
            self.list.iter().map(|p| serde_json::json!({ "name": p.name, "tracks": p.paths })).collect();
        let text = serde_json::to_string_pretty(&serde_json::Value::Array(list)).unwrap_or_default();
        let path = paths::data_file("playlists.json");
        let _ = std::fs::create_dir_all(paths::data_dir());
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(tmp, path);
        }
    }

    pub fn names(&self) -> Vec<String> {
        self.list.iter().map(|p| p.name.clone()).collect()
    }

    pub fn find(&self, name: &str) -> Option<&Playlist> {
        self.list.iter().find(|p| p.name == name)
    }

    pub fn paths(&self, name: &str) -> Vec<String> {
        self.find(name).map(|p| p.paths.clone()).unwrap_or_default()
    }

    /// A name no playlist has: "Name", "Name 2", "Name 3"…
    pub fn unique_name(&self, wanted: &str) -> String {
        let base =
            if wanted.trim().is_empty() { tr("PlaylistManager", "New playlist") } else { wanted.trim().to_owned() };
        let mut name = base.clone();
        let mut n = 2;
        while self.find(&name).is_some() {
            name = format!("{base} {n}");
            n += 1;
        }
        name
    }

    pub fn create(&mut self, name: &str) -> String {
        let unique = self.unique_name(name);
        self.list.push(Playlist { name: unique.clone(), paths: Vec::new() });
        self.save();
        unique
    }

    pub fn remove(&mut self, name: &str) {
        self.list.retain(|p| p.name != name);
        self.save();
    }

    pub fn rename(&mut self, from: &str, to: &str) -> String {
        if self.find(from).is_none() || to.trim().is_empty() || to.trim() == from {
            return from.to_owned();
        }
        let unique = self.unique_name(to);
        if let Some(p) = self.list.iter_mut().find(|p| p.name == from) {
            p.name = unique.clone();
        }
        self.save();
        unique
    }

    pub fn add_tracks(&mut self, name: &str, paths: &[String]) {
        if let Some(p) = self.list.iter_mut().find(|p| p.name == name) {
            if paths.is_empty() {
                return;
            }
            p.paths.extend(paths.iter().cloned());
            self.save();
        }
    }

    /// The entries at `rows` moved to before entry `target` (taken out, put
    /// back where `target` ends up).
    pub fn move_rows(&mut self, name: &str, rows: &[usize], target: usize) {
        let Some(p) = self.list.iter_mut().find(|p| p.name == name) else { return };
        let mut rows: Vec<usize> = rows.iter().copied().filter(|&r| r < p.paths.len()).collect();
        rows.sort_unstable();
        rows.dedup();
        if rows.is_empty() {
            return;
        }
        let moved: Vec<String> = rows.iter().map(|&r| p.paths[r].clone()).collect();
        let at = target.min(p.paths.len()) - rows.iter().filter(|&&r| r < target).count();
        for &r in rows.iter().rev() {
            p.paths.remove(r);
        }
        let at = at.min(p.paths.len());
        for (i, path) in moved.into_iter().enumerate() {
            p.paths.insert(at + i, path);
        }
        self.save();
    }

    /// Removes the entries at `rows` (indices into the playlist).
    pub fn remove_rows(&mut self, name: &str, rows: &[usize]) {
        if let Some(p) = self.list.iter_mut().find(|p| p.name == name) {
            let mut i = 0;
            p.paths.retain(|_| {
                let keep = !rows.contains(&i);
                i += 1;
                keep
            });
            self.save();
        }
    }

    /// Exports a playlist: an extended M3U, UTF-8; `info` gives a
    /// library track's (seconds, artist, title) for its #EXTINF line.
    pub fn export_m3u(
        &self,
        name: &str,
        file: &std::path::Path,
        info: impl Fn(&str) -> Option<(f64, String, String)>,
    ) -> bool {
        let Some(p) = self.find(name) else { return false };
        let mut text = String::from("#EXTM3U\n");
        for path in &p.paths {
            if let Some((seconds, artist, title)) = info(path) {
                text.push_str(&format!("#EXTINF:{},{} - {}\n", seconds as i64, artist, title));
            }
            text.push_str(path);
            text.push('\n');
        }
        std::fs::write(file, text).is_ok()
    }

    /// Imports an M3U: a playlist named after the file, its
    /// entries (relative ones from the file's folder, file:// URLs too).
    pub fn import_m3u(&mut self, file: &std::path::Path) -> Option<String> {
        let bytes = std::fs::read(file).ok()?;
        let text = String::from_utf8_lossy(&bytes);
        let base = file.parent().unwrap_or(std::path::Path::new("/"));
        let entries: Vec<String> = text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(|l| {
                let local = sonkore_platform::url::to_local_file(l).unwrap_or_else(|| l.into());
                let full = if local.is_relative() { base.join(local) } else { local };
                clean(&full)
            })
            .collect();
        let stem = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let name = self.create(&stem);
        self.add_tracks(&name, &entries);
        Some(name)
    }
}

/// A clean path: no "." or "x/.." parts, with '/' on Windows too (as
/// the library stores paths, so the entries match its tracks).
fn clean(path: &std::path::Path) -> String {
    let mut out: Vec<std::path::Component> = Vec::new();
    for c in path.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir if matches!(out.last(), Some(std::path::Component::Normal(_))) => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    paths::stored_path(&out.iter().collect::<std::path::PathBuf>().to_string_lossy())
}
