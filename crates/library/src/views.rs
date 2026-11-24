//! The lists the pages show, as plain data over a `Library` (the track
//! filter, the track list, the albums and the folder tree). Each keeps its
//! own settings; `refresh(&library)` recomputes it (after a change of its
//! settings or of the library), and the queries read the result.

use std::collections::{HashMap, HashSet};
use std::fs;

use crate::collate::case_insensitive_cmp;
use crate::library::{Library, UNKNOWN};
use crate::meta::{FieldValue, TrackMeta, lower};
use crate::metadata;
use crate::sort;

/// The words of a search: lower case, split at spaces.
fn words_of(text: &str) -> Vec<String> {
    lower(text).split(' ').filter(|w| !w.is_empty()).map(str::to_string).collect()
}

// --- TrackFilter -------------------------------------------------------------------

/// A searchable, sortable view of the library (TrackFilterModel): rows are
/// rows of `Library::tracks()`.
#[derive(Clone, Debug, Default)]
pub struct TrackFilter {
    pub search: String,
    pub album_key: String,
    /// Tracks inside this folder, recursively.
    pub folder: String,
    /// Browser filters ("" = any; UNKNOWN = the tag is empty). The artist
    /// matches the track artist or the album artist.
    pub genre: String,
    pub artist: String,
    pub album: String,
    pub favorites_only: bool,
    /// Any track field's role name; "" = library order.
    pub sort_field: String,
    pub descending: bool,
    rows: Vec<usize>,
    revision: Option<(u64, u64)>,
}

impl TrackFilter {
    pub fn new() -> TrackFilter {
        TrackFilter::default()
    }

    fn accepts(&self, lib: &Library, row: usize, words: &[String], folder_prefix: &str) -> bool {
        let t = &lib.tracks()[row];
        if self.favorites_only && !lib.is_favorite(&t.path) {
            return false;
        }
        if !self.album_key.is_empty() && t.album_key() != self.album_key {
            return false;
        }
        if !folder_prefix.is_empty() && !t.path.starts_with(folder_prefix) {
            return false;
        }
        let matches = |wanted: &str, value: &str| {
            if wanted == UNKNOWN { value.is_empty() } else { value == wanted }
        };
        if !self.genre.is_empty() && !matches(&self.genre, &t.genre) {
            return false;
        }
        if !self.artist.is_empty() {
            let artist = t.display_artist(); // empty only without both tags
            let ok = if self.artist == UNKNOWN {
                artist.is_empty()
            } else {
                artist == self.artist || *t.album_artist == self.artist
            };
            if !ok {
                return false;
            }
        }
        if !self.album.is_empty() && !matches(&self.album, &t.album) {
            return false;
        }
        if words.is_empty() {
            return true;
        }
        let hay = lib.search_text_at(row);
        words.iter().all(|w| hay.contains(w.as_str()))
    }

    /// Filters and sorts again from the library and the settings.
    pub fn refresh(&mut self, lib: &Library) {
        let words = words_of(&self.search);
        let folder_prefix = if self.folder.is_empty() {
            String::new()
        } else if self.folder.ends_with('/') {
            self.folder.clone()
        } else {
            format!("{}/", self.folder)
        };
        let mut rows: Vec<usize> = (0..lib.count()).filter(|&r| self.accepts(lib, r, &words, &folder_prefix)).collect();
        // A field the tracks have (or "favorite"): it and its tie-breakers.
        let known_field = !self.sort_field.is_empty()
            && (self.sort_field == "favorite"
                || lib.tracks().first().map(|t| t.value(&self.sort_field).is_some()).unwrap_or(true));
        if known_field {
            let tracks = lib.tracks();
            sort::sort_by_field(&mut rows, &self.sort_field, self.descending, |&r, key| {
                let t = &tracks[r];
                sort::track_value(t, key, lib.is_favorite(&t.path))
            });
        } else if self.descending {
            rows.reverse(); // the library order, turned round
        }
        self.rows = rows;
        self.revision = Some((lib.revision(), lib.favorites_revision()));
    }

    /// True when the library changed since the last refresh.
    pub fn stale(&self, lib: &Library) -> bool {
        self.revision != Some((lib.revision(), lib.favorites_revision()))
    }

