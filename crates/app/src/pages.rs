//! The pages beyond the plain track lists: Albums
//! (the grid and an album), Artists and Genres, Now playing, the queue, a
//! playlist; their headings, their buttons, the menus they open. A child
//! module of the controller: it reads and changes the controller's state.

use std::collections::HashMap;
use std::rc::Rc;

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use super::Controller;
use super::menus::{Entry, Menu, Pending, Place};
use crate::i18n::{tr, trf};
use crate::lib_shim::Track;
use crate::playlists::Playlists;
use crate::views::{self, UNKNOWN, tracks_text};
use crate::{AlbumTile, App, BrowseEntry, Menus, Pages, PathStage};

/// What the pages keep between refreshes.
#[derive(Default)]
pub struct PageState {
    /// The album shown on the Albums page (its key), else the grid.
    pub album: Option<String>,
    /// The album's own order (not remembered: the default order).
    pub album_sort: (String, bool),
    pub albums: Vec<views::Album>,
    pub albums_dirty: bool,
    /// The grid's model (its covers are asked for row by row).
    pub album_tiles: Option<Rc<AlbumTiles>>,
    /// The artist and the genre picked ("" = all), by field.
    pub browse: HashMap<String, String>,
    pub browse_entries: usize,
    pub playlists: Playlists,
    /// The queue's or the playlist's index of each row shown, and its paths
    /// (rows map to library tracks; the lists may hold others).
    pub list_rows: Vec<usize>,
    pub list_paths: Vec<String>,
    pub pending: Pending,
}

impl PageState {
    pub fn new() -> PageState {
        PageState { playlists: Playlists::load(), albums_dirty: true, ..Default::default() }
    }
}

/// How a page's rows are ordered.
pub enum Order {
    /// As they are (the queue, a playlist).
    Keep,
    /// By the page's sort (key, descending); "" is the library's order.
    Sort(String, bool),
}

fn field_of(page: &str) -> &'static str {
    if page == "genres" { "genre" } else { "artist" }
}

/// A random start for shuffle (no generator needed for one number).
fn random_below(n: usize) -> usize {
    let nanos =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    (nanos as usize).wrapping_mul(2654435761) % n.max(1)
}

impl Controller {
    // --- The rows of a page ------------------------------------------------------------

    /// The rows of the pages this module draws (None: a library list the
    /// controller fills itself), and how to order them.
    pub(super) fn page_rows(&self, page: &str, arg: &str) -> Option<(Vec<usize>, Order)> {
        let lib = self.library.borrow();
        let by_path =
            || -> HashMap<&str, usize> { lib.tracks.iter().enumerate().map(|(i, t)| (t.path.as_str(), i)).collect() };
        let mut ui = self.ui.borrow_mut();
        match page {
            "queue" | "playing" => {
                let map = by_path();
                let paths: Vec<String> = self.player.borrow().queue().paths().to_vec();
                let (mut rows, mut list_rows) = (Vec::new(), Vec::new());
                for (qi, p) in paths.iter().enumerate() {
                    if let Some(&i) = map.get(p.as_str()) {
                        rows.push(i);
                        list_rows.push(qi);
                    }
                }
                ui.list_rows = list_rows;
                ui.list_paths = paths;
                Some((rows, Order::Keep))
            }
            "playlist" => {
                let map = by_path();
                let paths = ui.playlists.paths(arg);
                let (mut rows, mut list_rows) = (Vec::new(), Vec::new());
                for (pi, p) in paths.iter().enumerate() {
                    if let Some(&i) = map.get(p.as_str()) {
                        rows.push(i);
                        list_rows.push(pi);
                    }
                }
                ui.list_rows = list_rows;
                ui.list_paths = paths;
                Some((rows, Order::Keep))
            }
            "albums" => {
                let rows = match &ui.album {
                    Some(key) => (0..lib.tracks.len()).filter(|&i| views::album_key(&lib.tracks[i]) == *key).collect(),
                    None => Vec::new(),
                };
                let (key, desc) = ui.album_sort.clone();
                Some((rows, Order::Sort(key, desc)))
            }
            "artists" | "genres" => {
                let field = field_of(page);
                let key = ui.browse.get(field).cloned().unwrap_or_default();
                let rows =
                    (0..lib.tracks.len()).filter(|&i| views::matches_browse(&lib.tracks[i], field, &key)).collect();
                drop(ui);
                let (k, d) = self.sort_order();
                Some((rows, Order::Sort(k, d)))
            }
            _ => None,
        }
    }

    /// The page's order: the album's own, else the remembered one.
    pub(super) fn page_sort(&self) -> (String, bool) {
        if self.page.borrow().0 == "albums" {
            return self.ui.borrow().album_sort.clone();
        }
        let s = self.settings.borrow();
        (s.text("ui/trackSort", "path"), s.flag("ui/trackSortDescending", false))
    }

    /// Sorts as asked (header, sort menu): the album's order stays its own.
    pub(super) fn request_sort(&self, key: &str, descending: bool) {
        if self.page.borrow().0 == "albums" {
            self.ui.borrow_mut().album_sort = (key.to_owned(), descending);
        } else {
            let mut s = self.settings.borrow_mut();
            s.set_text("ui/trackSort", key);
            s.set_flag("ui/trackSortDescending", descending);
        }
        self.fill_list();
    }

    /// Sorted by the column, then the other way, then back to
    /// the library's order; favourites start on top.
    pub(super) fn sort_by_column(&self, key: &str) {
        let (current, desc) = self.page_sort();
        let first = key == "favorite";
        if current != key {
            self.request_sort(key, first);
        } else if desc == first {
            self.request_sort(key, !first);
        } else {
            self.request_sort("", false);
        }
    }

    /// The heading of the pages drawn here (title, subtitle in the top bar).
    pub(super) fn page_heading(&self, page: &str, count: usize) -> Option<(String, String)> {
        let shuffle = self.player.borrow().queue().shuffle();
        let queued = self.player.borrow().queue().len();
        match page {
            "albums" => {
                self.refresh_albums();
                let ui = self.ui.borrow();
                let n = views::filter_albums(&ui.albums, &self.search.borrow()).len();
                let sub = if n == 0 && self.search.borrow().is_empty() {
                    tr("AlbumsPage", "Add a music folder to see your albums")
                } else if n == 0 {
                    tr("AlbumsPage", "No albums match")
                } else {
                    trf("AlbumsPage", "{0} albums", &[&n.to_string()])
                };
                Some((tr("AlbumsPage", "Albums"), sub))
            }
            "artists" | "genres" => {
                self.refresh_browse(page, count);
                let n = self.ui.borrow().browse_entries.to_string();
                let mut sub = if page == "genres" {
                    trf("BrowsePage", "{0} genres", &[&n])
                } else {
                    trf("BrowsePage", "{0} artists", &[&n])
                };
                let selected = self.list.state.borrow().selected.len();
                if selected > 1 {
                    sub += &trf("BrowsePage", "  ·  {0} selected", &[&selected.to_string()]);
                }
                Some((if page == "genres" { tr("BrowsePage", "Genres") } else { tr("BrowsePage", "Artists") }, sub))
            }
            "queue" => {
                let mut sub = tracks_text(queued);
                if shuffle {
                    sub += &tr("QueuePage", " · shuffle on");
                }
                self.window().global::<Pages>().set_queue_has_tracks(queued > 0);
                Some((tr("QueuePage", "Play queue"), sub))
            }
            "playing" => {
                let mut sub = tracks_text(queued) + &tr("NowPlayingPage", " in the queue");
                if shuffle {
                    sub += &tr("NowPlayingPage", " · shuffle on");
                }
                self.window().global::<Pages>().set_queue_has_tracks(queued > 0);
                self.refresh_now();
                Some((tr("NowPlayingPage", "Now playing"), sub))
            }
            // PlaylistPage sets no heading: the top bar shows the search alone.
            "playlist" => {
                self.refresh_playlist_page();
                Some((String::new(), String::new()))
            }
            "folders" => {
                let arg = self.page.borrow().1.clone();
                let title = if arg.is_empty() {
                    tr("FoldersPage", "All folders")
                } else {
                    arg.rsplit('/').next().unwrap_or(&arg).to_owned()
                };
                Some((title, String::new()))
            }
            _ => None,
        }
    }

