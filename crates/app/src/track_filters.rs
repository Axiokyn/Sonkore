//! The Tracks page's filters (ui/trackBrowser): the three panes'
//! entries, what a click on them picks, the rows they leave for the list.
//! A child module of the controller; the filtering itself is filters.rs.
//!
//! The index is made again only when the library changes (its generation);
//! a pick counts the panes again and patches the rows whose highlight
//! changed, so the models stay (a pane made again would lose a double
//! click's first press, and its place).

use std::rc::Rc;

use slint::{ComponentHandle, Model, SharedString, VecModel};

use super::Controller;
use crate::filters::{Index, Item, Selection};
use crate::i18n::tr;
use crate::lib_shim::Library;
use crate::{FilterEntry, Filters};

#[derive(Default)]
pub struct FilterState {
    /// Settings › General › Filters on the Tracks page.
    pub on: bool,
    /// The index and the library generation it was made from.
    index: Option<(u64, Index)>,
    pub selection: Selection,
    /// The three panes' models (genres, artists, albums), kept.
    models: [Rc<VecModel<FilterEntry>>; 3],
}

/// A pane's rows: "All genres (12)" first, then its entries.
fn entries(pane: usize, items: &[Item], picked: &str) -> Vec<FilterEntry> {
    let (all, unknown) = match pane {
        0 => (tr("ColumnBrowser", "All genres"), tr("ColumnBrowser", "No genre")),
        1 => (tr("ColumnBrowser", "All artists"), tr("ColumnBrowser", "Unknown artist")),
        _ => (tr("ColumnBrowser", "All albums"), tr("ColumnBrowser", "No album")),
    };
    let mut rows = Vec::with_capacity(items.len() + 1);
    rows.push(FilterEntry {
        key: SharedString::default(),
        name: format!("{all} ({})", items.len()).into(),
        count: SharedString::default(),
        unknown: false,
        selected: picked.is_empty(),
    });
    rows.extend(items.iter().map(|i| FilterEntry {
        key: i.key.as_str().into(),
        name: if i.unknown { unknown.as_str().into() } else { i.name.as_str().into() },
        count: i.count.to_string().into(),
        unknown: i.unknown,
        selected: i.key == picked,
    }));
    rows
}

impl Controller {
    pub(super) fn connect_filters(self: &Rc<Self>, window: &crate::MainWindow) {
        let f = window.global::<Filters>();
        {
            let s = self.settings.borrow();
            let mut st = self.filters.borrow_mut();
            st.on = s.flag("ui/trackBrowser", false);
            st.selection = Selection {
                genre: s.text("ui/trackBrowserGenre", ""),
                artist: s.text("ui/trackBrowserArtist", ""),
                album: s.text("ui/trackBrowserAlbum", ""),
            };
            f.set_genres(st.models[0].clone().into());
            f.set_artists(st.models[1].clone().into());
            f.set_albums(st.models[2].clone().into());
            f.set_height(s.int("ui/trackBrowserHeight", -1).max(-1) as f32);
        }
        let weak = Rc::downgrade(self);
        f.on_pick(move |pane, row| {
            if let Some(c) = weak.upgrade() {
                c.filter_pick(pane as usize, row as usize);
            }
        });
        let weak = Rc::downgrade(self);
        f.on_activate(move |pane, row| {
            if let Some(c) = weak.upgrade() {
                c.filter_pick(pane as usize, row as usize);
                // The tracks the list shows, in its order, from the first.
                let paths: Vec<String> = {
                    let lib = c.library.borrow();
                    let st = c.list.state.borrow();
                    st.indices.iter().filter_map(|&i| lib.tracks.get(i)).map(|t| t.path.clone()).collect()
                };
                c.play_list(paths, Some(0), false);
            }
        });
        let weak = Rc::downgrade(self);
        f.on_height_changed(move |h| {
            if let Some(c) = weak.upgrade() {
                c.settings.borrow_mut().set_int("ui/trackBrowserHeight", if h < 0.0 { -1 } else { h.round() as i64 });
            }
        });
        self.show_filters();
    }

    /// The panes shown when the option is on and there is a library to filter.
    fn show_filters(&self) {
        let on = self.filters.borrow().on && !self.library.borrow().tracks.is_empty();
        self.window().global::<Filters>().set_on(on);
    }

