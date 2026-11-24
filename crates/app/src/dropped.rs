//! Files and folders dragged from the file manager onto the window.
//!
//! Where the drops come from:
//! - X11, Windows and macOS: winit's `HoveredFile` / `DroppedFile` (one
//!   event per file; window_drag.rs's event filter hands them here).
//! - Wayland: winit 0.30 has no drag and drop there, so `watch_wayland`
//!   listens on the window's own Wayland connection, as the clipboard does
//!   (smithay-clipboard): a `wl_data_device` of its own on a thread of its
//!   own, the `text/uri-list` of what is dragged over the main window.
//!
//! Either way the controller gets one `DropEvent` per hover and per drop, on the
//! interface's thread (`set_handler`); what a drop does is drop_files.rs.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use slint::winit_030::winit;
use sonkore_library::{cue, metadata};

/// What is dragged over the window, or dropped on it.
#[derive(Clone, Debug, PartialEq)]
pub enum DropEvent {
    /// Something is over the window (its paths; empty while unknown).
    Hover(Vec<PathBuf>),
    /// It left, or the drag was cancelled.
    Left,
    Dropped(Vec<PathBuf>),
}

/// The dropped paths sorted: what plays, what joins the library, the rest.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sorted {
    /// Audio files and CUE sheets (the formats the library lists).
    pub files: Vec<String>,
    pub folders: Vec<String>,
    /// Neither (pictures, documents, files that are gone).
    pub ignored: Vec<String>,
}

pub fn sort(paths: &[PathBuf]) -> Sorted {
    let mut s = Sorted::default();
    for p in paths {
        let text = p.to_string_lossy().into_owned();
        if p.is_dir() {
            s.folders.push(text);
        } else if p.is_file() && (metadata::is_audio_file(&text) || cue::is_sheet(&text)) {
            s.files.push(text);
        } else {
            s.ignored.push(text);
        }
    }
    s
}

/// The local paths of a `text/uri-list` (RFC 2483: one URI per line,
/// comments after '#'); what is not a local file is left out. Wayland only
/// (elsewhere winit hands the paths over).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn uri_list(text: &str) -> Vec<PathBuf> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(sonkore_platform::url::to_local_file)
        .collect()
}

type Handler = Rc<dyn Fn(DropEvent)>;

thread_local! {
    static HANDLER: RefCell<Option<Handler>> = const { RefCell::new(None) };
    /// winit's files of the drag in progress, and those dropped (gathered:
    /// winit sends one event per file).
    static GATHERED: RefCell<(Vec<PathBuf>, Vec<PathBuf>, bool)> = const { RefCell::new((Vec::new(), Vec::new(), false)) };
}

/// Who gets the drops (the controller), on the interface's thread.
pub fn set_handler(f: impl Fn(DropEvent) + 'static) {
    HANDLER.with(|h| *h.borrow_mut() = Some(Rc::new(f)));
}

fn emit(event: DropEvent) {
    // (Cloned out: the handler may open a dialog or set models.)
    if let Some(h) = HANDLER.with(|h| h.borrow().clone()) {
        h(event);
    }
}

/// winit's drag and drop events (X11, Windows, macOS): the files of one
/// drag are handed on together, once the event loop turns.
pub fn winit_event(event: &winit::event::WindowEvent) {
    use winit::event::WindowEvent;
    let schedule = |g: &mut (Vec<PathBuf>, Vec<PathBuf>, bool)| {
        if !std::mem::replace(&mut g.2, true) {
            slint::Timer::single_shot(std::time::Duration::ZERO, flush_gathered);
        }
    };
    match event {
        WindowEvent::HoveredFile(p) => GATHERED.with(|g| {
            let mut g = g.borrow_mut();
            g.0.push(p.clone());
            schedule(&mut g);
        }),
        WindowEvent::DroppedFile(p) => GATHERED.with(|g| {
            let mut g = g.borrow_mut();
            g.1.push(p.clone());
            schedule(&mut g);
        }),
        WindowEvent::HoveredFileCancelled => {
            GATHERED.with(|g| g.borrow_mut().0.clear());
            emit(DropEvent::Left);
        }
        _ => {}
    }
}

fn flush_gathered() {
    let event = GATHERED.with(|g| {
        let mut g = g.borrow_mut();
        g.2 = false;
        if !g.1.is_empty() {
            g.0.clear();
            Some(DropEvent::Dropped(std::mem::take(&mut g.1)))
        } else if !g.0.is_empty() {
            Some(DropEvent::Hover(g.0.clone()))
        } else {
            None
        }
    });
    if let Some(e) = event {
        emit(e);
    }
}

