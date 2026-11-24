//! Renders the window offscreen to a PNG (SONKORE_SCREENSHOT=file.png,
//! SONKORE_SCREENSHOT_SIZE=1320x840, SONKORE_SCREENSHOT_SCALE=1.5): for
//! checking the interface without a display.

use std::rc::Rc;

use slint::platform::software_renderer::{MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::{PhysicalSize, PlatformError};

struct Offscreen {
    window: Rc<MinimalSoftwareWindow>,
}

impl Platform for Offscreen {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.window.clone())
    }
}

/// Installs the offscreen platform; returns its window.
pub fn install() -> Rc<MinimalSoftwareWindow> {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(Offscreen { window: window.clone() })).expect("platform");
    window
}

/// Lays out, lets timers and animations settle, and saves a frame.
pub fn capture(window: &MinimalSoftwareWindow, path: &str) {
    let (w, h) = std::env::var("SONKORE_SCREENSHOT_SIZE")
        .ok()
        .and_then(|s| s.split_once('x').and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?))))
        .unwrap_or((1320u32, 840u32));
    let scale: f32 = std::env::var("SONKORE_SCREENSHOT_SCALE").ok().and_then(|s| s.parse().ok()).unwrap_or(1.0);
    let (pw, ph) = ((w as f32 * scale).round() as u32, (h as f32 * scale).round() as u32);
    window.dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged { scale_factor: scale });
    window.set_size(PhysicalSize::new(pw, ph));
    let mut buffer = vec![PremultipliedRgbaColor::default(); (pw * ph) as usize];
    // SONKORE_SCREENSHOT_EVENTS="move:x,y;click:x,y;rclick:x,y;press:x,y": the pointer
    // moved / clicked there (logical pixels) once the window is laid out.
    let events = std::env::var("SONKORE_SCREENSHOT_EVENTS").unwrap_or_default();
    for frame in 0..30 {
        if frame == 10 {
            pointer_events(window, &events);
        }
        slint::platform::update_timers_and_animations();
        std::thread::sleep(std::time::Duration::from_millis(20));
        window.request_redraw();
        window.draw_if_needed(|renderer| {
            renderer.render(&mut buffer, pw as usize);
        });
    }
    let mut img = image::RgbaImage::new(pw, ph);
    for (i, p) in buffer.iter().enumerate() {
        let a = p.alpha as u32;
        let un = |c: u8| if a == 0 { 0 } else { ((c as u32 * 255 + a / 2) / a).min(255) as u8 };
        img.put_pixel(i as u32 % pw, i as u32 / pw, image::Rgba([un(p.red), un(p.green), un(p.blue), p.alpha]));
    }
    img.save(path).expect("png");
}

