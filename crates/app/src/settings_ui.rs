//! The settings window: fills `SettingsWindow` from the settings and the
//! player, writes every change back under the stored keys (and in the
//! stored form: "true"/"false", whole numbers, the mini player's appearance as
//! JSON), and tells the rest of the app through [`SettingsHooks`].
//!
//! Open it with [`open`]; it stays one window, raised when opened again.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use serde_json::{Map, Value as Json};
use slint::{Color, ComponentHandle, LogicalSize, Model, ModelRc, SharedString, Timer, TimerMode, VecModel};
use sonkore_core::settings::{Settings, Value};

use crate::context::Ctx;
use crate::i18n::{self, tr, trf};
use crate::theme;
use crate::tracklist::{CATALOG, DEFAULT_KEYS};
use crate::{
    Appearance, ColumnLine, Opt, OutputDevice, SettingsData, SettingsOverlay, SettingsWindow, SkinTile, Skins,
    WindowActions,
};

const CTX: &str = "SettingsPage";

fn t(text: &str) -> String {
    tr(CTX, text)
}

// --- What the engine and the library tell the window ------------------------------

/// The output's state: what the Audio tab shows besides the settings.
/// `None` fields fall back to what is shown on Linux.
#[derive(Clone, Debug, Default)]
pub struct AudioStatus {
    pub loaded: bool,
    /// "ALSA · Topping DX1 · 24-bit · 96 kHz".
    pub output_description: String,
    pub bit_perfect: bool,
    /// Sonkore holds the DAC.
    pub output_reserved: bool,
    /// The output can be held exclusively.
    pub output_exclusive_capable: bool,
    /// The DAC's own volume control is supported.
    pub hardware_volume_supported: Option<bool>,
    /// The volume row's description in Automatic / DAC.
    pub hardware_volume_info: Option<String>,
    /// The headroom applied now.
    pub effective_headroom_db: Option<f64>,
    /// The track clips (clipped samples > 0).
    pub clipping: bool,
    /// The current track's ReplayGain; `None`: no data.
    pub current_gain_db: Option<f64>,
    pub test_signal_playing: bool,
    pub dts_test_available: bool,
    /// The About card's second line (the audio components).
    pub components: Option<String>,
}

/// The library's state.
#[derive(Clone, Debug, Default)]
pub struct LibraryStatus {
    pub scanning: bool,
    pub scanned_files: usize,
    /// Tracks indexed; `None`: the library the window was given.
    pub count: Option<usize>,
    /// Tracks removed by hand.
    pub hidden_count: usize,
    pub analyzing_loudness: bool,
    pub loudness_done: usize,
    pub loudness_total: usize,
    pub tracks_without_loudness: usize,
}

/// The cover downloads (OnlineArt).
#[derive(Clone, Debug, Default)]
pub struct CoversStatus {
    pub pending: usize,
    pub fetched: usize,
    pub last_error: String,
}

/// What the window asks of the rest of the app. Every setting is already
/// written (and saved) when `setting_changed` is called: an implementation
/// reads it back from the settings and applies it. Everything has a default
/// that does nothing, so the window works before the engine is wired.
pub trait SettingsHooks {
    /// A setting was written: its key ("playback/limiter", "ui/liveSeek",
    /// "appearance/font", "library/excludedFolders", "columns/keys"…).
    /// The output device, the "follow the track's rate" switch and the
    /// buffer are applied to the player by the window itself before this.
    fn setting_changed(&self, _key: &str) {}
    /// The appearance changed (a skin, the accent, a style…): theme the
    /// other windows again (theme::Look::load + theme::apply).
    fn appearance_changed(&self) {}
    fn audio_status(&self) -> AudioStatus {
        AudioStatus::default()
    }
    fn library_status(&self) -> LibraryStatus {
        LibraryStatus::default()
    }
    fn covers_status(&self) -> CoversStatus {
        CoversStatus::default()
    }
    fn rescan_devices(&self) {}
    fn release_output(&self) {}
    /// Ctrl+Q in the window: Sonkore quits.
    fn quit(&self) {}
    /// Plays to `key` from now on (the track carries on there).
    fn set_device(&self, _key: &str) {}
    /// A test signal: kind "tone" (rate in Hz), "dsd" (rate 64/128/256),
    /// "channels" or "dts".
    fn play_test_signal(&self, _kind: &str, _rate: u32, _channels: u32) {}
    fn stop_test_signal(&self) {}
    fn clear_downloaded_covers(&self) {}
    fn rescan_library(&self) {}
    fn restore_hidden_tracks(&self) {}
    fn add_library_folder(&self, _path: &str) {}
    /// Its tracks leave the queue too (what the player changed reaches the
    /// main window, as when the folder is removed from the sidebar).
    fn remove_library_folder(&self, _path: &str) {}
    /// Restart Sonkore (a new interface scale).
    fn restart(&self) {}
    /// Every setting back to its default, then a restart (the settings are
    /// already cleared when this is called).
    fn restore_defaults(&self) {}
    /// The window was closed.
    fn closed(&self) {}
}

/// Hooks that do nothing.
pub struct NoHooks;
impl SettingsHooks for NoHooks {}

/// A folder or a file chosen in the desktop's dialog, which runs on a
/// thread of its own (zenity used to freeze the whole app while open); `then`
/// runs on the Settings window if it is still open.
fn choose(title: &str, folder: bool, then: impl FnOnce(&SettingsUi, String) + 'static) {
    let request = crate::file_dialog::Request { title: title.to_owned(), directory: folder, ..Default::default() };
    crate::file_dialog::ask(request, move |paths| {
        if let (Some(path), Some(ui)) = (paths.into_iter().next(), current()) {
            then(&ui, path);
        }
    });
}

// --- The window ---------------------------------------------------------------------

thread_local! {
    static CURRENT: RefCell<Option<Rc<SettingsUi>>> = const { RefCell::new(None) };
    /// The outputs last listed (shown while they are listed again).
    static KNOWN_DEVICES: RefCell<Option<Vec<sonkore_engine::output::DeviceInfo>>> = const { RefCell::new(None) };
}

/// Opens the settings window (or raises it), the tab last shown.
pub fn open(ctx: &Rc<Ctx>, hooks: Rc<dyn SettingsHooks>) -> Result<Rc<SettingsUi>, slint::PlatformError> {
    if let Some(ui) = CURRENT.with(|c| c.borrow().clone()) {
        ui.refresh();
        ui.show()?;
        return Ok(ui);
    }
    let ui = SettingsUi::new(ctx, hooks)?;
    CURRENT.with(|c| *c.borrow_mut() = Some(ui.clone()));
    ui.show()?;
    Ok(ui)
}