    /// Library rows, in this view's order.
    pub fn rows(&self) -> &[usize] {
        &self.rows
    }
    pub fn count(&self) -> usize {
        self.rows.len()
    }
    pub fn track<'a>(&self, lib: &'a Library, row: usize) -> Option<&'a TrackMeta> {
        self.rows.get(row).map(|&r| &lib.tracks()[r])
    }
    pub fn path_at(&self, lib: &Library, row: usize) -> Option<String> {
        self.track(lib, row).map(|t| t.path.clone())
    }
    pub fn paths(&self, lib: &Library) -> Vec<String> {
        self.rows.iter().map(|&r| lib.tracks()[r].path.clone()).collect()
    }
    pub fn paths_of(&self, lib: &Library, rows: &[usize]) -> Vec<String> {
        rows.iter().filter_map(|&r| self.path_at(lib, r)).collect()
    }
    pub fn row_of_path(&self, lib: &Library, path: &str) -> Option<usize> {
        let lib_row = lib.row_of(path)?;
        self.rows.iter().position(|&r| r == lib_row)
    }
}

// --- TrackList -----------------------------------------------------------------------

/// A row of a TrackList: the track (the library's, or read from the file).
#[derive(Clone, Debug)]
pub struct ListRow {
    pub meta: TrackMeta,
    /// The file is not there.
    pub missing: bool,
    /// Its place in the whole list.
    pub list_index: usize,
}

/// An ordered list of paths (the queue, a playlist) shown with the
/// library's fields (TrackListModel); with a search, only the matching
/// tracks are rows. Files outside the library are read once and kept.
#[derive(Clone, Debug, Default)]
pub struct TrackList {
    paths: Vec<String>,
    search: String,
    words: Vec<String>,
    rows: Vec<usize>,
    external: HashMap<String, (TrackMeta, bool)>,
}

impl TrackList {
    pub fn new() -> TrackList {
        TrackList::default()
    }

    pub fn paths(&self) -> &[String] {
        &self.paths
    }

    pub fn set_paths(&mut self, lib: &Library, paths: Vec<String>) {
        if paths == self.paths {
            return;
        }
        self.paths = paths;
        // The tags read for files no longer listed go.
        let kept: HashSet<&String> = self.paths.iter().collect();
        self.external.retain(|k, _| kept.contains(k));
        self.refilter(lib);
    }

    pub fn search(&self) -> &str {
        &self.search
    }

    pub fn set_search(&mut self, lib: &Library, text: &str) {
        if text == self.search {
            return;
        }
        self.search = text.to_string();
        self.words = words_of(text);
        self.refilter(lib);
    }

    /// The library changed: files read again, the search redone.
    pub fn library_changed(&mut self, lib: &Library) {
        self.external.clear();
        self.refilter(lib);
    }

    fn filtered(&self) -> bool {
        !self.words.is_empty()
    }

    fn external(&mut self, path: &str) -> &(TrackMeta, bool) {
        self.external.entry(path.to_string()).or_insert_with(|| {
            // A CD image track (no file of its own) is missing.
            let missing = fs::metadata(path).is_err();
            let mut meta = TrackMeta::default();
            if missing || !metadata::read_into(path, &mut meta) {
                meta = TrackMeta {
                    path: path.to_string(),
                    title: metadata::complete_base_name(path),
                    ..Default::default()
                };
            }
            (meta, missing)
        })
    }

    fn refilter(&mut self, lib: &Library) {
        self.rows.clear();
        if !self.filtered() {
            return;
        }
        let words = self.words.clone();
        for i in 0..self.paths.len() {
            let path = self.paths[i].clone();
            let hay = match lib.search_text(&path) {
                Some(h) if !h.is_empty() => h.to_string(),
                _ => self.external(&path).0.search_text(),
            };
            if words.iter().all(|w| hay.contains(w.as_str())) {
                self.rows.push(i);
            }
        }
    }

    pub fn count(&self) -> usize {
        if self.filtered() { self.rows.len() } else { self.paths.len() }
    }

    /// A row's place in the whole list (None: past the end).
    pub fn list_index_at(&self, row: usize) -> Option<usize> {
        if row >= self.count() {
            return None;
        }
        Some(if self.filtered() { self.rows[row] } else { row })
    }

    /// The row showing a place of the list (None: filtered out).
    pub fn row_of_list_index(&self, index: usize) -> Option<usize> {
        if index >= self.paths.len() {
            return None;
        }
        if !self.filtered() {
            return Some(index);
        }
        self.rows.binary_search(&index).ok()
    }

    pub fn row_of_path(&self, path: &str) -> Option<usize> {
        self.row_of_list_index(self.paths.iter().position(|p| p == path)?)
    }

    pub fn path_at(&self, row: usize) -> Option<&str> {
        self.list_index_at(row).map(|i| self.paths[i].as_str())
    }

    pub fn paths_of(&self, rows: &[usize]) -> Vec<String> {
        rows.iter().filter_map(|&r| self.path_at(r).map(str::to_string)).collect()
    }