fn pointer_events(window: &MinimalSoftwareWindow, events: &str) {
    use slint::LogicalPosition;
    use slint::platform::{PointerEventButton, WindowEvent};
    for event in events.split(';').filter(|e| !e.is_empty()) {
        let Some((kind, at)) = event.split_once(':') else { continue };
        // "key:space|down|up|enter|escape|ctrl+a…": pressed and released.
        if kind == "key" {
            use slint::platform::Key;
            let (ctrl, name) = match at.strip_prefix("ctrl+") {
                Some(n) => (true, n),
                None => (false, at),
            };
            let text: slint::SharedString = match name {
                "space" => " ".into(),
                "down" => Key::DownArrow.into(),
                "up" => Key::UpArrow.into(),
                "enter" => Key::Return.into(),
                "escape" => Key::Escape.into(),
                "delete" => Key::Delete.into(),
                other => other.into(),
            };
            if ctrl {
                window.dispatch_event(WindowEvent::KeyPressed { text: Key::Control.into() });
            }
            window.dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
            window.dispatch_event(WindowEvent::KeyReleased { text });
            if ctrl {
                window.dispatch_event(WindowEvent::KeyReleased { text: Key::Control.into() });
            }
            slint::platform::update_timers_and_animations();
            continue;
        }
        // "wheel:x,y,dy": a wheel turned there (dy px, 60 a notch, up > 0),
        // then the time an ease takes.
        if kind == "wheel" {
            let v: Vec<f32> = at.split(',').filter_map(|n| n.trim().parse().ok()).collect();
            if v.len() == 3 {
                let position = LogicalPosition::new(v[0], v[1]);
                window.dispatch_event(WindowEvent::PointerMoved { position });
                window.dispatch_event(WindowEvent::PointerScrolled { position, delta_x: 0.0, delta_y: v[2] });
                for _ in 0..20 {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    slint::platform::update_timers_and_animations();
                }
            }
            continue;
        }
        // "drag:x1,y1,x2,y2": pressed at the first point, moved in steps to
        // the second, released there.
        if kind == "drag" {
            let v: Vec<f32> = at.split(',').filter_map(|n| n.trim().parse().ok()).collect();
            if v.len() == 4 {
                let at = |t: f32| LogicalPosition::new(v[0] + (v[2] - v[0]) * t, v[1] + (v[3] - v[1]) * t);
                window.dispatch_event(WindowEvent::PointerMoved { position: at(0.0) });
                window.dispatch_event(WindowEvent::PointerPressed {
                    position: at(0.0),
                    button: PointerEventButton::Left,
                });
                for step in 1..=10 {
                    window.dispatch_event(WindowEvent::PointerMoved { position: at(step as f32 / 10.0) });
                    slint::platform::update_timers_and_animations();
                }
                window.dispatch_event(WindowEvent::PointerReleased {
                    position: at(1.0),
                    button: PointerEventButton::Left,
                });
                slint::platform::update_timers_and_animations();
            }
            continue;
        }
        let Some((x, y)) =
            at.split_once(',').and_then(|(x, y)| Some((x.trim().parse::<f32>().ok()?, y.trim().parse::<f32>().ok()?)))
        else {
            continue;
        };
        let position = LogicalPosition::new(x, y);
        window.dispatch_event(WindowEvent::PointerMoved { position });
        // "press": pressed, never released here (as when the compositor
        // takes the drag and keeps the release).
        if kind == "press" {
            window.dispatch_event(WindowEvent::PointerPressed { position, button: PointerEventButton::Left });
            for _ in 0..3 {
                slint::platform::update_timers_and_animations();
            }
            continue;
        }
        let button = match kind {
            "click" => Some(PointerEventButton::Left),
            "rclick" => Some(PointerEventButton::Right),
            _ => None,
        };
        if let Some(button) = button {
            window.dispatch_event(WindowEvent::PointerPressed { position, button });
            window.dispatch_event(WindowEvent::PointerReleased { position, button });
        }
        slint::platform::update_timers_and_animations();
    }
}

/// The settings window alone (SONKORE_SCREENSHOT_WINDOW=settings), at
/// SONKORE_SCREENSHOT_SIZE (920x760, the window's first size), scrolled down by
/// SONKORE_SCREENSHOT_SCROLL pixels.
pub fn capture_settings(window: &MinimalSoftwareWindow, path: &str, settings: sonkore_core::settings::Settings) {
    use slint::ComponentHandle;
    use std::cell::RefCell;
    let equalizer =
        Rc::new(RefCell::new(sonkore_eq::controller::EqController::with_default_paths(Default::default(), &settings)));
    let ctx = Rc::new(crate::context::Ctx {
        settings: Rc::new(RefCell::new(settings)),
        player: Rc::new(RefCell::new(sonkore_engine::Player::new(sonkore_engine::Engine::new()))),
        library: Rc::new(RefCell::new(crate::lib_shim::Library::load())),
        equalizer,
        notify: Box::new(|_| {}),
        online_art: None,
    });
    let ui = crate::settings_ui::SettingsUi::new(&ctx, Rc::new(crate::settings_ui::NoHooks)).expect("settings window");
    crate::i18n::select_slint();
    ui.window().show().expect("show");
    if std::env::var_os("SONKORE_SCREENSHOT_SIZE").is_none() {
        // SAFETY: single-threaded here.
        unsafe { std::env::set_var("SONKORE_SCREENSHOT_SIZE", "920x760") };
    }
    if let Some(y) = std::env::var("SONKORE_SCREENSHOT_SCROLL").ok().and_then(|v| v.parse::<f32>().ok()) {
        ui.window().global::<crate::SettingsData>().set_scroll(-y);
    }
    capture(window, path);
}
