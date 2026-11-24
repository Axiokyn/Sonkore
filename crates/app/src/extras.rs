//! What hangs off the main window: the Equalizer and Pitch & tempo windows,
//! the library's folder actions and the tray icon.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use slint::winit_030::WinitWindowAccessor;
use slint::{ComponentHandle, Timer, TimerMode};
use sonkore_platform::{Bus, Tray, TrayCommand, TrayLabels};

use crate::controller::Controller;
use crate::equalizer_model::EqAdapter;
use crate::equalizer_ui::EqualizerUi;
use crate::i18n::tr;
use crate::mini_ui::MiniUi;
use crate::pitch_ui::PitchUi;
use crate::settings_ui::{AudioStatus, LibraryStatus, SettingsHooks};
use crate::{App, MainWindow, Skins, WindowActions, theme};

pub struct Extras {
    window: slint::Weak<MainWindow>,
    controller: Rc<Controller>,
    equalizer: RefCell<Option<Rc<EqualizerUi>>>,
    pitch: RefCell<Option<Rc<PitchUi>>>,
    tray: RefCell<Option<(Tray, std::sync::mpsc::Receiver<TrayCommand>)>>,
    /// Quit asked for (the tray's Quit): close-to-tray lets it through.
    pub quitting: Cell<bool>,
    timer: Timer,
}

/// The screen's height the windows open within (logical px).
fn available_height(window: &MainWindow) -> f32 {
    let mut h = 1000.0;
    window.window().with_winit_window(|w| {
        if let Some(m) = w.current_monitor() {
            h = m.size().height as f32 / m.scale_factor() as f32;
        }
    });
    h
}

impl Extras {
    pub fn new(window: &MainWindow, controller: Rc<Controller>, mini: &Rc<MiniUi>) -> Rc<Extras> {
        let extras = Rc::new(Extras {
            window: window.as_weak(),
            controller,
            equalizer: RefCell::new(None),
            pitch: RefCell::new(None),
            tray: RefCell::new(None),
            quitting: Cell::new(false),
            timer: Timer::default(),
        });
        let app = window.global::<App>();
        // An output's own sound put back: the windows that show it follow.
        let weak = Rc::downgrade(&extras);
        crate::sound_profiles::on_restored(move || {
            let Some(e) = weak.upgrade() else { return };
            if let Some(ui) = e.equalizer.borrow().as_ref() {
                ui.refresh();
            }
            if let Some(ui) = crate::settings_ui::current() {
                ui.refresh_sound();
            }
        });
        let weak = Rc::downgrade(&extras);
        app.on_open_equalizer(move || {
            if let Some(e) = weak.upgrade() {
                e.open_equalizer();
            }
        });
        // Ctrl+Q: quits even when closing goes to the tray.
        let weak = Rc::downgrade(&extras);
        app.on_quit(move || {
            if let Some(e) = weak.upgrade() {
                e.quitting.set(true);
                if let Some(w) = e.main() {
                    w.global::<WindowActions>().invoke_close();
                }
            }
        });
        let weak = Rc::downgrade(&extras);
        app.on_open_pitch(move || {
            if let Some(e) = weak.upgrade() {
                e.open_pitch();
            }
        });
        let weak = Rc::downgrade(&extras);
        app.on_add_folder(move || {
            if let Some(e) = weak.upgrade() {
                e.add_folder();
            }
        });
        let weak = Rc::downgrade(&extras);
        app.on_rescan_library(move || {
            if let Some(e) = weak.upgrade() {
                e.controller.ctx.library.borrow_mut().rescan();
            }
        });
        let weak = Rc::downgrade(&extras);
        app.on_open_settings(move || {
            if let Some(e) = weak.upgrade() {
                e.open_settings();
            }
        });
        // The mini player's source menu: a folder's trash.
        let weak = Rc::downgrade(&extras);
        *mini.remove_folder.borrow_mut() = Some(Box::new(move |folder: String| {
            if let Some(e) = weak.upgrade() {
                e.controller.confirm_remove_folder(&folder);
            }
        }));
        let weak = Rc::downgrade(&extras);
        *mini.add_folder.borrow_mut() = Some(Box::new(move || {
            if let Some(e) = weak.upgrade() {
                e.add_folder();
            }
        }));
        extras.update_tray();
        let weak = Rc::downgrade(&extras);
        extras.timer.start(TimerMode::Repeated, Duration::from_millis(100), move || {
            if let Some(e) = weak.upgrade() {
                e.tick();
            }
        });
        extras
    }

