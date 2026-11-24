//! The main window's state and the player: the page shown,
//! its list, the sidebar, the player and its timer, the session kept
//! between runs.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Duration;

use slint::{ComponentHandle, Model, ModelRc, SharedString, Timer, TimerMode, VecModel};
use sonkore_core::paths;
use sonkore_core::settings::Settings;
use sonkore_engine::{Player, Repeat};

use crate::context::Ctx;
use crate::i18n::{tr, trf};
use crate::icons::nav_icon;
use crate::lib_shim::Library;
use crate::tracklist::{self, DEFAULT_KEYS, TrackListModel};
use crate::{App, FolderRow, MainWindow, NavItem, PlaylistRow};

// The pages, menus and dialogs: child modules, so they reach the state here.
#[path = "menus.rs"]
mod menus;
#[path = "pages.rs"]
mod pages;

const VOLUME_RANGE_DB: f64 = 60.0;

/// The slider's position for a gain: linear in dB over 60 dB.
pub fn volume_to_position(gain: f64) -> f64 {
    if gain <= 0.0 { 0.0 } else { (1.0 + 20.0 * gain.log10() / VOLUME_RANGE_DB).clamp(0.0, 1.0) }
}

/// The gain for a slider position: whole tenths of a dB, exactly 1 at the top.
pub fn position_to_volume(position: f64) -> f64 {
    if position <= 0.0 {
        return 0.0;
    }
    if position >= 0.999 {
        return 1.0;
    }
    let db = ((position - 1.0) * VOLUME_RANGE_DB * 10.0).round() / 10.0;
    10f64.powf(db / 20.0)
}

pub struct Controller {
    window: slint::Weak<MainWindow>,
    pub ctx: Rc<Ctx>,
    settings: Rc<RefCell<Settings>>,
    library: Rc<RefCell<Library>>,
    list: Rc<TrackListModel>,
    player: Rc<RefCell<Player>>,
    page: RefCell<(String, String)>,
    search: RefCell<String>,
    expanded: RefCell<std::collections::HashSet<String>>,
    list_width: RefCell<f32>,
    unmuted: RefCell<f64>,
    anchor: RefCell<Option<usize>>,
    /// The last live seek (one per 45 ms while the bar is dragged).
    last_seek: std::cell::Cell<Option<std::time::Instant>>,
    /// The list's current row (the keyboard moves it).
    pub(super) cursor: std::cell::Cell<Option<usize>>,
    /// Itself, for what answers later (the file dialogs).
    me: RefCell<std::rc::Weak<Controller>>,
    /// The track list's columns as shown (kept, see layout_columns).
    columns_model: Rc<VecModel<crate::Column>>,
    /// The sidebar folder last clicked, and when (a second click folds it).
    last_folder_click: RefCell<Option<(String, std::time::Instant)>>,
    last_path: RefCell<String>,
    message_timer: Timer,
    poll_timer: Timer,
    session_dirty: RefCell<bool>,
    mpris: sonkore_platform::Mpris,
    /// The playing track as MPRIS shows it, built once per track (the
    /// 50 ms poll used to search the whole library every time).
    mpris_track: RefCell<Option<sonkore_platform::TrackInfo>>,
    /// session.json as last written (compared here, not read back from disk).
    session_written: RefCell<String>,
    spectrum: RefCell<crate::spectrum::Spectrum>,
    visual: RefCell<crate::spectrum::View>,
    commands: std::sync::mpsc::Receiver<sonkore_platform::Command>,
    ui: RefCell<pages::PageState>,
    /// Samples the quantizer clipped in this track (the signal path's warning).
    clipped: std::cell::Cell<u64>,
    /// The tags of the file playing when it is not in the library (for ReplayGain).
    loose_track: RefCell<Option<(String, Option<crate::lib_shim::Track>)>>,
    /// A test signal playing (Settings › Audio › Test signals) and what it interrupted.
    test_signal: RefCell<Option<TestSignalRun>>,
    /// Since when a DAC played to directly has been open without playing.
    idle_since: std::cell::Cell<Option<std::time::Instant>>,
    /// Since when the settings have changes not on disk yet.
    unsaved_since: std::cell::Cell<Option<std::time::Instant>>,
}

impl Controller {
    pub fn new(window: &MainWindow, settings: Settings) -> Rc<Controller> {
        let library = Rc::new(RefCell::new(Library::load()));
        // ReplayGain on: the tracks without its tags are measured in the background.
        if settings.int("playback/replayGain", 0) != 0 {
            library.borrow_mut().with(|l| l.set_auto_analyze_loudness(true));
        }
        let list = TrackListModel::new(library.clone());
        let mut engine = crate::audio::engine();
        // Offscreen captures never reach a real sound card (SONKORE_SCREENSHOT_DEVICE:
        // the output to show, for the volume's look; nothing plays offscreen).
        let device = if std::env::var_os("SONKORE_SCREENSHOT").is_some() {
            std::env::var("SONKORE_SCREENSHOT_DEVICE").unwrap_or_else(|_| "null".to_owned())
        } else {
            settings.text("playback/device", "")
        };
        engine.set_device(&device);
        engine.set_follow_rate(settings.flag("playback/followTrackRate", true));
        engine.set_buffer_ms(settings.int("playback/bufferMs", 150) as u32);
        let digital = settings.number("playback/volume", 1.0).clamp(0.0, 1.0);
        engine.set_volume(digital);
        crate::volume::set_digital(digital);
        // The equalizer's state first: the auto headroom follows its curve.
        let equalizer = Rc::new(RefCell::new(sonkore_eq::controller::EqController::with_default_paths(
            engine.dsp().eq.clone(),
            &settings,
        )));
        crate::audio::apply(&settings, &mut engine);
        engine.set_analysis(settings.text("ui/visualizer", "bars") != "off");
        // Where the volume is set (the DAC's control, the sink's, the engine's).
        if std::env::var_os("SONKORE_SCREENSHOT").is_none() || std::env::var_os("SONKORE_SCREENSHOT_DEVICE").is_some() {
            crate::volume::update(&settings, &engine);
        }
        let mut player = Player::new(engine);
        player.set_shuffle(settings.flag("playback/shuffle", false));
        player.set_repeat(Repeat::from_index(settings.int("playback/repeat", 0)));
        let settings = Rc::new(RefCell::new(settings));
        let player = Rc::new(RefCell::new(player));
        crate::audio::set_player(&player);
        let weak = window.as_weak();
        let ctx = Rc::new(Ctx {
            settings: settings.clone(),
            player: player.clone(),
            library: library.clone(),
            equalizer,
            notify: Box::new(move |text: &str| {
                if let Some(w) = weak.upgrade() {
                    w.global::<App>().set_message(text.into());
                }
            }),
            online_art: Some(RefCell::new(sonkore_library::online_art::OnlineArt::standard())),
        });
        // Covers from the internet for the tracks without one (only asked for
        // when Settings › Library › Missing covers is on).
        if let Some(art) = &ctx.online_art {
            let art = art.borrow();
            sonkore_library::covers::set_online_lookup(Some(art.lookup()));
            art.reindex(library.borrow().inner());
        }
        // MPRIS: the desktop's media keys and player widgets (system/mpris).
        let (sender, commands) = std::sync::mpsc::channel();
        let sender = std::sync::Mutex::new(sender);
        let mpris = sonkore_platform::Mpris::new(sonkore_platform::Bus::Session, move |command| {
            let _ = sender.lock().map(|s| s.send(command));
        });
        mpris.set_enabled(settings.borrow().flag("system/mpris", true));
        let c = Rc::new(Controller {
            window: window.as_weak(),
            ctx,
            settings,
            library,
            list,
            player,
            page: RefCell::new(("tracks".into(), String::new())),
            search: RefCell::new(String::new()),
            expanded: RefCell::new(Default::default()),
            list_width: RefCell::new(900.0),
            unmuted: RefCell::new(1.0),
            anchor: RefCell::new(None),
            last_seek: std::cell::Cell::new(None),
            cursor: std::cell::Cell::new(None),
            me: RefCell::new(std::rc::Weak::new()),
            loose_track: RefCell::new(None),
            test_signal: RefCell::new(None),
            idle_since: std::cell::Cell::new(None),
            unsaved_since: std::cell::Cell::new(None),
            columns_model: Rc::new(VecModel::default()),
            last_folder_click: RefCell::new(None),
            last_path: RefCell::new(String::new()),
            message_timer: Timer::default(),
            poll_timer: Timer::default(),
            session_dirty: RefCell::new(false),
            ui: RefCell::new(pages::PageState::new()),
            clipped: std::cell::Cell::new(0),
            mpris,
            mpris_track: RefCell::new(None),
            session_written: RefCell::new(String::new()),
            commands,
            spectrum: RefCell::new(Default::default()),
            visual: RefCell::new(Default::default()),
        });
        *c.me.borrow_mut() = Rc::downgrade(&c);
        // The volume moved elsewhere (the desktop, the DAC's knob): shown.
        {
            let weak = Rc::downgrade(&c);
            crate::volume::on_change(move || {
                if let Some(c) = weak.upgrade() {
                    c.refresh_player();
                }
            });
        }
        c.restore_session();
        crate::covers::set_scale(window.window().scale_factor());
        // Each track's ReplayGain before it plays (on its first sample, also
        // at a gapless join).
        let weak = Rc::downgrade(&c);
        c.player.borrow_mut().set_gain_source(Box::new(move |path| weak.upgrade()?.gain_for(path)));
        let weak = Rc::downgrade(&c);
        crate::covers::on_arrival(move || {
            if let Some(c) = weak.upgrade() {
                c.covers_arrived();
            }
        });
        c.connect(window);
        c.connect_pages(window);
        c.refresh_sidebar();
        let start = c.start_page();
        c.open(&start.0, &start.1);
        c.refresh_player();
        c.screenshot_hooks();
        let weak = Rc::downgrade(&c);
        c.poll_timer.start(TimerMode::Repeated, Duration::from_millis(50), move || {
            if let Some(c) = weak.upgrade() {
                c.poll();
            }
        });
        c
    }