    /// Pages whose search filters their tracks (the others filter their own
    /// list, or hand the search over to Tracks).
    pub(super) fn searches_tracks(page: &str) -> bool {
        matches!(page, "tracks" | "favorites" | "folders")
    }

    /// The top bar's search typed into: pages without one hand over to
    /// Tracks.
    pub(super) fn search_edited(&self, text: &str) {
        *self.search.borrow_mut() = text.to_owned();
        let page = self.page.borrow().0.clone();
        if matches!(page.as_str(), "queue" | "playing" | "playlist") && !text.is_empty() {
            self.open("tracks", "");
            return;
        }
        self.fill_list();
    }

    // --- Albums ------------------------------------------------------------------------

    /// Covers decoded meanwhile: the tiles on screen and the open album's.
    pub(super) fn album_covers_arrived(&self) {
        let (tiles, cover) = {
            let ui = self.ui.borrow();
            let open = ui.album.as_ref().and_then(|k| ui.albums.iter().find(|a| &a.key == k));
            (ui.album_tiles.clone(), open.map(|a| a.cover_path.clone()))
        };
        if let Some(tiles) = tiles {
            tiles.covers_arrived();
        }
        if let Some(path) = cover {
            let w = self.window();
            let side = if w.global::<crate::Theme>().get_compact() { 140.0 } else { 210.0 };
            let cover = crate::covers::get(&path, side, 0.0);
            let p = w.global::<Pages>();
            p.set_album_has_cover(cover.is_some());
            p.set_album_cover(cover.unwrap_or_default());
        }
    }

    pub(super) fn refresh_albums(&self) {
        let dirty = self.ui.borrow().albums_dirty;
        if dirty {
            let albums = views::albums(&self.library.borrow());
            let mut ui = self.ui.borrow_mut();
            ui.albums = albums;
            ui.albums_dirty = false;
        }
        let unknown = tr("AlbumsPage", "Unknown artist");
        let model = {
            let ui = self.ui.borrow();
            let shown = views::filter_albums(&ui.albums, &self.search.borrow());
            let mut covers = Vec::with_capacity(shown.len());
            let tiles: Vec<AlbumTile> = shown
                .into_iter()
                .map(|i| {
                    let a = &ui.albums[i];
                    let mut sub = if a.artist.is_empty() { unknown.clone() } else { a.artist.clone() };
                    if a.year > 0 {
                        sub += &format!(" · {}", a.year);
                    }
                    covers.push(a.cover_path.clone());
                    AlbumTile {
                        key: a.key.as_str().into(),
                        title: a.album.as_str().into(),
                        subtitle: sub.into(),
                        ..Default::default()
                    }
                })
                .collect();
            Rc::new(AlbumTiles { tiles, covers, notify: Default::default() })
        };
        self.ui.borrow_mut().album_tiles = Some(model.clone());
        let ui = self.ui.borrow();
        let w = self.window();
        let p = w.global::<Pages>();
        p.set_albums(ModelRc::from(model));
        // The album open: its name, artist and details.
        let album = ui.album.as_ref().and_then(|k| ui.albums.iter().find(|a| &a.key == k));
        p.set_album_open(album.is_some());
        if let Some(a) = album {
            p.set_album_title(a.album.as_str().into());
            p.set_album_artist(if a.artist.is_empty() { unknown.clone() } else { a.artist.clone() }.into());
            let mut bits = Vec::new();
            if a.year > 0 {
                bits.push(a.year.to_string());
            }
            bits.push(trf("AlbumsPage", "{0} tracks", &[&a.tracks.to_string()]));
            bits.push(views::duration_text(a.duration));
            if !a.format.is_empty() {
                bits.push(a.format.clone());
            }
            p.set_album_info(bits.join("  ·  ").into());
            let side = if w.global::<crate::Theme>().get_compact() { 140.0 } else { 210.0 };
            let cover = crate::covers::get(&a.cover_path, side, 0.0);
            p.set_album_has_cover(cover.is_some());
            p.set_album_cover(cover.unwrap_or_default());
        }
    }

    fn open_album(&self, key: &str) {
        let known = self.ui.borrow().albums.iter().any(|a| a.key == key);
        if !known {
            return;
        }
        self.ui.borrow_mut().album = Some(key.to_owned());
        self.fill_list();
    }

    fn album_paths(&self, key: &str) -> Vec<String> {
        let lib = self.library.borrow();
        views::library_order(&lib)
            .into_iter()
            .map(|i| &lib.tracks[i])
            .filter(|t| views::album_key(t) == key)
            .map(|t| t.path.clone())
            .collect()
    }

    // --- Artists and genres ------------------------------------------------------------

    /// The Artists / Genres page: the names (the search filters
    /// them), "All" first; the picked one's name and count.
    fn refresh_browse(&self, page: &str, count: usize) {
        let field = field_of(page);
        let genres = field == "genre";
        let all = views::browse(&self.library.borrow(), field);
        let words: Vec<String> = self.search.borrow().to_lowercase().split_whitespace().map(str::to_owned).collect();
        let entries: Vec<&views::BrowseItem> =
            all.iter().filter(|e| words.iter().all(|w| e.name.to_lowercase().contains(w.as_str()))).collect();
        let all_name = if genres { tr("ColumnBrowser", "All genres") } else { tr("ColumnBrowser", "All artists") };
        let mut rows = vec![BrowseEntry {
            key: SharedString::default(),
            name: all_name.as_str().into(),
            detail: trf("ColumnBrowser", "{0} items", &[&entries.len().to_string()]).into(),
            unknown: false,
        }];
        let mut selected = self.ui.borrow().browse.get(field).cloned().unwrap_or_default();
        let mut name = all_name.clone();
        let mut kept = selected.is_empty();
        for e in &entries {
            let shown = if e.key == UNKNOWN {
                if genres { tr("ColumnBrowser", "No genre") } else { tr("ColumnBrowser", "Unknown artist") }
            } else {
                e.name.clone()
            };
            let albums = if e.albums == 1 {
                tr("ColumnBrowser", "1 album")
            } else {
                trf("ColumnBrowser", "{0} albums", &[&e.albums.to_string()])
            };
            if e.key == selected {
                kept = true;
                name = shown.clone();
            }
            rows.push(BrowseEntry {
                key: e.key.as_str().into(),
                name: shown.into(),
                detail: format!("{albums} · {}", tracks_text(e.count)).into(),
                unknown: e.key == UNKNOWN,
            });
        }
        if !kept {
            // A search that no longer shows the picked one falls back to All.
            selected.clear();
            self.ui.borrow_mut().browse.insert(field.to_owned(), String::new());
        }
        self.ui.borrow_mut().browse_entries = entries.len();
        let w = self.window();
        let p = w.global::<Pages>();
        // The same names as shown: kept (made again, the rows would lose a
        // double click's first press).
        let current = p.get_entries();
        let same = current.row_count() == rows.len()
            && rows.iter().enumerate().all(|(i, r)| current.row_data(i).as_ref() == Some(r));
        if !same {
            p.set_entries(ModelRc::new(VecModel::from(rows)));
        }
        p.set_selected(selected.into());
        p.set_browse_name(name.into());
        p.set_browse_count(tracks_text(count).into());
        p.set_browse_playable(count > 0);
    }

