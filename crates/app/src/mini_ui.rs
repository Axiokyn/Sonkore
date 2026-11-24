//! The mini player (and entering and fitting compact mode): the same window shows ui/mini.slint instead of the full
//! view and takes the player's size — its width (ui/compactWidth), and the
//! list's height when it shows (ui/compactHeight), both kept once the size
//! has settled. The list shows the play queue or a library folder
//! (ui/compactSource), with its own columns, order and search (ui/compact*).
//!
//! Buttons of the transport go through the `App` callbacks the controller
//! already handles; what only the mini player does is here.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;
use std::time::Duration;

use slint::{ComponentHandle, Model, ModelNotify, ModelRc, ModelTracker, SharedString, Timer, TimerMode, VecModel};
use sonkore_engine::Changes;

use crate::context::Ctx;
use crate::controller::{natural_cmp, position_to_volume, volume_to_position};
use crate::i18n::{tr, trf};
use crate::lib_shim::{Library, Track};
use crate::tracklist::{self, DEFAULT_KEYS, cell_text};
use crate::{
    App, MainWindow, Mini, MiniCell, MiniChoice, MiniColumn, MiniFolder, MiniRow, Skins, Theme, WindowActions,
};

const CTX: &str = "MiniPlayer";
const GAP: f32 = 8.0; // between the list's columns

/// The fields the sort menu offers.
const SORT_FIELDS: [&str; 12] = [
    "path",
    "fileName",
    "title",
    "artist",
    "albumArtist",
    "album",
    "genre",
    "duration",
    "year",
    "trackNo",
    "added",
    "favorite",
];

/// The columns that keep a width of their own.
fn fixed_width(key: &str) -> Option<f32> {
    Some(match key {
        "duration" => 44.0,
        "year" => 40.0,
        "trackNo" | "discNo" => 34.0,
        "format" => 92.0,
        "codec" => 52.0,
        "sampleRate" => 66.0,
        "bits" => 54.0,
        "channels" => 60.0,
        "bitrate" => 82.0,
        "fileSize" => 64.0,
        "added" | "modified" => 84.0,
        "favorite" => 26.0,
        _ => return None,
    })
}

/// A column's title in the head and the menus ("path" is the folder's order).
fn field_title(key: &str) -> String {
    if key == "path" {
        return tr(CTX, "As in the folder");
    }
    tracklist::column(key).map(|c| tr("TrackColumns", c.title)).unwrap_or_else(|| key.to_owned())
}

// --- The list's rows --------------------------------------------------------------------

/// A row: a track's path and, in the queue, its place there.
#[derive(Clone)]
struct Entry {
    path: String,
    index: usize,
}

#[derive(Default)]
struct ListData {
    entries: Vec<Entry>,
    queue: bool,
    shown: Vec<String>,
    current_index: Option<usize>,
    current_path: String,
    /// The rows in sight: from `first`, `count` of them.
    first: usize,
    count: usize,
}

/// The rows, made when the list view asks for them (a queue or a folder of
/// thousands of tracks costs a vector of paths).
struct RowsModel {
    data: RefCell<ListData>,
    library: Rc<RefCell<Library>>,
    notify: ModelNotify,
}

impl Model for RowsModel {
    type Data = MiniRow;

    fn row_count(&self) -> usize {
        let d = self.data.borrow();
        d.entries.len().saturating_sub(d.first).min(d.count)
    }

    fn row_data(&self, row: usize) -> Option<MiniRow> {
        let d = self.data.borrow();
        let row = d.first + row;
        let e = d.entries.get(row)?;
        let lib = self.library.borrow();
        let track = lib.index_of(&e.path).and_then(|i| lib.tracks.get(i));
        let artist_shown = d.shown.iter().any(|k| k == "artist");
        let cells: Vec<MiniCell> = d
            .shown
            .iter()
            .map(|k| {
                let text = match track {
                    Some(t) if k == "title" && !artist_shown && !t.artist.is_empty() => {
                        format!("{} — {}", t.artist, cell_text("title", t))
                    }
                    Some(t) => cell(k, t),
                    None if k == "title" || k == "fileName" => readable(&e.path),
                    None => String::new(),
                };
                let special = k == "format"
                    && track.is_some_and(|t| t.bits > 16 || t.sample_rate > 48000 || t.codec.contains("DSD"));
                let badge_width = if k == "format" { crate::metrics::badge_width(&text) } else { 0.0 };
                MiniCell { text: text.into(), special, badge_width }
            })
            .collect();
        let current = if d.queue {
            d.current_index == Some(e.index)
        } else {
            !d.current_path.is_empty() && e.path == d.current_path
        };
        let cover = crate::covers::get(&e.path, 40.0, 0.0);
        Some(MiniRow {
            number: format!("{}.", if d.queue { e.index } else { row } + 1).into(),
            has_cover: cover.is_some(),
            cover: cover.unwrap_or_default(),
            cells: ModelRc::new(VecModel::from(cells)),
            current,
            missing: d.queue && !std::path::Path::new(&e.path).exists(),
            favorite: lib.favorites.contains(&e.path),
        })
    }

    fn model_tracker(&self) -> &dyn ModelTracker {
        &self.notify
    }
}

/// What a cell shows, with the average bitrate (from
/// the file's size when the tags have none).
fn cell(key: &str, t: &Track) -> String {
    if key == "bitrate" && t.bitrate <= 0 && t.duration > 0.0 && t.file_size > 0 {
        let kbps = (t.file_size as f64 * 8.0 / t.duration / 1000.0 + 0.5) as i64;
        return if kbps > 0 { format!("{kbps} kbps") } else { String::new() };
    }
    cell_text(key, t)
}

/// The name of a file not in the library: without its folder and extension.
fn readable(path: &str) -> String {
    let t = Track { path: path.to_owned(), ..Default::default() };
    cell_text("fileName", &t)
}

// --- Folders as a tree --------------------------------------------------------------------

#[derive(Clone, Debug)]
struct FolderNode {
    path: String,
    name: String,
    label: String,
    depth: i32,
    total: usize,
    last: bool,
}