    fn window(&self) -> MainWindow {
        self.window.upgrade().expect("window")
    }

    // --- Wiring ------------------------------------------------------------------------

    fn connect(self: &Rc<Self>, window: &MainWindow) {
        let app = window.global::<App>();
        app.set_rows(ModelRc::from(self.list.clone() as Rc<dyn Model<Data = crate::TrackRow>>));
        app.set_sidebar_width(self.settings.borrow().int("ui/sidebarWidth", 248).clamp(200, 520) as f32);
        app.set_library_height(self.settings.borrow().int("ui/libraryHeight", -1) as f32);
        app.set_playlists_height(self.settings.borrow().int("ui/playlistsHeight", 150).max(0) as f32);
        macro_rules! on {
            ($name:ident, |$c:ident $(, $arg:ident: $t:ty)*| $body:block) => {{
                let weak = Rc::downgrade(self);
                app.$name(move |$($arg: $t),*| {
                    if let Some($c) = weak.upgrade() $body
                });
            }};
        }
        on!(on_navigate, |c, page: SharedString, arg: SharedString| {
            // A folder clicked twice: folded / unfolded (Sidebar's double click;
            // told here, the rows are made again between the clicks).
            if page == "folders" && !arg.is_empty() {
                let now = std::time::Instant::now();
                let double = c.last_folder_click.borrow().as_ref().is_some_and(|(p, t)| {
                    *p == arg.as_str() && now.duration_since(*t) < std::time::Duration::from_millis(400)
                });
                *c.last_folder_click.borrow_mut() = if double { None } else { Some((arg.to_string(), now)) };
                if double {
                    c.window().global::<App>().invoke_toggle_folder(arg);
                    return;
                }
            }
            c.open(&page, &arg);
            // The folder the music plays from: its playing track in view.
            if page == "folders" && !arg.is_empty() {
                let playing = c.player.borrow().current_path().unwrap_or("").to_owned();
                if playing.starts_with(&format!("{}/", arg.trim_end_matches('/'))) {
                    c.reveal_playing();
                }
            }
        });
        on!(on_search, |c, text: SharedString| {
            c.search_edited(&text);
        });
        on!(on_toggle_folder, |c, path: SharedString| {
            let p = path.to_string();
            {
                let mut e = c.expanded.borrow_mut();
                if !e.remove(&p) {
                    e.insert(p);
                }
            }
            c.refresh_sidebar();
        });
        on!(on_sidebar_resized, |c, w: f32| {
            c.window().global::<App>().set_sidebar_width(w);
            c.settings.borrow_mut().set_int("ui/sidebarWidth", w.round() as i64);
        });
        on!(on_sidebar_sections_changed, |c, library: f32, lists: f32| {
            let mut s = c.settings.borrow_mut();
            s.set_int("ui/libraryHeight", if library < 0.0 { -1 } else { library.round() as i64 });
            s.set_int("ui/playlistsHeight", lists.round() as i64);
        });
        on!(on_list_resized, |c, w: f32| {
            if (w - *c.list_width.borrow()).abs() >= 1.0 {
                *c.list_width.borrow_mut() = w;
                c.layout_columns();
            }
        });
        on!(on_list_look_changed, |c| {
            c.layout_columns();
        });
        on!(on_activate_row, |c, row: i32| {
            c.activate_row(row.max(0) as usize);
        });
        on!(on_select_row, |c, row: i32, ctrl: bool, shift: bool| {
            c.select(row.max(0) as usize, ctrl, shift);
        });
        on!(on_toggle_favorite, |c, row: i32| {
            if let Some(path) = c.list.path_at(row.max(0) as usize) {
                c.library.borrow_mut().toggle_favorite(&path);
                c.list.changed(row as usize);
                c.refresh_sidebar();
            }
        });
        on!(on_sort_by, |c, key: SharedString| {
            c.sort_by_column(&key);
        });
        on!(on_toggle_play, |c| {
            let ch = c.player.borrow_mut().toggle();
            c.after(ch);
        });
        on!(on_previous, |c| {
            let ch = c.player.borrow_mut().previous();
            c.after(ch);
        });
        on!(on_next, |c| {
            let ch = c.player.borrow_mut().next();
            c.after(ch);
        });
        on!(on_stop, |c| {
            let ch = c.player.borrow_mut().stop();
            c.after(ch);
        });
        on!(on_seek, |c, pos: f32| {
            let mut p = c.player.borrow_mut();
            let d = p.engine().duration();
            p.seek(pos as f64 * d);
        });
        {
            let weak = Rc::downgrade(self);
            app.on_key(move |name, shift| weak.upgrade().is_some_and(|c| c.key(name.as_str(), shift)));
        }
        on!(on_drag_started, |c| {
            let n = c.list.state.borrow().selected.len().max(1);
            c.window().global::<App>().set_drag_label(crate::views::tracks_text(n).into());
        });
        on!(on_drop_tracks, |c| {
            c.drop_tracks();
        });
        on!(on_column_resize, |c, index: i32, width: f32, _commit: bool| {
            c.resize_column(index.max(0) as usize, width);
        });
        on!(on_column_reset_width, |c, index: i32| {
            c.reset_column_width(index.max(0) as usize);
        });
        on!(on_column_move, |c, from: i32, target: i32| {
            c.move_column(from.max(0) as usize, target.max(0) as usize);
        });
        {
            let weak = Rc::downgrade(self);
            app.on_column_target(move |x| weak.upgrade().map_or(0, |c| c.column_target(x)));
        }
        on!(on_seek_drag, |c, pos: f32| {
            // Live seeking (Settings › General): one seek per 45 ms at most;
            // the release seeks to where it ends anyway.
            if !c.settings.borrow().flag("ui/liveSeek", true) {
                return;
            }
            let now = std::time::Instant::now();
            if c.last_seek.get().is_some_and(|t| now.duration_since(t) < std::time::Duration::from_millis(45)) {
                return;
            }
            c.last_seek.set(Some(now));
            let mut p = c.player.borrow_mut();
            let d = p.engine().duration();
            p.seek(pos as f64 * d);
        });
        on!(on_step_volume, |c, dir: i32| {
            c.step_volume(dir as f64);
        });
        on!(on_set_volume, |c, pos: f32| {
            c.set_volume(position_to_volume(pos as f64));
        });
        on!(on_toggle_mute, |c| {
            // Fixed at 100 %: the click offers the ways to set it instead.
            if crate::volume::fixed() {
                c.volume_menu_at_mute();
                return;
            }
            let v = c.volume();
            if v > 0.0 {
                *c.unmuted.borrow_mut() = v;
                c.set_volume(0.0);
            } else {
                let back = *c.unmuted.borrow();
                c.set_volume(if back > 0.0 { back } else { 1.0 });
            }
        });
        on!(on_toggle_shuffle, |c| {
            let on = !c.player.borrow().queue().shuffle();
            c.player.borrow_mut().set_shuffle(on);
            c.settings.borrow_mut().set_flag("playback/shuffle", on);
            c.refresh_player();
        });
        on!(on_cycle_repeat, |c| {
            let next = Repeat::from_index((c.player.borrow().queue().repeat().index() + 1) % 3);
            c.player.borrow_mut().set_repeat(next);
            c.settings.borrow_mut().set_int("playback/repeat", next.index());
            c.refresh_player();
        });
        for name in ["open_equalizer", "open_settings", "open_pitch", "open_mini", "show_signal_path", "show_outputs"] {
            let _ = name; // the windows and popups come with their pages
        }
    }

    // --- Pages ------------------------------------------------------------------------

    /// The start page: the page chosen in Settings; "Last place" is the last
    /// folder (or the playing track's), else Tracks.
    fn start_page(&self) -> (String, String) {
        // For checking pages offscreen: SONKORE_START_PAGE=page[:argument].
        if let Ok(page) = std::env::var("SONKORE_START_PAGE") {
            let (p, a) = page.split_once(':').unwrap_or((page.as_str(), ""));
            return (p.to_owned(), a.to_owned());
        }
        let s = self.settings.borrow();
        let start = s.text("ui/startPage", "last");
        if start == "last" || start == "folders" {
            let mut dir = s.text("ui/lastFolder", "");
            let lib = self.library.borrow();
            let has = |d: &str| !d.is_empty() && lib.tracks.iter().any(|t| t.path.starts_with(&format!("{d}/")));
            if !has(&dir) {
                dir = self
                    .player
                    .borrow()
                    .current_path()
                    .and_then(|p| p.rsplit_once('/').map(|(d, _)| d.to_owned()))
                    .unwrap_or_default();
            }
            if has(&dir) {
                return ("folders".into(), dir);
            }
            return ("tracks".into(), String::new());
        }
        let known = ["playing", "tracks", "favorites", "albums", "artists", "genres", "folders", "queue"];
        (if known.contains(&start.as_str()) { start } else { "tracks".into() }, String::new())
    }