    fn browse_select(&self, key: &str) {
        let page = self.page.borrow().0.clone();
        if page != "artists" && page != "genres" {
            return;
        }
        self.ui.borrow_mut().browse.insert(field_of(&page).to_owned(), key.to_owned());
        self.fill_list();
    }

    // --- Playing --------------------------------------------------------------------------

    /// Plays `paths` from `start`, shuffled (from a random one) or in order.
    pub(super) fn play_list(&self, paths: Vec<String>, start: Option<usize>, shuffle: bool) {
        if paths.is_empty() {
            return;
        }
        self.player.borrow_mut().set_shuffle(shuffle);
        self.settings.borrow_mut().set_flag("playback/shuffle", shuffle);
        let start = start.unwrap_or_else(|| if shuffle { random_below(paths.len()) } else { 0 });
        let changes = self.player.borrow_mut().play_paths(paths, start);
        self.after(changes);
    }

    /// A row double-clicked: the queue plays that entry, a playlist plays
    /// from it, the other lists play what they show from it.
    /// A key the window's fields left (the window's shortcuts and the track
    /// list's keys); false: not one of ours.
    pub(super) fn key(&self, name: &str, shift: bool) -> bool {
        match name {
            "play-pause" => {
                let ch = self.player.borrow_mut().toggle();
                self.after(ch);
                return true;
            }
            "next" | "previous" => {
                let ch =
                    if name == "next" { self.player.borrow_mut().next() } else { self.player.borrow_mut().previous() };
                self.after(ch);
                return true;
            }
            "volume-up" | "volume-down" => {
                self.step_volume(if name == "volume-up" { 2.0 } else { -2.0 });
                return true;
            }
            _ => {}
        }
        // The rest is the list's, in the full view only.
        if self.window().global::<crate::Mini>().get_on() {
            return false;
        }
        let count = self.list.row_count();
        let selected = || {
            let mut v: Vec<usize> = self.list.state.borrow().selected.iter().copied().filter(|&r| r < count).collect();
            v.sort_unstable();
            v
        };
        match name {
            "select-all" => {
                self.set_selection((0..count).collect());
                true
            }
            "escape" => {
                if self.list.state.borrow().selected.is_empty() {
                    return false;
                }
                self.set_selection(Default::default());
                true
            }
            "enter" => {
                let row = self.cursor.get().filter(|&r| r < count).or_else(|| selected().first().copied());
                match row {
                    Some(r) => {
                        self.activate_row(r);
                        true
                    }
                    None => false,
                }
            }
            "delete" => {
                let rows = selected();
                if rows.is_empty() {
                    return false;
                }
                self.delete_rows(&rows);
                true
            }
            "up" | "down" | "home" | "end" | "page-up" | "page-down" => {
                if count == 0 {
                    return false;
                }
                // About a screenful: the rows the list shows.
                let page = 10;
                let at = self.cursor.get().filter(|&r| r < count);
                let to = match (name, at) {
                    ("home", _) => 0,
                    ("end", _) => count - 1,
                    (_, None) => 0,
                    ("up", Some(r)) => r.saturating_sub(1),
                    ("down", Some(r)) => (r + 1).min(count - 1),
                    ("page-up", Some(r)) => r.saturating_sub(page),
                    (_, Some(r)) => (r + page).min(count - 1),
                };
                if shift {
                    let anchor = self.anchor.borrow().unwrap_or(to);
                    self.set_selection((anchor.min(to)..=anchor.max(to)).collect());
                } else {
                    *self.anchor.borrow_mut() = Some(to);
                    self.set_selection([to].into_iter().collect());
                }
                self.cursor.set(Some(to));
                self.reveal_row(to);
                true
            }
            _ => false,
        }
    }

    /// The selected tracks let go (the list's drop and the sidebar's
    /// targets): onto Now playing (the queue), a playlist, under the
    /// playlists (a new one), or elsewhere in a reorderable list (moved).
    pub(super) fn drop_tracks(&self) {
        let w = self.window();
        let app = w.global::<App>();
        let (item, area, row) =
            (app.get_drop_item().to_string(), app.get_drop_area().to_string(), app.get_drop_row().max(0) as usize);
        let count = self.list.row_count();
        let mut rows: Vec<usize> = self.list.state.borrow().selected.iter().copied().filter(|&r| r < count).collect();
        rows.sort_unstable();
        if rows.is_empty() {
            return;
        }
        let paths: Vec<String> = rows.iter().filter_map(|&r| self.list.path_at(r)).collect();
        if item == "playing" {
            let changes = self.player.borrow_mut().enqueue(paths);
            self.after(changes);
        } else if let Some(name) = item.strip_prefix("playlist:") {
            self.ui.borrow_mut().playlists.add_tracks(name, &paths);
            self.show_message(&trf("Sidebar", "Added {0} to “{1}”", &[&tracks_text(paths.len()), name]));
            self.refresh_sidebar();
            if self.page.borrow().0 == "playlist" && self.page.borrow().1 == name {
                self.fill_list();
            }
        } else if area == "new-playlist" {
            self.new_playlist(paths);
        } else if area == "list" {
            let page = self.page.borrow().clone();
            let list_rows = self.ui.borrow().list_rows.clone();
            let indices: Vec<usize> = rows.iter().filter_map(|&r| list_rows.get(r).copied()).collect();
            let total = match page.0.as_str() {
                "playlist" => self.ui.borrow().playlists.paths(&page.1).len(),
                _ => self.player.borrow().queue().len(),
            };
            // The drop row as an index into the queue / playlist.
            let target = if row >= list_rows.len() { total } else { list_rows[row] };
            match page.0.as_str() {
                "queue" | "playing" => {
                    let changes = self.player.borrow_mut().move_entries(&indices, target);
                    self.after(changes);
                }
                "playlist" => {
                    self.ui.borrow_mut().playlists.move_rows(&page.1, &indices, target);
                    self.fill_list();
                }
                _ => return,
            }
            self.set_selection(Default::default());
        }
    }

    /// The list scrolled so that `row` shows.
    pub(super) fn reveal_row(&self, row: usize) {
        self.reveal_row_at(row, false);
    }

    /// `row` scrolled into view; `centred`: in the middle of the list
    /// (a track revealed), else just inside its edge (the keyboard).
    pub(super) fn reveal_row_at(&self, row: usize, centred: bool) {
        let w = self.window();
        let app = w.global::<App>();
        app.set_reveal_row(row as i32);
        app.set_reveal_centred(centred);
        app.set_reveal_serial(app.get_reveal_serial().wrapping_add(1));
    }

    /// The playing track, when the list shows it: selected and in the
    /// middle of the list (a click on the folder it plays from).
    pub(super) fn reveal_playing(&self) {
        let path = self.player.borrow().current_path().unwrap_or("").to_owned();
        if path.is_empty() {
            return;
        }
        if let Some(r) = self.list.paths().iter().position(|p| *p == path) {
            self.select(r, false, false);
            self.reveal_row_at(r, true);
        }
    }

    /// Delete on the list: out of the queue, out
    /// of the playlist, out of the favourites; the library keeps its tracks
    /// (its menu asks first).
    fn delete_rows(&self, rows: &[usize]) {
        let page = self.page.borrow().clone();
        let indices =
            || -> Vec<usize> { rows.iter().filter_map(|&r| self.ui.borrow().list_rows.get(r).copied()).collect() };
        match page.0.as_str() {
            "queue" => {
                let i = indices();
                let changes = self.player.borrow_mut().remove(&i);
                self.after(changes);
            }
            "playlist" => {
                let i = indices();
                self.ui.borrow_mut().playlists.remove_rows(&page.1, &i);
                self.fill_list();
                self.refresh_sidebar();
            }
            "favorites" => {
                let paths: Vec<String> = rows.iter().filter_map(|&r| self.list.path_at(r)).collect();
                self.library.borrow_mut().set_favorites(&paths, false);
                self.fill_list();
                self.refresh_sidebar();
            }
            _ => return,
        }
        self.set_selection(Default::default());
    }

