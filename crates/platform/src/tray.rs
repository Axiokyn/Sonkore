//! The tray icon on Linux: a StatusNotifierItem (`org.kde.StatusNotifierItem`)
//! registered with `org.kde.StatusNotifierWatcher`, with its menu exported as
//! `com.canonical.dbusmenu`. KDE Plasma, GNOME's AppIndicator extension,
//! waybar, XFCE's and LXQt's panels all read it.
//!
//! The menu, in order: Play/Pause (its text follows the player), Next,
//! Previous, a separator, Sonkore (raise the window) and Quit. A left click
//! on the icon is [`TrayCommand::Activate`]; the app toggles the window.
//!
//! What the panel asks for arrives as [`TrayCommand`]s on a D-Bus thread:
//! through a channel ([`Tray::start`]) or a callback ([`Tray::start_with`]),
//! for the app to act on from its own (UI) thread. Dropping the [`Tray`]
//! takes the icon off the panel.

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, mpsc};
use std::task::{Poll, Waker};
use std::thread::JoinHandle;
use std::time::Duration;

use futures_lite::StreamExt;
use zbus::blocking::Connection;
use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::names::{BusName, WellKnownName};
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

use crate::bus::{Bus, get_property, name_has_owner};

pub const WATCHER: &str = "org.kde.StatusNotifierWatcher";
pub const WATCHER_PATH: &str = "/StatusNotifierWatcher";
pub const ITEM_INTERFACE: &str = "org.kde.StatusNotifierItem";
pub const ITEM_PATH: &str = "/StatusNotifierItem";
pub const MENU_INTERFACE: &str = "com.canonical.dbusmenu";
/// Where a tray icon's menu is exported.
pub const MENU_PATH: &str = "/MenuBar";
/// The icon by name, from the theme (the desktop entry's `Icon=`).
pub const ICON_NAME: &str = "com.axiokyn.Sonkore";
/// The item's `Id` (the application name) and `Title` (its display name).
pub const ID: &str = "sonkore";
pub const TITLE: &str = "Sonkore";

/// The menu's item ids (0 is the root).
pub mod item {
    pub const ROOT: i32 = 0;
    pub const PLAY: i32 = 1;
    pub const NEXT: i32 = 2;
    pub const PREVIOUS: i32 = 3;
    pub const SEPARATOR: i32 = 4;
    pub const RAISE: i32 = 5;
    pub const QUIT: i32 = 6;
    /// The root's children, in order.
    pub const ALL: [i32; 6] = [PLAY, NEXT, PREVIOUS, SEPARATOR, RAISE, QUIT];
}

/// The bus name an item takes: `org.kde.StatusNotifierItem-<pid>-<n>`
/// (`n` counts the tray icons the process has made, from 1).
pub fn item_service_name(pid: u32, n: u32) -> String {
    format!("{ITEM_INTERFACE}-{pid}-{n}")
}

pub use crate::media::{TrayCommand, TrayLabels};

/// A dbusmenu label for plain text: `_` marks an access key there, so a
/// literal one is doubled.
pub fn menu_label(text: &str) -> String {
    text.replace('_', "__")
}

/// SNI pixmaps: (width, height, ARGB32 in network byte order).
pub type Pixmap = (i32, i32, Vec<u8>);

/// The app icon as SNI pixmaps, 16 to 64 px (only the sizes up to 64, to
/// save D-Bus bandwidth; the panel scales the closest one).
pub fn icon_pixmaps() -> &'static [Pixmap] {
    static PIXMAPS: OnceLock<Vec<Pixmap>> = OnceLock::new();
    PIXMAPS.get_or_init(|| {
        const PNGS: [&[u8]; 6] = [
            include_bytes!("../../../packaging/icons/png/16.png"),
            include_bytes!("../../../packaging/icons/png/22.png"),
            include_bytes!("../../../packaging/icons/png/24.png"),
            include_bytes!("../../../packaging/icons/png/32.png"),
            include_bytes!("../../../packaging/icons/png/48.png"),
            include_bytes!("../../../packaging/icons/png/64.png"),
        ];
        PNGS.iter().filter_map(|png| argb_pixmap(png)).collect()
    })
}