    pub fn open(&self, page: &str, argument: &str) {
        // A playlist that is gone: Tracks.
        let missing = page == "playlist" && self.ui.borrow().playlists.find(argument).is_none();
        let (page, argument) = if missing { ("tracks", "") } else { (page, argument) };
        *self.page.borrow_mut() = (page.to_owned(), argument.to_owned());
        // Rows can be carried within the queue, Now playing and a playlist.
        self.window().global::<App>().set_reorderable(matches!(page, "queue" | "playing" | "playlist"));
        if page == "folders" && !argument.is_empty() {
            self.settings.borrow_mut().set_text("ui/lastFolder", argument);
            // Its parents unfold so it shows in the tree.
            let mut e = self.expanded.borrow_mut();
            let mut p = argument;
            while let Some((parent, _)) = p.rsplit_once('/') {
                e.insert(parent.to_owned());
                p = parent;
            }
        }
        let w = self.window();
        let app = w.global::<App>();
        app.set_page(page.into());
        app.set_page_argument(argument.into());
        self.fill_list();
        self.refresh_sidebar();
    }

    fn sort_order(&self) -> (String, bool) {
        self.page_sort()
    }

    /// The page's tracks, filtered by the search and sorted.
    fn fill_list(&self) {
        let (page, arg) = self.page.borrow().clone();
        let search = self.search.borrow().to_lowercase();
        // The pages of pages.rs give their own rows and order.
        let own = self.page_rows(&page, &arg);
        let lib = self.library.borrow();
        let player = self.player.borrow();
        let (mut indices, order): (Vec<usize>, Option<(String, bool)>) = match own {
            Some((rows, pages::Order::Keep)) => (rows, None),
            Some((rows, pages::Order::Sort(k, d))) => (rows, Some((k, d))),
            None => (
                match page.as_str() {
                    "favorites" => {
                        (0..lib.tracks.len()).filter(|&i| lib.favorites.contains(&lib.tracks[i].path)).collect()
                    }
                    "folders" if !arg.is_empty() => {
                        let prefix = format!("{arg}/");
                        (0..lib.tracks.len()).filter(|&i| lib.tracks[i].path.starts_with(&prefix)).collect()
                    }
                    _ => (0..lib.tracks.len()).collect(),
                },
                Some(self.sort_order()),
            ),
        };
        if !search.is_empty() && Self::searches_tracks(&page) {
            indices.retain(|&i| match lib.search_keys.get(i) {
                Some(k) => k.contains(&search),
                None => lib.tracks[i].search_key().contains(&search),
            });
        }
        let (key, desc) = order.clone().unwrap_or_default();
        if order.is_some() {
            // Lower-cased once per row, not twice per comparison (the sort
            // made ~800,000 comparisons for 50,000 tracks, each allocating).
            let numeric = matches!(
                key.as_str(),
                "" | "favorite" | "duration" | "year" | "trackNo" | "added" | "modified" | "fileSize"
            );
            let paths: Vec<String> = indices.iter().map(|&i| lib.tracks[i].path.to_lowercase()).collect();
            let texts: Vec<String> = if numeric || key == "path" {
                Vec::new()
            } else {
                indices.iter().map(|&i| tracklist::cell_text(&key, &lib.tracks[i]).to_lowercase()).collect()
            };
            let mut perm: Vec<usize> = (0..indices.len()).collect();
            perm.sort_by(|&pa, &pb| {
                let (a, b) = (indices[pa], indices[pb]);
                let (ta, tb) = (&lib.tracks[a], &lib.tracks[b]);
                let o = match key.as_str() {
                    "" => a.cmp(&b),
                    "favorite" => lib.favorites.contains(&ta.path).cmp(&lib.favorites.contains(&tb.path)),
                    "path" => natural_cmp(&paths[pa], &paths[pb]),
                    "duration" => ta.duration.partial_cmp(&tb.duration).unwrap_or(std::cmp::Ordering::Equal),
                    "year" => ta.year.cmp(&tb.year),
                    "trackNo" => ta.track_no.cmp(&tb.track_no),
                    "added" => ta.added.cmp(&tb.added),
                    "modified" => ta.modified.cmp(&tb.modified),
                    "fileSize" => ta.file_size.cmp(&tb.file_size),
                    _ => natural_cmp(&texts[pa], &texts[pb]),
                }
                .then_with(|| natural_cmp(&paths[pa], &paths[pb]));
                if desc { o.reverse() } else { o }
            });
            indices = perm.into_iter().map(|p| indices[p]).collect();
        }
        let count = indices.len();
        let total = lib.tracks.len();
        let duration: f64 = lib.tracks.iter().map(|t| t.duration).sum();
        drop(lib);
        drop(player);
        {
            let mut st = self.list.state.borrow_mut();
            st.indices = indices;
            st.selected.clear();
            st.keys = self.column_keys();
            st.artist_line = !st.keys.iter().any(|k| k == "artist");
        }
        self.list.reset();
        self.layout_columns();
        let w = self.window();
        let app = w.global::<App>();
        app.set_sort_key(key.into());
        app.set_sort_descending(desc);
        let (heading, sub) = match self.page_heading(&page, count) {
            Some(h) => h,
            None => self.heading(&page, &arg, count, total, duration),
        };
        app.set_heading(heading.into());
        app.set_subheading(sub.into());
        // Every page shows the search; those without one hand it to Tracks.
        app.set_searchable(true);
        app.set_search_placeholder(
            match page.as_str() {
                "folders" => tr("Main", "Search in folder…"),
                "albums" => tr("Main", "Search albums or artists…"),
                "artists" => tr("Main", "Search artists…"),
                "genres" => tr("Main", "Search genres…"),
                _ => tr("Main", "Search title, artist, album, genre…"),
            }
            .into(),
        );
        let (empty_title, empty_text) = if page == "favorites" {
            (tr("TracksPage", "No favorites yet"), tr("TracksPage", "Click the heart on any track to keep it here."))
        } else {
            (
                tr("TracksPage", "Your library is empty"),
                tr(
                    "TracksPage",
                    "Add a music folder — it's watched and kept in sync automatically.\nFLAC, WAV, MP3, ALAC, AAC, Vorbis, DSD and CUE sheets.",
                ),
            )
        };
        app.set_empty_title(empty_title.into());
        app.set_empty_text(empty_text.into());
        self.refresh_playing_row();
    }

    fn heading(&self, page: &str, arg: &str, count: usize, total: usize, duration: f64) -> (String, String) {
        let tracks = if count == 1 {
            tr("AppActions", "1 track")
        } else {
            trf("AppActions", "{0} tracks", &[&count.to_string()])
        };
        // The Tracks page's count: all of them with their length, else "x of y".
        let minutes = (duration / 60.0 + 0.5) as i64;
        let length = if minutes >= 60 {
            trf("TracksPage", "{0} h {1} min", &[&(minutes / 60).to_string(), &(minutes % 60).to_string()])
        } else {
            trf("TracksPage", "{0} min", &[&minutes.to_string()])
        };
        let all_tracks = if count == total {
            format!("{tracks} · {length}")
        } else {
            trf("TracksPage", "{0} of {1} tracks", &[&count.to_string(), &total.to_string()])
        };
        match page {
            "favorites" => (tr("Sidebar", "Favorites"), tracks),
            "folders" => (arg.rsplit('/').next().unwrap_or(arg).to_owned(), String::new()),
            "queue" => (tr("QueuePage", "Play queue"), tracks),
            "playlist" => (arg.to_owned(), tracks),
            "albums" => (tr("Sidebar", "Albums"), String::new()),
            "artists" => (tr("Sidebar", "Artists"), String::new()),
            "genres" => (tr("Sidebar", "Genres"), String::new()),
            "playing" => (tr("Sidebar", "Now playing"), String::new()),
            _ => (tr("Sidebar", "Tracks"), all_tracks),
        }
    }

    /// Covers asked for have been decoded: what shows them asks again.
    fn covers_arrived(&self) {
        self.list.refresh_all();
        self.refresh_player();
        match self.page.borrow().0.as_str() {
            "albums" => self.album_covers_arrived(),
            "playing" => self.refresh_now(),
            _ => {}
        }
    }

    /// Settings changed the columns: the lists again.
    pub fn columns_changed(&self) {
        self.fill_list();
    }

    fn column_keys(&self) -> Vec<String> {
        let s = self.settings.borrow();
        let keys: Vec<String> = serde_json::from_str::<Vec<String>>(&s.text("columns/keys", ""))
            .unwrap_or_default()
            .into_iter()
            .filter(|k| tracklist::column(k).is_some())
            .collect();
        if keys.is_empty() { DEFAULT_KEYS.iter().map(|s| s.to_string()).collect() } else { keys }
    }