/// The folder tree: the folders that hold music, named from their
/// library folder, as a tree with the tracks under each.
fn folder_tree(lib: &Library) -> Vec<FolderNode> {
    // The folders that hold tracks.
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for t in &lib.tracks {
        if let Some((dir, _)) = t.path.rsplit_once('/') {
            if !dir.is_empty() {
                *counts.entry(dir.to_owned()).or_default() += 1;
            }
        }
    }
    struct Node {
        path: String,
        name: String,
        label: String,
        depth: i32,
        total: usize,
        children: Vec<usize>,
    }
    let mut nodes: Vec<Node> = Vec::new();
    let mut by_path: HashMap<String, usize> = HashMap::new();
    let mut roots: Vec<usize> = Vec::new();
    for (dir, count) in &counts {
        let mut name = dir.rsplit('/').next().unwrap_or(dir).to_owned();
        for root in &lib.folders {
            if dir == root {
                break;
            }
            if let Some(rest) = dir.strip_prefix(&format!("{root}/")) {
                name = rest.to_owned();
                break;
            }
        }
        let base = &dir[..dir.len() - name.len()];
        let parts: Vec<&str> = name.split('/').collect();
        let mut parent: Option<usize> = None;
        for i in 0..parts.len() {
            let p = format!("{base}{}", parts[..=i].join("/"));
            let n = match by_path.get(&p) {
                Some(&n) => n,
                None => {
                    let n = nodes.len();
                    nodes.push(Node {
                        path: p.clone(),
                        name: parts[i].to_owned(),
                        label: parts[..=i].join("  ›  "),
                        depth: i as i32,
                        total: 0,
                        children: Vec::new(),
                    });
                    by_path.insert(p, n);
                    match parent {
                        Some(pa) => nodes[pa].children.push(n),
                        None => roots.push(n),
                    }
                    n
                }
            };
            nodes[n].total += count;
            parent = Some(n);
        }
    }
    let mut out = Vec::new();
    fn walk(nodes: &[Node], mut list: Vec<usize>, out: &mut Vec<FolderNode>) {
        list.sort_by(|&a, &b| natural_cmp(&nodes[a].name.to_lowercase(), &nodes[b].name.to_lowercase()));
        let n = list.len();
        for (i, &k) in list.iter().enumerate() {
            let node = &nodes[k];
            out.push(FolderNode {
                path: node.path.clone(),
                name: node.name.clone(),
                label: node.label.clone(),
                depth: node.depth,
                total: node.total,
                last: i + 1 == n,
            });
            walk(nodes, node.children.clone(), out);
        }
    }
    walk(&nodes, roots, &mut out);
    out
}

// --- Order --------------------------------------------------------------------------------

/// The order of two tracks by `field`: the field, then the path;
/// blanks last.
fn compare(
    field: &str,
    a: (&str, Option<&Track>),
    b: (&str, Option<&Track>),
    favorites: &std::collections::HashSet<String>,
) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let natural = |x: &str, y: &str| natural_cmp(&x.to_lowercase(), &y.to_lowercase());
    let o = match field {
        "path" => Ordering::Equal,
        "fileName" => natural(a.0.rsplit('/').next().unwrap_or(a.0), b.0.rsplit('/').next().unwrap_or(b.0)),
        "favorite" => favorites.contains(a.0).cmp(&favorites.contains(b.0)),
        _ => match (a.1, b.1) {
            (Some(ta), Some(tb)) => match field {
                "duration" => ta.duration.partial_cmp(&tb.duration).unwrap_or(Ordering::Equal),
                "year" | "trackNo" | "added" => {
                    let (x, y) = match field {
                        "year" => (ta.year, tb.year),
                        "trackNo" => (ta.track_no, tb.track_no),
                        _ => (ta.added, tb.added),
                    };
                    // Blanks (0) last.
                    match (x > 0, y > 0) {
                        (true, false) => Ordering::Less,
                        (false, true) => Ordering::Greater,
                        _ => x.cmp(&y),
                    }
                }
                k => {
                    let (x, y) = (cell_text(k, ta), cell_text(k, tb));
                    match (x.is_empty(), y.is_empty()) {
                        (false, true) => Ordering::Less,
                        (true, false) => Ordering::Greater,
                        _ => natural(&x, &y),
                    }
                }
            },
            _ => Ordering::Equal,
        },
    };
    o
}

// --- The mini player ----------------------------------------------------------------------

pub struct MiniUi {
    window: slint::Weak<MainWindow>,
    ctx: Rc<Ctx>,
    rows: Rc<RowsModel>,
    /// What the controller does after the player changed (Controller::after):
    /// the session saved, the sidebar and lists brought up to date.
    after: RefCell<Option<Box<dyn Fn(Changes)>>>,
    /// Folder add / remove, once the library can (a dialog, a confirmation).
    pub add_folder: RefCell<Option<Box<dyn Fn()>>>,
    pub remove_folder: RefCell<Option<Box<dyn Fn(String)>>>,
    on: Cell<bool>,
    /// The size asked for has been applied (m_compactReady): resizes from
    /// now on are the user's and are kept.
    ready: Cell<bool>,
    settle: Timer,
    refit: Timer,
    clock: Timer,
    /// The full view's size and state, to go back to.
    full: Cell<Option<(f32, f32, bool)>>,
    viewport: Cell<(f32, f32)>,
    columns: RefCell<Vec<MiniColumn>>,
    search: RefCell<String>,
    search_open: Cell<bool>,
    signature: RefCell<String>,
    /// folder_tree per library generation (it was built three times for each
    /// source menu and on every refresh).
    tree_cache: RefCell<Option<(u64, Rc<Vec<FolderNode>>)>>,
    scroll_y: Cell<f32>,
    bar_timer: Timer,
}

impl MiniUi {
    pub fn new(window: &MainWindow, ctx: Rc<Ctx>) -> Rc<MiniUi> {
        let rows = Rc::new(RowsModel {
            data: RefCell::new(ListData::default()),
            library: ctx.library.clone(),
            notify: ModelNotify::default(),
        });
        let m = Rc::new(MiniUi {
            window: window.as_weak(),
            ctx,
            rows,
            after: RefCell::new(None),
            add_folder: RefCell::new(None),
            remove_folder: RefCell::new(None),
            on: Cell::new(false),
            ready: Cell::new(false),
            settle: Timer::default(),
            refit: Timer::default(),
            clock: Timer::default(),
            full: Cell::new(None),
            viewport: Cell::new((0.0, 0.0)),
            columns: RefCell::new(Vec::new()),
            search: RefCell::new(String::new()),
            search_open: Cell::new(false),
            signature: RefCell::new(String::new()),
            tree_cache: RefCell::new(None),
            scroll_y: Cell::new(0.0),
            bar_timer: Timer::default(),
        });
        m.connect(window);
        // Covers decoded since: the rows on screen ask again.
        let rows = Rc::downgrade(&m.rows);
        crate::covers::on_arrival(move || {
            if let Some(r) = rows.upgrade() {
                let n = r.data.borrow().entries.len().saturating_sub(r.data.borrow().first);
                for i in 0..n {
                    r.notify.row_changed(i);
                }
            }
        });
        // The full view's columns, the first time.
        if m.ctx.settings.borrow().text("ui/compactColumns", "").is_empty() {
            let keys = m.full_view_keys();
            m.set_text("ui/compactColumns", &serde_json::to_string(&keys).unwrap_or_default());
        }
        // The library's first folder instead of the empty queue (compactPickFolder).
        if m.flag("ui/compactPickFolder", false) {
            let tree = m.tree();
            if let Some(first) = tree.first() {
                m.set_flag("ui/compactPickFolder", false);
                m.set_text("ui/compactSource", &first.path);
            }
        }
        m
    }