    pub(super) fn activate_row(&self, row: usize) {
        let page = self.page.borrow().0.clone();
        match page.as_str() {
            "queue" | "playing" => {
                let Some(&index) = self.ui.borrow().list_rows.get(row) else { return };
                let changes = self.player.borrow_mut().play_index(index);
                self.after(changes);
            }
            "playlist" => {
                let (paths, index) = {
                    let ui = self.ui.borrow();
                    (ui.list_paths.clone(), ui.list_rows.get(row).copied())
                };
                if let Some(i) = index {
                    let changes = self.player.borrow_mut().play_paths(paths, i);
                    self.after(changes);
                }
            }
            _ => {
                let paths = self.list.paths();
                let changes = self.player.borrow_mut().play_paths(paths, row);
                self.after(changes);
            }
        }
    }

    // --- Now playing ------------------------------------------------------------------------

    /// Now playing: the track, its format and output, the
    /// signal path.
    pub(super) fn refresh_now(&self) {
        let path = self.player.borrow().current_path().unwrap_or("").to_owned();
        let has = !path.is_empty();
        let lib = self.library.borrow();
        let track = lib.tracks.iter().find(|t| t.path == path);
        let w = self.window();
        let p = w.global::<Pages>();
        p.set_now_has(has);
        let cover = if has { crate::covers::get(&path, 320.0, 0.0) } else { None };
        p.set_now_has_cover(cover.is_some());
        p.set_now_cover(cover.unwrap_or_default());
        if has {
            let title = track
                .map(|t| crate::tracklist::cell_text("title", t))
                .unwrap_or_else(|| path.rsplit('/').next().unwrap_or("").to_owned());
            p.set_now_title(title.into());
            let artist = track
                .map(|t| t.artist.clone())
                .filter(|a| !a.is_empty())
                .unwrap_or_else(|| tr("NowPlayingPage", "Unknown artist"));
            let album = track.map(|t| t.album.clone()).unwrap_or_default();
            p.set_now_artist(if album.is_empty() { artist } else { format!("{artist}  ·  {album}") }.into());
        }
        p.set_now_format(self.track_description(track).into());
        let (summary, good) = self.output_summary();
        p.set_now_output(summary.into());
        p.set_now_output_good(good);
        drop(lib);
        let stages = self.signal_path();
        let worst = ["lossless", "high", "enhanced", "warning", "low"]
            .iter()
            .rposition(|q| stages.iter().any(|s| s.quality == *q))
            .map_or("low", |i| ["lossless", "high", "enhanced", "warning", "low"][i]);
        let bit_perfect = self.player.borrow().engine().bit_perfect();
        p.set_verdict(
            if bit_perfect { tr("NowPlayingPage", "BIT-PERFECT") } else { quality_label(worst).to_uppercase() }.into(),
        );
        p.set_verdict_quality(worst.into());
        p.set_stages(ModelRc::new(VecModel::from(stages)));
    }

    /// The track's description: "FLAC 24-bit · 96 kHz", or the
    /// library's format while nothing is open.
    fn track_description(&self, track: Option<&Track>) -> String {
        let p = self.player.borrow();
        match p.engine().info() {
            Some(i) if i.rate > 0 && p.loaded() => {
                let mut s = i.codec.clone();
                if i.bits > 0 {
                    s += &format!(" {}-bit", i.bits);
                }
                s += &format!(" · {} kHz", g6(i.rate as f64 / 1000.0));
                if i.channels != 2 {
                    s += &format!(" · {} ch", i.channels);
                }
                s
            }
            _ => track.map(|t| t.format_label()).unwrap_or_default(),
        }
    }

    /// The output's summary: what the output does with the music.
    fn output_summary(&self) -> (String, bool) {
        let p = self.player.borrow();
        if !p.loaded() {
            return (String::new(), false);
        }
        (crate::signal::output_summary(p.engine()), p.engine().output_status().bit_perfect_now)
    }

    /// The signal path's stages.
    fn signal_path(&self) -> Vec<PathStage> {
        let p = self.player.borrow();
        if !p.loaded() {
            return Vec::new();
        }
        let gain_db = {
            let lib = self.library.borrow();
            let current = p.current_path().map(str::to_owned);
            let track = lib.tracks.iter().find(|t| Some(t.path.as_str()) == current.as_deref());
            let g = crate::audio::track_gain(&self.settings.borrow(), track);
            (g != 1.0).then(|| 20.0 * g.log10())
        };
        let state = crate::signal::PathState {
            gain_db,
            clipped: self.clipped.get(),
            volume_fixed: crate::volume::fixed(),
            hardware_volume: crate::volume::hardware_control(),
            system_volume: crate::volume::system_volume(),
            ..Default::default()
        };
        crate::signal::build(&self.settings.borrow(), p.engine(), &state)
            .into_iter()
            .map(|st| PathStage {
                stage: st.stage.into(),
                detail: st.detail.into(),
                quality: st.quality.into(),
                label: quality_label(st.quality).into(),
                action: st.action.into(),
            })
            .collect()
    }

    // --- Playlists ----------------------------------------------------------------------------

    fn refresh_playlist_page(&self) {
        let name = self.page.borrow().1.clone();
        let (n, duration) = {
            let ui = self.ui.borrow();
            let paths = ui.playlists.paths(&name);
            let lib = self.library.borrow();
            let by_path: HashMap<&str, f64> = lib.tracks.iter().map(|t| (t.path.as_str(), t.duration)).collect();
            (paths.len(), paths.iter().filter_map(|p| by_path.get(p.as_str())).sum::<f64>())
        };
        let w = self.window();
        let p = w.global::<Pages>();
        p.set_playlist_name(name.into());
        p.set_playlist_subtitle(format!("{} · {}", tracks_text(n), crate::tracklist::format_time(duration)).into());
        p.set_playlist_empty(n == 0);
    }

    /// The sidebar's playlists: names and counts.
    pub(super) fn playlist_rows(&self) -> Vec<(String, usize)> {
        self.ui.borrow().playlists.list.iter().map(|p| (p.name.clone(), p.paths.len())).collect()
    }

    fn playlist_action(&self, action: &str) {
        let name = self.page.borrow().1.clone();
        match action {
            "rename" => {
                let from = name.clone();
                self.ask_name(
                    &tr("PlaylistPage", "Rename playlist"),
                    "",
                    &name,
                    &tr("PlaylistPage", "Rename"),
                    "",
                    move |c, to| {
                        if to != from {
                            let made = c.ui.borrow_mut().playlists.rename(&from, &to);
                            c.open("playlist", &made);
                        }
                    },
                );
            }
            "delete" => {
                self.ui.borrow_mut().playlists.remove(&name);
                self.open("tracks", "");
            }
            "add-folder" => {
                let request = crate::file_dialog::Request {
                    title: tr("PlaylistPage", "Add a folder…"),
                    directory: true,
                    ..Default::default()
                };
                let me = self.later();
                crate::file_dialog::ask(request, move |dirs| {
                    let Some(c) = me.upgrade() else { return };
                    let paths = c.library.borrow().inner().expand_audio_paths(&dirs);
                    c.add_to_playlist(&name, paths);
                });
            }
            "add-files" => {
                let patterns: Vec<String> =
                    sonkore_library::metadata::audio_extensions().iter().map(|e| format!("*.{e}")).collect();
                let request = crate::file_dialog::Request {
                    title: tr("PlaylistPage", "Add files…"),
                    multiple: true,
                    // (The audio files' filter: the patterns are its name.)
                    filters: vec![(patterns.join(" "), patterns)],
                    ..Default::default()
                };
                let me = self.later();
                crate::file_dialog::ask(request, move |files| {
                    let Some(c) = me.upgrade() else { return };
                    let paths = c.library.borrow().inner().audio_paths(&files);
                    c.add_to_playlist(&name, paths);
                });
            }
            "export" => {
                let request = crate::file_dialog::Request {
                    title: tr("PlaylistPage", "Export as M3U"),
                    save_name: Some(format!("{name}.m3u8")),
                    filters: vec![("M3U".into(), vec!["*.m3u8".into(), "*.m3u".into()])],
                    ..Default::default()
                };
                let me = self.later();
                crate::file_dialog::ask(request, move |files| {
                    let (Some(c), Some(file)) = (me.upgrade(), files.first()) else { return };
                    let lib = c.library.borrow();
                    let ok = c.ui.borrow().playlists.export_m3u(&name, std::path::Path::new(file), |p| {
                        lib.tracks.iter().find(|t| t.path == p).map(|t| (t.duration, t.artist.clone(), t.title.clone()))
                    });
                    drop(lib);
                    c.show_message(&if ok {
                        tr("PlaylistPage", "Playlist exported")
                    } else {
                        tr("PlaylistPage", "Export failed")
                    });
                });
            }
            _ => {}
        }
    }