    fn main(&self) -> Option<MainWindow> {
        self.window.upgrade()
    }

    fn look(&self) -> Option<theme::Look> {
        let w = self.main()?;
        let count = slint::Model::row_count(&w.global::<Skins>().get_list());
        Some(theme::Look::load(&self.controller.ctx.settings.borrow(), count))
    }

    pub fn open_equalizer(&self) {
        let Some(w) = self.main() else { return };
        if self.equalizer.borrow().is_none() {
            let ctx = self.controller.ctx.clone();
            let model = Rc::new(RefCell::new(EqAdapter::with_controller(ctx.equalizer.clone(), ctx.settings.clone())));
            let notify_ctx = ctx.clone();
            let Ok(ui) = EqualizerUi::new(model, Rc::new(move |m: &str| (notify_ctx.notify)(m))) else { return };
            // The auto headroom follows the curve; the signal path shows it.
            let c = ctx.clone();
            *ui.on_change.borrow_mut() = Some(Box::new(move || {
                crate::audio::apply_headroom(&c.settings.borrow(), c.player.borrow().engine());
            }));
            let main = self.window.clone();
            ui.window.global::<crate::EqUi>().on_quit(move || {
                if let Some(w) = main.upgrade() {
                    w.global::<App>().invoke_quit();
                }
            });
            *self.equalizer.borrow_mut() = Some(ui);
        }
        let ui = self.equalizer.borrow().clone().unwrap();
        if let Some(look) = self.look() {
            theme::apply(&ui.window, &look);
        }
        ui.refresh();
        let _ = ui.open(available_height(&w));
    }

    pub fn open_pitch(&self) {
        let Some(w) = self.main() else { return };
        if self.pitch.borrow().is_none() {
            let ctx = self.controller.ctx.clone();
            let Ok(ui) = PitchUi::new(ctx.settings.clone()) else { return };
            let c = ctx.clone();
            ui.on_change(move |_| {
                crate::audio::apply_pitch(&c.settings.borrow(), c.player.borrow().engine());
            });
            *self.pitch.borrow_mut() = Some(ui);
        }
        let ui = self.pitch.borrow().clone().unwrap();
        if let Some(look) = self.look() {
            theme::apply(&ui.window, &look);
        }
        self.refresh_pitch_output();
        ui.refresh();
        let _ = ui.open(available_height(&w));
    }

    pub fn open_settings(self: &Rc<Self>) {
        let hooks: Rc<dyn SettingsHooks> = Rc::new(Hooks { extras: Rc::downgrade(self) });
        if let Ok(ui) = crate::settings_ui::open(&self.controller.ctx, hooks) {
            if let Some(look) = self.look() {
                theme::apply(ui.window(), &look);
            }
        }
    }

    /// The look changed in Settings: every window shows it.
    fn retheme(&self) {
        let Some(look) = self.look() else { return };
        if let Some(w) = self.main() {
            theme::apply(&w, &look);
        }
        if let Some(ui) = self.equalizer.borrow().as_ref() {
            theme::apply(&ui.window, &look);
        }
        if let Some(ui) = self.pitch.borrow().as_ref() {
            theme::apply(&ui.window, &look);
            ui.refresh();
        }
        if let Some(ui) = crate::settings_ui::current() {
            theme::apply(ui.window(), &look);
        }
    }