    /// A row's track.
    pub fn row(&mut self, lib: &Library, row: usize) -> Option<ListRow> {
        let index = self.list_index_at(row)?;
        let path = self.paths[index].clone();
        if let Some(t) = lib.find(&path) {
            return Some(ListRow { meta: t.clone(), missing: false, list_index: index });
        }
        let (meta, missing) = self.external(&path).clone();
        Some(ListRow { meta, missing, list_index: index })
    }
}

// --- Albums ---------------------------------------------------------------------------

/// An album (AlbumModel): grouped by album artist (else artist) and album.
#[derive(Clone, Debug, PartialEq)]
pub struct Album {
    pub key: String,
    pub album: String,
    pub artist: String,
    pub year: i32,
    /// The track to take the cover from (one with embedded art if any).
    pub cover_path: String,
    pub tracks: usize,
    pub duration: f64,
    /// The first track's format label.
    pub format: String,
    has_embedded_cover: bool,
    search_text: String,
}

#[derive(Clone, Debug, Default)]
pub struct AlbumList {
    all: Vec<Album>,
    visible: Vec<usize>,
    search: String,
    revision: Option<u64>,
}

impl AlbumList {
    pub fn new() -> AlbumList {
        AlbumList::default()
    }

    /// Groups the library's tracks again (in library order).
    pub fn rebuild(&mut self, lib: &Library) {
        let mut index: HashMap<String, usize> = HashMap::new();
        let mut albums: Vec<Album> = Vec::new();
        for t in lib.tracks() {
            let key = t.album_key();
            let i = *index.entry(key.clone()).or_insert_with(|| {
                let artist = t.album_or_track_artist().to_string();
                albums.push(Album {
                    search_text: lower(&format!("{} {}", t.album, artist)),
                    key,
                    album: t.album.to_string(),
                    artist,
                    year: t.year,
                    cover_path: t.path.clone(),
                    tracks: 0,
                    duration: 0.0,
                    format: t.format_label(),
                    has_embedded_cover: false,
                });
                albums.len() - 1
            });
            let a = &mut albums[i];
            a.tracks += 1;
            a.duration += t.duration;
            if t.has_embedded_cover && !a.has_embedded_cover {
                // Prefer a track that carries art.
                a.cover_path = t.path.clone();
                a.has_embedded_cover = true;
            }
        }
        self.all = albums;
        self.revision = Some(lib.revision());
        self.refilter();
    }

    pub fn stale(&self, lib: &Library) -> bool {
        self.revision != Some(lib.revision())
    }

    pub fn search(&self) -> &str {
        &self.search
    }

    pub fn set_search(&mut self, text: &str) {
        if text == self.search {
            return;
        }
        self.search = text.to_string();
        self.refilter();
    }

    fn refilter(&mut self) {
        let words = words_of(&self.search);
        self.visible = (0..self.all.len())
            .filter(|&i| words.iter().all(|w| self.all[i].search_text.contains(w.as_str())))
            .collect();
    }

    pub fn count(&self) -> usize {
        self.visible.len()
    }
    pub fn get(&self, row: usize) -> Option<&Album> {
        self.visible.get(row).map(|&i| &self.all[i])
    }
    pub fn by_key(&self, key: &str) -> Option<&Album> {
        self.all.iter().find(|a| a.key == key)
    }
    /// The visible albums, in order.
    pub fn albums(&self) -> impl Iterator<Item = &Album> {
        self.visible.iter().map(|&i| &self.all[i])
    }
}

// --- FolderTree -------------------------------------------------------------------------

/// A folder of the tree: its name, path, how many tracks it holds (all the
/// way down) and its sub-folders that hold music.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct FolderNode {
    pub name: String,
    pub path: String,
    pub count: usize,
    pub children: Vec<FolderNode>,
}

/// The library's folder tree (FolderTreeModel): one root per watched
/// folder; files added one by one outside them are not in it.
#[derive(Clone, Debug, Default)]
pub struct FolderTree {
    root: FolderNode,
    revision: Option<(u64, Vec<String>)>,
}

impl FolderTree {
    pub fn new() -> FolderTree {
        FolderTree::default()
    }

    pub fn stale(&self, lib: &Library) -> bool {
        self.revision.as_ref() != Some(&(lib.revision(), lib.folders().to_vec()))
    }

    fn signature(n: &FolderNode, out: &mut String) {
        out.push_str(&n.path);
        out.push('\n');
        for c in &n.children {
            FolderTree::signature(c, out);
        }
    }