    /// The tracks added at the end of the playlist.
    pub(super) fn add_to_playlist(&self, name: &str, paths: Vec<String>) {
        if paths.is_empty() {
            self.show_message(&tr("PlaylistPage", "No audio files in that folder"));
            return;
        }
        self.ui.borrow_mut().playlists.add_tracks(name, &paths);
        self.show_message(&trf("Main", "Added {0} to “{1}”", &[&tracks_text(paths.len()), name]));
        self.fill_list();
        self.refresh_sidebar();
    }

    // --- Menus ------------------------------------------------------------------------------

    /// The track actions: play, add to the queue, add to a playlist (or a new
    /// one), open the folder.
    fn add_track_actions(&self, menu: &mut Menu, paths: &[String]) {
        if paths.is_empty() {
            return;
        }
        let p = paths.to_vec();
        menu.push(Entry::item(tr("TrackMenu", "Play"), move |c| {
            let changes = c.player.borrow_mut().play_paths(p.clone(), 0);
            c.after(changes);
        }));
        let p = paths.to_vec();
        menu.push(Entry::item(tr("TrackMenu", "Add to queue"), move |c| {
            let changes = c.player.borrow_mut().enqueue(p.clone());
            c.after(changes);
            c.show_message(&tr("TrackMenu", "Added to queue"));
        }));
        menu.push(Entry::submenu(tr("TrackMenu", "Add to playlist")));
        let p = paths.to_vec();
        menu.sub.push(Entry::item(tr("TrackMenu", "New playlist…"), move |c| c.new_playlist(p.clone())));
        let names = self.ui.borrow().playlists.names();
        if !names.is_empty() {
            menu.sub.push(Entry::separator());
        }
        for name in names {
            let p = paths.to_vec();
            menu.sub.push(Entry::item(name.clone(), move |c| {
                c.ui.borrow_mut().playlists.add_tracks(&name, &p);
                c.refresh_sidebar();
                c.show_message(&trf("TrackMenu", "Added to “{0}”", &[&name]));
            }));
        }
        let first = paths[0].clone();
        menu.push(Entry::item(tr("TrackMenu", "Open containing folder"), move |_| open_containing_folder(&first)));
    }

    /// The selected rows' paths (the row right-clicked selected alone when
    /// it was not among them).
    fn menu_rows(&self, row: usize) -> Vec<usize> {
        if !self.list.state.borrow().selected.contains(&row) {
            self.select(row, false, false);
        }
        let mut rows: Vec<usize> = self.list.state.borrow().selected.iter().copied().collect();
        rows.sort_unstable();
        rows
    }

    /// The context menu of a list's rows, as each page builds it.
    pub(super) fn row_menu(&self, row: usize, x: f32, y: f32) {
        let rows = self.menu_rows(row);
        let paths: Vec<String> = rows.iter().filter_map(|&r| self.list.path_at(r)).collect();
        if paths.is_empty() {
            return;
        }
        let page = self.page.borrow().clone();
        let mut menu = Menu::default();
        match page.0.as_str() {
            "queue" => {
                let indices: Vec<usize> =
                    rows.iter().filter_map(|&r| self.ui.borrow().list_rows.get(r).copied()).collect();
                let first = indices.first().copied();
                menu.push(Entry::item(tr("TrackMenu", "Play"), move |c| {
                    if let Some(i) = first {
                        let changes = c.player.borrow_mut().play_index(i);
                        c.after(changes);
                    }
                }));
                self.add_track_actions(&mut menu, &paths);
                // The library's "Play" (it replaces the queue) goes.
                if menu.entries.len() > 1 {
                    menu.entries.remove(1);
                }
                menu.separator();
                menu.push(Entry::item(tr("TrackMenu", "Remove from queue"), move |c| {
                    let changes = c.player.borrow_mut().remove(&indices);
                    c.after(changes);
                }));
            }
            "playing" => self.add_track_actions(&mut menu, &paths),
            "playlist" => {
                self.add_track_actions(&mut menu, &paths);
                let indices: Vec<usize> =
                    rows.iter().filter_map(|&r| self.ui.borrow().list_rows.get(r).copied()).collect();
                let name = page.1.clone();
                menu.separator();
                menu.push(Entry::item(tr("TrackMenu", "Remove from playlist"), move |c| {
                    c.ui.borrow_mut().playlists.remove_rows(&name, &indices);
                    c.fill_list();
                    c.refresh_sidebar();
                }));
            }
            _ => {
                self.add_track_actions(&mut menu, &paths);
                menu.separator();
                let text = if paths.len() > 1 {
                    trf("TrackMenu", "Remove {0} tracks from library", &[&paths.len().to_string()])
                } else {
                    tr("TrackMenu", "Remove from library")
                };
                let p = paths.clone();
                menu.push(Entry::item(text, move |c| c.remove_from_library(&p)));
            }
        }
        self.show_menu(menu, Place::Pointer(x, y));
    }

    /// The tracks taken out of the library.
    fn remove_from_library(&self, paths: &[String]) {
        self.library.borrow_mut().remove_tracks(paths);
        self.ui.borrow_mut().albums_dirty = true;
        self.fill_list();
        self.refresh_sidebar();
    }

    /// The sort menu: the orders, the one in use checked.
    pub(super) fn sort_menu(&self) {
        const CHOICES: [(&str, bool, &str); 16] = [
            ("", false, "Library order"),
            ("path", false, "As in the folder (A → Z)"),
            ("path", true, "As in the folder (Z → A)"),
            ("title", false, "Title"),
            ("artist", false, "Artist"),
            ("albumArtist", false, "Album artist"),
            ("album", false, "Album"),
            ("genre", false, "Genre"),
            ("year", false, "Year (oldest first)"),
            ("year", true, "Year (newest first)"),
            ("duration", false, "Duration (shortest first)"),
            ("duration", true, "Duration (longest first)"),
            ("fileName", false, "File name"),
            ("added", true, "Recently added"),
            ("modified", true, "Recently modified"),
            ("favorite", true, "Favorites first"),
        ];
        let (current, desc) = self.page_sort();
        let favorites = self.page.borrow().0 == "favorites";
        let mut menu = Menu { min_width: 260.0, ..Default::default() };
        for (key, descending, text) in CHOICES {
            if key == "favorite" && favorites {
                continue;
            }
            let checked = current == key && (key.is_empty() || desc == descending);
            menu.push(Entry::check(tr("TrackTable", text), checked, move |c| c.request_sort(key, descending)));
        }
        let (x, y, _, h) = self.anchor();
        self.show_menu(menu, Place::At(x, y + h));
    }