    /// Controller::after, for what the mini player changes in the player.
    pub fn set_after(&self, after: impl Fn(Changes) + 'static) {
        *self.after.borrow_mut() = Some(Box::new(after));
    }

    #[allow(dead_code)] // for the window's shortcuts (Ctrl+F opens the mini list's search while compact)
    /// The full view's size (logical) and maximized state to come back to.
    pub fn full_size(&self) -> Option<(f32, f32, bool)> {
        self.full.get()
    }

    /// Sets what the full view comes back to (the saved geometry at start-up).
    pub fn set_full_size(&self, width: f32, height: f32, maximized: bool) {
        self.full.set(Some((width, height, maximized)));
    }

    pub fn is_compact(&self) -> bool {
        self.on.get()
    }

    fn window(&self) -> MainWindow {
        self.window.upgrade().expect("window")
    }

    // --- Settings ---------------------------------------------------------------------------

    fn flag(&self, key: &str, fallback: bool) -> bool {
        self.ctx.settings.borrow().flag(key, fallback)
    }
    fn text(&self, key: &str, fallback: &str) -> String {
        self.ctx.settings.borrow().text(key, fallback)
    }
    fn int(&self, key: &str, fallback: i64) -> i64 {
        self.ctx.settings.borrow().int(key, fallback)
    }
    fn set_flag(&self, key: &str, v: bool) {
        self.ctx.settings.borrow_mut().set_flag(key, v);
    }
    fn set_text(&self, key: &str, v: &str) {
        self.ctx.settings.borrow_mut().set_text(key, v);
    }
    fn set_int(&self, key: &str, v: i64) {
        self.ctx.settings.borrow_mut().set_int(key, v);
    }

    fn source(&self) -> String {
        self.text("ui/compactSource", "")
    }
    fn showing_queue(&self) -> bool {
        self.source().is_empty()
    }
    fn list_shown(&self) -> bool {
        self.flag("ui/compactPlaylist", false)
    }

    fn after_changes(&self, changes: Changes) {
        if let Some(after) = self.after.borrow().as_ref() {
            after(changes);
        }
        self.refresh();
    }

    // --- Wiring ---------------------------------------------------------------------------

    fn connect(self: &Rc<Self>, window: &MainWindow) {
        let mini = window.global::<Mini>();
        mini.set_rows(ModelRc::from(self.rows.clone() as Rc<dyn Model<Data = MiniRow>>));
        macro_rules! on {
            ($name:ident, |$m:ident $(, $arg:ident: $t:ty)*| $body:block) => {{
                let weak = Rc::downgrade(self);
                mini.$name(move |$($arg: $t),*| {
                    if let Some($m) = weak.upgrade() $body
                });
            }};
        }
        on!(on_full_view, |m| {
            m.set_compact(false);
        });
        on!(on_toggle_list, |m| {
            let on = !m.list_shown();
            m.set_flag("ui/compactPlaylist", on);
            m.refresh_state();
            m.fit(false);
        });
        on!(on_set_always_on_top, |m, on: bool| {
            m.set_flag("ui/alwaysOnTop", on);
            m.window().global::<Mini>().set_always_on_top(on);
        });
        on!(on_clear_queue, |m| {
            let ch = m.ctx.player.borrow_mut().clear();
            m.after_changes(ch);
        });
        on!(on_resized, |m, w: f32, h: f32| {
            m.resized(w, h);
        });
        on!(on_player_height_changed, |m, _h: f32| {
            // Resized: the window refits.
            if m.on.get() {
                let weak = Rc::downgrade(&m);
                Timer::single_shot(Duration::ZERO, move || {
                    if let Some(m) = weak.upgrade() {
                        m.fit(false);
                    }
                });
            }
        });
        on!(on_step_volume, |m, dir: i32| {
            m.step_volume(dir as f64);
        });
        on!(on_set_volume_mode, |m, mode: i32| {
            m.set_int("playback/volumeMode", mode as i64);
            m.window().global::<Mini>().set_volume_mode(mode);
            crate::volume::update(&m.ctx.settings.borrow(), m.ctx.player.borrow().engine());
            m.refresh_state();
            // The main bar's volume and an open Settings window follow.
            if let Some(after) = m.after.borrow().as_ref() {
                after(Changes { state: true, ..Default::default() });
            }
            crate::settings_ui::refresh_audio();
        });
        on!(on_seek, |m, v: f32| {
            let d = m.duration();
            m.ctx.player.borrow_mut().seek(v as f64 * d);
            m.refresh_display();
        });

        on!(on_list_resized, |m, w: f32, h: f32| {
            if (w, h) != m.viewport.get() {
                m.viewport.set((w, h));
                m.layout_columns();
                m.update_window();
            }
        });
        on!(on_list_scrolled, |m, y: f32| {
            m.scroll_y.set(y);
            m.update_window();
            // The overlay bar shows while it scrolls, and a moment after.
            let win = m.window();
            win.global::<Mini>().set_bar_recent(true);
            let weak = win.as_weak();
            m.bar_timer.start(TimerMode::SingleShot, Duration::from_millis(900), move || {
                if let Some(w) = weak.upgrade() {
                    w.global::<Mini>().set_bar_recent(false);
                }
            });
        });
        on!(on_play_row, |m, row: i32| {
            m.play_row(row);
        });
        on!(on_remove_row, |m, row: i32| {
            let index = m.rows.data.borrow().entries.get(row.max(0) as usize).map(|e| e.index);
            if let Some(i) = index {
                let ch = m.ctx.player.borrow_mut().remove(&[i]);
                m.after_changes(ch);
            }
        });
        on!(on_enqueue_row, |m, row: i32| {
            let path = m.rows.data.borrow().entries.get(row.max(0) as usize).map(|e| e.path.clone());
            if let Some(p) = path {
                let ch = m.ctx.player.borrow_mut().enqueue(vec![p]);
                m.after_changes(ch);
            }
        });
        on!(on_toggle_favorite, |m, row: i32| {
            m.toggle_favorite(row.max(0) as usize);
        });
        on!(on_head_clicked, |m, key: SharedString| {
            if !m.showing_queue() && m.text("ui/compactSort", "path") == key.as_str() {
                let desc = m.flag("ui/compactSortDesc", false);
                m.set_flag("ui/compactSortDesc", !desc);
                m.rebuild();
            } else {
                m.sort_by(&key);
            }
        });
        on!(on_sort_by, |m, key: SharedString| {
            m.sort_by(&key);
        });
        on!(on_reverse, |m| {
            if m.showing_queue() {
                let ch = m.ctx.player.borrow_mut().reverse();
                m.after_changes(ch);
            } else {
                let desc = m.flag("ui/compactSortDesc", false);
                m.set_flag("ui/compactSortDesc", !desc);
                m.rebuild();
            }
        });
        on!(on_move_column, |m, key: SharedString, target: i32| {
            m.move_column(&key, target);
        });
        on!(on_set_column, |m, key: SharedString, on: bool| {
            let mut k = m.column_keys();
            k.retain(|x| x != key.as_str());
            if on {
                k.push(key.to_string());
            }
            if !k.is_empty() {
                m.set_text("ui/compactColumns", &serde_json::to_string(&k).unwrap_or_default());
            }
            m.rebuild();
        });
        on!(on_reset_columns, |m| {
            let keys: Vec<&str> = DEFAULT_KEYS.to_vec();
            m.set_text("ui/compactColumns", &serde_json::to_string(&keys).unwrap_or_default());
            m.rebuild();
        });
        on!(on_pick_source, |m, path: SharedString| {
            m.set_text("ui/compactSource", &path);
            m.rebuild();
            m.reveal(false);
        });
        on!(on_add_folder, |m| {
            if let Some(f) = m.add_folder.borrow().as_ref() {
                f();
            }
        });
        on!(on_remove_folder, |m, path: SharedString| {
            if let Some(f) = m.remove_folder.borrow().as_ref() {
                f(path.to_string());
            }
        });
        on!(on_set_subfolders, |m, on: bool| {
            m.set_flag("ui/compactSubfolders", on);
            m.refresh_folders();
        });
        on!(on_list_look_changed, |m| {
            m.layout_columns();
        });
        on!(on_set_folder_levels, |m, n: i32| {
            m.set_int("ui/compactFolderDepth", n.max(1) as i64);
            m.refresh_folders();
        });
        on!(on_search_edited, |m, text: SharedString| {
            *m.search.borrow_mut() = text.to_string();
            m.rebuild();
        });
        on!(on_search_accepted, |m| {
            if !m.rows.data.borrow().entries.is_empty() {
                m.play_row(0);
            }
        });
        on!(on_search_opened, |m| {
            m.open_search();
        });
        on!(on_search_closed, |m| {
            m.close_search();
        });
        on!(on_show_playing, |m| {
            m.show_playing();
        });
        let weak = Rc::downgrade(self);
        mini.on_column_at(move |x| {
            let Some(m) = weak.upgrade() else { return -1 };
            let cols = m.columns.borrow();
            cols.iter().position(|c| x >= c.x && x < c.x + c.width).map_or(-1, |i| i as i32)
        });
        let weak = Rc::downgrade(self);
        mini.on_target_at(move |x| {
            let Some(m) = weak.upgrade() else { return 0 };
            let cols = m.columns.borrow();
            cols.iter()
                .enumerate()
                .filter(|(_, c)| x > c.x + c.width / 2.0)
                .map(|(i, _)| i as i32 + 1)
                .next_back()
                .unwrap_or(0)
        });
    }