    /// The saved widths (columns/widths), changed by `f`, saved, laid out.
    fn change_widths(&self, f: impl FnOnce(&mut serde_json::Map<String, serde_json::Value>)) {
        let mut saved: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(&self.settings.borrow().text("columns/widths", "")).unwrap_or_default();
        f(&mut saved);
        self.settings.borrow_mut().set_text("columns/widths", serde_json::Value::Object(saved).to_string());
        self.layout_columns();
    }

    /// A column's new width: between the header's least and 1200 px.
    fn resize_column(&self, index: usize, width: f32) {
        let Some(key) = self.list.state.borrow().keys.get(index).cloned() else { return };
        let w = (width as f64).round().clamp(tracklist::min_width_of(&key, self.widest_badge()), 1200.0);
        self.change_widths(|m| {
            m.insert(key, serde_json::json!(w as i64));
        });
    }

    /// The column's default width again.
    fn reset_column_width(&self, index: usize) {
        let Some(key) = self.list.state.borrow().keys.get(index).cloned() else { return };
        self.change_widths(|m| {
            m.remove(&key);
        });
    }

    /// The drop target: past the middle of a column, after it.
    fn column_target(&self, x: f32) -> i32 {
        let model = &self.columns_model;
        (0..model.row_count())
            .filter(|&i| model.row_data(i).is_some_and(|c| x > c.x + c.width / 2.0))
            .map(|i| i as i32 + 1)
            .max()
            .unwrap_or(0)
    }

    /// Moves a column, as the header's drop asks it.
    fn move_column(&self, from: usize, target: usize) {
        let mut keys = self.column_keys();
        if from >= keys.len() || target == from || target == from + 1 {
            return;
        }
        let to = if target > from { target - 1 } else { target };
        let key = keys.remove(from);
        keys.insert(to.min(keys.len()), key);
        self.settings.borrow_mut().set_text("columns/keys", serde_json::to_string(&keys).unwrap_or_default());
        self.fill_list();
    }

    /// The volume, wherever it is set now.
    pub(crate) fn volume(&self) -> f64 {
        crate::volume::get(self.player.borrow().engine())
    }

    /// Sets the volume (the slider, the wheel, the keys, MPRIS).
    pub(crate) fn set_volume(&self, v: f64) {
        crate::volume::change(&self.settings, self.player.borrow().engine(), v);
        self.refresh_player();
    }

    /// The volume `db` up or down.
    pub(crate) fn step_volume(&self, db: f64) {
        self.set_volume(crate::audio::stepped_volume(self.volume(), db));
    }

    /// The mode or the output changed: where the volume is set, again.
    pub(crate) fn update_volume(&self) {
        crate::volume::update(&self.settings.borrow(), self.player.borrow().engine());
        self.refresh_player();
    }

    /// The volume modes under the mute button (its click while fixed).
    fn volume_menu_at_mute(&self) {
        let w = self.window();
        let app = w.global::<App>();
        app.invoke_volume_menu();
    }

    /// The widest format badge in the library, at least a long hi-res one.
    pub(super) fn widest_badge(&self) -> f64 {
        let lib = self.library.borrow();
        let mut labels: std::collections::HashSet<String> = lib.tracks.iter().map(|t| t.format_label()).collect();
        labels.insert("FLAC 24/192".into());
        let widest = labels.iter().map(|l| crate::metrics::badge_width(l) as f64 - 12.0).fold(0.0, f64::max);
        let cap = crate::metrics::badge_width("WavPack 24/352.8") as f64 - 12.0;
        widest.min(cap)
    }

    pub(super) fn layout_columns(&self) {
        let keys = self.list.state.borrow().keys.clone();
        let saved: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(&self.settings.borrow().text("columns/widths", "")).unwrap_or_default();
        let widest = self.widest_badge();
        let w = self.window();
        let theme = w.global::<crate::Theme>();
        let layout = tracklist::layout_columns(
            &keys,
            &saved,
            *self.list_width.borrow(),
            theme.get_list_covers(),
            theme.get_list_density() == "spacious",
            widest,
        );
        let app = w.global::<App>();
        app.set_cover_width(layout.cover);
        {
            let mut st = self.list.state.borrow_mut();
            st.cover_side =
                if layout.cover > 0.0 { if theme.get_list_density() == "spacious" { 52.0 } else { 40.0 } } else { 0.0 };
            st.cover_radius = theme.get_radius_small();
        }
        // The same model while the columns are the same ones: a divider
        // dragged keeps its grip (a new model would make the header again).
        let model = self.columns_model.clone();
        let same = model.row_count() == layout.columns.len()
            && layout.columns.iter().enumerate().all(|(i, c)| model.row_data(i).is_some_and(|old| old.key == c.key));
        if same {
            for (i, c) in layout.columns.into_iter().enumerate() {
                if model.row_data(i).as_ref() != Some(&c) {
                    model.set_row_data(i, c);
                }
            }
        } else {
            model.set_vec(layout.columns);
        }
        if app.get_columns().row_count() != model.row_count() || !same {
            app.set_columns(ModelRc::from(model as Rc<dyn Model<Data = crate::Column>>));
        }
    }

    /// The selection replaced; only the rows that changed are asked again.
    pub(super) fn set_selection(&self, selected: std::collections::HashSet<usize>) {
        let before = std::mem::replace(&mut self.list.state.borrow_mut().selected, selected);
        let after = self.list.state.borrow().selected.clone();
        for r in before.symmetric_difference(&after) {
            self.list.changed(*r);
        }
    }

    fn select(&self, row: usize, ctrl: bool, shift: bool) {
        self.cursor.set(Some(row));
        let before = self.list.state.borrow().selected.clone();
        {
            let mut st = self.list.state.borrow_mut();
            let anchor = *self.anchor.borrow();
            if shift {
                let a = anchor.unwrap_or(row);
                st.selected = (a.min(row)..=a.max(row)).collect();
            } else if ctrl {
                if !st.selected.remove(&row) {
                    st.selected.insert(row);
                }
                *self.anchor.borrow_mut() = Some(row);
            } else {
                st.selected.clear();
                st.selected.insert(row);
                *self.anchor.borrow_mut() = Some(row);
            }
        }
        // Only the rows whose selection changed: a reset would make the rows
        // again, and the second click of a double click would land on a new
        // row (no double click then).
        let after = self.list.state.borrow().selected.clone();
        for r in before.symmetric_difference(&after) {
            self.list.changed(*r);
        }
    }

    // --- The sidebar --------------------------------------------------------------------