/// Lists the outputs on a thread of its own; the settings window shows them
/// when they come (called at start too, so the first opening has them).
pub fn list_devices() {
    std::thread::spawn(|| {
        let list = sonkore_engine::output::devices();
        let _ = slint::invoke_from_event_loop(move || {
            let before = KNOWN_DEVICES.with(|k| k.borrow_mut().replace(list.clone()));
            if let Some(ui) = current() {
                ui.set_devices(list.clone());
            }
            crate::hotplug::listed(before.as_deref(), &list);
        });
    });
}

/// The open settings window, if any (to refresh its status from outside).
pub fn current() -> Option<Rc<SettingsUi>> {
    CURRENT.with(|c| c.borrow().clone())
}

/// Settings › Restore all settings is restarting the app: the close must not
/// write back what was just cleared (the window's geometry, the output's sound
/// profile from the equalizer still in memory).
pub static RESTORING_DEFAULTS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn restoring_defaults() -> bool {
    RESTORING_DEFAULTS.load(std::sync::atomic::Ordering::Relaxed)
}

/// An audio setting changed elsewhere (the volume menus): the open window
/// shows it (its radio kept the old mode).
pub fn refresh_audio() {
    if let Some(ui) = current() {
        ui.fill_audio();
    }
}

pub struct SettingsUi {
    window: SettingsWindow,
    ctx: Rc<Ctx>,
    hooks: Rc<dyn SettingsHooks>,
    /// Editing the mini player's appearance.
    editing_compact: Cell<bool>,
    /// The fixed rate to come back to (Sample rate › Mode).
    last_fixed: Cell<i64>,
    /// The test signal started here ("rate", "channels", "dts").
    test_running: RefCell<String>,
    devices: RefCell<Vec<sonkore_engine::output::DeviceInfo>>,
    save_timer: Timer,
    status_timer: Timer,
}

/// The size a window had when closed (widgets/windows/<key>, @Size(w h)).
fn saved_size(settings: &Settings, key: &str) -> Option<(f32, f32)> {
    let Some(Value::Other(s)) = settings.value(&format!("widgets/windows/{key}")) else { return None };
    let inner = s.strip_prefix("@Size(")?.strip_suffix(')')?;
    let (w, h) = inner.split_once(' ')?;
    let (w, h) = (w.trim().parse::<f32>().ok()?, h.trim().parse::<f32>().ok()?);
    (w > 0.0 && h > 0.0).then_some((w, h))
}

impl SettingsUi {
    /// The window, filled and connected; not shown yet.
    pub fn new(ctx: &Rc<Ctx>, hooks: Rc<dyn SettingsHooks>) -> Result<Rc<SettingsUi>, slint::PlatformError> {
        let window = SettingsWindow::new()?;
        crate::equalizer_ui::install_metrics(&window);
        let ui = Rc::new(SettingsUi {
            window,
            ctx: ctx.clone(),
            hooks,
            editing_compact: Cell::new(false),
            last_fixed: Cell::new(96000),
            test_running: RefCell::new(String::new()),
            devices: RefCell::new(Vec::new()),
            save_timer: Timer::default(),
            status_timer: Timer::default(),
        });
        ui.connect();
        ui.load_devices();
        ui.refresh();
        Ok(ui)
    }

    pub fn window(&self) -> &SettingsWindow {
        &self.window
    }

    /// Shows the window at the size it had (920 × 760 the first time).
    pub fn show(self: &Rc<Self>) -> Result<(), slint::PlatformError> {
        let (w, h) = saved_size(&self.ctx.settings.borrow(), "settings").unwrap_or((920.0, 760.0));
        self.window.window().set_size(LogicalSize::new(w.max(560.0), h.max(420.0)));
        self.window.show()?;
        let weak = Rc::downgrade(self);
        self.status_timer.start(TimerMode::Repeated, std::time::Duration::from_millis(500), move || {
            if let Some(ui) = weak.upgrade() {
                ui.refresh_status();
            }
        });
        Ok(())
    }

    /// Everything read again from the settings and the hooks.
    pub fn refresh(&self) {
        self.fill_general();
        self.fill_audio();
        self.fill_library();
        self.fill_appearance();
        self.refresh_status();
        let tab = self.ctx.settings.borrow().text("ui/settingsTab", "general");
        let tab =
            if ["general", "appearance", "audio", "library"].contains(&tab.as_str()) { tab } else { "general".into() };
        self.window.global::<SettingsData>().set_tab(tab.into());
    }

    /// What the engine and the library report (called twice a second while
    /// the window is open; call it after a change to show it at once).
    pub fn refresh_status(&self) {
        let data = self.window.global::<SettingsData>();
        let audio = self.hooks.audio_status();
        let settings = self.ctx.settings.borrow();
        data.set_loaded(audio.loaded);
        data.set_output_description(audio.output_description.clone().into());
        data.set_bit_perfect(audio.bit_perfect);
        data.set_output_reserved(audio.output_reserved);
        let key = settings.text("playback/device", "");
        let direct = self.devices.borrow().iter().any(|d| d.direct && d.key == key) || audio.output_exclusive_capable;
        data.set_direct_output(direct);
        data.set_hardware_volume_supported(audio.hardware_volume_supported.unwrap_or(cfg!(target_os = "linux")));
        let mode = settings.int("playback/volumeMode", 2).clamp(0, 2);
        let info = audio.hardware_volume_info.clone().unwrap_or_else(|| volume_info(mode, direct, &key));
        data.set_hardware_volume_info(info.into());
        let headroom = if settings.flag("playback/headroomAuto", false) {
            audio.effective_headroom_db.unwrap_or(3.0)
        } else {
            settings.number("playback/headroomDb", 0.0)
        };
        data.set_effective_headroom(i18n::locale_number(headroom, 1).into());
        data.set_clipping(audio.clipping);
        data.set_current_gain(match audio.current_gain_db {
            // In the language's way ("−2,9" in Spanish).
            Some(g) if g.is_finite() => {
                format!("{}{}", if g >= 0.0 { "+" } else { "" }, i18n::locale_number(g, 1)).into()
            }
            _ => SharedString::new(),
        });
        if !audio.test_signal_playing {
            self.test_running.borrow_mut().clear();
        }
        data.set_test_running(self.test_running.borrow().clone().into());
        data.set_dts_available(audio.dts_test_available);
        data.set_components(audio.components.clone().unwrap_or_else(default_components).into());
        // The library.
        let lib = self.hooks.library_status();
        let count = lib.count.unwrap_or_else(|| self.ctx.library.borrow().tracks.len());
        data.set_scanning(lib.scanning);
        data.set_count_text(
            if lib.scanning {
                trf(CTX, "Scanning… {0} files", &[&lib.scanned_files.to_string()])
            } else {
                trf(CTX, "{0} tracks indexed", &[&count.to_string()])
            }
            .into(),
        );
        data.set_hidden_count(lib.hidden_count as i32);
        data.set_loudness_status(
            if lib.analyzing_loudness {
                trf(CTX, "Measuring tracks without ReplayGain tags… {0} of {1}. Files are never modified.", &[&lib.loudness_done.to_string(), &lib.loudness_total.to_string()])
            } else if lib.tracks_without_loudness > 0 {
                trf(CTX, "{0} tracks have no loudness data yet. They are measured in the background; files are never modified.", &[&lib.tracks_without_loudness.to_string()])
            } else {
                t("Every track in the library has loudness data, from its tags or measured by Sonkore.")
            }
            .into(),
        );
        let covers = self.hooks.covers_status();
        let online = settings.flag("library/onlineCovers", false);
        data.set_covers_status(
            if !online {
                t("Only the pictures in your files and folders are shown.")
            } else if covers.pending > 0 {
                trf(
                    CTX,
                    "Looking up {0} albums… {1} covers fetched so far.",
                    &[&covers.pending.to_string(), &covers.fetched.to_string()],
                )
            } else if !covers.last_error.is_empty() {
                covers.last_error.clone()
            } else {
                t("Albums without a picture get one from the Cover Art Archive as they are shown.")
            }
            .into(),
        );
    }