    /// Settings › General › Filters on the Tracks page changed.
    pub(crate) fn track_filters_changed(&self) {
        let on = self.settings.borrow().flag("ui/trackBrowser", false);
        self.filters.borrow_mut().on = on;
        self.show_filters();
        if self.page.borrow().0 == "tracks" {
            self.fill_list();
        }
    }

    /// The Tracks page's rows with the filters on (None: off). The index is
    /// made again for a new library, and the panes with it.
    pub(super) fn filtered_rows(&self, lib: &Library) -> Option<Vec<usize>> {
        let fresh = {
            let mut st = self.filters.borrow_mut();
            if !st.on || lib.tracks.is_empty() {
                drop(st);
                self.show_filters();
                return None;
            }
            let stale = st.index.as_ref().is_none_or(|(g, i)| *g != lib.generation || i.len() != lib.tracks.len());
            if stale {
                st.index = Some((lib.generation, Index::new(&lib.tracks)));
            }
            stale
        };
        if fresh {
            self.show_filters();
            self.refresh_panes(true);
        }
        let st = self.filters.borrow();
        st.index.as_ref().map(|(_, index)| index.rows(&st.selection))
    }

    /// The panes for the picks: a pane whose entries are the same only
    /// moves its highlight (`all`: every pane scrolls to its pick).
    fn refresh_panes(&self, all: bool) {
        let mut st = self.filters.borrow_mut();
        let Some((_, index)) = st.index.as_ref() else { return };
        let mut selection = st.selection.clone();
        let panes = index.panes(&mut selection);
        st.selection = selection.clone();
        let w = self.window();
        let f = w.global::<Filters>();
        let picks = [&selection.genre, &selection.artist, &selection.album];
        for (pane, items) in [&panes.genres, &panes.artists, &panes.albums].into_iter().enumerate() {
            let rows = entries(pane, items, picks[pane]);
            let model = &st.models[pane];
            let same = model.row_count() == rows.len()
                && rows.iter().enumerate().all(|(i, r)| {
                    model.row_data(i).is_some_and(|m| m.key == r.key && m.name == r.name && m.count == r.count)
                });
            if same {
                for (i, r) in rows.into_iter().enumerate() {
                    if model.row_data(i).is_some_and(|m| m.selected != r.selected) {
                        model.set_row_data(i, r);
                    }
                }
                if !all {
                    continue;
                }
            } else {
                model.set_vec(rows);
            }
            // New entries (or a new library): to the pick, else the top.
            let reveal = if picks[pane].is_empty() {
                -1
            } else {
                items.iter().position(|i| i.key == *picks[pane]).map_or(-1, |p| p as i32 + 1)
            };
            match pane {
                0 => {
                    f.set_genres_reveal(reveal);
                    f.set_genres_serial(f.get_genres_serial() + 1);
                }
                1 => {
                    f.set_artists_reveal(reveal);
                    f.set_artists_serial(f.get_artists_serial() + 1);
                }
                _ => {
                    f.set_albums_reveal(reveal);
                    f.set_albums_serial(f.get_albums_serial() + 1);
                }
            }
        }
    }

    /// A click on a pane's row: that entry picked (All: none), the panes
    /// after it narrowed, the list filtered.
    fn filter_pick(&self, pane: usize, row: usize) {
        {
            let mut st = self.filters.borrow_mut();
            let Some(entry) = st.models.get(pane).and_then(|m| m.row_data(row)) else { return };
            let key = entry.key.to_string();
            let sel = &mut st.selection;
            let slot = match pane {
                0 => &mut sel.genre,
                1 => &mut sel.artist,
                _ => &mut sel.album,
            };
            if *slot == key {
                return;
            }
            *slot = key;
            // A new genre or artist: the album picked belonged to the old one.
            if pane < 2 {
                sel.album.clear();
            }
        }
        self.refresh_panes(false);
        {
            let st = self.filters.borrow();
            let mut s = self.settings.borrow_mut();
            s.set_text("ui/trackBrowserGenre", &st.selection.genre);
            s.set_text("ui/trackBrowserArtist", &st.selection.artist);
            s.set_text("ui/trackBrowserAlbum", &st.selection.album);
        }
        self.fill_list();
    }

    /// For the live tests' probe: "genre|artist|album" picked.
    pub(super) fn filters_probe(&self) -> String {
        let st = self.filters.borrow();
        if !st.on {
            return String::new();
        }
        let s = &st.selection;
        format!("{}|{}|{}", s.genre, s.artist, s.album.replace('\u{1f}', "/"))
    }
}