    /// Builds the tree again. Returns false when only counts changed (the
    /// same folders as before: a view keeps its expanded / selected state).
    pub fn rebuild(&mut self, lib: &Library) -> bool {
        let mut root = FolderNode::default();
        for folder in lib.folders() {
            let name = folder.rsplit('/').find(|s| !s.is_empty()).map(str::to_string).unwrap_or_else(|| folder.clone());
            root.children.push(FolderNode { name, path: folder.clone(), count: 0, children: Vec::new() });
        }
        // Deepest watched folder first.
        let mut roots: Vec<&String> = lib.folders().iter().collect();
        roots.sort_by_key(|r| std::cmp::Reverse(r.encode_utf16().count()));
        for t in lib.tracks() {
            let Some(r) = roots.iter().find(|r| t.path.starts_with(&format!("{r}/"))) else {
                continue;
            };
            let relative = &t.path[r.len() + 1..];
            let Some(mut node) = root.children.iter_mut().find(|c| &c.path == *r) else {
                continue;
            };
            node.count += 1;
            let parts: Vec<&str> = relative.split('/').filter(|s| !s.is_empty()).collect();
            for part in parts.iter().take(parts.len().saturating_sub(1)) {
                let i = match node.children.iter().position(|c| c.name == *part) {
                    Some(i) => i,
                    None => {
                        let path = format!("{}/{}", node.path, part);
                        node.children.push(FolderNode { name: part.to_string(), path, count: 0, children: Vec::new() });
                        node.children.len() - 1
                    }
                };
                node = &mut node.children[i];
                node.count += 1;
            }
        }
        fn sort_tree(n: &mut FolderNode) {
            n.children.sort_by(|a, b| case_insensitive_cmp(&a.name, &b.name));
            for c in &mut n.children {
                sort_tree(c);
            }
        }
        sort_tree(&mut root);
        let (mut old, mut new) = (String::new(), String::new());
        FolderTree::signature(&self.root, &mut old);
        FolderTree::signature(&root, &mut new);
        self.root = root;
        self.revision = Some((lib.revision(), lib.folders().to_vec()));
        old != new
    }

    /// The roots (one per watched folder).
    pub fn roots(&self) -> &[FolderNode] {
        &self.root.children
    }

    /// The folder at `path`.
    pub fn node(&self, path: &str) -> Option<&FolderNode> {
        let mut node = &self.root;
        loop {
            let mut next = None;
            for c in &node.children {
                if c.path == path {
                    return Some(c);
                }
                if path.starts_with(&format!("{}/", c.path)) {
                    next = Some(c);
                    break;
                }
            }
            node = next?;
        }
    }

    /// The row numbers from the roots down to `path` (indexOfPath).
    pub fn index_of_path(&self, path: &str) -> Option<Vec<usize>> {
        let mut node = &self.root;
        let mut rows = Vec::new();
        loop {
            let mut next = None;
            for (i, c) in node.children.iter().enumerate() {
                if c.path == path {
                    rows.push(i);
                    return Some(rows);
                }
                if path.starts_with(&format!("{}/", c.path)) {
                    rows.push(i);
                    next = Some(c);
                    break;
                }
            }
            node = next?;
        }
    }

    /// The sub-folders of `path` ("" = the roots), as childrenOf().
    pub fn children_of(&self, path: &str) -> Vec<&FolderNode> {
        fn find<'a>(n: &'a FolderNode, path: &str) -> Option<&'a FolderNode> {
            if n.path == path {
                return Some(n);
            }
            for c in &n.children {
                if path.starts_with(&c.path)
                    && let Some(hit) = find(c, path)
                {
                    return Some(hit);
                }
            }
            None
        }
        let node = if path.is_empty() { Some(&self.root) } else { find(&self.root, path) };
        node.map(|n| n.children.iter().collect()).unwrap_or_default()
    }
}

/// Sorts a list of paths (the play queue) by a field as the track lists do:
/// `info(path)` gives a path's track (the library's or read from the file),
/// `favorite(path)` whether it is one.
pub fn sort_paths(
    paths: &mut Vec<String>,
    field: &str,
    descending: bool,
    info: impl Fn(&str) -> TrackMeta,
    favorite: impl Fn(&str) -> bool,
) {
    let metas: HashMap<String, TrackMeta> = paths.iter().map(|p| (p.clone(), info(p))).collect();
    sort::sort_by_field(paths, field, descending, |p, key| match key {
        "path" => Some(FieldValue::Text(p.clone())),
        "fileName" => Some(FieldValue::Text(p.rsplit('/').next().unwrap_or("").to_string())),
        "favorite" => Some(FieldValue::Int(i64::from(favorite(p)))),
        _ => metas.get(p).and_then(|t| t.value(key)),
    });
}