    // --- Filling the tabs ----------------------------------------------------------

    fn fill_general(&self) {
        let data = self.window.global::<SettingsData>();
        let s = self.ctx.settings.borrow();
        let mut languages = vec![opt(&t("Automatic (system)"), "")];
        for (code, name) in [
            ("en", "English"),
            ("es", "Español"),
            ("pt_BR", "Português (Brasil)"),
            ("fr", "Français"),
            ("de", "Deutsch"),
            ("it", "Italiano"),
        ] {
            languages.push(opt(name, code));
        }
        data.set_languages(model(languages));
        data.set_language(s.text("appearance/language", "").into());
        let mut fonts = vec![opt(&t("System"), "")];
        for (id, name, _) in crate::fonts::FONTS {
            fonts.push(opt(name, id));
        }
        data.set_fonts(model(fonts));
        data.set_font(crate::fonts::id_of(&s.text("appearance/font", "inter")).into());
        let scale = s.number("appearance/uiScale", 1.0);
        data.set_scale(scale_value(scale).into());
        let language_restart = i18n::differs_from_loaded(&s.text("appearance/language", ""));
        data.set_language_restart(language_restart);
        data.set_scale_restart((scale - applied_scale()).abs() > 1e-3);
        data.set_restart_required(language_restart || (scale - applied_scale()).abs() > 1e-3);
        data.set_resume(s.flag("playback/resume", true));
        data.set_live_seek(s.flag("ui/liveSeek", true));
        data.set_stop_button(s.flag("ui/stopButton", false));
        data.set_search_bar(s.flag("ui/searchBar", true));
        let start = s.text("ui/startPage", "last");
        let pages = ["last", "playing", "tracks", "favorites", "albums", "artists", "genres", "folders", "queue"];
        data.set_start_page(if pages.contains(&start.as_str()) { start } else { "last".into() }.into());
        data.set_track_browser(if s.flag("ui/trackBrowser", false) { "true" } else { "false" }.into());
        data.set_wheel_step(s.int("ui/wheelStep", 180).to_string().into());
        data.set_tray_icon(s.flag("ui/trayIcon", false));
        data.set_close_to_tray(s.flag("ui/closeToTray", false));
        data.set_mpris_available(cfg!(target_os = "linux"));
        data.set_mpris(s.flag("system/mpris", true));
        data.set_version(env!("CARGO_PKG_VERSION").into());
    }

    /// Lists the outputs off the interface's thread (a sound server may take
    /// its time to answer); the list known from before shows meanwhile.
    fn load_devices(&self) {
        if let Some(known) = KNOWN_DEVICES.with(|k| k.borrow().clone()) {
            *self.devices.borrow_mut() = known;
        }
        if std::env::var_os("SONKORE_SCREENSHOT").is_some() {
            // Offscreen there is no event loop to come back to.
            *self.devices.borrow_mut() = sonkore_engine::output::devices();
            return;
        }
        list_devices();
    }

    /// The outputs to list, when the app has enumerated them itself (else
    /// the window asks sonkore_engine::output::devices() when it opens and
    /// on Rescan devices).
    pub fn set_devices(&self, devices: Vec<sonkore_engine::output::DeviceInfo>) {
        *self.devices.borrow_mut() = devices;
        self.fill_audio();
        self.refresh_status();
    }

    /// The Audio tab shown again from the settings (an output's own sound
    /// was put back).
    pub fn refresh_sound(&self) {
        self.fill_audio();
        self.refresh_status();
    }

    fn fill_audio(&self) {
        let data = self.window.global::<SettingsData>();
        let s = self.ctx.settings.borrow();
        // The outputs, "System default" heading the sound server's group.
        let devices = self.devices.borrow();
        let current = s.text("playback/device", "");
        // (Not while the list is still coming.)
        let missing = !current.is_empty() && !devices.is_empty() && !devices.iter().any(|d| d.key == current);
        data.set_device_missing(missing);
        let effective = if missing { String::new() } else { current.clone() };
        let all = crate::audio::output_list(&devices);
        let mut backends: Vec<String> = Vec::new();
        for d in &all {
            if !backends.contains(&d.backend) {
                backends.push(d.backend.clone());
            }
        }
        let mut rows = Vec::new();
        for backend in &backends {
            for (i, d) in all.iter().filter(|d| &d.backend == backend).enumerate() {
                rows.push(OutputDevice {
                    key: d.key.clone().into(),
                    name: d.name.clone().into(),
                    detail: d.detail.clone().into(),
                    backend: d.backend.clone().into(),
                    hint: backend_hint(backend).into(),
                    first: i == 0,
                    direct: d.direct,
                    is_default: d.is_default,
                    selected: d.key == effective,
                });
            }
        }
        data.set_devices(ModelRc::new(VecModel::from(rows)));
        data.set_release_when_idle(s.flag("playback/releaseDacWhenIdle", true));
        data.set_sound_per_output(s.flag("playback/soundPerOutput", true));
        data.set_volume_mode(s.int("playback/volumeMode", 2).clamp(0, 2).to_string().into());
        data.set_follow_rate(s.flag("playback/followTrackRate", true));
        data.set_windows(cfg!(windows));
        data.set_exclusive(s.flag("playback/exclusive", true));
        data.set_play_from_memory(s.flag("playback/playFromMemory", true));
        data.set_buffer(pick(s.int("playback/bufferMs", 150), &[100, 150, 250, 400, 750]).into());
        // Sample rate: a fixed rate wins over upsampling (as the engine does).
        let fixed = s.int("playback/fixedRate", 0).clamp(0, 768000);
        let upsampling = s.int("playback/upsampling", 0).clamp(0, 3);
        data.set_rate_mode(if fixed != 0 {
            1
        } else if upsampling != 0 {
            2
        } else {
            0
        });
        data.set_fixed_rate(pick(fixed, &[44100, 48000, 88200, 96000, 176400, 192000, 352800, 384000]).into());
        data.set_upsampling(upsampling.max(1).to_string().into());
        data.set_resample_filter(s.int("playback/resampleFilter", 0).clamp(0, 3).to_string().into());
        data.set_resync(pick(s.int("playback/resyncDelayMs", 250), &[0, 250, 500, 1000, 2000]).into());
        data.set_dsd_mode(s.int("playback/dsdOutput", 1).clamp(0, 2).to_string().into());
        data.set_pcm_to_dsd(s.int("playback/pcmToDsd", 0).clamp(0, 5).to_string().into());
        data.set_convolution_file(s.text("playback/convolutionFile", "").into());
        data.set_convolution_enabled(s.flag("playback/convolution", false));
        data.set_crossfeed(s.int("playback/crossfeed", 0).clamp(0, 3).to_string().into());
        data.set_headroom_auto(s.flag("playback/headroomAuto", false));
        data.set_headroom_db(s.number("playback/headroomDb", 0.0).clamp(-24.0, 0.0) as f32);
        data.set_limiter(s.flag("playback/limiter", false));
        let ceiling = s.number("playback/limiterCeilingDb", -1.0).clamp(-6.0, 0.0);
        let rounded = (ceiling * 10.0).round() / 10.0;
        let ceiling_value = [("-0.1", -0.1), ("-0.5", -0.5), ("-1", -1.0), ("-2", -2.0), ("-3", -3.0)]
            .iter()
            .find(|(_, v)| (v - rounded).abs() < 1e-6)
            .map_or("-0.1", |(k, _)| k);
        data.set_limiter_ceiling(ceiling_value.into());
        data.set_limiter_ceiling_text(i18n::locale_number(ceiling, 1).into());
        data.set_dither(s.int("playback/ditherMode", 1).clamp(0, 2).to_string().into());
        data.set_hdcd(s.flag("playback/hdcd", false));
        data.set_replay_gain(s.int("playback/replayGain", 0).clamp(0, 2).to_string().into());
        data.set_replay_gain_preamp(s.number("playback/replayGainPreamp", 0.0).clamp(-15.0, 15.0) as f32);
        data.set_replay_gain_prevent_clipping(s.flag("playback/replayGainPreventClipping", true));
    }