    // --- Compact mode ----------------------------------------------------------------------

    /// Ctrl+M, the player bar's button, the mini player's own: the window
    /// becomes the mini player, or the full view again.
    pub fn set_compact(self: &Rc<Self>, on: bool) {
        if on == self.on.get() {
            return;
        }
        let w = self.window();
        // The keyboard back to the window's shortcuts once the change is
        // through (it was on the view that goes away: Ctrl+M, Space… died).
        {
            let weak = w.as_weak();
            Timer::single_shot(Duration::from_millis(50), move || {
                if let Some(w) = weak.upgrade() {
                    let app = w.global::<crate::App>();
                    app.set_focus_request(app.get_focus_request() + 1);
                }
            });
        }
        self.ready.set(false);
        if on {
            let scale = w.window().scale_factor();
            let size = w.window().size();
            self.full.set(Some((size.width as f32 / scale, size.height as f32 / scale, w.window().is_maximized())));
            self.on.set(true);
            self.set_flag("ui/compactMode", true);
            self.apply_look();
            w.global::<Mini>().set_on(true);
            if w.window().is_maximized() {
                w.window().set_maximized(false);
                w.global::<WindowActions>().set_maximized(false);
            }
            self.refresh();
            self.fit(true);
            // The current track revealed once it is shown (its list is made with the mode).
            let weak = Rc::downgrade(self);
            Timer::single_shot(Duration::from_millis(150), move || {
                if let Some(m) = weak.upgrade() {
                    m.reveal(false);
                }
            });
            // Leaving the maximized state is asynchronous on some desktops: once more.
            let weak = Rc::downgrade(self);
            self.refit.start(TimerMode::SingleShot, Duration::from_millis(120), move || {
                if let Some(m) = weak.upgrade() {
                    m.refit_after_restore();
                }
            });
            let weak = Rc::downgrade(self);
            self.clock.start(TimerMode::Repeated, Duration::from_millis(250), move || {
                if let Some(m) = weak.upgrade() {
                    m.refresh();
                }
            });
        } else {
            self.on.set(false);
            self.refit.stop();
            self.settle.stop();
            self.clock.stop();
            self.set_flag("ui/compactMode", false);
            self.apply_look();
            w.global::<Mini>().set_on(false);
            let (fw, fh, max) = self.full.get().unwrap_or((1320.0, 840.0, false));
            w.window().set_size(slint::LogicalSize::new(fw.max(720.0), fh.max(480.0)));
            if max {
                w.window().set_maximized(true);
                w.global::<WindowActions>().set_maximized(true);
            }
        }
    }

    fn refit_after_restore(self: &Rc<Self>) {
        if !self.on.get() {
            return;
        }
        let w = self.window();
        if w.window().is_maximized() {
            w.window().set_maximized(false);
            let weak = Rc::downgrade(self);
            self.refit.start(TimerMode::SingleShot, Duration::from_millis(120), move || {
                if let Some(m) = weak.upgrade() {
                    m.refit_after_restore();
                }
            });
            return;
        }
        self.fit(true);
    }