    /// The columns menu: every column, the shown ones checked
    /// (never none), then "Reset columns".
    pub(super) fn columns_menu(&self) {
        let keys = self.column_keys();
        let mut menu = Menu::default();
        for c in crate::tracklist::CATALOG.iter() {
            let key = c.key;
            let on = keys.iter().any(|k| k == key);
            let entry = Entry::check(tr("TrackColumns", c.title), on, move |ctl| ctl.set_column_visible(key, !on))
                .enabled(!(on && keys.len() == 1));
            menu.push(entry);
        }
        menu.separator();
        menu.push(Entry::item(tr("TrackTable", "Reset columns"), |c| {
            {
                let mut s = c.settings.borrow_mut();
                s.set_text("columns/keys", "");
                s.set_text("columns/widths", "");
            }
            c.fill_list();
        }));
        let (x, y, _, _) = self.anchor();
        self.show_menu(menu, Place::Pointer(x, y));
    }

    /// A column shown or hidden: added at the end, at least one kept.
    fn set_column_visible(&self, key: &str, on: bool) {
        let mut keys = self.column_keys();
        if on == keys.iter().any(|k| k == key) || (!on && keys.len() == 1) {
            return;
        }
        if on {
            keys.push(key.to_owned());
        } else {
            keys.retain(|k| k != key);
        }
        self.settings.borrow_mut().set_text("columns/keys", serde_json::to_string(&keys).unwrap_or_default());
        self.fill_list();
    }

    /// The levels menu: subfolders on or off, one level fewer or more.
    pub(super) fn subfolders_menu(&self) {
        let (on, shown) = {
            let s = self.settings.borrow();
            (s.flag("ui/treeSubfolders", true), self.tree_levels())
        };
        let deepest = self.tree_available_levels() as i32;
        let mut menu = Menu::default();
        // A switch to show them and how many levels deep
        // (− N +, from 1 to the deepest there is); the menu stays open.
        let set = |c: &Rc<Controller>, levels: i64| {
            c.settings.borrow_mut().set_int("ui/treeFolderDepth", levels);
            c.refresh_sidebar();
            c.subfolders_menu();
        };
        menu.push(Entry::levels(
            tr("SubfolderLevels", "Subfolders"),
            on,
            shown as i32,
            deepest,
            move |c| set(c, (shown as i64 - 1).max(1)),
            move |c| set(c, (shown as i64 + 1).min(deepest as i64)),
            move |c| {
                c.settings.borrow_mut().set_flag("ui/treeSubfolders", !on);
                c.refresh_sidebar();
                c.subfolders_menu();
            },
        ));
        let (x, y, w, h) = self.anchor();
        self.show_menu(menu, Place::RightAt(x + w, y + h + 2.0));
    }

    /// The levels of subfolders the tree shows.
    pub(super) fn tree_levels(&self) -> usize {
        let setting = self.settings.borrow().int("ui/treeFolderDepth", 99).max(1) as usize;
        setting.min(self.tree_available_levels()).max(1)
    }

    /// The levels of subfolders the library has under its watched folders
    /// (at least one).
    pub(super) fn tree_available_levels(&self) -> usize {
        let lib = self.library.borrow();
        // The deepest folder under a watched one, in levels.
        let mut deepest = 0;
        for t in &lib.tracks {
            for root in &lib.folders {
                if let Some(rest) = t.folder().strip_prefix(root.as_str()) {
                    deepest = deepest.max(rest.matches('/').count() + 1);
                }
            }
        }
        (deepest.max(1) - 1).max(1)
    }

    /// The deepest level the tree shows (0: the watched folders only).
    pub(super) fn tree_max_depth(&self) -> i32 {
        let on = self.settings.borrow().flag("ui/treeSubfolders", true);
        if on { self.tree_levels() as i32 } else { 0 }
    }

    /// The playlists' add menu: a new playlist, from a folder, or imported.
    pub(super) fn playlists_menu(&self) {
        let mut menu = Menu::default();
        menu.push(Entry::item(tr("Sidebar", "New playlist…"), |c| c.new_playlist(Vec::new())));
        menu.push(Entry::item(tr("Sidebar", "New playlist from a folder…"), |c| {
            let request = crate::file_dialog::Request {
                title: tr("Sidebar", "New playlist from a folder…"),
                directory: true,
                ..Default::default()
            };
            let me = Rc::downgrade(c);
            crate::file_dialog::ask(request, move |dirs| {
                let (Some(c), Some(dir)) = (me.upgrade(), dirs.first()) else { return };
                let files = c.library.borrow().inner().expand_audio_paths(std::slice::from_ref(dir));
                if files.is_empty() {
                    c.show_message(&tr("PlaylistPage", "No audio files in that folder"));
                    return;
                }
                let base =
                    std::path::Path::new(dir).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let made = c.ui.borrow_mut().playlists.create(&base);
                c.ui.borrow_mut().playlists.add_tracks(&made, &files);
                c.open("playlist", &made);
            });
        }));
        menu.push(Entry::item(tr("Sidebar", "Import M3U…"), |c| {
            let request = crate::file_dialog::Request {
                title: tr("Sidebar", "Import M3U…"),
                filters: vec![("M3U".into(), vec!["*.m3u".into(), "*.m3u8".into()])],
                ..Default::default()
            };
            let me = Rc::downgrade(c);
            crate::file_dialog::ask(request, move |files| {
                let (Some(c), Some(file)) = (me.upgrade(), files.first()) else { return };
                let made = c.ui.borrow_mut().playlists.import_m3u(std::path::Path::new(file));
                if let Some(made) = made {
                    c.open("playlist", &made);
                }
            });
        }));
        let (x, y, _, h) = self.anchor();
        self.show_menu(menu, Place::At(x, y + h));
    }

    /// A folder of the tree right-clicked: the
    /// folder's tracks as a track menu, then taking the folder out of the
    /// library (asked first: a watched folder stops being watched, one
    /// inside it is skipped).
    pub(super) fn folder_menu(&self, folder: &str) {
        let paths = self.library.borrow().inner().folder_paths(folder);
        let mut menu = Menu::default();
        self.add_track_actions(&mut menu, &paths);
        if !menu.entries.is_empty() {
            menu.separator();
        }
        let f = folder.to_owned();
        menu.push(Entry::item(tr("TrackMenu", "Remove folder from library"), move |c| c.confirm_remove_folder(&f)));
        let (x, y, _, _) = self.anchor();
        self.show_menu(menu, Place::Pointer(x, y));
    }

    /// A playlist's menu: play, rename, delete.
    pub(super) fn playlist_menu(&self, name: &str) {
        let mut menu = Menu::default();
        let n = name.to_owned();
        menu.push(Entry::item(tr("Sidebar", "Play"), move |c| {
            let paths = c.ui.borrow().playlists.paths(&n);
            c.play_list(paths, Some(0), false);
        }));
        let n = name.to_owned();
        menu.push(Entry::item(tr("Sidebar", "Rename…"), move |c| {
            let from = n.clone();
            c.ask_name(&tr("Sidebar", "Rename playlist"), "", &n, &tr("Sidebar", "Rename"), "", move |c, to| {
                if to == from {
                    return;
                }
                let made = c.ui.borrow_mut().playlists.rename(&from, &to);
                let page = c.page.borrow().clone();
                if page.0 == "playlist" && page.1 == from {
                    c.open("playlist", &made);
                } else {
                    c.refresh_sidebar();
                }
            });
        }));
        menu.separator();
        let n = name.to_owned();
        menu.push(Entry::item(tr("Sidebar", "Delete playlist…"), move |c| {
            let doomed = n.clone();
            c.confirm(
                &trf("Sidebar", "Delete “{0}”?", &[&n]),
                &tr("Sidebar", "The playlist is removed; its tracks stay in the library and on disk."),
                &tr("PlaylistPage", "Delete playlist"),
                move |c| {
                    c.ui.borrow_mut().playlists.remove(&doomed);
                    let page = c.page.borrow().clone();
                    if page.0 == "playlist" && page.1 == doomed {
                        c.open("tracks", "");
                    } else {
                        c.refresh_sidebar();
                    }
                },
            );
        }));
        let (x, y, _, _) = self.anchor();
        self.show_menu(menu, Place::Pointer(x, y));
    }