    fn fill_library(&self) {
        let data = self.window.global::<SettingsData>();
        let s = self.ctx.settings.borrow();
        data.set_online_covers(s.flag("library/onlineCovers", false));
        let folders: Vec<SharedString> = self.ctx.library.borrow().folders.iter().map(|f| f.into()).collect();
        data.set_folders(ModelRc::new(VecModel::from(folders)));
        let excluded: Vec<SharedString> =
            s.list("library/excludedFolders").into_iter().filter(|f| !f.is_empty()).map(Into::into).collect();
        data.set_excluded(ModelRc::new(VecModel::from(excluded)));
        data.set_min_duration(pick(s.int("library/minDuration", 0), &[0, 5, 10, 30, 60]).into());
        data.set_watch_folders(s.flag("library/watchFolders", true));
        data.set_rescan_on_startup(s.flag("library/rescanOnStartup", true));
        let keys = column_keys(&s);
        let count = keys.len() as i32;
        let lines: Vec<ColumnLine> = CATALOG
            .iter()
            .map(|c| ColumnLine {
                key: c.key.into(),
                title: tr("TrackColumns", c.title).into(),
                position: keys.iter().position(|k| k == c.key).map_or(-1, |p| p as i32),
                count,
            })
            .collect();
        data.set_columns(ModelRc::new(VecModel::from(lines)));
    }

    fn fill_appearance(&self) {
        let a = self.window.global::<Appearance>();
        let skins = self.window.global::<Skins>().get_list();
        let n = skins.row_count();
        let settings = self.ctx.settings.borrow();
        let look = theme::Look::load(&settings, n);
        let compact = self.editing_compact.get();
        let values = compact_values(&settings);
        let edited = |key: &str| -> Json {
            if compact {
                if let Some(v) = values.get(key) {
                    return v.clone();
                }
            }
            full_value(&settings, key)
        };
        a.set_editing_compact(compact);
        // The skins: the default first, the graphite one (the app icon's) next.
        let inverted: Vec<String> = settings
            .text("appearance/invertedSkins", "")
            .split(',')
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        let mut order = vec![0usize];
        if n > 1 {
            order.push(n - 1);
            order.extend(1..n - 1);
        }
        let tiles: Vec<SkinTile> = order
            .into_iter()
            .filter_map(|i| skins.row_data(i).map(|s| (i, s)))
            .map(|(i, s)| {
                let inv = inverted.iter().any(|name| *name == s.name.as_str());
                let (frame, pane) = theme::tones(&s, inv);
                SkinTile {
                    index: i as i32,
                    name: s.name.clone(),
                    hint: tr("Theme", s.hint.as_str()).into(),
                    frame,
                    pane,
                    accent: s.accent,
                    border: s.border,
                    surface_hi: s.surface_hi,
                    text_dim: s.text_dim,
                    text_faint: s.text_faint,
                    radius: s.radius as f32,
                    inverted: inv,
                }
            })
            .collect();
        a.set_skins(ModelRc::new(VecModel::from(tiles)));
        let skin_index = int_of(&edited("skinIndex"), 0).clamp(0, n.saturating_sub(1) as i64);
        a.set_skin(skin_index as i32);
        // The accent swatches and the density preview: the skin shown now.
        if let Some(shown) = skins.row_data(look.skin_index) {
            a.set_skin_accent(shown.accent);
            a.set_skin_on_accent(shown.on_accent);
            a.set_skin_sidebar(shown.sidebar);
        }
        a.set_accent_override(text_of(&edited("accentOverride"), "").into());
        a.set_icon_style(text_of(&edited("iconStyle"), "solid").into());
        a.set_transport_style(text_of(&edited("transportStyle"), "sonkore").into());
        a.set_slider_style(text_of(&edited("sliderStyle"), "line").into());
        a.set_volume_style(text_of(&edited("volumeStyle"), "line").into());
        a.set_window_buttons(text_of(&edited("windowButtons"), "windows").into());
        a.set_window_radius(int_of(&edited("windowRadius"), 8).to_string().into());
        a.set_window_outline(text_of(&edited("windowOutline"), "highlight").into());
        a.set_backdrop(text_of(&edited("backdrop"), "plain").into());
        a.set_visualizer(text_of(&edited("visualizer"), "bars").into());
        let density = text_of(&edited("density"), "comfortable");
        let skin_compact = skins.row_data(skin_index as usize).is_some_and(|s| s.compact);
        a.set_density(
            if density == "auto" { if skin_compact { "compact" } else { "comfortable" } } else { density.as_str() }
                .into(),
        );
        // The list density being edited.
        let list = text_of(&edited("listDensity"), "");
        let list = if ["spacious", "comfortable", "compact", "list"].contains(&list.as_str()) {
            list
        } else if density == "compact" || (density == "auto" && skin_compact) {
            "compact".into()
        } else if density == "spacious" {
            "spacious".into()
        } else {
            "comfortable".into()
        };
        a.set_list_density(list.into());
        a.set_list_covers(bool_of(&edited("listCovers"), true));
        a.set_alternate_rows(bool_of(&edited("alternateRows"), false));
        // The list density the window shows.
        let shown_compact = if look.density == "auto" {
            skins.row_data(look.skin_index).is_some_and(|s| s.compact)
        } else {
            look.density == "compact"
        };
        let shown = match look.list_density.as_str() {
            d @ ("spacious" | "comfortable" | "compact" | "list") => d.to_owned(),
            _ if shown_compact => "compact".into(),
            _ if look.density == "spacious" => "spacious".into(),
            _ => "comfortable".into(),
        };
        a.set_shown_list_density(shown.into());
        a.set_restart_for_corners(false); // the window is always see-through here
        let own: Vec<SharedString> = values.keys().map(|k| k.as_str().into()).collect();
        a.set_own(ModelRc::new(VecModel::from(own)));
        // The backdrop preview's colours: the accent's hue turned.
        let accent = window_accent(&look, &skins);
        a.set_backdrop_a2(shifted(accent, 0.11));
        a.set_backdrop_a3(shifted(accent, 0.86));
        a.set_wave(ModelRc::new(VecModel::from(wave_preview())));
        drop(settings);
        theme::apply(&self.window, &look);
    }