/// A PNG as an SNI pixmap.
fn argb_pixmap(png: &[u8]) -> Option<Pixmap> {
    let image = image::load_from_memory_with_format(png, image::ImageFormat::Png).ok()?.to_rgba8();
    let (w, h) = image.dimensions();
    let mut data = Vec::with_capacity(image.as_raw().len());
    for &[r, g, b, a] in image.as_raw().as_chunks::<4>().0 {
        data.extend_from_slice(&[a, r, g, b]);
    }
    Some((i32::try_from(w).ok()?, i32::try_from(h).ok()?, data))
}

// --- Shared state ---------------------------------------------------------------

type CommandFn = Box<dyn Fn(TrayCommand) + Send + Sync>;

struct State {
    labels: TrayLabels,
    playing: bool,
    tooltip: String,
    /// The menu layout's revision (LayoutUpdated).
    revision: u32,
}

impl State {
    /// An item's properties, or None for an id the menu doesn't have.
    fn properties(&self, id: i32) -> Option<Vec<(&'static str, Value<'static>)>> {
        let label = |text: &str| vec![("label", Value::from(menu_label(text)))];
        let l = &self.labels;
        Some(match id {
            item::ROOT => vec![("children-display", Value::from("submenu"))],
            item::PLAY => label(if self.playing { &l.pause } else { &l.play }),
            item::NEXT => label(&l.next),
            item::PREVIOUS => label(&l.previous),
            item::SEPARATOR => vec![("type", Value::from("separator"))],
            item::RAISE => label(TITLE),
            item::QUIT => label(&l.quit),
            _ => return None,
        })
    }

    /// The properties asked for (all of them when `names` is empty).
    fn filtered(&self, id: i32, names: &[String]) -> Option<HashMap<String, OwnedValue>> {
        let props = self.properties(id)?;
        Some(
            props
                .into_iter()
                .filter(|(k, _)| names.is_empty() || names.iter().any(|n| n == k))
                .map(|(k, v)| (k.to_string(), owned(v)))
                .collect(),
        )
    }

    /// `(ia{sv}av)` for `id`, children down to `depth` levels (-1: all).
    fn layout(&self, id: i32, depth: i32, names: &[String]) -> Option<Layout> {
        let props = self.filtered(id, names)?;
        let mut children = Vec::new();
        if id == item::ROOT && depth != 0 {
            for child in item::ALL {
                if let Some(layout) = self.layout(child, depth - 1, names) {
                    children.push(owned(Value::from(layout)));
                }
            }
        }
        Some((id, props, children))
    }
}

/// A dbusmenu layout node: (id, properties, children as variants of nodes).
type Layout = (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>);

fn owned(v: Value<'_>) -> OwnedValue {
    OwnedValue::try_from(v).expect("no file descriptors")
}

struct Ctx {
    state: Mutex<State>,
    on_command: CommandFn,
    activation_token: Mutex<Option<String>>,
}

impl Ctx {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn send(&self, command: TrayCommand) {
        (self.on_command)(command);
    }
}

/// Wakes the watcher thread to end it.
#[derive(Default)]
struct Stop {
    state: Mutex<(bool, Option<Waker>)>,
}

impl Stop {
    fn stop(&self) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        s.0 = true;
        if let Some(waker) = s.1.take() {
            waker.wake();
        }
    }

    fn wait(&self) -> impl Future<Output = ()> + '_ {
        std::future::poll_fn(move |cx| {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if s.0 {
                Poll::Ready(())
            } else {
                s.1 = Some(cx.waker().clone());
                Poll::Pending
            }
        })
    }
}

// --- The tray icon ---------------------------------------------------------------

/// Tray icons made by this process, for their bus names.
static INSTANCES: AtomicU32 = AtomicU32::new(0);

/// The tray icon on the panel, while it lives.
pub struct Tray {
    conn: Connection,
    ctx: Arc<Ctx>,
    /// The well-known name held (None on a peer-to-peer connection).
    name: Option<String>,
    registered: Arc<AtomicBool>,
    stop: Arc<Stop>,
    watcher: Option<JoinHandle<()>>,
}

