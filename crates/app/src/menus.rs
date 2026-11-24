//! The menus, the questions in the window (asking a name, confirming) and
//! the signal path popup: laid out here, drawn by ui/menus.slint and
//! ui/dialogs.slint. A child module of
//! the controller: the actions reach its state.

use std::collections::HashMap;
use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use super::Controller;
use crate::i18n::{tr, trf};
use crate::{App, Dialogs, MenuEntry, Menus};

/// What a menu row does when picked.
pub type Action = Rc<dyn Fn(&Rc<Controller>)>;
/// What a name dialog does with the name (trimmed, never empty).
pub type NameAction = Rc<dyn Fn(&Rc<Controller>, String)>;

/// A row of a menu being built.
#[derive(Clone, Default)]
pub struct Entry {
    pub text: String,
    pub separator: bool,
    pub checkable: bool,
    pub checked: bool,
    pub enabled: bool,
    pub submenu: bool,
    pub action: Option<Action>,
    /// A subfolder levels row: (shown, value, maximum) and its controls'
    /// actions (fewer, more, the switch), which keep the menu open.
    pub levels: Option<(bool, i32, i32)>,
    pub controls: Vec<(&'static str, Action)>,
}

impl Entry {
    pub fn item(text: impl Into<String>, action: impl Fn(&Rc<Controller>) + 'static) -> Entry {
        Entry { text: text.into(), enabled: true, action: Some(Rc::new(action)), ..Default::default() }
    }
    pub fn check(text: impl Into<String>, checked: bool, action: impl Fn(&Rc<Controller>) + 'static) -> Entry {
        Entry { checkable: true, checked, ..Entry::item(text, action) }
    }
    pub fn separator() -> Entry {
        Entry { separator: true, enabled: true, ..Default::default() }
    }
    pub fn submenu(text: impl Into<String>) -> Entry {
        Entry { text: text.into(), enabled: true, submenu: true, ..Default::default() }
    }
    /// "Subfolders  − N +  [switch]".
    pub fn levels(
        text: impl Into<String>,
        on: bool,
        value: i32,
        maximum: i32,
        fewer: impl Fn(&Rc<Controller>) + 'static,
        more: impl Fn(&Rc<Controller>) + 'static,
        toggle: impl Fn(&Rc<Controller>) + 'static,
    ) -> Entry {
        Entry {
            text: text.into(),
            enabled: true,
            levels: Some((on, value, maximum)),
            controls: vec![("-", Rc::new(fewer)), ("+", Rc::new(more)), ("t", Rc::new(toggle))],
            ..Default::default()
        }
    }
    pub fn enabled(mut self, on: bool) -> Entry {
        self.enabled = on;
        self
    }
}

/// A menu: its rows, the rows of its one sub-menu, its least width.
#[derive(Default)]
pub struct Menu {
    pub entries: Vec<Entry>,
    pub sub: Vec<Entry>,
    pub min_width: f32,
}

impl Menu {
    pub fn push(&mut self, e: Entry) {
        self.entries.push(e);
    }
    pub fn separator(&mut self) {
        self.entries.push(Entry::separator());
    }
}

/// Where a menu goes (the point asked for and the place it is moved to when
/// it does not fit).
#[derive(Clone, Copy, Debug)]
pub enum Place {
    /// Its corner at the pointer (a context menu); flipped to the left /
    /// upwards when it would leave the window.
    Pointer(f32, f32),
    /// Its corner here (under a button).
    At(f32, f32),
    /// Its right edge at `right`, its top at `y`.
    RightAt(f32, f32),
    /// Centred over a button (x, width), 6 px above `top`.
    Above { x: f32, width: f32, top: f32 },
}

// The menus' measurements.
const PADDING: f32 = 5.0;
const SIDE: f32 = 10.0;
const CHECK: f32 = 16.0;
const SPACING: f32 = 8.0;
const ARROW: f32 = 20.0;
const LEAST: f32 = 220.0 - 2.0 * PADDING;
const SEPARATOR: f32 = 4.0 + 1.0 + 4.0;

/// The pending actions of the menu and dialog shown.
#[derive(Default)]
pub struct Pending {
    pub menu: HashMap<String, Action>,
    pub name: Option<NameAction>,
    pub confirm: Option<Action>,
}

impl Controller {
    fn row_height(&self) -> f32 {
        let w = self.window();
        let t = w.global::<crate::Theme>();
        if t.get_compact() {
            28.0
        } else if t.get_spacious() {
            36.0
        } else {
            32.0
        }
    }

    /// The window's size in logical pixels.
    fn window_size(&self) -> (f32, f32) {
        let w = self.window();
        let size = w.window().size();
        let scale = w.window().scale_factor().max(0.1);
        (size.width as f32 / scale, size.height as f32 / scale)
    }