    /// The look of the mode: the mini player's own appearance values over
    /// the full view's (appearanceCompact/values), or the full view's.
    fn apply_look(&self) {
        let w = self.window();
        let skins = w.global::<Skins>().get_list().row_count();
        let look = crate::theme::Look::load(&self.ctx.settings.borrow(), skins);
        crate::theme::apply(&w, &look);
        self.layout_columns();
    }

    /// The theme's density.
    fn ui(&self) -> f32 {
        let win = self.window();
        let t = win.global::<Theme>();
        if t.get_compact() {
            0.88
        } else if t.get_spacious() {
            1.14
        } else {
            1.0
        }
    }

    /// The window's height without the list at `width`.
    pub fn player_height(&self, width: f32) -> f32 {
        let u = self.ui();
        let wide = width >= 640.0;
        let inline = width >= 560.0;
        let display = ((if wide { 80.0 } else { 72.0 }) * u).round();
        let sh = (28.0 * u).round();
        let seek = 20.0;
        let toolbar = sh.max((32.0 * u).round()).max(28.0).max(if inline { seek } else { 0.0 });
        let bottom = 12.0 + display;
        let y = if inline { bottom + 12.0 } else { bottom + 6.0 + seek + 6.0 };
        (y + toolbar + 12.0).ceil()
    }

    fn row_height(&self) -> f32 {
        match self.window().global::<Theme>().get_list_density().as_str() {
            "list" => 22.0,
            "compact" => 26.0,
            "spacious" => 44.0,
            _ => 34.0,
        }
    }

    /// Fits compact mode: its width as it is now (entering, the one it
    /// had); with the list a few rows at least, else the height it had;
    /// without it just the player.
    fn fit(self: &Rc<Self>, entering: bool) {
        if !self.on.get() {
            return;
        }
        self.ready.set(false);
        let w = self.window();
        let scale = w.window().scale_factor();
        let now = w.window().size().width as f32 / scale;
        let width = if entering { (self.int("ui/compactWidth", 480) as f32).max(340.0) } else { now.max(340.0) };
        let player = self.player_height(width);
        let mini = w.global::<Mini>();
        if self.list_shown() {
            let least = player + 52.0 + 3.0 * self.row_height();
            mini.set_least_height(least);
            mini.set_most_height(least);
            let height = (self.int("ui/compactHeight", 460) as f32).max(least);
            w.window().set_size(slint::LogicalSize::new(width, height));
        } else {
            mini.set_least_height(player);
            mini.set_most_height(player);
            w.window().set_size(slint::LogicalSize::new(width, player));
        }
        let weak = Rc::downgrade(self);
        self.settle.start(TimerMode::SingleShot, Duration::from_millis(600), move || {
            if let Some(m) = weak.upgrade() {
                m.ready.set(true);
            }
        });
    }

    /// Resized: the mini player's size once the desktop has
    /// applied the one asked for (a late answer at the full view's size
    /// must not be kept as the mini player's).
    fn resized(&self, w: f32, h: f32) {
        if !self.on.get() || !self.ready.get() || self.window().window().is_maximized() {
            return;
        }
        self.set_int("ui/compactWidth", w.round() as i64);
        if self.list_shown() {
            self.set_int("ui/compactHeight", h.round() as i64);
        }
    }

    /// Ctrl+F while compact: the list opens with its search field.
    pub fn open_search(self: &Rc<Self>) {
        if !self.list_shown() {
            self.set_flag("ui/compactPlaylist", true);
            self.refresh_state();
            self.fit(false);
        }
        self.search_open.set(true);
        let win = self.window();
        let mini = win.global::<Mini>();
        mini.set_search_open(true);
    }

    fn close_search(&self) {
        self.search_open.set(false);
        self.search.borrow_mut().clear();
        let win = self.window();
        let mini = win.global::<Mini>();
        mini.set_search_open(false);
        mini.set_search_text(SharedString::default());
        self.rebuild();
    }

    // --- The player's state ----------------------------------------------------------------

    /// Everything, 4 times a second while shown (and after a change).
    pub fn refresh(&self) {
        if !self.on.get() {
            return;
        }
        self.refresh_state();
        self.refresh_display();
        // The list again when the queue, the track or the library changed.
        let sig = {
            let p = self.ctx.player.borrow();
            let q = p.queue();
            let lib = self.ctx.library.borrow();
            format!(
                "{}|{:?}|{}|{}|{}|{}|{}|{}",
                q.len(),
                q.current(),
                p.current_path().unwrap_or(""),
                q.paths().first().map_or("", |s| s.as_str()),
                q.paths().last().map_or("", |s| s.as_str()),
                lib.generation,
                lib.favorites.len(),
                self.source()
            )
        };
        if *self.signature.borrow() != sig {
            let track_changed = self.signature.borrow().split('|').nth(2) != sig.split('|').nth(2);
            *self.signature.borrow_mut() = sig;
            self.rebuild();
            if track_changed {
                self.reveal(false);
            }
        }
    }

    fn duration(&self) -> f64 {
        let p = self.ctx.player.borrow();
        let d = p.engine().duration();
        if d > 0.0 {
            return d;
        }
        let path = p.current_path().unwrap_or("");
        self.ctx.library.borrow().tracks.iter().find(|t| t.path == path).map_or(0.0, |t| t.duration)
    }

    /// The buttons' state, the volume, the window's options.
    fn refresh_state(&self) {
        let w = self.window();
        let mini = w.global::<Mini>();
        mini.set_list_shown(self.list_shown());
        mini.set_always_on_top(self.flag("ui/alwaysOnTop", false));
        mini.set_stop_button(self.flag("ui/stopButton", false));
        mini.set_volume_mode(self.int("playback/volumeMode", 2) as i32);
        let p = self.ctx.player.borrow();
        let gain = crate::volume::get(p.engine());
        mini.set_volume_fixed(crate::volume::fixed());
        mini.set_gain(gain as f32);
        mini.set_queue_empty(p.queue().is_empty());
        let gain_text = if gain > 0.0 {
            format!("{:.1} dB", 20.0 * gain.log10()).replace('-', "\u{2212}")
        } else {
            "\u{2212}\u{221e} dB".into()
        };
        mini.set_volume_tooltip(trf(CTX, "Volume · {0}", &[&gain_text]).into());
        let app = w.global::<App>();
        app.set_volume(volume_to_position(gain) as f32);
        app.set_muted(gain <= 0.0);
    }