    fn refresh_pitch_output(&self) {
        let Some(ui) = self.pitch.borrow().clone() else { return };
        let p = self.controller.ctx.player.borrow();
        let text = p.loaded().then(|| crate::signal::output_description(p.engine()));
        ui.set_output(text.as_deref());
    }

    /// "Add music folder": the desktop's folder dialog, then the library.
    pub fn add_folder(&self) {
        let request = crate::file_dialog::Request {
            title: tr("Sidebar", "Add music folder"),
            directory: true,
            ..Default::default()
        };
        let ctx = self.controller.ctx.clone();
        let weak = Rc::downgrade(&ctx);
        crate::file_dialog::ask(request, move |paths| {
            let Some(ctx) = weak.upgrade() else { return };
            for p in paths {
                ctx.library.borrow_mut().add_folder(&p);
            }
        });
    }

    // --- The tray ---------------------------------------------------------------------

    /// Shows or removes the tray icon as Settings › Desktop asks (ui/trayIcon).
    pub fn update_tray(&self) {
        let wanted = self.controller.ctx.settings.borrow().flag("ui/trayIcon", false)
            && std::env::var_os("SONKORE_SCREENSHOT").is_none();
        if !wanted {
            self.tray.borrow_mut().take();
            return;
        }
        if self.tray.borrow().is_none() && Tray::available(&Bus::Session) {
            let labels = TrayLabels {
                play: tr("Main", "Play"),
                pause: tr("Main", "Pause"),
                next: tr("NowPlayingBar", "Next"),
                previous: tr("NowPlayingBar", "Previous"),
                quit: tr("SettingsPage", "Quit"),
                rtl: false,
            };
            if let Ok(t) = Tray::start(Bus::Session, labels) {
                *self.tray.borrow_mut() = Some(t);
            }
        }
    }

    /// The tray's icon is there and closing goes to it (ui/closeToTray).
    pub fn closes_to_tray(&self) -> bool {
        !self.quitting.get()
            && self.tray.borrow().is_some()
            && self.controller.ctx.settings.borrow().flag("ui/closeToTray", false)
    }

    fn tick(&self) {
        let commands: Vec<TrayCommand> = match &*self.tray.borrow() {
            Some((tray, rx)) => {
                tray.set_playing(self.controller.ctx.player.borrow().playing());
                if let Some(w) = self.main() {
                    let a = w.global::<App>();
                    let title = if a.get_loaded() {
                        format!("{}  ·  {}", a.get_title(), a.get_artist())
                    } else {
                        "Sonkore".into()
                    };
                    tray.set_tooltip(&title);
                }
                rx.try_iter().collect()
            }
            None => Vec::new(),
        };
        for c in commands {
            self.tray_command(c);
        }
        if self.pitch.borrow().as_ref().is_some_and(|p| p.window.window().is_visible()) {
            self.refresh_pitch_output();
        }
        self.refresh_modes();
    }

    /// The bar's equalizer, pitch and settings buttons lit as in
    /// The bar's mode buttons: the EQ when on or its window open, pitch when
    /// SoundTouch changes the sound, settings while its window is open.
    fn refresh_modes(&self) {
        let Some(w) = self.main() else { return };
        let app = w.global::<App>();
        let ctx = &self.controller.ctx;
        let eq_open = self.equalizer.borrow().as_ref().is_some_and(|e| e.window.window().is_visible());
        app.set_eq_on(ctx.equalizer.borrow().enabled() || eq_open);
        app.set_pitch_on(!ctx.player.borrow().engine().dsp().stretch.is_neutral());
        app.set_settings_open(crate::settings_ui::current().is_some_and(|s| s.window().window().is_visible()));
        app.set_stop_button(ctx.settings.borrow().flag("ui/stopButton", false));
        app.set_wheel_step(ctx.settings.borrow().int("ui/wheelStep", 180).clamp(20, 2000) as f32);
        let show_search = ctx.settings.borrow().flag("ui/searchBar", true);
        if show_search != app.get_show_search() {
            app.set_show_search(show_search);
            app.set_search_summoned(false);
        }
        let p = ctx.player.borrow();
        app.set_output_description(crate::signal::output_description(p.engine()).into());
    }