    // --- Answering the window --------------------------------------------------------

    fn connect(self: &Rc<Self>) {
        let data = self.window.global::<SettingsData>();
        let weak = Rc::downgrade(self);
        data.on_set(move |key, value| {
            if let Some(ui) = weak.upgrade() {
                ui.set(&key, &value);
            }
        });
        let weak = Rc::downgrade(self);
        data.on_action(move |name, arg| {
            if let Some(ui) = weak.upgrade() {
                ui.action(&name, &arg);
            }
        });
        let a = self.window.global::<Appearance>();
        let weak = Rc::downgrade(self);
        a.on_set_compact(move |on| {
            if let Some(ui) = weak.upgrade() {
                ui.editing_compact.set(on);
                ui.fill_appearance();
            }
        });
        let weak = Rc::downgrade(self);
        a.on_edit(move |key, value| {
            if let Some(ui) = weak.upgrade() {
                ui.edit_appearance(&key, &value);
            }
        });
        let weak = Rc::downgrade(self);
        a.on_inherit(move |key| {
            if let Some(ui) = weak.upgrade() {
                {
                    let mut s = ui.ctx.settings.borrow_mut();
                    let mut values = compact_values(&s);
                    values.remove(key.as_str());
                    s.set_text("appearanceCompact/values", Json::Object(values).to_string());
                }
                ui.appearance_written();
            }
        });
        let weak = Rc::downgrade(self);
        a.on_pick_skin(move |index| {
            if let Some(ui) = weak.upgrade() {
                ui.edit_appearance("skinIndex", &index.to_string());
            }
        });
        let weak = Rc::downgrade(self);
        a.on_invert_skin(move |index| {
            if let Some(ui) = weak.upgrade() {
                ui.invert_skin(index as usize);
            }
        });
        let weak = Rc::downgrade(self);
        a.on_restart(move || {
            if let Some(ui) = weak.upgrade() {
                ui.hooks.restart();
            }
        });
        self.connect_window_actions();
    }

    /// The bar's buttons and the edges, through winit.
    fn connect_window_actions(self: &Rc<Self>) {
        let actions = self.window.global::<WindowActions>();
        let weak = self.window.as_weak();
        actions.on_minimize(move || {
            if let Some(w) = weak.upgrade() {
                crate::window_drag::minimize(&w);
            }
        });
        let weak = self.window.as_weak();
        actions.on_toggle_maximize(move || {
            if let Some(w) = weak.upgrade() {
                let on = !w.window().is_maximized();
                w.window().set_maximized(on);
                w.global::<WindowActions>().set_maximized(on);
            }
        });
        let weak = Rc::downgrade(self);
        actions.on_close(move || {
            if let Some(ui) = weak.upgrade() {
                ui.close();
            }
        });
        let weak = self.window.as_weak();
        actions.on_start_move(move || {
            if let Some(w) = weak.upgrade() {
                if crate::window_drag::start_move(&w) {
                    w.global::<WindowActions>().invoke_toggle_maximize();
                }
            }
        });
        let weak = self.window.as_weak();
        actions.on_start_resize(move |edge| {
            if let Some(w) = weak.upgrade() {
                crate::window_drag::start_resize(&w, edge.as_str());
            }
        });
        crate::window_drag::follow_maximized(&self.window, |w, on| w.global::<WindowActions>().set_maximized(on));
        let weak = Rc::downgrade(self);
        self.window.window().on_close_requested(move || {
            if let Some(ui) = weak.upgrade() {
                ui.close();
            }
            slint::CloseRequestResponse::HideWindow
        });
    }

    /// Hides the window, keeping its size.
    pub fn close(&self) {
        let w = self.window.window();
        if !w.is_maximized() {
            let size = w.size().to_logical(w.scale_factor());
            self.ctx.settings.borrow_mut().set(
                "widgets/windows/settings",
                Value::Other(format!("@Size({} {})", size.width.round(), size.height.round())),
            );
            self.save_soon();
        }
        self.status_timer.stop();
        let _ = self.window.hide();
        self.hooks.closed();
    }

    fn save_soon(&self) {
        let settings = self.ctx.settings.clone();
        self.save_timer.start(TimerMode::SingleShot, std::time::Duration::from_millis(300), move || {
            let _ = settings.borrow_mut().save();
        });
    }

    /// A setting written: saved, applied, shown.
    fn changed(&self, key: &str) {
        self.save_soon();
        self.hooks.setting_changed(key);
    }