    /// The display: the track, the stream, the place in the queue, the clock.
    fn refresh_display(&self) {
        let w = self.window();
        let mini = w.global::<Mini>();
        let p = self.ctx.player.borrow();
        let path = p.current_path().unwrap_or("").to_owned();
        let lib = self.ctx.library.borrow();
        let track = lib.tracks.iter().find(|t| t.path == path);
        let info = p.engine().info();
        mini.set_has_track(!path.is_empty());
        mini.set_title(
            if path.is_empty() {
                String::new()
            } else {
                track.map(|t| cell_text("title", t)).unwrap_or_else(|| readable(&path))
            }
            .into(),
        );
        mini.set_artist(track.map(|t| t.artist.clone()).unwrap_or_default().into());
        // Codec · rate · depth · bitrate.
        let mut parts: Vec<String> = Vec::new();
        match track {
            Some(t) => {
                parts.push(t.codec.split(' ').next().unwrap_or("").to_owned());
                for k in ["sampleRate", "bits", "bitrate"] {
                    parts.push(cell(k, t));
                }
            }
            None => {
                if let Some(i) = info {
                    parts.push(i.codec.clone());
                    parts.push(format!("{} kHz", crate::lib_shim::g4(i.rate as f64 / 1000.0)));
                    if i.bits > 0 {
                        parts.push(format!("{}-bit", i.bits));
                    }
                }
            }
        }
        parts.retain(|s| !s.is_empty() && s != "—");
        mini.set_stream(parts.join("  ·  ").into());
        let q = p.queue();
        mini.set_place(
            if q.is_empty() { String::new() } else { format!("{} / {}", q.current().map_or(0, |i| i + 1), q.len()) }
                .into(),
        );
        mini.set_channels(track.map_or_else(|| info.map_or(0, |i| i.channels as i64), |t| t.channels) as i32);
        drop(lib);
        drop(p);
        let p = self.ctx.player.borrow();
        let seconds = p.position();
        drop(p);
        let duration = self.duration();
        mini.set_seconds(seconds as f32);
        mini.set_duration(duration as f32);
        mini.set_progress(if duration > 0.0 { (seconds / duration).clamp(0.0, 1.0) as f32 } else { 0.0 });
    }

    /// A dB up or down; the bottom is silence.
    fn step_volume(&self, db: f64) {
        let p = self.ctx.player.borrow();
        let v = crate::audio::stepped_volume(crate::volume::get(p.engine()), db);
        crate::volume::change(&self.ctx.settings, p.engine(), v);
        drop(p);
        self.refresh_state();
        let _ = position_to_volume; // the slider goes through App.set-volume
    }

    // --- The list -------------------------------------------------------------------------

    fn full_view_keys(&self) -> Vec<String> {
        let keys: Vec<String> = serde_json::from_str::<Vec<String>>(&self.text("columns/keys", ""))
            .unwrap_or_default()
            .into_iter()
            .filter(|k| tracklist::column(k).is_some())
            .collect();
        if keys.is_empty() { DEFAULT_KEYS.iter().map(|s| s.to_string()).collect() } else { keys }
    }

    /// ui/compactColumns, in their order (else the full view's).
    fn column_keys(&self) -> Vec<String> {
        let keys: Vec<String> = serde_json::from_str::<Vec<String>>(&self.text("ui/compactColumns", ""))
            .unwrap_or_default()
            .into_iter()
            .filter(|k| tracklist::column(k).is_some())
            .collect();
        if keys.is_empty() { self.full_view_keys() } else { keys }
    }

    /// The rows for the source, the order and the search (TrackListModel for
    /// the queue, filtered for a folder and its subfolders).
    /// The folder tree, built once per library change.
    fn tree(&self) -> Rc<Vec<FolderNode>> {
        let lib = self.ctx.library.borrow();
        if let Some((generation, tree)) = &*self.tree_cache.borrow()
            && *generation == lib.generation
        {
            return tree.clone();
        }
        let tree = Rc::new(folder_tree(&lib));
        *self.tree_cache.borrow_mut() = Some((lib.generation, tree.clone()));
        tree
    }

    fn rebuild(&self) {
        let queue = self.showing_queue();
        let source = self.source();
        let search = self.search.borrow().to_lowercase();
        let p = self.ctx.player.borrow();
        let lib = self.ctx.library.borrow();
        // The library's index and lower-case keys (a map of every path was
        // built, and 5 fields lower-cased per track, on each key typed).
        let matches = |path: &str| {
            if search.is_empty() {
                return true;
            }
            match lib.index_of(path) {
                Some(i) => {
                    lib.search_keys.get(i).is_some_and(|k| k.contains(&search))
                        || lib.tracks[i].genre.to_lowercase().contains(&search)
                }
                None => path.to_lowercase().contains(&search),
            }
        };
        let entries: Vec<Entry> = if queue {
            p.queue()
                .paths()
                .iter()
                .enumerate()
                .filter(|(_, path)| matches(path))
                .map(|(i, path)| Entry { path: path.clone(), index: i })
                .collect()
        } else {
            let prefix = format!("{source}/");
            let mut list: Vec<usize> = (0..lib.tracks.len())
                .filter(|&i| lib.tracks[i].path.starts_with(&prefix) && matches(&lib.tracks[i].path))
                .collect();
            let field = self.text("ui/compactSort", "path");
            let desc = self.flag("ui/compactSortDesc", false);
            list.sort_by(|&a, &b| {
                let (ta, tb) = (&lib.tracks[a], &lib.tracks[b]);
                let o = compare(&field, (&ta.path, Some(ta)), (&tb.path, Some(tb)), &lib.favorites);
                let o = if desc { o.reverse() } else { o };
                o.then_with(|| natural_cmp(&ta.path.to_lowercase(), &tb.path.to_lowercase()))
            });
            list.into_iter()
                .enumerate()
                .map(|(row, i)| Entry { path: lib.tracks[i].path.clone(), index: row })
                .collect()
        };
        {
            let mut d = self.rows.data.borrow_mut();
            d.entries = entries;
            d.queue = queue;
            d.current_index = p.queue().current();
            d.current_path = p.current_path().unwrap_or("").to_owned();
        }
        drop(lib);
        drop(p);
        let w = self.window();
        let mini = w.global::<Mini>();
        mini.set_showing_queue(queue);
        mini.set_source_name(self.source_name().into());
        let sort = self.text("ui/compactSort", "path");
        let desc = self.flag("ui/compactSortDesc", false);
        mini.set_sort_key(sort.clone().into());
        mini.set_sort_desc(!queue && desc);
        mini.set_sort_active(!queue && (sort != "path" || desc));
        let fields: Vec<MiniChoice> = SORT_FIELDS
            .iter()
            .map(|f| MiniChoice {
                key: (*f).into(),
                title: field_title(f).into(),
                on: !queue && sort == *f,
                locked: false,
            })
            .collect();
        mini.set_sort_fields(ModelRc::new(VecModel::from(fields)));
        let keys = self.column_keys();
        let choices: Vec<MiniChoice> = tracklist::CATALOG
            .iter()
            .map(|c| {
                let on = keys.iter().any(|k| k == c.key);
                MiniChoice {
                    key: c.key.into(),
                    title: tr("TrackColumns", c.title).into(),
                    on,
                    locked: on && keys.len() == 1,
                }
            })
            .collect();
        mini.set_column_choices(ModelRc::new(VecModel::from(choices)));
        mini.set_search_placeholder(
            if queue { tr(CTX, "Search the queue…") } else { tr(CTX, "Search in this folder…") }.into(),
        );
        mini.set_rows_total(self.rows.data.borrow().entries.len() as i32);
        self.layout_columns();
        self.update_window();
        self.rows.notify.reset();
        self.refresh_empty();
        self.refresh_folders();
    }