    fn refresh_sidebar(&self) {
        let lib = self.library.borrow();
        let w = self.window();
        let app = w.global::<App>();
        let count = |n: usize| if n == 0 { String::new() } else { n.to_string() };
        let queue = self.player.borrow().queue().len();
        let nav = vec![
            ("playing", "Now playing", count(queue)),
            ("tracks", "Tracks", count(lib.tracks.len())),
            ("favorites", "Favorites", count(lib.favorites.len())),
            ("albums", "Albums", String::new()),
            ("artists", "Artists", String::new()),
            ("genres", "Genres", String::new()),
        ];
        let nav: Vec<NavItem> = nav
            .into_iter()
            .map(|(id, label, badge)| NavItem {
                id: id.into(),
                icon: nav_icon(&self.window(), id),
                label: tr("Sidebar", label).into(),
                badge: badge.into(),
            })
            .collect();
        app.set_nav(ModelRc::new(VecModel::from(nav)));

        // The folder tree: each watched folder and the folders under it that
        // hold music, with their track counts.
        let (page, arg) = self.page.borrow().clone();
        let playing = self.player.borrow().current_path().unwrap_or("").to_owned();
        let expanded = self.expanded.borrow();
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for t in &lib.tracks {
            let mut dir = t.folder();
            loop {
                *counts.entry(dir.to_owned()).or_default() += 1;
                if lib.folders.iter().any(|f| f == dir) {
                    break;
                }
                match dir.rsplit_once('/') {
                    Some((parent, _)) if !parent.is_empty() => dir = parent,
                    _ => break,
                }
            }
        }
        let mut rows = Vec::new();
        let mut roots: Vec<&String> = lib.folders.iter().collect();
        roots.sort_by(|a, b| natural_cmp(&a.to_lowercase(), &b.to_lowercase()));
        fn children<'a>(counts: &'a BTreeMap<String, usize>, dir: &str) -> Vec<&'a String> {
            let prefix = format!("{dir}/");
            let mut c: Vec<&String> =
                counts.keys().filter(|k| k.starts_with(&prefix) && !k[prefix.len()..].contains('/')).collect();
            c.sort_by(|a, b| natural_cmp(&a.to_lowercase(), &b.to_lowercase()));
            c
        }
        let mut stack: Vec<(String, i32)> = roots.into_iter().rev().map(|r| (r.clone(), 0)).collect();
        // Subfolders on or off, and how many levels.
        drop(lib);
        let max_depth = self.tree_max_depth();
        while let Some((dir, depth)) = stack.pop() {
            let kids = if depth < max_depth { children(&counts, &dir) } else { Vec::new() };
            let is_open = depth == 0 && !expanded.contains(&format!("!{dir}")) || expanded.contains(&dir);
            let selected = page == "folders" && arg == dir;
            let holds = !playing.is_empty() && playing.starts_with(&format!("{dir}/"));
            let here = holds && playing.rsplit_once('/').map(|(d, _)| d) == Some(dir.as_str());
            let show_bars = !selected && (here || (holds && !kids.is_empty() && !is_open));
            rows.push(FolderRow {
                path: dir.as_str().into(),
                name: dir.rsplit('/').next().unwrap_or(&dir).into(),
                depth,
                has_children: !kids.is_empty(),
                expanded: is_open,
                count: counts.get(&dir).copied().unwrap_or(0).to_string().into(),
                selected,
                holds_playing: holds,
                playing_here: here,
                show_bars,
            });
            if is_open {
                for k in kids.into_iter().rev() {
                    stack.push((k.clone(), depth + 1));
                }
            }
        }
        app.set_folders(ModelRc::new(VecModel::from(rows)));
        let playlists: Vec<PlaylistRow> = self
            .playlist_rows()
            .into_iter()
            .map(|(name, n)| PlaylistRow {
                current: page == "playlist" && arg == name,
                name: name.into(),
                count: n.to_string().into(),
            })
            .collect();
        app.set_playlists(ModelRc::new(VecModel::from(playlists)));
    }

    // --- The player --------------------------------------------------------------------

    /// ReplayGain for the track now playing: the library's tags, or the
    /// file's own when it is not in the library (opened from a file manager).
    /// A track's ReplayGain (linear) under the current settings: from the
    /// library, or read from the file once for a track outside it. None when
    /// the settings or the library are busy (the gain then stays).
    fn gain_for(&self, path: &str) -> Option<f64> {
        let settings = self.settings.try_borrow().ok()?;
        let lib = self.library.try_borrow().ok()?;
        Some(match lib.tracks.iter().find(|t| t.path == path) {
            Some(track) => crate::audio::track_gain(&settings, Some(track)),
            None if path.is_empty() || settings.int("playback/replayGain", 0) == 0 => 1.0,
            None => {
                let mut loose = self.loose_track.try_borrow_mut().ok()?;
                if loose.as_ref().is_none_or(|(p, _)| p != path) {
                    *loose = Some((path.to_owned(), crate::lib_shim::Track::read(path)));
                }
                crate::audio::track_gain(&settings, loose.as_ref().and_then(|(_, t)| t.as_ref()))
            }
        })
    }

    pub fn apply_track_gain(&self) {
        let path = self.player.borrow().current_path().unwrap_or("").to_owned();
        let gain = self.gain_for(&path).unwrap_or(1.0);
        self.player.borrow_mut().refresh_next_gain();
        self.player.borrow().engine().set_track_gain(gain);
    }

    /// Plays a test signal (Settings › Audio › Test signals): `kind` "tone"
    /// at `rate`, "channels" (each channel in turn) or "dsd" (DSD`rate`). What
    /// was playing comes back where it was once the signal ends or is stopped.
    pub fn play_test_signal(&self, kind: &str, rate: u32, channels: u32) {
        use sonkore_dsp::test_signals::TestSignal;
        let signal = match kind {
            "dsd" => TestSignal::Dsd { multiple: rate, channels },
            "channels" => TestSignal::Channels { rate, channels },
            _ => TestSignal::Tone { rate, channels },
        };
        let made = if kind == "dsd" {
            let mut modulator = TestModulator::new(rate, channels as usize);
            signal.make(Some(&mut modulator))
        } else {
            signal.make(None)
        };
        let file = match made {
            Ok(path) => path.to_string_lossy().into_owned(),
            Err(e) => {
                self.show_message(&e);
                return;
            }
        };
        let changes = {
            let mut p = self.player.borrow_mut();
            let mut run = self.test_signal.borrow_mut();
            if run.is_none() {
                *run = Some(TestSignalRun {
                    file: String::new(),
                    paths: p.queue().paths().to_vec(),
                    index: p.queue().current().unwrap_or(0),
                    position: p.position(),
                    playing: p.playing(),
                    repeat: p.queue().repeat(),
                });
            }
            if let Some(run) = run.as_mut() {
                run.file = file.clone();
            }
            p.set_repeat(Repeat::Off);
            p.play_paths(vec![file], 0)
        };
        self.after(changes);
    }

    /// The test signal stopped (or ended): the queue as it was, playing on if it played.
    pub fn stop_test_signal(&self) {
        let Some(run) = self.test_signal.borrow_mut().take() else { return };
        let changes = {
            let mut p = self.player.borrow_mut();
            let mut c = p.stop();
            p.set_repeat(run.repeat);
            if run.paths.is_empty() {
                c.merge(p.clear());
            } else {
                p.restore(run.paths, run.index, run.position);
                if run.playing {
                    c.merge(p.play());
                }
            }
            c.queue = true;
            c.track = true;
            c
        };
        self.after(changes);
    }

    /// Settings › Audio › Give the DAC back when paused: a card played to
    /// directly is closed (and its reservation given back) after a minute
    /// open without playing; play opens it again where the track was.
    /// SONKORE_IDLE_RELEASE_MS shortens the minute for the tests.
    fn release_when_idle(&self) {
        let idle = {
            let p = self.player.borrow();
            let e = p.engine();
            !p.playing()
                && e.device_format().is_some()
                && sonkore_engine::output::is_direct_key(e.device_key())
                && self.settings.borrow().flag("playback/releaseDacWhenIdle", true)
        };
        if !idle {
            self.idle_since.set(None);
            return;
        }
        let since = self.idle_since.get().unwrap_or_else(std::time::Instant::now);
        self.idle_since.set(Some(since));
        let after = std::env::var("SONKORE_IDLE_RELEASE_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .map_or(Duration::from_secs(60), Duration::from_millis);
        if since.elapsed() >= after {
            self.idle_since.set(None);
            crate::audio::release_output(&mut self.player.borrow_mut());
            self.after(sonkore_engine::Changes { state: true, ..Default::default() });
        }
    }

    /// What changed outside the Settings window (the equalizer, the volume,
    /// pitch and tempo, the shuffle…) reaches the disk within a second, not
    /// only when Sonkore quits: a crash or a logout keeps it.
    fn save_settings_soon(&self) {
        if !self.settings.borrow().has_changes() {
            self.unsaved_since.set(None);
            return;
        }
        let since = self.unsaved_since.get().unwrap_or_else(std::time::Instant::now);
        self.unsaved_since.set(Some(since));
        if since.elapsed() >= Duration::from_secs(1) {
            self.unsaved_since.set(None);
            let _ = self.settings.borrow_mut().save();
        }
    }

    /// The ReplayGain the track playing gets, in dB (Settings › Audio ›
    /// Loudness analysis); None while off or without data for it.
    pub fn current_gain_db(&self) -> Option<f64> {
        let settings = self.settings.borrow();
        if settings.int("playback/replayGain", 0) == 0 {
            return None;
        }
        let path = self.player.borrow().current_path()?.to_owned();
        let lib = self.library.borrow();
        let loose = self.loose_track.borrow();
        let track = lib
            .tracks
            .iter()
            .find(|t| t.path == path)
            .or_else(|| loose.as_ref().filter(|(p, _)| *p == path).and_then(|(_, t)| t.as_ref()))?;
        track.rg_track_gain?;
        Some(20.0 * crate::audio::track_gain(&settings, Some(track)).log10())
    }

    pub fn test_signal_playing(&self) -> bool {
        self.test_signal.borrow().is_some()
    }

    /// The library's rows are new: every list, count and album built from them.
    /// Covers downloaded: the tracks of their albums show them.
    fn process_online_art(&self) {
        let Some(art) = &self.ctx.online_art else { return };
        let events = art.borrow_mut().process(self.library.borrow().inner());
        for e in events {
            if let sonkore_library::online_art::ArtEvent::CoverArrived(paths) = e {
                crate::covers::forget(&paths);
            }
        }
    }

    fn library_changed(&self) {
        if let Some(art) = &self.ctx.online_art {
            art.borrow().reindex(self.library.borrow().inner());
        }
        self.ui.borrow_mut().albums_dirty = true;
        self.mpris_track.borrow_mut().take(); // new tags, maybe a new cover
        self.fill_list();
        self.refresh_sidebar();
    }

    pub fn after(&self, changes: sonkore_engine::Changes) {
        if changes.track {
            self.apply_track_gain();
            self.clipped.set(0);
        }
        if changes.queue || changes.track {
            *self.session_dirty.borrow_mut() = true;
        }
        if changes.output_lost {
            // Unplugged, or the sound server went away: the next Play opens it again.
            self.show_message(&tr("DspController", "The audio output stopped"));
        } else if changes.error {
            let e = self.player.borrow().last_error().to_owned();
            if !e.is_empty() {
                self.show_message(&e);
            }
        }
        self.refresh_player();
        if changes.track || changes.queue {
            self.refresh_playing_row();
            self.refresh_sidebar();
            if matches!(self.page.borrow().0.as_str(), "queue" | "playing") {
                self.fill_list();
            }
        }
    }

    fn poll(&self) {
        while let Ok(command) = self.commands.try_recv() {
            self.desktop_command(command);
        }
        self.release_when_idle();
        self.save_settings_soon();
        let changes = self.player.borrow_mut().poll();
        // A test signal that ended: what it interrupted comes back. Another
        // track chosen meanwhile stays (only the repeat mode comes back).
        let (ended, replaced) = self.test_signal.borrow().as_ref().map_or((false, false), |run| {
            let p = self.player.borrow();
            (!p.loaded(), p.loaded() && p.current_path() != Some(run.file.as_str()))
        });
        if replaced {
            if let Some(run) = self.test_signal.borrow_mut().take() {
                self.player.borrow_mut().set_repeat(run.repeat);
            }
        }
        if ended {
            self.after(changes);
            self.stop_test_signal();
            return;
        }
        // What the scan, the watcher and the loudness analysis changed.
        let lib = self.library.borrow_mut().process();
        self.process_online_art();
        if lib.rows {
            self.library_changed();
        } else {
            // Gains measured or hearts changed: the rows shown again, the
            // list (and its selection) stays.
            if !lib.loudness.is_empty() || lib.favorites {
                self.list.refresh_all();
            }
            if lib.favorites {
                self.refresh_sidebar();
            }
        }
        // The track playing may have come into the library, or its tags changed.
        if lib.rows || !lib.loudness.is_empty() {
            self.apply_track_gain();
        }
        let clipped = self.player.borrow().engine().take_clipped();
        if clipped > 0 {
            self.clipped.set(self.clipped.get() + clipped);
        }
        if changes != Default::default() {
            self.after(changes);
        }
        self.watch_visual();
        // While the visualizer's frames run, the position goes with them
        // (one frame for all: each frame costs Slint a pass over the window).
        if !self.visual.borrow().timer.running() {
            self.refresh_position();
        }
        self.publish();
        if crate::probe::enabled() {
            self.write_probe();
        }
        if *self.session_dirty.borrow() {
            *self.session_dirty.borrow_mut() = false;
            self.save_session();
        }
    }

    /// The visualizer and the playing bars move (a frame every 33 ms:
    /// smooth because they
    /// follow the clock) while something plays or the visualizer's bars are
    /// still falling, and stop at rest.
    fn watch_visual(&self) {
        let w = self.window();
        let style = w.global::<crate::Theme>().get_visualizer();
        let playing = self.player.borrow().playing();
        let v = self.visual.borrow();
        let shown = style != "off" && style != "none";
        // Hidden to the tray: nothing to draw (the FFT and the paint ran at
        // 30 Hz all the same). Not "minimized": Wayland never says when a
        // window comes back from it (CLAUDE.md).
        let moving = w.window().is_visible() && (playing || (shown && !v.meter.resting()));
        if moving && !v.timer.running() {
            drop(v);
            self.visual.borrow_mut().last = None;
            let weak = self.me.borrow().clone();
            self.visual.borrow().timer.start(TimerMode::Repeated, Duration::from_millis(33), move || {
                if let Some(c) = weak.upgrade() {
                    c.tick_visual();
                }
            });
        } else if !moving && v.timer.running() {
            v.timer.stop();
        }
    }

    fn tick_visual(&self) {
        let w = self.window();
        let theme = w.global::<crate::Theme>();
        let style = theme.get_visualizer().to_string();
        let peaks_on = style == "bars";
        let playing = self.player.borrow().playing();
        let mut v = self.visual.borrow_mut();
        let now = std::time::Instant::now();
        let dt = v.last.map_or(0.033, |t| now.duration_since(t).as_secs_f64());
        v.last = Some(now);
        if playing {
            w.global::<App>().set_bars_time(v.clock.elapsed().as_secs_f32());
        }
        self.refresh_position();
        if style == "off" || style == "none" {
            if !playing {
                v.timer.stop();
            }
            return;
        }
        if playing {
            let mut sp = self.spectrum.borrow_mut();
            if sp.update(self.player.borrow().engine()) {
                v.meter.set_target(&sp.levels, peaks_on);
            }
        } else {
            v.meter.set_target(&[0.0; crate::spectrum::BANDS], peaks_on);
        }
        v.meter.advance(dt, peaks_on);
        let visual = w.global::<crate::Visual>();
        let mini = w.global::<crate::Mini>().get_on();
        let (lw, lh) = if mini {
            (visual.get_mini_width(), visual.get_mini_height())
        } else {
            (visual.get_bar_width(), visual.get_bar_height())
        };
        let dpr = w.window().scale_factor();
        if let Some(mut canvas) = crate::paint::Canvas::new(lw, lh, dpr) {
            if lw > 0.0 && lh > 0.0 {
                let color = crate::paint::Rgba::from_slint(theme.get_accent());
                v.meter.paint(&mut canvas, lw, lh, &style, color, peaks_on, 0.62);
                if mini {
                    visual.set_mini(canvas.image());
                } else {
                    visual.set_bar(canvas.image());
                }
            }
        }
        if !playing && v.meter.resting() {
            v.timer.stop();
        }
    }

    /// Settings › Desktop › System media controls, applied at once (it was
    /// read only at startup, so the switch did nothing until a restart).
    pub fn set_mpris(&self, on: bool) {
        self.mpris.set_enabled(on);
    }

    /// The state the live tests wait for (SONKORE_STATE_FILE, see probe.rs).
    fn write_probe(&self) {
        let w = self.window();
        let (page, argument) = self.page.borrow().clone();
        let p = self.player.borrow();
        let e = p.engine();
        crate::probe::write(&[
            ("page", page),
            ("argument", argument),
            ("search", self.search.borrow().clone()),
            ("playing", p.playing().to_string()),
            ("loaded", p.loaded().to_string()),
            // What the playback bar shows (the player may have moved on).
            ("bar-title", w.global::<App>().get_title().to_string()),
            ("path", p.current_path().unwrap_or_default().to_owned()),
            ("position", format!("{:.1}", p.position())),
            ("queue", p.queue().len().to_string()),
            ("index", p.queue().current().map(|i| i.to_string()).unwrap_or_default()),
            ("device", e.device_key().to_owned()),
            ("format", e.device_format().map(|f| format!("{f:?}")).unwrap_or_default()),
            ("buffer-ms", e.buffer_in_use_ms().map(|b| b.to_string()).unwrap_or_default()),
            ("volume", format!("{:.3}", e.volume())),
            ("error", p.last_error().to_owned()),
            ("message", w.global::<App>().get_message().to_string()),
            ("crossfeed", self.settings.borrow().int("playback/crossfeed", 0).to_string()),
            ("eq", self.ctx.equalizer.borrow().enabled().to_string()),
            ("tracks", self.library.borrow().tracks.len().to_string()),
            ("cursor", self.cursor.get().map(|i| i.to_string()).unwrap_or_default()),
            ("covers", w.global::<App>().get_cover_width().to_string()),
            ("density", w.global::<crate::Theme>().get_list_density().to_string()),
            ("font", w.global::<crate::Theme>().get_font_family().to_string()),
            ("lang", crate::i18n::loaded().to_string()),
            ("stop-button", w.global::<App>().get_stop_button().to_string()),
            ("search-bar", w.global::<App>().get_show_search().to_string()),
            ("wheel-step", w.global::<App>().get_wheel_step().to_string()),
            ("mpris", self.mpris.enabled().to_string()),
            ("columns", self.column_keys().join(",")),
            ("look", {
                // What the main window shows of Settings › Appearance.
                let t = w.global::<crate::Theme>();
                let a = t.get_accent();
                format!(
                    "accent=#{:02x}{:02x}{:02x} icons={} transport={} slider={} volume={} buttons={} radius={} outline={} backdrop={} visualizer={} compact={} alternate={}",
                    a.red(),
                    a.green(),
                    a.blue(),
                    t.get_icon_style(),
                    t.get_transport_style(),
                    t.get_slider_style(),
                    t.get_volume_style(),
                    t.get_window_buttons(),
                    t.get_window_radius(),
                    t.get_window_outline(),
                    t.get_backdrop(),
                    t.get_visualizer(),
                    t.get_compact(),
                    t.get_alternate_rows()
                )
            }),
            ("mini-first", w.global::<crate::Mini>().get_first_row().to_string()),
            ("mini-scroll", w.global::<crate::Mini>().get_scrolled_to().round().to_string()),
            ("mini-on", w.global::<crate::Mini>().get_on().to_string()),
            ("mini-search", w.global::<crate::Mini>().get_search_text().to_string()),
            ("mini-x", w.global::<crate::Mini>().get_columns().row_data(0).map_or(String::new(), |c| c.x.to_string())),
        ]);
    }

    /// What the desktop asked for (MPRIS).
    fn desktop_command(&self, command: sonkore_platform::Command) {
        use sonkore_platform::Command as C;
        let changes = {
            let mut p = self.player.borrow_mut();
            match command {
                C::Play => p.play(),
                C::Pause | C::Stop if !p.playing() => Default::default(),
                C::Pause => p.pause(),
                C::PlayPause => p.toggle(),
                C::Stop => p.stop(),
                C::Next => p.next(),
                C::Previous => p.previous(),
                C::SeekTo(seconds) => {
                    p.seek(seconds);
                    Default::default()
                }
                C::SetRepeat(r) => {
                    let repeat = match r {
                        sonkore_platform::Repeat::Off => Repeat::Off,
                        sonkore_platform::Repeat::All => Repeat::All,
                        sonkore_platform::Repeat::One => Repeat::One,
                    };
                    p.set_repeat(repeat);
                    // Saved like the bar's button does: it was lost at the next start.
                    self.settings.borrow_mut().set_int("playback/repeat", repeat.index());
                    Default::default()
                }
                C::SetShuffle(on) => {
                    p.set_shuffle(on);
                    self.settings.borrow_mut().set_flag("playback/shuffle", on);
                    Default::default()
                }
                C::SetVolume(v) => {
                    crate::volume::change(&self.settings, p.engine(), v);
                    Default::default()
                }
                C::Open(path) => p.play_paths(vec![path], 0),
                C::Raise => {
                    drop(p);
                    let w = self.window();
                    w.window().set_minimized(false);
                    let _ = w.show();
                    // Windows only flashes the taskbar button without it
                    // (the second instance allowed us to the front).
                    slint::winit_030::WinitWindowAccessor::with_winit_window(w.window(), |ww| ww.focus_window());
                    return;
                }
                C::Quit => {
                    drop(p);
                    self.shutdown();
                    let _ = slint::quit_event_loop();
                    return;
                }
            }
        };
        self.after(changes);
    }

    /// The player's state for the desktop (MPRIS), sent when it changed.
    fn publish(&self) {
        let p = self.player.borrow();
        let path = p.current_path().unwrap_or("").to_owned();
        let track = (!path.is_empty()).then(|| {
            let mut cached = self.mpris_track.borrow_mut();
            if cached.as_ref().is_none_or(|t| t.path.as_os_str() != path.as_str()) {
                let lib = self.library.borrow();
                let t = lib.tracks.iter().find(|t| t.path == path);
                *cached = Some(sonkore_platform::TrackInfo {
                    path: path.clone().into(),
                    title: t
                        .map(|t| t.title.clone())
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| path.rsplit('/').next().unwrap_or("").to_owned()),
                    album: t.map(|t| t.album.clone()).unwrap_or_default(),
                    artist: t.map(|t| t.artist.clone()).unwrap_or_default(),
                    album_artist: t.map(|t| t.album_artist.clone()).unwrap_or_default(),
                    track_no: t.map_or(0, |t| t.track_no.max(0) as u32),
                    genre: t.map(|t| t.genre.clone()).unwrap_or_default(),
                    duration: 0.0,
                    art_url: mpris_art_url(&path),
                });
            }
            let mut info = cached.clone().unwrap_or_default();
            info.duration = p.engine().duration();
            info
        });
        let repeat = match p.queue().repeat() {
            Repeat::Off => sonkore_platform::Repeat::Off,
            Repeat::All => sonkore_platform::Repeat::All,
            Repeat::One => sonkore_platform::Repeat::One,
        };
        self.mpris.update(sonkore_platform::PlayerState {
            status: sonkore_platform::PlaybackStatus::from_player(p.playing(), p.loaded()),
            repeat,
            shuffle: p.queue().shuffle(),
            volume: crate::volume::get(p.engine()).min(1.0),
            position: p.position(),
            duration: p.engine().duration(),
            has_queue: !p.queue().is_empty(),
            track,
        });
    }

    /// Files given on the command line (or forwarded): played now.
    pub fn play_files(&self, files: Vec<String>) {
        if files.is_empty() {
            return;
        }
        let changes = self.player.borrow_mut().play_paths(files, 0);
        self.after(changes);
    }

    fn refresh_position(&self) {
        let p = self.player.borrow();
        let w = self.window();
        let app = w.global::<App>();
        let (pos, dur) = (p.position(), p.engine().duration());
        // Only what changed: every new value is a frame of the whole window
        // for Slint. The bar moves in thousandths (under a screen pixel on
        // any seek bar), the clock once a second.
        let position = if dur > 0.0 { ((pos / dur) * 1000.0).round() as f32 / 1000.0 } else { 0.0 };
        if app.get_position() != position {
            app.set_position(position);
        }
        let elapsed: slint::SharedString = tracklist::format_time(pos).into();
        if app.get_elapsed() != elapsed {
            app.set_elapsed(elapsed);
        }
        if app.get_duration() != dur as f32 {
            app.set_duration(dur as f32);
            app.set_total(tracklist::format_time(dur).into());
        }
        if app.get_playing() != p.playing() {
            app.set_playing(p.playing());
        }
        if app.get_bit_perfect() != p.engine().bit_perfect() {
            app.set_bit_perfect(p.engine().bit_perfect());
        }
    }

    fn refresh_player(&self) {
        let p = self.player.borrow();
        let w = self.window();
        let app = w.global::<App>();
        let path = p.current_path().unwrap_or("").to_owned();
        app.set_loaded(p.loaded() || !path.is_empty());
        crate::waveform::set_path(&path);
        let lib = self.library.borrow();
        let track = lib.tracks.iter().find(|t| t.path == path);
        let file = path.rsplit('/').next().unwrap_or("").to_owned();
        // The cover beside the title (and in the mini player's display).
        let cover = crate::covers::get(&path, 96.0, 0.0);
        app.set_has_cover(cover.is_some());
        app.set_cover(cover.unwrap_or_default());
        app.set_title(
            track.map(|t| if t.title.is_empty() { file.clone() } else { t.title.clone() }).unwrap_or(file).into(),
        );
        // The player bar's track: "artist  ·  album", "Unknown artist" without one.
        let artist = track.map_or(String::new(), |t| t.artist.clone());
        let artist = if artist.is_empty() { tr("NowPlayingBar", "Unknown artist") } else { artist };
        let album = track.map_or(String::new(), |t| t.album.clone());
        app.set_artist(if album.is_empty() { artist } else { format!("{artist}  ·  {album}") }.into());
        let info = p.engine().info().cloned();
        // The track's description: what the decoder reads ("FLAC
        // 16-bit · 44.1 kHz", " · 1 ch" when not stereo); the library's
        // label until the stream is open.
        let format = match (&info, track) {
            (Some(i), _) if i.rate > 0 => {
                let mut s = i.codec.clone();
                if i.bits > 0 {
                    s += &format!(" {}-bit", i.bits);
                }
                s += &format!(" · {} kHz", crate::i18n::number_g(i.rate as f64 / 1000.0, false));
                if i.channels != 2 {
                    s += &format!(" · {} ch", i.channels);
                }
                s
            }
            (_, Some(t)) => t.format_label(),
            _ => String::new(),
        };
        app.set_format_special(
            track.is_some_and(|t| t.bits > 16 || t.sample_rate > 48000)
                || info.as_ref().is_some_and(|i| i.bits > 16 || i.rate > 48000),
        );
        app.set_format(format.into());
        app.set_output_loaded(p.loaded());
        let volume = crate::volume::get(p.engine());
        app.set_volume(volume_to_position(volume) as f32);
        app.set_muted(volume <= 0.0);
        app.set_volume_fixed(crate::volume::fixed());
        app.set_volume_kind(if crate::volume::fixed() {
            SharedString::new()
        } else if crate::volume::hardware() || crate::volume::system_in_hardware() {
            "DAC".into()
        } else if crate::volume::system() {
            tr("NowPlayingBar", "System").into()
        } else {
            SharedString::new()
        });
        let gain = if volume > 0.0 {
            format!("{:.1} dB", 20.0 * volume.log10()).replace('-', "\u{2212}")
        } else {
            "\u{2212}\u{221e} dB".into()
        };
        app.set_volume_tooltip(
            if crate::volume::hardware() || crate::volume::system_in_hardware() {
                trf("NowPlayingBar", "DAC volume · {0} · bit-perfect", &[&gain])
            } else if crate::volume::system() {
                trf("NowPlayingBar", "System volume · {0}", &[&gain])
            } else {
                trf("NowPlayingBar", "Volume · {0}", &[&gain])
            }
            .into(),
        );
        app.set_shuffle(p.queue().shuffle());
        app.set_repeat(p.queue().repeat().index() as i32);
        let bp = p.engine().bit_perfect();
        app.set_path_quality(
            if bp {
                "lossless"
            } else if p.loaded() {
                "high"
            } else {
                ""
            }
            .into(),
        );
        // The output's summary ("Bit-perfect · 44.1 kHz", "Processed · …").
        let summary = if p.loaded() { crate::signal::output_summary(p.engine()) } else { String::new() };
        app.set_path_summary(summary.into());
        drop(lib);
        drop(p);
        self.refresh_position();
        if self.page.borrow().0 == "playing" {
            self.refresh_now();
        }
        if path != *self.last_path.borrow() {
            *self.last_path.borrow_mut() = path;
            self.refresh_sidebar();
        }
    }

    /// The playing mark moves: only the rows of the two tracks are told. A
    /// reset recreated every row, so a drag or the second click of a
    /// double-click that met a track change was lost.
    fn refresh_playing_row(&self) {
        let path = self.player.borrow().current_path().unwrap_or("").to_owned();
        let old = std::mem::replace(&mut self.list.state.borrow_mut().playing, path.clone());
        if old == path {
            return;
        }
        for row in self.list.rows_of(&[&old, &path]) {
            self.list.changed(row);
        }
    }

    /// Runs `f` on this controller later (a dialog's answer).
    pub(super) fn later(&self) -> std::rc::Weak<Controller> {
        self.me.borrow().clone()
    }

    /// Plays through `key` ("" = System default). With playback/soundPerOutput
    /// the output being left keeps its sound and this one gets its own back
    /// (sound_profiles.rs).
    pub fn select_output(&self, key: &str) {
        let old = self.settings.borrow().text("playback/device", "");
        let per_output = crate::sound_profiles::enabled(&self.settings.borrow());
        if per_output && old != key {
            crate::sound_profiles::store(&mut self.settings.borrow_mut(), &self.ctx.equalizer.borrow(), &old);
        }
        self.settings.borrow_mut().set_text("playback/device", key);
        let changes = self.player.borrow_mut().set_device(key);
        if per_output
            && old != key
            && crate::sound_profiles::restore(
                &mut self.settings.borrow_mut(),
                &mut self.ctx.equalizer.borrow_mut(),
                key,
            )
        {
            crate::audio::apply(&self.settings.borrow(), self.player.borrow_mut().engine_mut());
        }
        let _ = self.settings.borrow_mut().save();
        self.after(changes);
        self.update_volume();
        self.refresh_player();
    }

    /// Sound devices plugged in or out (hotplug.rs): said in the status line.
    pub fn devices_plugged(&self, added: &[String], removed: &[String]) {
        let mut lines = Vec::new();
        if !added.is_empty() {
            lines.push(trf("Main", "Sound device connected: {0}", &[&added.join(", ")]));
        }
        if !removed.is_empty() {
            lines.push(trf("Main", "Sound device disconnected: {0}", &[&removed.join(", ")]));
        }
        self.show_message(&lines.join(" · "));
    }

    fn show_message(&self, text: &str) {
        let w = self.window();
        let app = w.global::<App>();
        app.set_message(text.into());
        let weak = self.window.clone();
        self.message_timer.start(TimerMode::SingleShot, Duration::from_millis(3200), move || {
            if let Some(w) = weak.upgrade() {
                w.global::<App>().set_message(SharedString::default());
            }
        });
    }

    // --- Persistence --------------------------------------------------------------------

    /// session.json: {queue, index, position}.
    fn restore_session(&self) {
        if !self.settings.borrow().flag("playback/resume", true) {
            return;
        }
        let Ok(bytes) = std::fs::read(paths::data_file("session.json")) else { return };
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) else { return };
        let queue: Vec<String> =
            v.get("queue").and_then(|q| serde_json::from_value(q.clone()).ok()).unwrap_or_default();
        let index = v.get("index").and_then(|i| i.as_i64()).unwrap_or(0).max(0) as usize;
        let position = v.get("position").and_then(|p| p.as_f64()).unwrap_or(0.0);
        if !queue.is_empty() {
            self.player.borrow_mut().restore(queue, index.min(usize::MAX), position);
        }
    }

    pub fn save_session(&self) {
        let p = self.player.borrow();
        let json = serde_json::json!({
            "queue": p.queue().paths(),
            "index": p.queue().current().map_or(-1, |i| i as i64),
            "position": p.position(),
        });
        let text = serde_json::to_string(&json).unwrap_or_default();
        if *self.session_written.borrow() == text {
            return;
        }
        *self.session_written.borrow_mut() = text.clone();
        let _ = session_writer().send(SessionWrite::Text(text));
    }

    /// At exit: the session and the settings.
    pub fn shutdown(&self) {
        self.save_session();
        // Written before the process ends.
        let (done, wait) = std::sync::mpsc::channel();
        if session_writer().send(SessionWrite::Flush(done)).is_ok() {
            let _ = wait.recv_timeout(std::time::Duration::from_secs(5));
        }
        // This output's sound as it is now: a restart does not bring back
        // what it had when it was last left.
        if crate::sound_profiles::enabled(&self.settings.borrow()) {
            let device = self.settings.borrow().text("playback/device", "");
            crate::sound_profiles::store(&mut self.settings.borrow_mut(), &self.ctx.equalizer.borrow(), &device);
        }
        let _ = self.settings.borrow_mut().save();
    }

    /// The full view's size and state, to open with next time.
    pub fn save_geometry(&self, window: &MainWindow) {
        let w = window.window();
        let size = w.size().to_logical(w.scale_factor());
        self.save_full_size(size.width, size.height, w.is_maximized());
    }

    /// Saves a full-view size (logical) and maximized state as widgets/geometry.
    pub fn save_full_size(&self, width: f32, height: f32, maximized: bool) {
        let mut g = crate::geometry::Geometry {
            x: 0,
            y: 0,
            width: width.round() as u32,
            height: height.round() as u32,
            maximized,
        };
        // Maximized: keep the size it comes back to (the one saved before).
        if maximized {
            if let Some(old) = self.settings.borrow().bytes("widgets/geometry").and_then(crate::geometry::parse) {
                (g.width, g.height) = (old.width, old.height);
            }
        }
        let data = crate::geometry::encode(&g, 0);
        self.settings.borrow_mut().set_bytes("widgets/geometry", data);
    }

    pub fn save_settings(&self) {
        let _ = self.settings.borrow_mut().save();
    }
}