    fn set(&self, key: &str, value: &str) {
        let on = value == "true";
        // A number from a field comes as Slint writes it, with the
        // language's decimal comma ("-3,5" in Spanish).
        let number = |fallback: f64| i18n::parse_number(value).unwrap_or(fallback);
        let int = || number(0.0).round() as i64;
        match key {
            "ui/settingsTab" => {
                self.ctx.settings.borrow_mut().set_text(key, value);
                self.save_soon();
                return;
            }
            // Text.
            "appearance/language" | "appearance/font" | "ui/startPage" => {
                // (A new language shows after a restart: fill_general offers it.)
                self.ctx.settings.borrow_mut().set_text(key, value);
            }
            "appearance/uiScale" => {
                let v = number(1.0);
                self.ctx.settings.borrow_mut().set_number(key, ((v * 100.0).round() / 100.0).clamp(0.5, 3.0));
            }
            // Switches.
            "playback/resume"
            | "ui/liveSeek"
            | "ui/stopButton"
            | "ui/searchBar"
            | "ui/trackBrowser"
            | "ui/trayIcon"
            | "ui/closeToTray"
            | "system/mpris"
            | "playback/releaseDacWhenIdle"
            | "playback/soundPerOutput"
            | "playback/followTrackRate"
            | "playback/exclusive"
            | "playback/playFromMemory"
            | "playback/convolution"
            | "playback/headroomAuto"
            | "playback/limiter"
            | "playback/hdcd"
            | "playback/replayGainPreventClipping"
            | "library/onlineCovers"
            | "library/watchFolders"
            | "library/rescanOnStartup" => {
                // (The engine takes playback/… in the hook's audio::apply,
                // which also opens the track again if the output closed.)
                self.ctx.settings.borrow_mut().set_flag(key, on);
            }
            // Whole numbers.
            "ui/wheelStep"
            | "playback/volumeMode"
            | "playback/bufferMs"
            | "playback/fixedRate"
            | "playback/upsampling"
            | "playback/resampleFilter"
            | "playback/resyncDelayMs"
            | "playback/dsdOutput"
            | "playback/pcmToDsd"
            | "playback/crossfeed"
            | "playback/ditherMode"
            | "playback/replayGain"
            | "library/minDuration" => {
                self.ctx.settings.borrow_mut().set_int(key, int());
            }
            // Decimals.
            "playback/headroomDb" => {
                let v = number(0.0).clamp(-24.0, 0.0);
                self.ctx.settings.borrow_mut().set_number(key, (v * 10.0).round() / 10.0);
            }
            "playback/limiterCeilingDb" => {
                let v = number(-1.0).clamp(-6.0, 0.0);
                self.ctx.settings.borrow_mut().set_number(key, v);
            }
            "playback/replayGainPreamp" => {
                let v = number(0.0).clamp(-15.0, 15.0);
                self.ctx.settings.borrow_mut().set_number(key, (v * 10.0).round() / 10.0);
            }
            // Sample rate › Mode (the fixed rate
            // chosen before comes back).
            "rate-mode" => {
                let mode = int();
                {
                    let mut s = self.ctx.settings.borrow_mut();
                    let fixed = s.int("playback/fixedRate", 0);
                    if fixed != 0 {
                        self.last_fixed.set(fixed);
                    }
                    s.set_int("playback/fixedRate", if mode == 1 { self.last_fixed.get() } else { 0 });
                    s.set_int("playback/upsampling", if mode == 2 { 1 } else { 0 });
                }
                self.fill_audio();
                self.changed("playback/fixedRate");
                self.changed("playback/upsampling");
                return;
            }
            _ => {
                eprintln!("settings: unknown key {key}");
                return;
            }
        }
        match key {
            k if k.starts_with("playback/") => self.fill_audio(),
            k if k.starts_with("library/") => self.fill_library(),
            _ => self.fill_general(),
        }
        self.refresh_status();
        self.changed(key);
    }

    fn action(&self, name: &str, arg: &str) {
        match name {
            "restart" => self.hooks.restart(),
            "restore-defaults" => {
                {
                    let mut s = self.ctx.settings.borrow_mut();
                    for group in GROUPS {
                        for key in s.keys_in(group) {
                            s.remove(&key);
                        }
                    }
                    let _ = s.save();
                }
                RESTORING_DEFAULTS.store(true, std::sync::atomic::Ordering::Relaxed);
                self.hooks.restore_defaults();
            }
            "select-device" => {
                // The hook first: it keeps the sound of the output being left.
                self.hooks.set_device(arg);
                self.ctx.settings.borrow_mut().set_text("playback/device", arg);
                self.fill_audio();
                self.refresh_status();
                self.changed("playback/device");
            }
            "rescan-devices" => {
                self.hooks.rescan_devices();
                self.load_devices();
            }
            "release-dac" => self.hooks.release_output(),
            "quit" => self.hooks.quit(),
            "test-signal" => {
                let audio = self.hooks.audio_status();
                if audio.test_signal_playing && *self.test_running.borrow() == arg {
                    self.hooks.stop_test_signal();
                    self.test_running.borrow_mut().clear();
                } else {
                    *self.test_running.borrow_mut() = arg.to_owned();
                    let data = self.window.global::<SettingsData>();
                    match arg {
                        "rate" => {
                            let i = data.get_test_rate().parse::<usize>().unwrap_or(0).min(TEST_RATES.len() - 1);
                            let (kind, rate) = TEST_RATES[i];
                            self.hooks.play_test_signal(kind, rate, 2);
                        }
                        "channels" => {
                            let channels = data.get_test_channels().parse::<u32>().unwrap_or(2);
                            self.hooks.play_test_signal("channels", 48000, channels);
                        }
                        _ => self.hooks.play_test_signal("dts", 44100, 2),
                    }
                }
                self.refresh_status();
                // Until the engine says it plays, the button shows Stop.
                self.window.global::<SettingsData>().set_test_running(self.test_running.borrow().clone().into());
            }
            "load-convolution" => {
                choose(&t("Convolution (FIR)"), false, |ui, path| {
                    {
                        let mut s = ui.ctx.settings.borrow_mut();
                        s.set_text("playback/convolutionFile", &path);
                        s.set_flag("playback/convolution", true);
                    }
                    ui.fill_audio();
                    ui.changed("playback/convolutionFile");
                });
            }
            "remove-convolution" => {
                {
                    let mut s = self.ctx.settings.borrow_mut();
                    s.set_text("playback/convolutionFile", "");
                    s.set_flag("playback/convolution", false);
                }
                self.fill_audio();
                self.changed("playback/convolutionFile");
            }
            "clear-covers" => self.hooks.clear_downloaded_covers(),
            "add-folder" => {
                choose(&tr("TracksPage", "Add music folder"), true, |ui, dir| {
                    {
                        let mut lib = ui.ctx.library.borrow_mut();
                        if !lib.folders.contains(&dir) {
                            lib.folders.push(dir.clone());
                        }
                    }
                    ui.hooks.add_library_folder(&dir);
                    ui.fill_library();
                });
            }
            "remove-folder" => {
                // Asks first, as removing a folder does everywhere.
                let name = arg.rsplit('/').next().unwrap_or(arg);
                let prefix = format!("{}/", arg.trim_end_matches('/'));
                let tracks = self.ctx.library.borrow().tracks.iter().filter(|t| t.path.starts_with(&prefix)).count();
                let o = self.window.global::<SettingsOverlay>();
                o.set_confirm_title(trf("Main", "Remove “{0}” from the library?", &[name]).into());
                o.set_confirm_text(trf("Main", "Its {0} tracks disappear from Sonkore; the files stay on disk. You can add the folder again with +.", &[&tracks.to_string()]).into());
                o.set_confirm_accept(tr("Main", "Remove folder").into());
                o.set_confirm_action("remove-folder-now".into());
                o.set_confirm_arg(arg.into());
                o.set_confirm_shown(true);
            }
            "remove-folder-now" => {
                self.ctx.library.borrow_mut().folders.retain(|f| f != arg);
                self.hooks.remove_library_folder(arg);
                self.fill_library();
            }
            "rescan" => self.hooks.rescan_library(),
            "restore-hidden" => self.hooks.restore_hidden_tracks(),
            "skip-folder" => {
                choose(&t("Skip a folder…"), true, |ui, dir| {
                    let mut list = ui.ctx.settings.borrow().list("library/excludedFolders");
                    list.retain(|f| !f.is_empty());
                    if !list.contains(&dir) {
                        list.push(dir);
                        ui.ctx.settings.borrow_mut().set_list("library/excludedFolders", &list);
                        ui.fill_library();
                        ui.changed("library/excludedFolders");
                    }
                });
            }
            "unskip-folder" => {
                let mut list = self.ctx.settings.borrow().list("library/excludedFolders");
                list.retain(|f| !f.is_empty() && f != arg);
                self.ctx.settings.borrow_mut().set_list("library/excludedFolders", &list);
                self.fill_library();
                self.changed("library/excludedFolders");
            }
            "column-show" | "column-hide" | "column-left" | "column-right" => {
                let mut keys = column_keys(&self.ctx.settings.borrow());
                let at = keys.iter().position(|k| k == arg);
                match (name, at) {
                    ("column-show", None) => keys.push(arg.to_owned()),
                    ("column-hide", Some(i)) if keys.len() > 1 => {
                        keys.remove(i);
                    }
                    ("column-left", Some(i)) if i > 0 => keys.swap(i, i - 1),
                    ("column-right", Some(i)) if i + 1 < keys.len() => keys.swap(i, i + 1),
                    _ => {
                        self.fill_library();
                        return;
                    }
                }
                let json = Json::Array(keys.into_iter().map(Json::String).collect()).to_string();
                self.ctx.settings.borrow_mut().set_text("columns/keys", json);
                self.fill_library();
                self.changed("columns/keys");
            }
            "reset-columns" => {
                {
                    let mut s = self.ctx.settings.borrow_mut();
                    s.set_text("columns/keys", "");
                    s.set_text("columns/widths", "");
                }
                self.fill_library();
                self.changed("columns/keys");
            }
            _ => eprintln!("settings: unknown action {name}"),
        }
    }

