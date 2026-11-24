//! The tray icon end to end over a peer-to-peer connection (no bus daemon,
//! nothing reaches the user's session bus, so no icon on their panel): the
//! peer plays the StatusNotifierWatcher and the panel.

#![cfg(target_os = "linux")]

use std::collections::HashMap;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use sonkore_platform::Bus;
use sonkore_platform::tray::{self, Tray, TrayCommand, TrayLabels, item};
use zbus::blocking::Connection;
use zbus::blocking::connection::Builder;
use zbus::names::BusName;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

/// (server, client)
#[allow(deprecated)] // unix_stream: fine without the tokio feature
fn pair() -> (Connection, Connection) {
    let (a, b) = UnixStream::pair().unwrap();
    let server = std::thread::spawn(move || {
        Builder::unix_stream(a).server(zbus::Guid::generate()).unwrap().p2p().build().unwrap()
    });
    let client = Builder::unix_stream(b).p2p().method_timeout(Duration::from_secs(5)).build().unwrap();
    let server = server.join().unwrap();
    // Both sides serve objects: listening before any call comes.
    let _ = server.object_server();
    let _ = client.object_server();
    std::thread::sleep(Duration::from_millis(100));
    (server, client)
}

struct FakeWatcher {
    registrations: Mutex<mpsc::Sender<String>>,
    host: Arc<AtomicBool>,
}

#[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
impl FakeWatcher {
    #[zbus(name = "RegisterStatusNotifierItem")]
    fn register_status_notifier_item(&self, service: String) {
        let _ = self.registrations.lock().unwrap().send(service);
    }

    #[zbus(property, name = "IsStatusNotifierHostRegistered")]
    fn is_status_notifier_host_registered(&self) -> bool {
        self.host.load(Ordering::SeqCst)
    }

    #[zbus(property, name = "ProtocolVersion")]
    fn protocol_version(&self) -> i32 {
        0
    }
}

fn call<B, R>(conn: &Connection, path: &str, iface: &str, method: &str, body: &B) -> zbus::Result<R>
where
    B: serde::ser::Serialize + zbus::zvariant::DynamicType,
    R: for<'d> zbus::zvariant::DynamicDeserialize<'d>,
{
    let reply = conn.call_method(None::<BusName<'_>>, path, Some(iface), method, body)?;
    reply.body().deserialize()
}

fn sni(conn: &Connection, name: &str) -> zbus::Result<OwnedValue> {
    call(conn, tray::ITEM_PATH, "org.freedesktop.DBus.Properties", "Get", &(tray::ITEM_INTERFACE, name))
}

fn string(v: OwnedValue) -> String {
    String::try_from(v).unwrap()
}

type Node = (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>);
type Pixmaps = Vec<(i32, i32, Vec<u8>)>;
type PropertiesUpdated = (Vec<(i32, HashMap<String, OwnedValue>)>, Vec<(i32, Vec<String>)>);

fn node(v: OwnedValue) -> Node {
    match Value::from(v) {
        Value::Structure(s) => {
            let mut f = s.into_fields().into_iter();
            let id = i32::try_from(f.next().unwrap()).unwrap();
            let props = HashMap::<String, OwnedValue>::try_from(f.next().unwrap()).unwrap();
            let children = match f.next().unwrap() {
                Value::Array(a) => a.iter().map(|c| OwnedValue::try_from(c.try_clone().unwrap()).unwrap()).collect(),
                other => panic!("children: {other:?}"),
            };
            (id, props, children)
        }
        other => panic!("not a layout node: {other:?}"),
    }
}

/// The labels of the root's children ("" for the separator).
fn menu_labels(client: &Connection) -> (u32, Vec<String>) {
    let (revision, root): (u32, Node) =
        call(client, tray::MENU_PATH, tray::MENU_INTERFACE, "GetLayout", &(0i32, -1i32, Vec::<String>::new())).unwrap();
    assert_eq!(root.0, item::ROOT);
    assert_eq!(string(root.1["children-display"].try_clone().unwrap()), "submenu");
    let labels = root
        .2
        .into_iter()
        .map(|c| {
            let (_, props, _) = node(c);
            props.get("label").map(|v| string(v.try_clone().unwrap())).unwrap_or_default()
        })
        .collect();
    (revision, labels)
}

fn signals(client: &Connection) -> mpsc::Receiver<zbus::Message> {
    let (tx, rx) = mpsc::channel();
    let iter = zbus::blocking::MessageIterator::from(client);
    std::thread::spawn(move || {
        for msg in iter.flatten() {
            if msg.message_type() == zbus::message::Type::Signal && tx.send(msg).is_err() {
                break;
            }
        }
    });
    rx
}

fn next_signal(sig: &mpsc::Receiver<zbus::Message>, member: &str) -> zbus::Message {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let m = sig.recv_timeout(deadline.saturating_duration_since(Instant::now())).expect(member);
        if m.header().member().is_some_and(|n| n.as_str() == member) {
            return m;
        }
    }
}