/// Natural order: digits compared as numbers ("Track 2" before "Track 10").
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, _) => return std::cmp::Ordering::Less,
            (_, None) => return std::cmp::Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let mut n1 = String::new();
                while let Some(c) = x.peek().copied().filter(char::is_ascii_digit) {
                    n1.push(c);
                    x.next();
                }
                let mut n2 = String::new();
                while let Some(d) = y.peek().copied().filter(char::is_ascii_digit) {
                    n2.push(d);
                    y.next();
                }
                let (t1, t2) = (n1.trim_start_matches('0'), n2.trim_start_matches('0'));
                let o = t1.len().cmp(&t2.len()).then_with(|| t1.cmp(t2));
                if o != std::cmp::Ordering::Equal {
                    return o;
                }
            }
            (Some(c), Some(d)) => {
                if c != d {
                    return c.cmp(&d);
                }
                x.next();
                y.next();
            }
        }
    }
}

/// What a test signal interrupted (see [`Controller::play_test_signal`]).
struct TestSignalRun {
    file: String,
    paths: Vec<String>,
    index: usize,
    position: f64,
    playing: bool,
    repeat: Repeat,
}

/// The engine's sigma-delta modulator for the DSD test tone.
struct TestModulator(sonkore_dsd::SigmaDelta);