    // --- The appearance -------------------------------------------------------------

    /// A setting of the view being edited: the full view's, or the mini
    /// player's own (appearanceCompact/values, JSON).
    fn edit_appearance(&self, key: &str, value: &str) {
        let json = match key {
            "skinIndex" | "windowRadius" => Json::from(value.parse::<i64>().unwrap_or(0)),
            "listCovers" | "alternateRows" => Json::Bool(value == "true"),
            _ => Json::String(value.to_owned()),
        };
        {
            let mut s = self.ctx.settings.borrow_mut();
            if self.editing_compact.get() {
                let mut values = compact_values(&s);
                values.insert(key.to_owned(), json);
                s.set_text("appearanceCompact/values", Json::Object(values).to_string());
            } else if key == "visualizer" {
                s.set_text("ui/visualizer", value);
            } else {
                match &json {
                    Json::Number(n) => s.set_int(&format!("appearance/{key}"), n.as_i64().unwrap_or(0)),
                    Json::Bool(b) => s.set_flag(&format!("appearance/{key}"), *b),
                    _ => s.set_text(&format!("appearance/{key}"), value),
                }
            }
        }
        self.appearance_written();
    }

    fn invert_skin(&self, index: usize) {
        let skins = self.window.global::<Skins>().get_list();
        let Some(skin) = skins.row_data(index) else { return };
        {
            let mut s = self.ctx.settings.borrow_mut();
            let mut names: Vec<String> = s
                .text("appearance/invertedSkins", "")
                .split(',')
                .filter(|n| !n.is_empty())
                .map(str::to_owned)
                .collect();
            let on = !names.iter().any(|n| *n == skin.name.as_str());
            names.retain(|n| *n != skin.name.as_str());
            if on {
                names.push(skin.name.to_string());
            }
            s.set_text("appearance/invertedSkins", names.join(","));
        }
        self.appearance_written();
    }

    fn appearance_written(&self) {
        self.fill_appearance();
        self.save_soon();
        self.hooks.appearance_changed();
    }
}

// --- Helpers ------------------------------------------------------------------------

/// The groups restoring the defaults clears.
const GROUPS: [&str; 16] = [
    "appearance",
    "appearanceCompact",
    "columns",
    "equalizer",
    "pitch",
    "playback",
    "soundProfiles",
    "soundtouch",
    "ui",
    "library",
    "system",
    "advanced",
    "widgets",
    "General",
    "eq",
    "session",
];

/// The test signals' rates (Test signals › DAC sample rate).
const TEST_RATES: [(&str, u32); 11] = [
    ("tone", 44100),
    ("tone", 48000),
    ("tone", 88200),
    ("tone", 96000),
    ("tone", 176400),
    ("tone", 192000),
    ("tone", 352800),
    ("tone", 384000),
    ("dsd", 64),
    ("dsd", 128),
    ("dsd", 256),
];

fn opt(text: &str, value: &str) -> Opt {
    Opt { text: text.into(), value: value.into() }
}

fn model(v: Vec<Opt>) -> ModelRc<Opt> {
    ModelRc::new(VecModel::from(v))
}

/// The value of a combo box: the setting when it is one of the choices,
/// else the first.
fn pick(v: i64, choices: &[i64]) -> String {
    if choices.contains(&v) { v } else { choices[0] }.to_string()
}

/// The interface scale as the combo box's value.
fn scale_value(scale: f64) -> &'static str {
    const SCALES: [(f64, &str); 9] = [
        (0.75, "0.75"),
        (0.85, "0.85"),
        (0.9, "0.9"),
        (1.0, "1"),
        (1.1, "1.1"),
        (1.25, "1.25"),
        (1.5, "1.5"),
        (1.75, "1.75"),
        (2.0, "2"),
    ];
    SCALES.iter().min_by(|a, b| (a.0 - scale).abs().total_cmp(&(b.0 - scale).abs())).map_or("1", |s| s.1)
}

/// The scale this run started with (main.rs sets SLINT_SCALE_FACTOR_MULTIPLIER from it).
fn applied_scale() -> f64 {
    std::env::var("SLINT_SCALE_FACTOR_MULTIPLIER").ok().and_then(|v| v.parse().ok()).unwrap_or(1.0)
}

fn default_components() -> String {
    "Slint 1.19 · PipeWire · ALSA".into()
}