impl Tray {
    /// Whether a panel shows tray icons: the watcher is on the bus and a host
    /// has registered with it.
    pub fn available(bus: &Bus) -> bool {
        let Ok(conn) = bus.connect(Duration::from_secs(2)) else {
            return false;
        };
        name_has_owner(&conn, WATCHER)
            && get_property(&conn, WATCHER, WATCHER_PATH, WATCHER, "IsStatusNotifierHostRegistered")
                .ok()
                .and_then(|v| bool::try_from(v).ok())
                .unwrap_or(false)
    }

    /// Puts the icon on the panel; the commands arrive on the receiver (poll
    /// it from the UI thread).
    pub fn start(bus: Bus, labels: TrayLabels) -> zbus::Result<(Tray, mpsc::Receiver<TrayCommand>)> {
        let (tx, rx) = mpsc::channel();
        let tray = Tray::start_with(bus, labels, move |c| {
            let _ = tx.send(c);
        })?;
        Ok((tray, rx))
    }

    /// Puts the icon on the panel; `on_command` runs on a D-Bus thread for
    /// each command (hand it to the UI thread and return).
    ///
    /// Fails when the bus can't be reached or the objects or name can't be
    /// taken. The watcher missing is no error: the icon registers whenever
    /// one appears (also again after the panel restarts).
    pub fn start_with(
        bus: Bus,
        labels: TrayLabels,
        on_command: impl Fn(TrayCommand) + Send + Sync + 'static,
    ) -> zbus::Result<Tray> {
        let conn = bus.connect(Duration::from_secs(5))?;
        let ctx = Arc::new(Ctx {
            state: Mutex::new(State { labels, playing: false, tooltip: String::new(), revision: 1 }),
            on_command: Box::new(on_command),
            activation_token: Mutex::new(None),
        });
        let mut tray = Tray {
            conn,
            ctx,
            name: None,
            registered: Arc::new(AtomicBool::new(false)),
            stop: Arc::new(Stop::default()),
            watcher: None,
        };
        {
            let server = tray.conn.object_server();
            server.at(ITEM_PATH, Item { ctx: Arc::clone(&tray.ctx) })?;
            server.at(MENU_PATH, Menu { ctx: Arc::clone(&tray.ctx) })?;
        }
        // From here on, an early return drops `tray`, which takes them off.
        let service = if tray.conn.is_bus() {
            let n = INSTANCES.fetch_add(1, Ordering::Relaxed) + 1;
            let name = item_service_name(std::process::id(), n);
            let dbus = zbus::blocking::fdo::DBusProxy::new(&tray.conn)?;
            match dbus.request_name(WellKnownName::try_from(name.as_str())?, RequestNameFlags::DoNotQueue.into()) {
                Ok(RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner) => {
                    tray.name = Some(name.clone());
                    name
                }
                // A Flatpak may own only the names its manifest lists (and
                // the pid is the sandbox's): the item goes by the
                // connection's unique name, which watchers take as well.
                _ => match tray.conn.unique_name() {
                    Some(unique) => unique.to_string(),
                    None => return Err(zbus::Error::NameTaken),
                },
            }
        } else {
            // No names on a peer-to-peer connection: the watcher takes an
            // object path and the sender instead (as KDE's does).
            ITEM_PATH.to_string()
        };
        let watcher = {
            let conn = tray.conn.inner().clone();
            let registered = Arc::clone(&tray.registered);
            let stop = Arc::clone(&tray.stop);
            std::thread::Builder::new()
                .name("sonkore-tray".into())
                .spawn(move || watch(conn, service, registered, stop))
                .map_err(|e| zbus::Error::Failure(e.to_string()))?
        };
        tray.watcher = Some(watcher);
        Ok(tray)
    }

    /// The watcher took the icon (it may come later than [`Tray::start`]).
    pub fn is_registered(&self) -> bool {
        self.registered.load(Ordering::SeqCst)
    }

    /// The bus name held (None on a peer-to-peer connection).
    pub fn service_name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// The menu's first entry reads Pause while playing, else Play.
    pub fn set_playing(&self, playing: bool) {
        let (label, revision) = {
            let mut s = self.ctx.state();
            if s.playing == playing {
                return;
            }
            s.playing = playing;
            s.revision = s.revision.wrapping_add(1);
            let l = &s.labels;
            (menu_label(if playing { &l.pause } else { &l.play }), s.revision)
        };
        self.emit_menu_changes(vec![(item::PLAY, HashMap::from([("label", Value::from(label))]))], revision);
    }