    /// The album's ⋯: the usual actions for all its tracks, under the button.
    fn album_more_menu(&self) {
        let Some(key) = self.ui.borrow().album.clone() else { return };
        let paths = self.album_paths(&key);
        let mut menu = Menu::default();
        self.add_track_actions(&mut menu, &paths);
        let (x, y, _, h) = self.anchor();
        self.show_menu(menu, Place::At(x, y + h));
    }

    /// The outputs menu (◆ = a direct, bit-perfect one),
    /// the one in use checked; centred above the button.
    pub(super) fn outputs_menu(&self) {
        // The output in use: the setting ("" = System default).
        let current = self.settings.borrow().text("playback/device", "");
        let mut menu = Menu::default();
        for d in crate::audio::output_list(&sonkore_engine::output::devices()) {
            let text = format!(
                "{}{}   —   {}",
                if d.direct { "◆ " } else { "" },
                d.name,
                crate::audio::backend_label(&d.backend)
            );
            let key = d.key.clone();
            menu.push(Entry::check(text, d.key == current, move |c| c.select_output(&key)));
        }
        if crate::audio::output_reserved() {
            menu.separator();
            menu.push(Entry::item(tr("NowPlayingBar", "Release DAC to the system"), |c| {
                crate::audio::release_output(&mut c.player.borrow_mut());
                c.after(sonkore_engine::Changes { state: true, ..Default::default() });
            }));
        }
        let (x, y, w, _) = self.anchor();
        self.show_menu(menu, Place::Above { x, width: w, top: y });
    }

    /// The volume modes: automatic, digital,
    /// the DAC's own control.
    pub(super) fn volume_menu(&self) {
        // The modes: digital = 0, hardware (the DAC's) = 1, automatic = 2.
        let mode = self.settings.borrow().int("playback/volumeMode", 2);
        let mut menu = Menu::default();
        for (m, text) in [(2, "Automatic"), (0, "Digital"), (1, "DAC")] {
            menu.push(Entry::check(tr("SettingsPage", text), mode == m, move |c| {
                c.settings.borrow_mut().set_int("playback/volumeMode", m);
                c.update_volume();
                crate::settings_ui::refresh_audio();
            }));
        }
        let (x, y, w, _) = self.anchor();
        self.show_menu(menu, Place::Above { x, width: w, top: y });
    }

    /// The signal path: above the pill, its right edges with it.
    pub(super) fn show_signal_path(&self) {
        self.refresh_now();
        let (x, y, w, _) = self.anchor();
        let window = self.window();
        let m = window.global::<Menus>();
        m.set_path_x((x + w - 360.0).max(0.0));
        m.set_path_bottom(y - 8.0);
        m.set_path_open(true);
    }

    // --- Wiring -----------------------------------------------------------------------------

    pub(super) fn connect_pages(self: &Rc<Self>, window: &crate::MainWindow) {
        self.connect_menus(window);
        let pages = window.global::<Pages>();
        let app = window.global::<App>();
        macro_rules! on {
            ($g:ident, $name:ident, |$c:ident $(, $arg:ident: $t:ty)*| $body:block) => {{
                let weak = Rc::downgrade(self);
                $g.$name(move |$($arg: $t),*| {
                    if let Some($c) = weak.upgrade() $body
                });
            }};
        }
        on!(pages, on_open_album, |c, key: SharedString| {
            c.open_album(&key);
        });
        on!(pages, on_play_album, |c, key: SharedString| {
            let paths = c.album_paths(&key);
            c.play_list(paths, Some(0), false);
        });
        on!(pages, on_close_album, |c| {
            c.ui.borrow_mut().album = None;
            c.fill_list();
        });
        on!(pages, on_album_play, |c| {
            c.play_list(c.list.paths(), Some(0), false);
        });
        on!(pages, on_album_shuffle, |c| {
            c.play_list(c.list.paths(), None, true);
        });
        on!(pages, on_album_more, |c| {
            c.album_more_menu();
        });
        on!(pages, on_browse_select, |c, key: SharedString| {
            c.browse_select(&key);
        });
        on!(pages, on_browse_activate, |c, key: SharedString| {
            c.browse_select(&key);
            c.play_list(c.list.paths(), Some(0), false);
        });
        on!(pages, on_browse_play, |c| {
            c.play_list(c.list.paths(), Some(0), false);
        });
        on!(pages, on_browse_shuffle, |c| {
            c.play_list(c.list.paths(), None, true);
        });
        on!(pages, on_save_queue, |c| {
            let paths = c.player.borrow().queue().paths().to_vec();
            c.new_playlist(paths);
        });
        on!(pages, on_clear_queue, |c| {
            let changes = c.player.borrow_mut().clear();
            c.after(changes);
        });
        on!(pages, on_show_in_library, |c| {
            let path = c.player.borrow().current_path().unwrap_or("").to_owned();
            c.reveal(&path);
        });
        on!(pages, on_stage_action, |c, action: SharedString| {
            // Automatic headroom on.
            if action == "headroomAuto" {
                c.settings.borrow_mut().set_flag("playback/headroomAuto", true);
                crate::audio::apply(&c.settings.borrow(), c.player.borrow_mut().engine_mut());
                c.clipped.set(0);
                c.refresh_player();
                c.refresh_now();
            }
        });
        on!(pages, on_playlist_play, |c| {
            let paths = c.ui.borrow().list_paths.clone();
            c.play_list(paths, Some(0), false);
        });
        on!(pages, on_playlist_shuffle, |c| {
            let paths = c.ui.borrow().list_paths.clone();
            c.play_list(paths, None, true);
        });
        on!(pages, on_playlist_action, |c, action: SharedString| {
            c.playlist_action(&action);
        });
        on!(app, on_row_menu, |c, row: i32, x: f32, y: f32| {
            c.row_menu(row.max(0) as usize, x, y);
        });
        on!(app, on_sort_menu, |c| {
            c.sort_menu();
        });
        on!(app, on_columns_menu, |c| {
            c.columns_menu();
        });
        on!(app, on_subfolders_menu, |c| {
            c.subfolders_menu();
        });
        on!(app, on_playlists_menu, |c| {
            c.playlists_menu();
        });
        on!(app, on_playlist_menu, |c, name: SharedString| {
            c.playlist_menu(&name);
        });
        on!(app, on_folder_menu, |c, path: SharedString| {
            c.folder_menu(&path);
        });
        on!(app, on_show_outputs, |c| {
            c.outputs_menu();
        });
        on!(app, on_volume_menu, |c| {
            c.volume_menu();
        });
        on!(app, on_show_signal_path, |c| {
            c.show_signal_path();
        });
    }

    /// Now playing › Show in library: Tracks, the search cleared, the track
    /// selected.
    fn reveal(&self, path: &str) {
        if path.is_empty() {
            return;
        }
        self.search.borrow_mut().clear();
        self.open("tracks", "");
        let row = self.list.paths().iter().position(|p| p == path);
        if let Some(r) = row {
            self.select(r, false, false);
            self.reveal_row_at(r, true);
        }
    }