    fn tray_command(&self, command: TrayCommand) {
        let Some(w) = self.main() else { return };
        let app = w.global::<App>();
        match command {
            TrayCommand::TogglePlay => app.invoke_toggle_play(),
            TrayCommand::Next => app.invoke_next(),
            TrayCommand::Previous => app.invoke_previous(),
            TrayCommand::Raise => raise(&w),
            TrayCommand::Quit => {
                self.quitting.set(true);
                w.global::<WindowActions>().invoke_close();
            }
            TrayCommand::Activate => {
                if w.window().is_visible() && !w.window().is_minimized() {
                    let _ = w.hide();
                } else {
                    raise(&w);
                }
            }
        }
    }
}

/// What the Settings window asks of the app.
struct Hooks {
    extras: std::rc::Weak<Extras>,
}

impl Hooks {
    fn ctx(&self) -> Option<Rc<crate::context::Ctx>> {
        self.extras.upgrade().map(|e| e.controller.ctx.clone())
    }
    fn library(&self, f: impl FnOnce(&mut sonkore_library::Library) -> Vec<sonkore_library::Event>) {
        if let Some(ctx) = self.ctx() {
            ctx.library.borrow_mut().with(f);
        }
    }
}

impl SettingsHooks for Hooks {
    fn setting_changed(&self, key: &str) {
        let Some(e) = self.extras.upgrade() else { return };
        let ctx = e.controller.ctx.clone();
        let settings = ctx.settings.clone();
        if key.starts_with("playback/") {
            // Turned on: the output in use keeps the sound it has now.
            if key == "playback/soundPerOutput" && crate::sound_profiles::enabled(&settings.borrow()) {
                let device = settings.borrow().text("playback/device", "");
                crate::sound_profiles::store(&mut settings.borrow_mut(), &ctx.equalizer.borrow(), &device);
            }
            let (was_open, playing, at) = {
                let p = ctx.player.borrow();
                (p.loaded() && p.engine().device_format().is_some(), p.playing(), p.position())
            };
            crate::audio::apply(&settings.borrow(), ctx.player.borrow_mut().engine_mut());
            // What only applies when the track opens: heard now, not at the next track.
            if was_open
                && matches!(
                    key,
                    "playback/bufferMs" | "playback/hdcd" | "playback/playFromMemory" | "playback/resampleFilter"
                )
            {
                ctx.player.borrow_mut().engine_mut().close_output();
            }
            // A setting that closed the output (rate mode, DSD, following the
            // track's rate…): the track goes on where it was.
            if was_open && ctx.player.borrow().engine().device_format().is_none() {
                let changes = ctx.player.borrow_mut().reopen(playing, at);
                e.controller.after(changes);
            }
            // The track playing gets its new ReplayGain now, not at the next one.
            if key.starts_with("playback/replayGain") {
                e.controller.apply_track_gain();
            }
            // On: the tracks without ReplayGain tags are measured in the background.
            if key == "playback/replayGain" {
                let on = settings.borrow().int("playback/replayGain", 0) != 0;
                ctx.library.borrow_mut().with(|l| l.set_auto_analyze_loudness(on));
            }
            // Windows: exclusive mode is what makes an output direct there.
            if key == "playback/volumeMode" || key == "playback/exclusive" {
                e.controller.update_volume();
            }
            return;
        }
        match key {
            "ui/visualizer" => {
                let on = settings.borrow().text("ui/visualizer", "bars") != "off";
                ctx.player.borrow_mut().engine_mut().set_analysis(on);
            }
            "ui/trayIcon" => e.update_tray(),
            "system/mpris" => e.controller.set_mpris(settings.borrow().flag("system/mpris", true)),
            // On: the covers that were missing are asked for again (the cover
            // cache remembered "none" for them). It used to do nothing.
            "library/onlineCovers" => {
                if let Some(art) = &ctx.online_art {
                    art.borrow_mut().set_enabled(settings.borrow().flag("library/onlineCovers", false));
                    let paths: Vec<String> = ctx.library.borrow().tracks.iter().map(|t| t.path.clone()).collect();
                    crate::covers::forget(&paths);
                }
            }
            "library/minDuration" => {
                let v = settings.borrow().int("library/minDuration", 0) as i32;
                self.library(|l| l.set_min_duration(v));
            }
            "library/watchFolders" => {
                let on = settings.borrow().flag("library/watchFolders", true);
                self.library(|l| l.set_watch_folders(on));
            }
            "library/rescanOnStartup" => {
                let on = settings.borrow().flag("library/rescanOnStartup", true);
                self.library(|l| l.set_rescan_on_startup(on));
            }
            "library/excludedFolders" => {
                let wanted: Vec<String> =
                    settings.borrow().list("library/excludedFolders").into_iter().filter(|f| !f.is_empty()).collect();
                self.library(|l| {
                    let now = l.excluded_folders().to_vec();
                    let mut ev = Vec::new();
                    for f in now.iter().filter(|f| !wanted.contains(f)) {
                        ev.extend(l.remove_excluded_folder(f));
                    }
                    for f in wanted.iter().filter(|f| !now.contains(f)) {
                        ev.extend(l.exclude_folder(f));
                    }
                    ev
                });
            }
            "columns/keys" | "columns/widths" => e.controller.columns_changed(),
            // Every window in the new font; the columns measured again.
            "appearance/font" => {
                e.retheme();
                e.controller.columns_changed();
            }
            _ => {}
        }
    }