    /// New texts (the interface language changed).
    pub fn set_labels(&self, labels: TrayLabels) {
        let (updated, revision) = {
            let mut s = self.ctx.state();
            if s.labels == labels {
                return;
            }
            s.labels = labels;
            s.revision = s.revision.wrapping_add(1);
            let updated: Vec<_> = item::ALL
                .iter()
                .filter_map(|&id| {
                    let props = s.properties(id)?;
                    let labels: HashMap<&str, Value<'static>> =
                        props.into_iter().filter(|(k, _)| *k == "label").collect();
                    (!labels.is_empty()).then_some((id, labels))
                })
                .collect();
            (updated, s.revision)
        };
        self.emit_menu_changes(updated, revision);
    }

    /// The tooltip (the app uses the window title, "Artist — Title").
    pub fn set_tooltip(&self, text: &str) {
        {
            let mut s = self.ctx.state();
            if s.tooltip == text {
                return;
            }
            s.tooltip = text.to_string();
        }
        let _ = self.conn.emit_signal(None::<BusName<'_>>, ITEM_PATH, ITEM_INTERFACE, "NewToolTip", &());
    }

    /// The XDG activation token the panel sent before the last click
    /// (ProvideXdgActivationToken), for raising the window on Wayland.
    pub fn take_activation_token(&self) -> Option<String> {
        self.ctx.activation_token.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    fn emit_menu_changes(&self, updated: Vec<(i32, HashMap<&str, Value<'static>>)>, revision: u32) {
        let removed: Vec<(i32, Vec<&str>)> = Vec::new();
        let no_name = None::<BusName<'_>>;
        let _ = self.conn.emit_signal(
            no_name.clone(),
            MENU_PATH,
            MENU_INTERFACE,
            "ItemsPropertiesUpdated",
            &(updated, removed),
        );
        let _ = self.conn.emit_signal(no_name, MENU_PATH, MENU_INTERFACE, "LayoutUpdated", &(revision, item::ROOT));
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        self.stop.stop();
        if let Some(w) = self.watcher.take() {
            let _ = w.join();
        }
        // The watcher drops the item when its name goes.
        if let Some(name) = self.name.take()
            && let (Ok(dbus), Ok(n)) = (zbus::blocking::fdo::DBusProxy::new(&self.conn), WellKnownName::try_from(name))
        {
            let _ = dbus.release_name(n);
        }
        let server = self.conn.object_server();
        let _ = server.remove::<Item, _>(ITEM_PATH);
        let _ = server.remove::<Menu, _>(MENU_PATH);
    }
}

/// The watcher thread: registers the item, and again each time a watcher
/// takes the name (the panel restarted), until stopped.
fn watch(conn: zbus::Connection, service: String, registered: Arc<AtomicBool>, stop: Arc<Stop>) {
    let work = async {
        // Listening first: a watcher appearing meanwhile is not missed.
        let stream = match name_owner_rule(conn.is_bus()) {
            Ok(rule) => zbus::MessageStream::for_match_rule(rule, &conn, Some(8)).await.ok(),
            Err(_) => None,
        };
        register(&conn, &service, &registered).await;
        let Some(mut stream) = stream else {
            return;
        };
        while let Some(msg) = stream.next().await {
            let Ok(msg) = msg else {
                continue;
            };
            let Ok((name, _old, new)) = msg.body().deserialize::<(String, String, String)>() else {
                continue;
            };
            if name != WATCHER {
                continue;
            }
            if new.is_empty() {
                registered.store(false, Ordering::SeqCst);
            } else {
                register(&conn, &service, &registered).await;
            }
        }
    };
    futures_lite::future::block_on(futures_lite::future::or(stop.wait(), work));
}

/// NameOwnerChanged for the watcher (from the bus daemon; on a peer-to-peer
/// connection, from the peer).
fn name_owner_rule(bus: bool) -> zbus::Result<zbus::MatchRule<'static>> {
    let mut rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface("org.freedesktop.DBus")?
        .member("NameOwnerChanged")?
        .path("/org/freedesktop/DBus")?
        .add_arg(WATCHER)?;
    if bus {
        rule = rule.sender("org.freedesktop.DBus")?;
    }
    Ok(rule.build())
}

async fn register(conn: &zbus::Connection, service: &str, registered: &AtomicBool) {
    let destination = conn.is_bus().then_some(WATCHER);
    let ok = conn
        .call_method(destination, WATCHER_PATH, Some(WATCHER), "RegisterStatusNotifierItem", &(service,))
        .await
        .is_ok();
    registered.store(ok, Ordering::SeqCst);
}

// --- org.kde.StatusNotifierItem ---------------------------------------------------

struct Item {
    ctx: Arc<Ctx>,
}

#[zbus::interface(name = "org.kde.StatusNotifierItem")]
impl Item {
    /// A left click.
    #[zbus(name = "Activate")]
    fn activate(&self, _x: i32, _y: i32) {
        self.ctx.send(TrayCommand::Activate);
    }

    /// A middle click (ignored).
    #[zbus(name = "SecondaryActivate")]
    fn secondary_activate(&self, _x: i32, _y: i32) {}

    /// Only for panels that don't read `Menu`; they all do.
    #[zbus(name = "ContextMenu")]
    fn context_menu(&self, _x: i32, _y: i32) {}

    #[zbus(name = "Scroll")]
    fn scroll(&self, _delta: i32, _orientation: String) {}

    /// KDE: sent before Activate, for the window to take the focus on Wayland.
    #[zbus(name = "ProvideXdgActivationToken")]
    fn provide_xdg_activation_token(&self, token: String) {
        *self.ctx.activation_token.lock().unwrap_or_else(|e| e.into_inner()) = Some(token);
    }

    #[zbus(signal, name = "NewTitle")]
    async fn new_title(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(signal, name = "NewIcon")]
    async fn new_icon(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(signal, name = "NewAttentionIcon")]
    async fn new_attention_icon(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(signal, name = "NewOverlayIcon")]
    async fn new_overlay_icon(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(signal, name = "NewToolTip")]
    async fn new_tool_tip(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(signal, name = "NewStatus")]
    async fn new_status(emitter: &SignalEmitter<'_>, status: &str) -> zbus::Result<()>;

    #[zbus(property, name = "Category")]
    fn category(&self) -> &str {
        "ApplicationStatus"
    }

    #[zbus(property, name = "Id")]
    fn id(&self) -> &str {
        ID
    }

    #[zbus(property, name = "Title")]
    fn title(&self) -> &str {
        TITLE
    }

    #[zbus(property, name = "Status")]
    fn status(&self) -> &str {
        "Active"
    }

    #[zbus(property, name = "WindowId")]
    fn window_id(&self) -> i32 {
        0
    }

    #[zbus(property, name = "IconThemePath")]
    fn icon_theme_path(&self) -> &str {
        ""
    }

    #[zbus(property, name = "IconName")]
    fn icon_name(&self) -> &str {
        ICON_NAME
    }

    #[zbus(property, name = "IconPixmap")]
    fn icon_pixmap(&self) -> Vec<Pixmap> {
        icon_pixmaps().to_vec()
    }

    #[zbus(property, name = "OverlayIconName")]
    fn overlay_icon_name(&self) -> &str {
        ""
    }

    #[zbus(property, name = "OverlayIconPixmap")]
    fn overlay_icon_pixmap(&self) -> Vec<Pixmap> {
        Vec::new()
    }

    #[zbus(property, name = "AttentionIconName")]
    fn attention_icon_name(&self) -> &str {
        ""
    }

    #[zbus(property, name = "AttentionIconPixmap")]
    fn attention_icon_pixmap(&self) -> Vec<Pixmap> {
        Vec::new()
    }

    #[zbus(property, name = "AttentionMovieName")]
    fn attention_movie_name(&self) -> &str {
        ""
    }

    /// (icon name, icon pixmaps, title, description).
    #[zbus(property, name = "ToolTip")]
    fn tool_tip(&self) -> (String, Vec<Pixmap>, String, String) {
        let text = self.ctx.state().tooltip.clone();
        (ICON_NAME.to_string(), icon_pixmaps().to_vec(), text, String::new())
    }

    /// Left click activates; the menu is for the right one.
    #[zbus(property, name = "ItemIsMenu")]
    fn item_is_menu(&self) -> bool {
        false
    }

    #[zbus(property, name = "Menu")]
    fn menu(&self) -> ObjectPath<'static> {
        ObjectPath::from_static_str_unchecked(MENU_PATH)
    }
}

// --- com.canonical.dbusmenu -------------------------------------------------------

struct Menu {
    ctx: Arc<Ctx>,
}

fn unknown_id(id: i32) -> zbus::fdo::Error {
    zbus::fdo::Error::InvalidArgs(format!("no menu item {id}"))
}

impl Menu {
    /// An event on an item; false for an id the menu doesn't have.
    fn event(&self, id: i32, event_id: &str) -> bool {
        if self.ctx.state().properties(id).is_none() {
            return false;
        }
        if event_id == "clicked" {
            let command = match id {
                item::PLAY => Some(TrayCommand::TogglePlay),
                item::NEXT => Some(TrayCommand::Next),
                item::PREVIOUS => Some(TrayCommand::Previous),
                item::RAISE => Some(TrayCommand::Raise),
                item::QUIT => Some(TrayCommand::Quit),
                _ => None,
            };
            if let Some(c) = command {
                self.ctx.send(c);
            }
        }
        true
    }
}

#[zbus::interface(name = "com.canonical.dbusmenu")]
impl Menu {
    #[zbus(name = "GetLayout", out_args("revision", "layout"))]
    fn get_layout(
        &self,
        parent_id: i32,
        recursion_depth: i32,
        property_names: Vec<String>,
    ) -> zbus::fdo::Result<(u32, Layout)> {
        let s = self.ctx.state();
        let layout = s.layout(parent_id, recursion_depth, &property_names).ok_or_else(|| unknown_id(parent_id))?;
        Ok((s.revision, layout))
    }

    #[zbus(name = "GetGroupProperties")]
    fn get_group_properties(
        &self,
        ids: Vec<i32>,
        property_names: Vec<String>,
    ) -> Vec<(i32, HashMap<String, OwnedValue>)> {
        let s = self.ctx.state();
        let all: Vec<i32> = std::iter::once(item::ROOT).chain(item::ALL).collect();
        let ids = if ids.is_empty() { all } else { ids };
        ids.into_iter().filter_map(|id| Some((id, s.filtered(id, &property_names)?))).collect()
    }

    #[zbus(name = "GetProperty")]
    fn get_property(&self, id: i32, name: String) -> zbus::fdo::Result<OwnedValue> {
        let s = self.ctx.state();
        let props = s.properties(id).ok_or_else(|| unknown_id(id))?;
        props
            .into_iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| owned(v))
            .ok_or_else(|| zbus::fdo::Error::InvalidArgs(format!("no property {name} on item {id}")))
    }

    #[zbus(name = "Event")]
    fn event_method(&self, id: i32, event_id: String, _data: OwnedValue, _timestamp: u32) -> zbus::fdo::Result<()> {
        if self.event(id, &event_id) { Ok(()) } else { Err(unknown_id(id)) }
    }

    /// Returns the ids not found.
    #[zbus(name = "EventGroup")]
    fn event_group(&self, events: Vec<(i32, String, OwnedValue, u32)>) -> zbus::fdo::Result<Vec<i32>> {
        let total = events.len();
        let errors: Vec<i32> =
            events.into_iter().filter(|(id, event_id, _, _)| !self.event(*id, event_id)).map(|(id, ..)| id).collect();
        if total > 0 && errors.len() == total {
            return Err(zbus::fdo::Error::InvalidArgs("no such menu items".into()));
        }
        Ok(errors)
    }

    /// The menu never changes on opening.
    #[zbus(name = "AboutToShow")]
    fn about_to_show(&self, id: i32) -> zbus::fdo::Result<bool> {
        self.ctx.state().properties(id).map(|_| false).ok_or_else(|| unknown_id(id))
    }

    #[zbus(name = "AboutToShowGroup", out_args("updatesNeeded", "idErrors"))]
    fn about_to_show_group(&self, ids: Vec<i32>) -> (Vec<i32>, Vec<i32>) {
        let s = self.ctx.state();
        (Vec::new(), ids.into_iter().filter(|&id| s.properties(id).is_none()).collect())
    }

    #[zbus(signal, name = "ItemsPropertiesUpdated")]
    async fn items_properties_updated(
        emitter: &SignalEmitter<'_>,
        updated_props: Vec<(i32, HashMap<String, OwnedValue>)>,
        removed_props: Vec<(i32, Vec<String>)>,
    ) -> zbus::Result<()>;

    #[zbus(signal, name = "LayoutUpdated")]
    async fn layout_updated(emitter: &SignalEmitter<'_>, revision: u32, parent: i32) -> zbus::Result<()>;

    #[zbus(signal, name = "ItemActivationRequested")]
    async fn item_activation_requested(emitter: &SignalEmitter<'_>, id: i32, timestamp: u32) -> zbus::Result<()>;

    #[zbus(property, name = "Version")]
    fn version(&self) -> u32 {
        3
    }

    #[zbus(property, name = "TextDirection")]
    fn text_direction(&self) -> &str {
        if self.ctx.state().labels.rtl { "rtl" } else { "ltr" }
    }

    #[zbus(property, name = "Status")]
    fn status(&self) -> &str {
        "normal"
    }

    #[zbus(property, name = "IconThemePath")]
    fn icon_theme_path(&self) -> Vec<String> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(playing: bool) -> State {
        State { labels: TrayLabels::default(), playing, tooltip: String::new(), revision: 1 }
    }

    fn label(s: &State, id: i32) -> Option<String> {
        let props = s.filtered(id, &["label".to_string()])?;
        props.get("label").map(|v| String::try_from(v.try_clone().unwrap()).unwrap())
    }

    #[test]
    fn labels_follow_the_player() {
        assert_eq!(label(&state(false), item::PLAY).as_deref(), Some("Play"));
        assert_eq!(label(&state(true), item::PLAY).as_deref(), Some("Pause"));
        assert_eq!(label(&state(true), item::RAISE).as_deref(), Some("Sonkore"));
        assert_eq!(label(&state(true), item::SEPARATOR), None);
        assert!(state(true).properties(42).is_none());
    }

    #[test]
    fn underscores_are_literal() {
        assert_eq!(menu_label("Sa_lir"), "Sa__lir");
        assert_eq!(menu_label("Play"), "Play");
    }

    #[test]
    fn layout_depth() {
        let s = state(false);
        let (id, props, children) = s.layout(item::ROOT, -1, &[]).unwrap();
        assert_eq!(id, 0);
        assert!(props.contains_key("children-display"));
        assert_eq!(children.len(), 6);
        assert!(s.layout(item::ROOT, 0, &[]).unwrap().2.is_empty());
        assert!(s.layout(item::NEXT, -1, &[]).unwrap().2.is_empty());
        assert!(s.layout(99, -1, &[]).is_none());
    }

    #[test]
    fn icon_pixmaps_are_argb() {
        let pixmaps = icon_pixmaps();
        let sizes: Vec<i32> = pixmaps.iter().map(|p| p.0).collect();
        assert_eq!(sizes, [16, 22, 24, 32, 48, 64]);
        for (w, h, data) in pixmaps {
            assert_eq!(w, h);
            assert_eq!(data.len(), (w * h * 4) as usize);
            // A transparent corner and an opaque centre: alpha first.
            assert_eq!(data[0], 0, "{w} px corner");
            let centre = ((h / 2 * w + w / 2) * 4) as usize;
            assert_eq!(data[centre], 255, "{w} px centre");
        }
    }

    #[test]
    fn service_names() {
        assert_eq!(item_service_name(1234, 1), "org.kde.StatusNotifierItem-1234-1");
    }

    /// Only inside a Flatpak (as in the sandbox these tests run in): its bus
    /// proxy refuses `org.kde.StatusNotifierItem-<pid>-<n>`, and the icon
    /// still starts, by its unique name.
    #[test]
    fn starts_in_a_flatpak_without_its_own_name() {
        if !std::path::Path::new("/.flatpak-info").exists() || Bus::Session.connect(Duration::from_secs(2)).is_err() {
            return;
        }
        let tray = Tray::start(Bus::Session, TrayLabels::default()).unwrap().0;
        assert!(tray.name.is_none());
    }

    #[test]
    fn no_bus_no_tray() {
        let bus = Bus::Address("unix:path=/nonexistent/sonkore-test".into());
        assert!(!Tray::available(&bus));
        assert!(Tray::start(bus, TrayLabels::default()).is_err());
    }
}