    /// The rows in sight for the scroll position and the viewport's height
    /// (with one more at each end).
    fn update_window(&self) {
        let rh = self.row_height();
        let (_, vh) = self.viewport.get();
        let total = self.rows.data.borrow().entries.len();
        let first = ((self.scroll_y.get() / rh).floor().max(0.0) as usize).saturating_sub(1).min(total);
        let count = (vh / rh).ceil() as usize + 3;
        let changed = {
            let mut d = self.rows.data.borrow_mut();
            let changed = d.first != first || d.count != count;
            d.first = first;
            d.count = count;
            changed
        };
        if changed {
            self.window().global::<Mini>().set_first_row(first as i32);
            self.rows.notify.reset();
        }
    }

    /// The way to add music, or why the list is empty.
    fn refresh_empty(&self) {
        let win = self.window();
        let mini = win.global::<Mini>();
        let none = self.rows.data.borrow().entries.is_empty();
        let library_empty = self.ctx.library.borrow().tracks.is_empty();
        let search = self.search.borrow().clone();
        mini.set_empty_library(none && library_empty && search.is_empty());
        let text = if !none {
            String::new()
        } else if !search.is_empty() {
            trf(CTX, "No tracks match “{0}”", &[&search])
        } else if self.showing_queue() {
            tr(CTX, "The queue is empty.")
        } else {
            tr(CTX, "No tracks in this folder.")
        };
        mini.set_empty_text(text.into());
    }

    /// The chosen columns that fit, in their order —
    /// the first (at least 140 px) and the time first, then the others while
    /// they fit; the short ones keep a width, the text ones share the rest
    /// (the first a little more).
    fn layout_columns(&self) {
        let w = self.window();
        let mini = w.global::<Mini>();
        let (vw, vh) = self.viewport.get();
        let list_w = vw + 8.0; // the list's frame: the rows' viewport is 4 px in
        let rh = self.row_height();
        let count = self.rows.data.borrow().entries.len();
        let queue_len = self.ctx.player.borrow().queue().len();
        let longest = count.max(if self.showing_queue() { queue_len } else { 0 });
        let digits = longest.to_string().len();
        let number_w =
            22f32.max((crate::metrics::width(&format!("{}.", "8".repeat(digits)), 11.0, 400, 0.0)).ceil() + 2.0);
        let cover = if w.global::<Theme>().get_list_covers() { rh - 8.0 } else { 0.0 };
        let scrollable = count as f32 * rh > vh + 0.5;
        mini.set_scrollable(scrollable);
        mini.set_number_width(number_w);
        let space = list_w
            - 8.0
            - 12.0
            - number_w
            - GAP
            - if cover > 0.0 { cover + GAP } else { 0.0 }
            - if scrollable { 12.0 } else { 0.0 };
        let keys = self.column_keys();
        let mut shown: Vec<String> = Vec::new();
        let mut widths: HashMap<String, f32> = HashMap::new();
        if let Some(first) = keys.first().cloned() {
            let mut order = vec![first.clone()];
            if keys.iter().any(|k| k == "duration") && first != "duration" {
                order.push("duration".into());
            }
            order.extend(keys.iter().filter(|k| **k != first && *k != "duration").cloned());
            let mut chosen = std::collections::HashSet::new();
            let mut used = 0.0;
            for k in &order {
                let wk = fixed_width(k).unwrap_or(if *k == first { 140.0 } else { 90.0 }) + GAP;
                if *k != first && used + wk > space + GAP {
                    continue;
                }
                chosen.insert(k.clone());
                used += wk;
            }
            shown = keys.iter().filter(|k| chosen.contains(*k)).cloned().collect();
            let (mut fixed_sum, mut weights) = (0.0, 0.0);
            for k in &shown {
                match fixed_width(k) {
                    Some(f) => fixed_sum += f,
                    None => weights += if weights == 0.0 { 1.5 } else { 1.0 },
                }
            }
            let rest = (space - fixed_sum - GAP * (shown.len() as f32 - 1.0)).max(0.0);
            let mut first_text = true;
            for k in &shown {
                let wk = match fixed_width(k) {
                    Some(f) => f,
                    None => {
                        let v = (rest * if first_text { 1.5 } else { 1.0 } / weights).floor();
                        first_text = false;
                        v
                    }
                };
                widths.insert(k.clone(), wk);
            }
        }
        // Their left edges.
        let mut x = 6.0 + number_w + GAP + if cover > 0.0 { cover + GAP } else { 0.0 };
        let mut columns = Vec::new();
        let mut favorite_x = -1.0;
        for k in &shown {
            let wk = widths[k];
            if k == "favorite" {
                favorite_x = x + wk / 2.0;
            }
            columns.push(MiniColumn { key: k.as_str().into(), title: field_title(k).into(), x, width: wk });
            x += wk + GAP;
        }
        mini.set_favorite_x(favorite_x);
        let changed = {
            let d = self.rows.data.borrow();
            d.shown != shown
        };
        self.rows.data.borrow_mut().shown = shown;
        *self.columns.borrow_mut() = columns.clone();
        mini.set_columns(ModelRc::new(VecModel::from(columns)));
        if changed {
            self.rows.notify.reset();
        }
    }

    fn move_column(&self, key: &str, target: i32) {
        let shown: Vec<String> = self.columns.borrow().iter().map(|c| c.key.to_string()).collect();
        let Some(from) = shown.iter().position(|k| k == key) else { return };
        if target < 0 || target as usize == from || target as usize == from + 1 {
            return;
        }
        let target = target as usize;
        let mut k = self.column_keys();
        k.retain(|x| x != key);
        let at = if target < shown.len() {
            k.iter().position(|x| *x == shown[target]).unwrap_or(k.len())
        } else {
            shown.last().and_then(|l| k.iter().position(|x| x == l)).map_or(k.len(), |i| i + 1)
        };
        k.insert(at.min(k.len()), key.to_owned());
        self.set_text("ui/compactColumns", &serde_json::to_string(&k).unwrap_or_default());
        self.rebuild();
    }