    fn appearance_changed(&self) {
        if let Some(e) = self.extras.upgrade() {
            e.retheme();
        }
    }

    fn audio_status(&self) -> AudioStatus {
        let Some(ctx) = self.ctx() else { return AudioStatus::default() };
        let p = ctx.player.borrow();
        let e = p.engine();
        let s = e.output_status();
        AudioStatus {
            loaded: p.loaded(),
            output_description: crate::signal::output_description(e),
            bit_perfect: s.bit_perfect_now,
            // Held alone (a card's own endpoint, WASAPI exclusive): the DAC
            // rows apply. PipeWire's output reports nothing mixed in, but
            // it is no DAC to give back.
            output_exclusive_capable: s.exclusive && sonkore_engine::output::is_direct_key(e.device_key()),
            // Held: reserved from the sound server, or a card's own endpoint open.
            output_reserved: crate::audio::output_reserved()
                || (e.device_format().is_some() && sonkore_engine::output::is_direct_key(e.device_key())),
            effective_headroom_db: Some(e.dsp().snapshot().headroom_db),
            components: Some(format!(
                "Symphonia · ALSA · PipeWire · SoundTouch (Rust) · rubato · Slint {}",
                slint_version()
            )),
            hardware_volume_supported: Some(true),
            test_signal_playing: self.extras.upgrade().is_some_and(|x| x.controller.test_signal_playing()),
            current_gain_db: self.extras.upgrade().and_then(|x| x.controller.current_gain_db()),
            hardware_volume_info: Some(crate::volume::info(
                ctx.settings.borrow().int("playback/volumeMode", crate::volume::AUTO).clamp(0, 2),
                e.device_key(),
            )),
            ..Default::default()
        }
    }

    fn covers_status(&self) -> crate::settings_ui::CoversStatus {
        let Some(ctx) = self.ctx() else { return Default::default() };
        let Some(art) = &ctx.online_art else { return Default::default() };
        let art = art.borrow();
        crate::settings_ui::CoversStatus {
            pending: art.pending(),
            fetched: art.fetched(),
            last_error: art.last_error().to_owned(),
        }
    }

