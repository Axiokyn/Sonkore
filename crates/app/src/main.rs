//! Sonkore: the music player, in Rust.

// Windows: a window application, no console of its own (see attach_console).
#![cfg_attr(windows, windows_subsystem = "windows")]

// The interface Slint generates, in a crate of its own (crates/ui).
pub use sonkore_ui::*;

mod audio;
mod context;
mod controller;
mod covers;
mod dropped;
mod equalizer_model;
mod equalizer_ui;
mod extras;
mod file_dialog;
mod filters;
mod fonts;
mod geometry;
mod hinted_advances;
mod hotplug;
mod i18n;
mod icons;
mod lib_shim;
mod metrics;
mod mini_ui;
mod paint;
mod pitch_ui;
mod playlists;
mod probe;
mod screenshot;
mod settings_ui;
mod signal;
mod sound_profiles;
mod spectrum;
mod tap;
mod theme;
mod tracklist;
mod views;
mod volume;
mod waveform;
#[cfg(windows)]
mod win_volume;
mod window_drag;

use std::rc::Rc;

use slint::ComponentHandle;
use slint::winit_030::winit;
use sonkore_core::settings::Settings;

/// The desktop file's name (Wayland app_id), so the dock shows our icon.
#[cfg(target_os = "linux")]
fn desktop_identity(attributes: winit::window::WindowAttributes) -> winit::window::WindowAttributes {
    winit::platform::wayland::WindowAttributesExtWayland::with_name(attributes, "com.axiokyn.Sonkore", "sonkore")
}

/// The window's and the taskbar's icon (Windows takes it from the window).
/// Opaque, with the system's shadow and rounded corners: the software
/// renderer draws through GDI, which does not carry the see-through corners
/// the Linux build draws itself, so Windows 11 rounds the window (DWM) and
/// the theme's window radius is 0 there (theme.rs).
#[cfg(windows)]
fn desktop_identity(attributes: winit::window::WindowAttributes) -> winit::window::WindowAttributes {
    use winit::platform::windows::{CornerPreference, WindowAttributesExtWindows};
    let icon = |png: &[u8]| {
        let image = image::load_from_memory_with_format(png, image::ImageFormat::Png).ok()?.to_rgba8();
        let (w, h) = image.dimensions();
        winit::window::Icon::from_rgba(image.into_raw(), w, h).ok()
    };
    attributes
        .with_transparent(false)
        .with_undecorated_shadow(true)
        .with_corner_preference(CornerPreference::Round)
        .with_window_icon(icon(include_bytes!("../../../packaging/icons/png/32.png")))
        .with_taskbar_icon(icon(include_bytes!("../../../packaging/icons/png/256.png")))
}