/// The shown columns' keys (columns/keys, else the default).
fn column_keys(s: &Settings) -> Vec<String> {
    let keys: Vec<String> = serde_json::from_str::<Vec<String>>(&s.text("columns/keys", ""))
        .unwrap_or_default()
        .into_iter()
        .filter(|k| CATALOG.iter().any(|c| c.key == k.as_str()))
        .collect();
    if keys.is_empty() { DEFAULT_KEYS.iter().map(|k| k.to_string()).collect() } else { keys }
}

/// The hint under an output's backend.
fn backend_hint(backend: &str) -> String {
    match backend {
        "ALSA" => t(
            "Direct hardware (hw:) — exclusive, no mixer, bit-perfect capable. PipeWire must not be using the device.",
        ),
        "PipeWire" => t(
            "The desktop's sound server, spoken to directly — shared with other apps; it switches to each track's sample rate (Follow the track's rate).",
        ),
        b if b.starts_with("PipeWire") => {
            t("Desktop sound server — shared with other apps; sample rate follows PipeWire's clock settings.")
        }
        "WASAPI" => t("Windows audio — exclusive mode bypasses the system mixer."),
        "ASIO" => {
            t("The DAC's own ASIO driver — direct, bit-perfect capable, and native DSD where the driver offers it.")
        }
        "Core Audio" => t("macOS — the device switches to the track's sample rate."),
        _ => String::new(),
    }
}

/// The line under Volume, without a mixer to ask (Linux).
fn volume_info(mode: i64, direct: bool, key: &str) -> String {
    let c = "DspController";
    match mode {
        0 => String::new(),
        1 if direct => tr(
            c,
            "This DAC has no volume control of its own: the volume stays fixed at 100 %, set it on the DAC or the amplifier. Choose Digital to set it from Sonkore.",
        ),
        1 => tr(
            c,
            "Only for direct (hw:) outputs: with this one the volume stays fixed at 100 %. Choose Digital to set it from Sonkore.",
        ),
        _ if !direct => tr(
            c,
            "Automatic, shared output: Sonkore's digital volume is used. With a direct (hw:) output, the DAC's own volume is used instead.",
        ),
        _ if key.starts_with("alsa|") => {
            tr(c, "Automatic: this DAC has no volume control of its own, so Sonkore's digital volume is used.")
        }
        _ => tr(c, "Automatic: Sonkore's digital volume is used with this output."),
    }
}

// The appearance settings, read as the view being edited sees them.

fn compact_values(s: &Settings) -> Map<String, Json> {
    match serde_json::from_str::<Json>(&s.text("appearanceCompact/values", "")) {
        Ok(Json::Object(m)) => m,
        _ => Map::new(),
    }
}

fn default_of(key: &str) -> Json {
    match key {
        "skinIndex" => Json::from(0),
        "windowRadius" => Json::from(8),
        "listCovers" => Json::Bool(true),
        "alternateRows" => Json::Bool(false),
        "iconStyle" => "solid".into(),
        "density" => "comfortable".into(),
        "sliderStyle" | "volumeStyle" => "line".into(),
        "backdrop" => "plain".into(),
        "windowButtons" => "windows".into(),
        "windowOutline" => "highlight".into(),
        "transportStyle" => "sonkore".into(),
        "visualizer" => "bars".into(),
        _ => Json::String(String::new()),
    }
}

/// The full view's value: appearance/<key> (ui/visualizer), else the default.
fn full_value(s: &Settings, key: &str) -> Json {
    let full = if key == "visualizer" { "ui/visualizer".to_owned() } else { format!("appearance/{key}") };
    s.value(&full).map(|v| Json::String(v.as_text())).unwrap_or_else(|| default_of(key))
}

fn text_of(v: &Json, fallback: &str) -> String {
    match v {
        Json::String(s) if !s.is_empty() => s.clone(),
        Json::Number(n) => n.to_string(),
        Json::Bool(b) => b.to_string(),
        _ => fallback.to_owned(),
    }
}

fn int_of(v: &Json, fallback: i64) -> i64 {
    match v {
        Json::Number(n) => n.as_i64().unwrap_or_else(|| n.as_f64().map_or(fallback, |f| f.round() as i64)),
        Json::String(s) => s.trim().parse::<f64>().map(|f| f.round() as i64).unwrap_or(fallback),
        _ => fallback,
    }
}

fn bool_of(v: &Json, fallback: bool) -> bool {
    match v {
        Json::Bool(b) => *b,
        Json::String(s) => {
            let t = s.trim();
            if t.is_empty() { fallback } else { !(t == "0" || t.eq_ignore_ascii_case("false")) }
        }
        Json::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        _ => fallback,
    }
}

/// The accent the window shows (the override, else the skin's).
fn window_accent(look: &theme::Look, skins: &ModelRc<crate::Skin>) -> Color {
    let skin = skins.row_data(look.skin_index).map(|s| s.accent).unwrap_or(Color::from_rgb_u8(0x1e, 0xd7, 0x60));
    let hex = look.accent_override.trim().trim_start_matches('#');
    match (hex.len(), u32::from_str_radix(hex, 16)) {
        (6, Ok(v)) => Color::from_rgb_u8((v >> 16) as u8, (v >> 8) as u8, v as u8),
        _ => skin,
    }
}

/// HSL: hue 0..1 (or -1 for greys), saturation, lightness.
fn to_hsl(c: Color) -> (f64, f64, f64) {
    let (r, g, b) = (c.red() as f64 / 255.0, c.green() as f64 / 255.0, c.blue() as f64 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    if max == min {
        return (-1.0, 0.0, l);
    }
    let d = max - min;
    let s = if l < 0.5 { d / (max + min) } else { d / (2.0 - max - min) };
    let h = if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    } / 6.0;
    (h, s, l)
}

fn from_hsl(h: f64, s: f64, l: f64) -> Color {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h * 6.0;
    let x = c * (1.0 - (hp.rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match hp as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    let to = |v: f64| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    Color::from_rgb_u8(to(r), to(g), to(b))
}

/// The backdrop's other colours (the accent's hue turned).
fn shifted(accent: Color, turn: f64) -> Color {
    let (h, s, l) = to_hsl(accent);
    let h = if h < 0.0 { 0.0 } else { h };
    from_hsl((h + turn).rem_euclid(1.0), s.max(0.4), l.max(0.35))
}

/// The waveform preview: a line every 2 px over 76 px.
fn wave_preview() -> Vec<f32> {
    let peaks: Vec<f64> =
        (0..60).map(|i| 0.25 + 0.75 * ((i as f64 * 0.7).sin() * (i as f64 * 0.23).cos()).abs()).collect();
    let (w, n) = (76.0f64, peaks.len());
    let mut out = Vec::new();
    let mut x = 0.0;
    while x + 1.0 <= w {
        let a = (x / w * n as f64) as usize;
        let b = (a + 1).max(((x + 2.0) / w * n as f64) as usize);
        let v = (a..b.min(n)).map(|i| peaks[i]).fold(0.0, f64::max);
        out.push((v * 10.0).max(0.5) as f32);
        x += 2.0;
    }
    out
}