    fn clear_downloaded_covers(&self) {
        let Some(ctx) = self.ctx() else { return };
        if let Some(art) = &ctx.online_art {
            art.borrow_mut().clear_cache();
            let paths: Vec<String> = ctx.library.borrow().tracks.iter().map(|t| t.path.clone()).collect();
            crate::covers::forget(&paths);
        }
    }

    fn library_status(&self) -> LibraryStatus {
        let Some(ctx) = self.ctx() else { return LibraryStatus::default() };
        let lib = ctx.library.borrow();
        let l = lib.inner();
        LibraryStatus {
            scanning: l.scanning(),
            scanned_files: l.scanned_files(),
            count: Some(l.count()),
            hidden_count: l.hidden_count(),
            analyzing_loudness: l.analyzing_loudness(),
            loudness_done: l.loudness_done(),
            loudness_total: l.loudness_total(),
            tracks_without_loudness: l.tracks_without_loudness(),
        }
    }

    fn quit(&self) {
        if let Some(x) = self.extras.upgrade() {
            if let Some(w) = x.main() {
                w.global::<App>().invoke_quit();
            }
        }
    }

    fn set_device(&self, key: &str) {
        if let Some(x) = self.extras.upgrade() {
            x.controller.select_output(key);
        }
    }

    fn play_test_signal(&self, kind: &str, rate: u32, channels: u32) {
        if let Some(x) = self.extras.upgrade() {
            x.controller.play_test_signal(kind, rate, channels);
        }
    }

    fn stop_test_signal(&self) {
        if let Some(x) = self.extras.upgrade() {
            x.controller.stop_test_signal();
        }
    }

    fn release_output(&self) {
        if let Some(ctx) = self.ctx() {
            crate::audio::release_output(&mut ctx.player.borrow_mut());
        }
    }

    fn rescan_library(&self) {
        self.library(|l| l.rescan());
    }

    fn restore_hidden_tracks(&self) {
        self.library(|l| l.restore_hidden());
    }

    fn add_library_folder(&self, path: &str) {
        let path = path.to_owned();
        self.library(move |l| l.add_folder(&path));
    }

    fn remove_library_folder(&self, path: &str) {
        // Later in the loop: Settings is in the middle of a callback, and the
        // main window's refresh must not meet its borrows.
        let weak = self.extras.clone();
        let path = path.to_owned();
        slint::Timer::single_shot(std::time::Duration::ZERO, move || {
            let Some(e) = weak.upgrade() else { return };
            let changes = e.controller.ctx.player.borrow_mut().forget_folder(&path);
            e.controller.after(changes);
            e.controller.ctx.library.borrow_mut().with(move |l| l.remove_folder(&path));
        });
    }

    fn restart(&self) {
        self.restart_app();
    }

    fn restore_defaults(&self) {
        self.restart_app();
    }
}

fn slint_version() -> &'static str {
    "1.19"
}

impl Hooks {
    /// Quits as the close button does (the session, the window's size and
    /// the settings saved; never to the tray), then starts Sonkore again: a
    /// new interface scale or language.
    fn restart_app(&self) {
        if let Some(x) = self.extras.upgrade() {
            x.quitting.set(true);
            if let Some(w) = x.main() {
                w.global::<WindowActions>().invoke_close();
            }
        }
        let _ = slint::quit_event_loop();
        if let Ok(exe) = std::env::current_exe() {
            // Without arguments: files would play again, and a command
            // (--play-pause…) would go to this instance, still on the bus.
            let _ = std::process::Command::new(std::env::var_os("APPIMAGE").map(Into::into).unwrap_or(exe)).spawn();
        }
    }
}

fn raise(w: &MainWindow) {
    w.window().set_minimized(false);
    let _ = w.show();
    w.window().with_winit_window(|ww| ww.focus_window());
}