    /// Sorting: the queue is put in order; a folder keeps its order
    /// as a setting (favourites first).
    fn sort_by(&self, field: &str) {
        if self.showing_queue() {
            let changes = {
                let mut p = self.ctx.player.borrow_mut();
                let lib = self.ctx.library.borrow();
                let track = |p: &str| lib.index_of(p).and_then(|i| lib.tracks.get(i));
                let paths: Vec<String> = p.queue().paths().to_vec();
                let desc = field == "favorite";
                let mut order: Vec<usize> = (0..paths.len()).collect();
                order.sort_by(|&a, &b| {
                    let (pa, pb) = (paths[a].as_str(), paths[b].as_str());
                    let o = compare(field, (pa, track(pa)), (pb, track(pb)), &lib.favorites);
                    let o = if desc { o.reverse() } else { o };
                    o.then_with(|| natural_cmp(&pa.to_lowercase(), &pb.to_lowercase()))
                });
                drop(lib);
                p.apply_order(&order)
            };
            self.after_changes(changes);
        } else {
            self.set_text("ui/compactSort", field);
            self.set_flag("ui/compactSortDesc", field == "favorite");
            self.rebuild();
        }
    }

    fn play_row(&self, row: i32) {
        if row < 0 {
            return;
        }
        let (queue, index, paths) = {
            let d = self.rows.data.borrow();
            let Some(e) = d.entries.get(row as usize) else { return };
            (d.queue, e.index, d.entries.iter().map(|e| e.path.clone()).collect::<Vec<_>>())
        };
        // The queue plays from its place; a folder its rows (the matches only, while searching).
        let changes = if queue {
            self.ctx.player.borrow_mut().play_index(index)
        } else {
            self.ctx.player.borrow_mut().play_paths(paths, row as usize)
        };
        self.after_changes(changes);
    }

    fn toggle_favorite(&self, row: usize) {
        let Some(path) = self.rows.data.borrow().entries.get(row).map(|e| e.path.clone()) else { return };
        self.ctx.library.borrow_mut().toggle_favorite(&path);
        let first = self.rows.data.borrow().first;
        if row >= first {
            self.rows.notify.row_changed(row - first);
        }
    }

    // --- Folders ---------------------------------------------------------------------------

    /// How deep the source menu goes (0: no subfolders).
    fn folder_levels(&self, tree: &[FolderNode]) -> i32 {
        let deepest = tree.iter().map(|n| n.depth).max().unwrap_or(0);
        if self.flag("ui/compactSubfolders", true) {
            1.max((self.int("ui/compactFolderDepth", 2) as i32).min(deepest))
        } else {
            0
        }
    }

    fn playing_folder(&self, shown: &[FolderNode]) -> String {
        let path = self.ctx.player.borrow().current_path().unwrap_or("").to_owned();
        shown
            .iter()
            .filter(|f| path.starts_with(&format!("{}/", f.path)))
            .max_by_key(|f| f.depth)
            .map(|f| f.path.clone())
            .unwrap_or_default()
    }

    fn source_name(&self) -> String {
        if self.showing_queue() {
            return tr(CTX, "Play queue");
        }
        let source = self.source();
        let tree = self.tree();
        tree.iter()
            .find(|f| f.path == source)
            .map(|f| f.label.clone())
            .unwrap_or_else(|| source.rsplit('/').next().unwrap_or("").to_owned())
    }

    /// The source menu's tree; a source no longer in the library goes back
    /// to the queue.
    fn refresh_folders(&self) {
        let tree = self.tree();
        let source = self.source();
        if !source.is_empty() && !tree.iter().any(|f| f.path == source) {
            self.set_text("ui/compactSource", "");
            self.rebuild();
            return;
        }
        let levels = self.folder_levels(&tree);
        let shown: Vec<FolderNode> = tree.iter().filter(|n| n.depth <= levels).cloned().collect();
        let playing = self.playing_folder(&shown);
        let rows: Vec<MiniFolder> = shown
            .iter()
            .map(|n| {
                let selected = n.path == source;
                MiniFolder {
                    path: n.path.as_str().into(),
                    name: n.name.as_str().into(),
                    depth: n.depth,
                    count: n.total.to_string().into(),
                    last: n.last,
                    selected,
                    holds_selected: !selected && source.starts_with(&format!("{}/", n.path)),
                    playing_here: n.path == playing,
                    holds_playing: n.path != playing && playing.starts_with(&format!("{}/", n.path)),
                }
            })
            .collect();
        let win = self.window();
        let mini = win.global::<Mini>();
        mini.set_folders(ModelRc::new(VecModel::from(rows)));
        mini.set_subfolders(self.flag("ui/compactSubfolders", true));
        mini.set_folder_levels(levels);
        // The levels to choose from: as many as the library has.
        mini.set_folder_depth(self.tree().iter().map(|n| n.depth).max().unwrap_or(0).max(1));
    }

    // --- Going to the playing track ---------------------------------------------------------

    fn current_row(&self) -> Option<usize> {
        let d = self.rows.data.borrow();
        if d.queue {
            d.entries.iter().position(|e| Some(e.index) == d.current_index)
        } else {
            d.entries.iter().position(|e| !d.current_path.is_empty() && e.path == d.current_path)
        }
    }

    /// The playing row in view (centred, or just shown).
    fn reveal(&self, centre: bool) {
        let win = self.window();
        let mini = win.global::<Mini>();
        if let Some(r) = self.current_row() {
            mini.set_reveal_row(r as i32);
            mini.set_reveal_centre(centre);
            mini.set_reveal_serial(mini.get_reveal_serial() + 1);
        }
    }

    /// The playing track in the middle of the list;
    /// not in it, the search is cleared, then the list goes to its folder.
    fn show_playing(&self) {
        if self.ctx.player.borrow().current_path().unwrap_or("").is_empty() {
            return;
        }
        if self.current_row().is_none() && self.search_open.get() && !self.search.borrow().is_empty() {
            self.close_search();
        }
        if self.current_row().is_none() {
            let tree = self.tree();
            let levels = self.folder_levels(&tree);
            let shown: Vec<FolderNode> = tree.iter().filter(|n| n.depth <= levels).cloned().collect();
            let folder = self.playing_folder(&shown);
            self.set_text("ui/compactSource", &folder);
            self.rebuild();
        }
        self.reveal(true);
    }
}