    /// A row's width, the menu's width and height.
    fn menu_size(&self, entries: &[Entry], min_width: f32) -> (f32, f32) {
        let row = self.row_height();
        let mut width = LEAST;
        let mut height = 2.0 * PADDING;
        for e in entries {
            if e.separator {
                height += SEPARATOR;
                continue;
            }
            height += row;
            let text = e.text.split('\t').next().unwrap_or("");
            let weight = if e.checkable && e.checked { 600 } else { 400 };
            let mut w = SIDE + crate::metrics::width(text, 13.0, weight, 0.0).round() + SIDE;
            if e.levels.is_some() {
                w += 100.0; // − N + and the switch
            }
            if e.checkable {
                w += CHECK + SPACING;
            }
            if e.submenu {
                w += ARROW;
            }
            width = width.max(w);
        }
        ((width + 2.0 * PADDING).max(min_width), height)
    }

    fn entries_model(
        &self,
        entries: &[Entry],
        prefix: &str,
        pending: &mut HashMap<String, Action>,
    ) -> ModelRc<MenuEntry> {
        let rows: Vec<MenuEntry> = entries
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let id = format!("{prefix}{i}");
                if let Some(a) = &e.action {
                    pending.insert(id.clone(), a.clone());
                }
                for (suffix, a) in &e.controls {
                    pending.insert(format!("{id}{suffix}"), a.clone());
                }
                let (on, value, maximum) = e.levels.unwrap_or((false, 0, 0));
                MenuEntry {
                    id: id.into(),
                    text: e.text.split('\t').next().unwrap_or("").into(),
                    separator: e.separator,
                    checkable: e.checkable,
                    checked: e.checked,
                    enabled: e.enabled,
                    submenu: e.submenu,
                    levels: e.levels.is_some(),
                    on,
                    value,
                    maximum,
                }
            })
            .collect();
        ModelRc::new(VecModel::from(rows))
    }

    /// Shows `menu` at `place`.
    pub fn show_menu(&self, menu: Menu, place: Place) {
        if menu.entries.is_empty() {
            return;
        }
        let (w, h) = self.menu_size(&menu.entries, menu.min_width);
        let (win_w, win_h) = self.window_size();
        let (mut x, mut y) = match place {
            Place::Pointer(x, y) | Place::At(x, y) => (x, y),
            Place::RightAt(right, y) => (right - w, y),
            Place::Above { x, width, top } => (x + ((width - w) / 2.0).trunc(), top - h - 6.0),
        };
        if x + w > win_w {
            x = if matches!(place, Place::Pointer(..)) && x - w >= 0.0 { x - w } else { win_w - w };
        }
        if y + h > win_h {
            y = if matches!(place, Place::Pointer(..)) && y - h >= 0.0 { y - h } else { win_h - h };
        }
        let (x, y) = (x.max(0.0).round(), y.max(0.0).round());
        let mut pending = HashMap::new();
        let items = self.entries_model(&menu.entries, "m", &mut pending);
        let sub = self.entries_model(&menu.sub, "s", &mut pending);
        let window = self.window();
        let m = window.global::<Menus>();
        m.set_x(x);
        m.set_y(y);
        m.set_width(w);
        m.set_items(items);
        m.set_sub_items(sub);
        m.set_sub_open(false);
        // The sub-menu: its first row level with the row that opens it, its
        // rows starting where that row ends.
        if let Some(at) = menu.entries.iter().position(|e| e.submenu) {
            let row = self.row_height();
            let top: f32 =
                PADDING + menu.entries[..at].iter().map(|e| if e.separator { SEPARATOR } else { row }).sum::<f32>();
            let (sw, sh) = self.menu_size(&menu.sub, 0.0);
            let mut sx = x + w - 2.0 * PADDING;
            if sx + sw > win_w {
                sx = x - sw + 2.0 * PADDING;
            }
            let sy = (y + top - PADDING).min(win_h - sh).max(0.0);
            m.set_sub_x(sx.max(0.0));
            m.set_sub_y(sy);
            m.set_sub_width(sw);
        }
        self.ui.borrow_mut().pending.menu = pending;
        m.set_open(true);
    }

    pub fn close_menu(&self) {
        let w = self.window();
        let m = w.global::<Menus>();
        m.set_open(false);
        m.set_sub_open(false);
        m.set_items(ModelRc::default());
        m.set_sub_items(ModelRc::default());
        self.ui.borrow_mut().pending.menu.clear();
        // The menu's FocusScope had the keyboard: back to the window's
        // shortcuts, or Space, Ctrl+M… stop working until a row is clicked.
        restore_keys(&w);
    }

    /// The anchor the button that asked for a menu left in Menus.
    pub fn anchor(&self) -> (f32, f32, f32, f32) {
        let w = self.window();
        let m = w.global::<Menus>();
        (m.get_anchor_x(), m.get_anchor_y(), m.get_anchor_width(), m.get_anchor_height())
    }

    // --- Questions in the window -----------------------------------------------------------

    /// Asks a name: the name typed, trimmed, never empty.
    pub fn ask_name(
        &self,
        title: &str,
        placeholder: &str,
        initial: &str,
        accept: &str,
        note: &str,
        done: impl Fn(&Rc<Controller>, String) + 'static,
    ) {
        let w = self.window();
        let d = w.global::<Dialogs>();
        d.set_title(title.into());
        d.set_placeholder(placeholder.into());
        d.set_value(initial.into());
        d.set_accept_text(accept.into());
        d.set_note(note.into());
        d.set_text(SharedString::default());
        let mut ui = self.ui.borrow_mut();
        ui.pending.name = Some(Rc::new(done));
        ui.pending.confirm = None;
        drop(ui);
        d.set_kind("name".into());
    }

    /// Confirms: Cancel or the action.
    pub fn confirm(&self, title: &str, text: &str, accept: &str, done: impl Fn(&Rc<Controller>) + 'static) {
        let w = self.window();
        let d = w.global::<Dialogs>();
        d.set_title(title.into());
        d.set_text(text.into());
        d.set_accept_text(accept.into());
        d.set_note(SharedString::default());
        d.set_value(SharedString::default());
        let mut ui = self.ui.borrow_mut();
        ui.pending.confirm = Some(Rc::new(done));
        ui.pending.name = None;
        drop(ui);
        d.set_kind("confirm".into());
    }

    fn close_dialog(&self) {
        let w = self.window();
        w.global::<Dialogs>().set_kind(SharedString::default());
        let mut ui = self.ui.borrow_mut();
        ui.pending.name = None;
        ui.pending.confirm = None;
    }

    /// A new playlist: a name, then
    /// the playlist with `paths` in it, opened.
    pub fn new_playlist(&self, paths: Vec<String>) {
        let n = paths.len();
        let note =
            if paths.is_empty() { String::new() } else { trf("Main", "With {0}.", &[&crate::views::tracks_text(n)]) };
        self.ask_name(
            &tr("Main", "New playlist"),
            &tr("Main", "Playlist name"),
            "",
            &tr("Main", "Create"),
            &note,
            move |c, name| {
                let made = c.ui.borrow_mut().playlists.create(&name);
                if !paths.is_empty() {
                    c.ui.borrow_mut().playlists.add_tracks(&made, &paths);
                    c.show_message(&trf(
                        "Main",
                        "Added {0} to “{1}”",
                        &[&crate::views::tracks_text(paths.len()), &made],
                    ));
                }
                c.open("playlist", &made);
            },
        );
    }

    pub(super) fn connect_menus(self: &Rc<Self>, window: &crate::MainWindow) {
        let menus = window.global::<Menus>();
        let weak = Rc::downgrade(self);
        menus.on_activated(move |id| {
            let Some(c) = weak.upgrade() else { return };
            let action = c.ui.borrow().pending.menu.get(id.as_str()).cloned();
            c.close_menu();
            if let Some(a) = action {
                a(&c);
            }
        });
        let weak = Rc::downgrade(self);
        menus.on_kept(move |id| {
            let Some(c) = weak.upgrade() else { return };
            let action = c.ui.borrow().pending.menu.get(id.as_str()).cloned();
            if let Some(a) = action {
                a(&c);
            }
        });
        let weak = Rc::downgrade(self);
        menus.on_dismissed(move || {
            if let Some(c) = weak.upgrade() {
                c.close_menu();
            }
        });
        let weak = Rc::downgrade(self);
        menus.on_path_dismissed(move || {
            if let Some(c) = weak.upgrade() {
                let w = c.window();
                w.global::<Menus>().set_path_open(false);
                restore_keys(&w);
            }
        });
        let dialogs = window.global::<Dialogs>();
        dialogs.on_valid(|text| !text.trim().is_empty());
        let weak = Rc::downgrade(self);
        dialogs.on_accepted(move |text| {
            let Some(c) = weak.upgrade() else { return };
            let (name, confirm) = {
                let ui = c.ui.borrow();
                (ui.pending.name.clone(), ui.pending.confirm.clone())
            };
            let text = text.trim().to_owned();
            if name.is_some() && text.is_empty() {
                return;
            }
            c.close_dialog();
            if let Some(f) = name {
                f(&c, text);
            } else if let Some(f) = confirm {
                f(&c);
            }
        });
        let weak = Rc::downgrade(self);
        dialogs.on_cancelled(move || {
            if let Some(c) = weak.upgrade() {
                c.close_dialog();
            }
        });
    }
}

/// The keyboard back to the main window's shortcuts (main.slint `keys`).
fn restore_keys(w: &crate::MainWindow) {
    let app = w.global::<App>();
    app.set_focus_request(app.get_focus_request() + 1);
}