impl TestModulator {
    fn new(multiple: u32, channels: usize) -> Self {
        let mut m = sonkore_dsd::SigmaDelta::new();
        m.setup(multiple, channels, sonkore_engine::output::DsdPacking::NativeU32Be);
        TestModulator(m)
    }
}

impl sonkore_dsp::test_signals::DsdModulator for TestModulator {
    fn process(&mut self, input: &[f64], out: &mut [i32]) -> usize {
        self.0.process(input, out)
    }
    fn flush(&mut self, out: &mut [i32]) -> usize {
        self.0.flush(out)
    }
}

/// `mpris:artUrl`: the embedded cover (written once to the cache, as earlier
/// versions did), else the folder's cover image. Desktops showed none.
#[cfg(target_os = "linux")]
fn mpris_art_url(path: &str) -> Option<String> {
    let image = sonkore_library::metadata::embedded_cover(path);
    if let Some(url) = sonkore_platform::mpris::cached_cover_url(&image, &paths::cache_dir()) {
        return Some(url);
    }
    let folder = std::path::Path::new(path).parent()?.to_str()?;
    let cover = sonkore_library::metadata::folder_cover(folder)?;
    Some(sonkore_platform::url::file_url(std::path::Path::new(&cover)))
}

#[cfg(not(target_os = "linux"))]
fn mpris_art_url(_path: &str) -> Option<String> {
    None
}