    /// "Remove “x” from the library?": a watched folder leaves the library,
    /// one inside it is skipped from now on; the files stay on disk.
    pub(crate) fn confirm_remove_folder(&self, folder: &str) {
        let watched = self.library.borrow().inner().folders().iter().any(|f| f == folder);
        let count = self.library.borrow().inner().folder_paths(folder).len();
        let name = folder.rsplit('/').next().unwrap_or(folder).to_owned();
        let title = trf("Main", "Remove “{0}” from the library?", &[&name]);
        let text = if watched {
            trf(
                "Main",
                "Its {0} tracks disappear from Sonkore; the files stay on disk. You can add the folder again with +.",
                &[&count.to_string()],
            )
        } else {
            trf(
                "Main",
                "Its {0} tracks disappear from Sonkore; the files stay on disk. You can bring it back in Settings › Library › Skipped folders.",
                &[&count.to_string()],
            )
        };
        let folder = folder.to_owned();
        self.confirm(&title, &text, &tr("Main", "Remove folder"), move |c| {
            let changes = c.player.borrow_mut().forget_folder(&folder);
            c.after(changes);
            let f = folder.clone();
            if watched {
                c.change_library(move |l| l.remove_folder(&f));
            } else {
                c.change_library(move |l| l.exclude_folder(&f));
            }
        });
    }

    /// For checking the interface offscreen: SONKORE_SHOW=menu name, dialog
    /// or popup to show at start (track, track-sub, sort, columns,
    /// subfolders, playlists, playlist, album-more, outputs, volume, name,
    /// confirm, signal-path), SONKORE_OPEN_ALBUM=n opens the n-th album,
    /// SONKORE_BROWSE=n picks the n-th artist / genre.
    pub(super) fn screenshot_hooks(self: &Rc<Self>) {
        let album = std::env::var("SONKORE_OPEN_ALBUM").ok().and_then(|s| s.parse::<usize>().ok());
        if let Some(n) = album {
            self.refresh_albums();
            let key = self.ui.borrow().albums.get(n).map(|a| a.key.clone());
            if let Some(k) = key {
                self.open_album(&k);
            }
        }
        if let Some(n) = std::env::var("SONKORE_BROWSE").ok().and_then(|s| s.parse::<usize>().ok()) {
            let key = self.window().global::<Pages>().get_entries().row_data(n).map(|e| e.key.to_string());
            if let Some(k) = key {
                self.browse_select(&k);
            }
        }
        let Ok(what) = std::env::var("SONKORE_SHOW") else { return };
        let weak = Rc::downgrade(self);
        // After the window has its size (the menus are placed within it).
        slint::Timer::single_shot(std::time::Duration::from_millis(30), move || {
            let Some(c) = weak.upgrade() else { return };
            let window = c.window();
            let m = window.global::<Menus>();
            let set_anchor = |x: f32, y: f32, w: f32, h: f32| {
                m.set_anchor_x(x);
                m.set_anchor_y(y);
                m.set_anchor_width(w);
                m.set_anchor_height(h);
            };
            match what.as_str() {
                "track" | "track-sub" => {
                    c.row_menu(1, 700.0, 300.0);
                    if what == "track-sub" {
                        m.set_sub_open(true);
                    }
                }
                "sort" => {
                    set_anchor(264.0, 48.0, 1056.0, 34.0);
                    c.sort_menu();
                }
                "columns" => {
                    set_anchor(700.0, 70.0, 0.0, 0.0);
                    c.columns_menu();
                }
                "subfolders" => {
                    set_anchor(159.0, 336.0, 30.0, 30.0);
                    c.subfolders_menu();
                }
                "playlists" => {
                    set_anchor(209.0, 560.0, 30.0, 30.0);
                    c.playlists_menu();
                }
                "playlist" => {
                    set_anchor(120.0, 614.0, 0.0, 0.0);
                    let name = c.ui.borrow().playlists.names().first().cloned().unwrap_or_default();
                    c.playlist_menu(&name);
                }
                "album-more" => {
                    set_anchor(500.0, 220.0, 34.0, 34.0);
                    c.album_more_menu();
                }
                "outputs" => {
                    set_anchor(1018.0, 779.0, 34.0, 34.0);
                    c.outputs_menu();
                }
                "volume" => {
                    set_anchor(1149.0, 779.0, 34.0, 34.0);
                    c.volume_menu();
                }
                "signal-path" => {
                    set_anchor(960.0, 783.0, 120.0, 26.0);
                    c.show_signal_path();
                }
                "name" => c.new_playlist(vec!["a".into(), "b".into()]),
                "confirm" => c.confirm(
                    &trf("Sidebar", "Delete “{0}”?", &["Viaje"]),
                    &tr("Sidebar", "The playlist is removed; its tracks stay in the library and on disk."),
                    &tr("PlaylistPage", "Delete playlist"),
                    |_| {},
                ),
                _ => {}
            }
        });
    }
}

/// A signal path quality's label.
pub fn quality_label(quality: &str) -> String {
    match quality {
        "lossless" => tr("SignalPathList", "Lossless"),
        "high" => tr("SignalPathList", "High quality"),
        "enhanced" => tr("SignalPathList", "Enhanced"),
        "warning" => tr("SignalPathList", "Warning"),
        _ => tr("SignalPathList", "Low quality"),
    }
}

/// The 'g' format at precision 6.
fn g6(v: f64) -> String {
    let s = format!("{:.*}", (5 - v.abs().max(1e-9).log10().floor() as i32).max(0) as usize, v);
    if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_owned() } else { s }
}

/// The file manager at the folder.
#[cfg(not(windows))]
fn open_containing_folder(path: &str) {
    let dir = path.rsplit_once('/').map_or(path, |(d, _)| d).to_owned();
    // Waited for on a thread: a child never waited for stays a zombie.
    if let Ok(mut child) = std::process::Command::new("xdg-open").arg(dir).spawn() {
        std::thread::spawn(move || child.wait());
    }
}

/// On Windows: Explorer at the folder,
/// the file selected (a CD image's track: its sheet).
#[cfg(windows)]
fn open_containing_folder(path: &str) {
    use std::os::windows::process::CommandExt;
    let file = sheet_or_file(path).replace('/', "\\");
    // Explorer reads `/select,"<path>"`, not the argument quoted whole.
    if let Ok(mut child) = std::process::Command::new("explorer").raw_arg(format!("/select,\"{file}\"")).spawn() {
        std::thread::spawn(move || child.wait());
    }
}

/// A path without a CD image's "#<track>" ("disc.cue#3" -> "disc.cue").
#[cfg(windows)]
fn sheet_or_file(path: &str) -> &str {
    match path.rsplit_once('#') {
        Some((sheet, n)) if sheet.to_ascii_lowercase().ends_with(".cue") && n.bytes().all(|b| b.is_ascii_digit()) => {
            sheet
        }
        _ => path,
    }
}

/// The album grid's tiles. A tile's cover is asked for when Slint shows it
/// (the grid is a ListView: only the rows on screen). With every album's
/// cover asked for at each refresh, a library with more albums than the
/// cover cache holds decoded them again and again: the decoders never rested
/// and the whole grid was rebuilt every 60 ms.
pub struct AlbumTiles {
    tiles: Vec<AlbumTile>,
    covers: Vec<String>,
    notify: slint::ModelNotify,
}

impl AlbumTiles {
    fn covers_arrived(&self) {
        for row in 0..self.tiles.len() {
            self.notify.row_changed(row);
        }
    }
}

impl Model for AlbumTiles {
    type Data = AlbumTile;

    fn row_count(&self) -> usize {
        self.tiles.len()
    }

    fn row_data(&self, row: usize) -> Option<AlbumTile> {
        let mut tile = self.tiles.get(row)?.clone();
        // Tiles are about 200 px: one size for all, Slint fits it.
        if let Some(cover) = crate::covers::get(self.covers.get(row)?, 220.0, 0.0) {
            tile.has_cover = true;
            tile.cover = cover;
        }
        Some(tile)
    }

    fn model_tracker(&self) -> &dyn slint::ModelTracker {
        &self.notify
    }
}