/// Windows: started from a terminal, its messages (errors, the CI's logs)
/// go to that console; a window application has none of its own.
#[cfg(windows)]
fn attach_console() {
    use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
    // SAFETY: no arguments to get wrong; fails harmlessly without a parent console.
    let _ = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // A panic where a file is read (a damaged file) is caught there and is an
    // error; any other panic is a bug and ends the app, rather than leaving
    // it running with a dead thread (sonkore_core::panic_guard).
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if sonkore_core::panic_guard::catching() {
            eprintln!("sonkore: damaged file: {info}");
        } else {
            default_hook(info);
            std::process::abort();
        }
    }));
    #[cfg(windows)]
    attach_console();
    // Files to play, and the desktop entry's quick actions: an instance
    // already running gets them and this one ends.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let files: Vec<String> = args
        .iter()
        .filter(|a| !a.starts_with("--"))
        .map(|a| sonkore_platform::url::to_local_file(a).unwrap_or_else(|| a.into()))
        .filter(|p| p.exists())
        .map(|p| sonkore_platform::url::absolute(&p).to_string_lossy().into_owned())
        .collect();
    let session = sonkore_platform::Bus::Session;
    if !files.is_empty() && sonkore_platform::mpris::forward_files(&session, &files) {
        return Ok(());
    }
    if let Some(method) = sonkore_platform::mpris::player_command(&args) {
        if sonkore_platform::mpris::send_command(&session, method) {
            return Ok(());
        }
    }
    let settings = Settings::open_default();

    // Interface scale (Settings › Appearance) multiplies the screen's scale:
    // on top of the screen's own scale (the patched winit backend multiplies).
    let scale = settings.number("appearance/uiScale", 1.0).clamp(0.5, 3.0);
    if (scale - 1.0).abs() > 1e-6 && std::env::var_os("SLINT_SCALE_FACTOR").is_none() {
        // SAFETY: before any other thread starts.
        unsafe { std::env::set_var("SLINT_SCALE_FACTOR_MULTIPLIER", format!("{scale}")) };
    }

    // Offscreen, for a screenshot of the interface.
    let shot = std::env::var("SONKORE_SCREENSHOT").ok();
    let offscreen = shot.as_ref().map(|_| screenshot::install());
    // A see-through, frameless window: rounded corners, our own title bar.
    let renderer = std::env::var("SONKORE_RENDERER").unwrap_or_else(|_| "software".into());
    if offscreen.is_none() {
        slint::BackendSelector::new()
            .backend_name("winit".into())
            .renderer_name(renderer)
            .with_winit_window_attributes_hook(|attributes| {
                let attributes = attributes.with_transparent(true).with_decorations(false).with_title("Sonkore");
                desktop_identity(attributes)
            })
            .select()?;
    }

    // The language: Settings › Appearance › Language, else the system's.
    let lang = i18n::language(&settings.text("appearance/language", ""));
    i18n::load(&lang);
    // CD image tracks without a title are named "Track N" in this language.
    sonkore_library::metadata::set_untitled_track_label(&i18n::tr("Library", "Track {0}"));

    // The settings window alone, offscreen (SONKORE_SCREENSHOT_WINDOW=settings).
    if std::env::var("SONKORE_SCREENSHOT_WINDOW").as_deref() == Ok("settings") {
        if let (Some(path), Some(off)) = (&shot, &offscreen) {
            screenshot::capture_settings(off, path, settings);
            return Ok(());
        }
    }
    // SONKORE_SCREENSHOT_WINDOW=equalizer|pitch: that window alone.
    if let (Some(path), Some(off), Ok(which)) = (&shot, &offscreen, std::env::var("SONKORE_SCREENSHOT_WINDOW")) {
        return equalizer_ui::screenshot(&which, settings, off, path);
    }

    let window = MainWindow::new()?;
    // Slint's own texts (@tr): its bundle exists once a component does.
    i18n::select_slint();
    // Hinted text widths for what is sized from its text (pills, tabs).
    equalizer_ui::install_metrics(&window);
    // The size (and maximized state) it had last time, in the stored format.
    let restored = settings.bytes("widgets/geometry").and_then(geometry::parse);
    let look = theme::Look::load(&settings, window.global::<Skins>().get_list().iter().count());
    theme::apply(&window, &look);
    let controller = controller::Controller::new(&window, settings);
    // The decorated ground, painted once the change is through (several
    // properties change together when the theme is applied).
    {
        let weak = window.as_weak();
        let pending = Rc::new(std::cell::Cell::new(false));
        window.global::<App>().on_ground_changed(move || {
            if pending.replace(true) {
                return;
            }
            let (weak, pending) = (weak.clone(), pending.clone());
            slint::Timer::single_shot(std::time::Duration::ZERO, move || {
                pending.set(false);
                if let Some(w) = weak.upgrade() {
                    paint::refresh_backdrop(&w);
                }
            });
        });
    }
    // The waveform seek bar: painted again once the change is through.
    {
        let weak = window.as_weak();
        let pending = Rc::new(std::cell::Cell::new(false));
        let again = Rc::new(move || {
            if pending.replace(true) {
                return;
            }
            let (weak, pending) = (weak.clone(), pending.clone());
            slint::Timer::single_shot(std::time::Duration::ZERO, move || {
                pending.set(false);
                if let Some(w) = weak.upgrade() {
                    paint::refresh_wave(&w);
                }
            });
        });
        let a = again.clone();
        window.global::<Visual>().on_wave_changed(move || a());
        waveform::on_change(move || again());
    }
    // The mini player: the same window in compact mode (Ctrl+M).
    let mini = mini_ui::MiniUi::new(&window, controller.ctx.clone());
    {
        let c = Rc::downgrade(&controller);
        mini.set_after(move |changes| {
            if let Some(c) = c.upgrade() {
                c.after(changes);
            }
        });
        let m = Rc::downgrade(&mini);
        window.global::<App>().on_open_mini(move || {
            if let Some(m) = m.upgrade() {
                m.set_compact(true);
            }
        });
    }
    if let Some(g) = restored {
        window.window().set_size(slint::LogicalSize::new(g.width as f32, g.height as f32));
    }
    let start_compact = controller.ctx.settings.borrow().flag("ui/compactMode", false);
    if start_compact {
        mini.set_compact(true);
        if let Some(g) = restored {
            mini.set_full_size(g.width as f32, g.height as f32, g.maximized);
        }
    }
    if let (Some(path), Some(off)) = (shot, offscreen) {
        // A press on the title bar goes the way it goes on screen (the
        // compositor's drag is a no-op here; the release fix is not).
        let weak = window.as_weak();
        window.global::<WindowActions>().on_start_move(move || {
            if let Some(w) = weak.upgrade() {
                window_drag::start_move(&w);
            }
        });
        window.show()?;
        screenshot::capture(&off, &path);
        return Ok(());
    }
    let extras = extras::Extras::new(&window, controller.clone(), &mini);
    connect_window_actions(&window, &controller, &mini, &extras);
    window.show()?;
    // Files dropped on the window, on Wayland (winit has no drops there).
    dropped::watch_wayland(window.as_weak());
    if !start_compact && restored.is_some_and(|g| g.maximized) {
        window.window().set_maximized(true);
        window.global::<WindowActions>().set_maximized(true);
    }
    controller.play_files(files);
    let weak = window.as_weak();
    // A new screen scale (the window moved to another screen, the desktop's
    // setting): the covers, the ground and the waveform at the new pixels.
    let last_scale = std::cell::Cell::new(window.window().scale_factor());
    covers::follow_scale(move || {
        let w = weak.upgrade()?;
        let scale = w.window().scale_factor();
        if (last_scale.replace(scale) - scale).abs() > 1e-3 {
            w.global::<App>().invoke_ground_changed();
            w.global::<Visual>().invoke_wave_changed();
        }
        Some(scale)
    });
    // The outputs, listed in the background for Settings › Audio, and again
    // when a sound device is plugged in or out.
    settings_ui::list_devices();
    let weak = Rc::downgrade(&controller);
    hotplug::watch(move |added, removed| {
        if let Some(c) = weak.upgrade() {
            c.devices_plugged(added, removed);
        }
    });
    // Runs until Quit (not when the window hides: it may go to the tray).
    slint::run_event_loop_until_quit()?;
    drop(extras);
    controller.shutdown();
    Ok(())
}