/// Wayland: drops on the main window through a data device of our own.
/// Nothing on X11 (winit's events) or without a seat or a data device
/// manager. The winit window exists once the event loop runs: asked for
/// then (every 100 ms, for 5 s).
#[cfg(target_os = "linux")]
pub fn watch_wayland(window: slint::Weak<crate::MainWindow>) {
    watch_wayland_try(window, 50);
}

#[cfg(target_os = "linux")]
fn watch_wayland_try(window: slint::Weak<crate::MainWindow>, tries: u32) {
    use slint::ComponentHandle;
    use slint::winit_030::WinitWindowAccessor;
    use winit::raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
    let Some(w) = window.upgrade() else { return };
    let handles = w.window().with_winit_window(|w| {
        let display = match w.display_handle().ok()?.as_raw() {
            RawDisplayHandle::Wayland(d) => d.display.as_ptr() as usize,
            _ => return None,
        };
        let surface = match w.window_handle().ok()?.as_raw() {
            RawWindowHandle::Wayland(s) => s.surface.as_ptr() as usize,
            _ => return None,
        };
        Some((display, surface))
    });
    match handles {
        Some(Some((display, surface))) => {
            // SAFETY: the window's display lives as long as the app (the
            // main window is never destroyed before the process ends).
            unsafe { wayland::spawn(display as *mut std::ffi::c_void, surface as *mut std::ffi::c_void) };
        }
        // X11: winit's own events.
        Some(None) => {}
        None if tries > 0 => {
            slint::Timer::single_shot(std::time::Duration::from_millis(100), move || {
                watch_wayland_try(window, tries - 1)
            });
        }
        None => {}
    }
}

#[cfg(not(target_os = "linux"))]
pub fn watch_wayland(_window: slint::Weak<crate::MainWindow>) {}

#[cfg(target_os = "linux")]
mod wayland {
    use std::ffi::c_void;
    use std::io::Read;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::time::Duration;