enum SessionWrite {
    Text(String),
    Flush(std::sync::mpsc::Sender<()>),
}

/// session.json is written on a thread of its own, in order: a queue of the
/// whole library is megabytes, and it used to be read back and written on the
/// interface's thread at every track change.
fn session_writer() -> &'static std::sync::mpsc::Sender<SessionWrite> {
    static TX: std::sync::OnceLock<std::sync::mpsc::Sender<SessionWrite>> = std::sync::OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<SessionWrite>();
        let _ = std::thread::Builder::new().name("sonkore-session".into()).spawn(move || {
            while let Ok(msg) = rx.recv() {
                match msg {
                    SessionWrite::Text(mut text) => {
                        // Only the newest of what is waiting.
                        let mut flush = None;
                        while let Ok(next) = rx.try_recv() {
                            match next {
                                SessionWrite::Text(t) => text = t,
                                SessionWrite::Flush(done) => {
                                    flush = Some(done);
                                    break;
                                }
                            }
                        }
                        let path = paths::data_file("session.json");
                        let _ = std::fs::create_dir_all(paths::data_dir());
                        let tmp = path.with_extension("json.tmp");
                        if std::fs::write(&tmp, text).is_ok() {
                            let _ = std::fs::rename(tmp, path);
                        }
                        if let Some(done) = flush {
                            let _ = done.send(());
                        }
                    }
                    SessionWrite::Flush(done) => {
                        let _ = done.send(());
                    }
                }
            }
        });
        tx
    })
}