use slint::Model;

/// The title bar's buttons and edges, through winit.
fn connect_window_actions(
    window: &MainWindow,
    controller: &Rc<controller::Controller>,
    mini: &Rc<mini_ui::MiniUi>,
    extras: &Rc<extras::Extras>,
) {
    let actions = window.global::<WindowActions>();
    let weak = window.as_weak();
    actions.on_minimize(move || {
        if let Some(w) = weak.upgrade() {
            crate::window_drag::minimize(&w);
        }
    });
    let weak = window.as_weak();
    actions.on_toggle_maximize(move || {
        if let Some(w) = weak.upgrade() {
            let on = !w.window().is_maximized();
            w.window().set_maximized(on);
            w.global::<WindowActions>().set_maximized(on);
        }
    });
    let weak = window.as_weak();
    let c = controller.clone();
    let m = mini.clone();
    let x = Rc::downgrade(extras);
    actions.on_close(move || {
        // Close to the tray: hidden, playing on.
        if x.upgrade().is_some_and(|x| x.closes_to_tray()) {
            if let Some(w) = weak.upgrade() {
                let _ = w.hide();
            }
            return;
        }
        let restoring = settings_ui::restoring_defaults();
        if let Some(w) = weak.upgrade()
            && !restoring
        {
            if !m.is_compact() {
                c.save_geometry(&w);
            } else if let Some((width, height, maximized)) = m.full_size() {
                c.save_full_size(width, height, maximized);
            }
        }
        c.save_session();
        if !restoring {
            c.save_settings();
        }
        if let Some(w) = weak.upgrade() {
            let _ = w.hide();
        }
        let _ = slint::quit_event_loop();
    });
    // The desktop's close (Alt+F4, the taskbar) goes the same way.
    let weak = window.as_weak();
    window.window().on_close_requested(move || {
        if let Some(w) = weak.upgrade() {
            w.global::<WindowActions>().invoke_close();
        }
        slint::CloseRequestResponse::KeepWindowShown
    });
    let weak = window.as_weak();
    actions.on_start_move(move || {
        if let Some(w) = weak.upgrade() {
            if crate::window_drag::start_move(&w) {
                w.global::<WindowActions>().invoke_toggle_maximize();
            }
        }
    });
    let weak = window.as_weak();
    actions.on_start_resize(move |edge| {
        if let Some(w) = weak.upgrade() {
            crate::window_drag::start_resize(&w, edge.as_str());
        }
    });
    crate::window_drag::follow_window(
        window,
        |w, on| w.global::<WindowActions>().set_maximized(on),
        |w, x, y| {
            let pointer = w.global::<crate::Pointer>();
            pointer.set_x(x);
            pointer.set_y(y);
            pointer.set_presses(pointer.get_presses() + 1);
        },
    );
}