fn wait_for(what: &str, f: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn no_watcher_no_tray() {
    let (server, _client) = pair();
    assert!(!Tray::available(&Bus::Connection(server)));
}

#[test]
fn tray_end_to_end() {
    let (server, client) = pair();
    let (reg_tx, registrations) = mpsc::channel();
    let host = Arc::new(AtomicBool::new(false));
    client
        .object_server()
        .at(tray::WATCHER_PATH, FakeWatcher { registrations: Mutex::new(reg_tx), host: Arc::clone(&host) })
        .unwrap();

    // Available once a host has registered with the watcher.
    let bus = Bus::Connection(server);
    assert!(!Tray::available(&bus));
    host.store(true, Ordering::SeqCst);
    assert!(Tray::available(&bus));

    let labels = TrayLabels {
        play: "Reproducir".into(),
        pause: "Pausa".into(),
        next: "Siguiente".into(),
        previous: "Anterior".into(),
        quit: "Sa_lir".into(),
        rtl: false,
    };
    let (tray, rx) = Tray::start(bus, labels.clone()).unwrap();
    assert_eq!(tray.service_name(), None, "no names on a peer-to-peer connection");
    let service = registrations.recv_timeout(Duration::from_secs(5)).expect("a registration");
    assert_eq!(service, tray::ITEM_PATH);
    wait_for("registered", || tray.is_registered());

    // The item.
    assert_eq!(string(sni(&client, "Id").unwrap()), "sonkore");
    assert_eq!(string(sni(&client, "Title").unwrap()), "Sonkore");
    assert_eq!(string(sni(&client, "Category").unwrap()), "ApplicationStatus");
    assert_eq!(string(sni(&client, "Status").unwrap()), "Active");
    assert_eq!(string(sni(&client, "IconName").unwrap()), "com.axiokyn.Sonkore");
    assert!(!bool::try_from(sni(&client, "ItemIsMenu").unwrap()).unwrap());
    let menu = ObjectPath::try_from(sni(&client, "Menu").unwrap()).unwrap();
    assert_eq!(menu.as_str(), "/MenuBar");
    let pixmaps: Pixmaps = sni(&client, "IconPixmap").unwrap().try_into().unwrap();
    assert_eq!(pixmaps.iter().map(|p| p.0).collect::<Vec<_>>(), [16, 22, 24, 32, 48, 64]);
    let (w, h, data) = &pixmaps[5];
    assert_eq!(data.len(), (w * h * 4) as usize);
    assert_eq!(data[((h / 2 * w + w / 2) * 4) as usize], 255, "ARGB: alpha first");

    // The menu, in order; the underscore is literal.
    let (rev0, items) = menu_labels(&client);
    assert_eq!(items, ["Reproducir", "Siguiente", "Anterior", "", "Sonkore", "Sa__lir"]);
    let (_, sep): (u32, Node) = call(
        &client,
        tray::MENU_PATH,
        tray::MENU_INTERFACE,
        "GetLayout",
        &(item::SEPARATOR, 0i32, Vec::<String>::new()),
    )
    .unwrap();
    assert_eq!(string(sep.1["type"].try_clone().unwrap()), "separator");
    let (_, play): (u32, Node) =
        call(&client, tray::MENU_PATH, tray::MENU_INTERFACE, "GetLayout", &(item::PLAY, 0i32, vec!["label"])).unwrap();
    assert_eq!(play.1.len(), 1);
    assert!(play.2.is_empty());
    let none: zbus::Result<(u32, Node)> =
        call(&client, tray::MENU_PATH, tray::MENU_INTERFACE, "GetLayout", &(99i32, -1i32, Vec::<String>::new()));
    assert!(none.is_err());
    let group: Vec<(i32, HashMap<String, OwnedValue>)> = call(
        &client,
        tray::MENU_PATH,
        tray::MENU_INTERFACE,
        "GetGroupProperties",
        &(vec![item::NEXT, 99], vec!["label"]),
    )
    .unwrap();
    assert_eq!(group.len(), 1);
    assert_eq!(string(group[0].1["label"].try_clone().unwrap()), "Siguiente");
    let quit: OwnedValue =
        call(&client, tray::MENU_PATH, tray::MENU_INTERFACE, "GetProperty", &(item::QUIT, "label")).unwrap();
    assert_eq!(string(quit), "Sa__lir");
    let needs: bool = call(&client, tray::MENU_PATH, tray::MENU_INTERFACE, "AboutToShow", &(0i32,)).unwrap();
    assert!(!needs);

    // Clicks become commands.
    let click = |id: i32, event: &str| {
        call::<_, ()>(&client, tray::MENU_PATH, tray::MENU_INTERFACE, "Event", &(id, event, Value::from(0i32), 0u32))
    };
    for (id, command) in [
        (item::PLAY, TrayCommand::TogglePlay),
        (item::NEXT, TrayCommand::Next),
        (item::PREVIOUS, TrayCommand::Previous),
        (item::RAISE, TrayCommand::Raise),
        (item::QUIT, TrayCommand::Quit),
    ] {
        click(id, "clicked").unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), command);
    }
    click(item::NEXT, "hovered").unwrap();
    click(item::SEPARATOR, "clicked").unwrap();
    assert!(click(99, "clicked").is_err());
    let errors: Vec<i32> = call(
        &client,
        tray::MENU_PATH,
        tray::MENU_INTERFACE,
        "EventGroup",
        &(vec![(item::PREVIOUS, "clicked", Value::from(0i32), 0u32), (99, "clicked", Value::from(0i32), 0u32)],),
    )
    .unwrap();
    assert_eq!(errors, [99]);
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), TrayCommand::Previous);

    // A left click; the middle click, the scroll wheel and ContextMenu do nothing.
    let item_call =
        |m: &str| call::<_, ()>(&client, tray::ITEM_PATH, tray::ITEM_INTERFACE, m, &(10i32, 20i32)).unwrap();
    call::<_, ()>(&client, tray::ITEM_PATH, tray::ITEM_INTERFACE, "ProvideXdgActivationToken", &("token-1",)).unwrap();
    item_call("Activate");
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), TrayCommand::Activate);
    assert_eq!(tray.take_activation_token().as_deref(), Some("token-1"));
    assert_eq!(tray.take_activation_token(), None);
    item_call("SecondaryActivate");
    item_call("ContextMenu");
    call::<_, ()>(&client, tray::ITEM_PATH, tray::ITEM_INTERFACE, "Scroll", &(120i32, "vertical")).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert!(rx.try_recv().is_err(), "no other commands");

    // Playing: the first entry reads Pause, announced both ways.
    let sig = signals(&client);
    tray.set_playing(true);
    let msg = next_signal(&sig, "ItemsPropertiesUpdated");
    assert_eq!(msg.header().path().unwrap().as_str(), "/MenuBar");
    let (updated, removed): PropertiesUpdated = msg.body().deserialize().unwrap();
    assert_eq!(updated.len(), 1);
    assert_eq!(updated[0].0, item::PLAY);
    assert_eq!(string(updated[0].1["label"].try_clone().unwrap()), "Pausa");
    assert!(removed.is_empty());
    let msg = next_signal(&sig, "LayoutUpdated");
    let (revision, parent): (u32, i32) = msg.body().deserialize().unwrap();
    assert!(revision > rev0);
    assert_eq!(parent, 0);
    let (rev1, items) = menu_labels(&client);
    assert_eq!(rev1, revision);
    assert_eq!(items[0], "Pausa");
    tray.set_playing(false);
    next_signal(&sig, "LayoutUpdated");
    assert_eq!(menu_labels(&client).1[0], "Reproducir");

    // New labels (the language changed).
    tray.set_labels(TrayLabels { quit: "Quit".into(), ..labels });
    next_signal(&sig, "LayoutUpdated");
    assert_eq!(menu_labels(&client).1[5], "Quit");

    // The tooltip.
    tray.set_tooltip("Artist — Title");
    let msg = next_signal(&sig, "NewToolTip");
    assert_eq!(msg.header().path().unwrap().as_str(), "/StatusNotifierItem");
    let (icon, pix, title, description): (String, Pixmaps, String, String) =
        sni(&client, "ToolTip").unwrap().try_into().unwrap();
    assert_eq!(icon, "com.axiokyn.Sonkore");
    assert!(!pix.is_empty());
    assert_eq!(title, "Artist — Title");
    assert_eq!(description, "");

    // The watcher went away and came back (the panel restarted): again.
    let owner_changed = |old: &str, new: &str| {
        client
            .emit_signal(
                None::<BusName<'_>>,
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
                "NameOwnerChanged",
                &(tray::WATCHER, old, new),
            )
            .unwrap();
    };
    owner_changed(":1.7", "");
    wait_for("unregistered", || !tray.is_registered());
    owner_changed("", ":1.8");
    assert_eq!(registrations.recv_timeout(Duration::from_secs(5)).unwrap(), tray::ITEM_PATH);
    wait_for("registered again", || tray.is_registered());
    // Another name's owner changing is no reason to register.
    client
        .emit_signal(
            None::<BusName<'_>>,
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "NameOwnerChanged",
            &("org.example.Other", "", ":1.9"),
        )
        .unwrap();
    std::thread::sleep(Duration::from_millis(150));
    assert!(registrations.try_recv().is_err());

    // The names zbus derived are the protocol's.
    let xml: String = call(&client, tray::MENU_PATH, "org.freedesktop.DBus.Introspectable", "Introspect", &()).unwrap();
    for name in ["GetLayout", "GetGroupProperties", "AboutToShowGroup", "ItemsPropertiesUpdated", "LayoutUpdated"] {
        assert!(xml.contains(&format!("name=\"{name}\"")), "{name} not in the menu's introspection");
    }
    let xml: String = call(&client, tray::ITEM_PATH, "org.freedesktop.DBus.Introspectable", "Introspect", &()).unwrap();
    for name in ["Activate", "SecondaryActivate", "NewToolTip", "IconPixmap", "ToolTip", "ItemIsMenu"] {
        assert!(xml.contains(&format!("name=\"{name}\"")), "{name} not in the item's introspection");
    }

    // Dropped: off the panel, the objects gone.
    drop(tray);
    assert!(sni(&client, "Id").is_err());
    let gone: zbus::Result<(u32, Node)> =
        call(&client, tray::MENU_PATH, tray::MENU_INTERFACE, "GetLayout", &(0i32, -1i32, Vec::<String>::new()));
    assert!(gone.is_err());
}