    use wayland_backend::client::{Backend, ObjectId};
    use wayland_client::globals::{GlobalListContents, registry_queue_init};
    use wayland_client::protocol::wl_data_device::{self, WlDataDevice};
    use wayland_client::protocol::wl_data_device_manager::{DndAction, WlDataDeviceManager};
    use wayland_client::protocol::wl_data_offer::{self, WlDataOffer};
    use wayland_client::protocol::wl_registry::WlRegistry;
    use wayland_client::protocol::wl_seat::WlSeat;
    use wayland_client::protocol::wl_surface::WlSurface;
    use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, event_created_child};

    use super::DropEvent;

    const URI_LIST: &str = "text/uri-list";

    struct State {
        conn: Connection,
        /// The main window's surface (its id on this connection).
        surface: u32,
        /// The offer over the main window, its serial, and its paths once read.
        current: Option<(WlDataOffer, u32, Option<Vec<PathBuf>>)>,
    }

    /// The offered types, filled before `enter`.
    type Mimes = Mutex<Vec<String>>;

    /// # Safety
    /// `display` is the app's live `wl_display`, `surface` the main window's `wl_surface`.
    pub unsafe fn spawn(display: *mut c_void, surface: *mut c_void) {
        let backend = unsafe { Backend::from_foreign_display(display.cast()) };
        let conn = Connection::from_backend(backend);
        let Ok(id) = (unsafe { ObjectId::from_ptr(WlSurface::interface(), surface.cast()) }) else { return };
        let surface = id.protocol_id();
        let _ = std::thread::Builder::new().name("sonkore-drops".into()).spawn(move || run(conn, surface));
    }

    fn run(conn: Connection, surface: u32) {
        let Ok((globals, mut queue)) = registry_queue_init::<State>(&conn) else { return };
        let qh = queue.handle();
        let Ok(seat) = globals.bind::<WlSeat, _, _>(&qh, 1..=1, ()) else { return };
        let Ok(manager) = globals.bind::<WlDataDeviceManager, _, _>(&qh, 1..=3, ()) else { return };
        let _device = manager.get_data_device(&seat, &qh, ());
        let mut state = State { conn: conn.clone(), surface, current: None };
        while queue.blocking_dispatch(&mut state).is_ok() {}
    }

    /// What the source writes for `text/uri-list`. Read on a thread of its
    /// own and waited for 2 s: a source that never closes the pipe must not
    /// stop the drops that follow.
    fn read_paths(conn: &Connection, offer: &WlDataOffer) -> Option<Vec<PathBuf>> {
        let (mut reader, writer) = std::io::pipe().ok()?;
        offer.receive(URI_LIST.into(), std::os::fd::AsFd::as_fd(&writer));
        conn.flush().ok()?;
        std::mem::drop(writer);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut text = String::new();
            let _ = reader.read_to_string(&mut text);
            let _ = tx.send(text);
        });
        rx.recv_timeout(Duration::from_secs(2)).ok().map(|t| super::uri_list(&t))
    }

    fn send(event: DropEvent) {
        let _ = slint::invoke_from_event_loop(move || super::emit(event));
    }

    impl Dispatch<WlDataDevice, ()> for State {
        fn event(
            state: &mut Self,
            _: &WlDataDevice,
            event: wl_data_device::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            match event {
                wl_data_device::Event::Enter { serial, surface, id: Some(offer), .. } => {
                    let ours = surface.id().protocol_id() == state.surface;
                    let uris =
                        offer.data::<Mimes>().is_some_and(|m| m.lock().is_ok_and(|m| m.iter().any(|t| t == URI_LIST)));
                    if !(ours && uris) {
                        offer.accept(serial, None);
                        offer.destroy();
                        return;
                    }
                    offer.accept(serial, Some(URI_LIST.into()));
                    if offer.version() >= 3 {
                        offer.set_actions(DndAction::Copy, DndAction::Copy);
                    }
                    // Read now, so the hint can say what the drop will do.
                    let paths = read_paths(&state.conn, &offer);
                    send(DropEvent::Hover(paths.clone().unwrap_or_default()));
                    state.current = Some((offer, serial, paths));
                }
                wl_data_device::Event::Leave => {
                    if let Some((offer, ..)) = state.current.take() {
                        offer.destroy();
                        send(DropEvent::Left);
                    }
                }
                wl_data_device::Event::Drop => {
                    if let Some((offer, _, paths)) = state.current.take() {
                        let paths = paths.or_else(|| read_paths(&state.conn, &offer)).unwrap_or_default();
                        if offer.version() >= 3 {
                            offer.finish();
                        }
                        offer.destroy();
                        send(DropEvent::Dropped(paths));
                    }
                }
                // The clipboard's offers: not ours to keep.
                wl_data_device::Event::Selection { id: Some(offer) } => offer.destroy(),
                _ => {}
            }
        }

        event_created_child!(State, WlDataDevice, [
            wl_data_device::EVT_DATA_OFFER_OPCODE => (WlDataOffer, Mutex::new(Vec::new())),
        ]);
    }

    impl Dispatch<WlDataOffer, Mimes> for State {
        fn event(
            _: &mut Self,
            _: &WlDataOffer,
            event: wl_data_offer::Event,
            mimes: &Mimes,
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if let wl_data_offer::Event::Offer { mime_type } = event {
                if let Ok(mut m) = mimes.lock() {
                    m.push(mime_type);
                }
            }
        }
    }

    impl Dispatch<WlRegistry, GlobalListContents> for State {
        fn event(
            _: &mut Self,
            _: &WlRegistry,
            _: wayland_client::protocol::wl_registry::Event,
            _: &GlobalListContents,
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    impl Dispatch<WlSeat, ()> for State {
        fn event(
            _: &mut Self,
            _: &WlSeat,
            _: wayland_client::protocol::wl_seat::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    impl Dispatch<WlDataDeviceManager, ()> for State {
        fn event(
            _: &mut Self,
            _: &WlDataDeviceManager,
            _: wayland_client::protocol::wl_data_device_manager::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn dropped_paths_are_sorted() {
        let dir = std::env::temp_dir().join(format!("sonkore-drop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Album")).unwrap();
        for f in ["a.flac", "b.MP3", "c.cue", "cover.jpg", "notes.txt"] {
            std::fs::write(dir.join(f), b"").unwrap();
        }
        let paths: Vec<PathBuf> = ["a.flac", "Album", "b.MP3", "cover.jpg", "c.cue", "notes.txt", "gone.flac"]
            .iter()
            .map(|f| dir.join(f))
            .collect();
        let s = sort(&paths);
        let name = |v: &[String]| {
            v.iter().map(|p| Path::new(p).file_name().unwrap().to_string_lossy().into_owned()).collect::<Vec<_>>()
        };
        assert_eq!(name(&s.files), ["a.flac", "b.MP3", "c.cue"]);
        assert_eq!(name(&s.folders), ["Album"]);
        assert_eq!(name(&s.ignored), ["cover.jpg", "notes.txt", "gone.flac"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn uri_lists() {
        let list = "# from the file manager\r\nfile:///home/u/M%C3%BAsica/a%20b.flac\r\nhttps://example.com/x.mp3\r\n\r\nfile:///tmp/Album\r\n";
        assert_eq!(uri_list(list), [PathBuf::from("/home/u/Música/a b.flac"), PathBuf::from("/tmp/Album")]);
    }
}
