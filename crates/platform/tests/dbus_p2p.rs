//! The D-Bus objects end to end, over a peer-to-peer connection (no bus
//! daemon needed, nothing reaches the user's session bus): a zbus client
//! calls them as the desktop would.

#![cfg(target_os = "linux")]

use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use sonkore_platform::Bus;
use sonkore_platform::mpris::{self, Command, Mpris, PlaybackStatus, PlayerState, Repeat, TrackInfo};
use sonkore_platform::reservation::{self, DeviceReservation};
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
    // As Bus::connect does: the object server listens before any call comes
    // (zbus drops calls that arrive while its dispatch task is starting).
    let _ = server.object_server();
    std::thread::sleep(Duration::from_millis(100));
    (server, client)
}

fn call<B, R>(conn: &Connection, path: &str, iface: &str, method: &str, body: &B) -> zbus::Result<R>
where
    B: serde::ser::Serialize + zbus::zvariant::DynamicType,
    R: for<'d> zbus::zvariant::DynamicDeserialize<'d>,
{
    let reply = conn.call_method(None::<BusName<'_>>, path, Some(iface), method, body)?;
    reply.body().deserialize()
}

fn get(conn: &Connection, path: &str, iface: &str, name: &str) -> zbus::Result<OwnedValue> {
    call(conn, path, "org.freedesktop.DBus.Properties", "Get", &(iface, name))
}

fn set(conn: &Connection, iface: &str, name: &str, value: Value<'_>) -> zbus::Result<()> {
    let reply = conn.call_method(
        None::<BusName<'_>>,
        mpris::PATH,
        Some("org.freedesktop.DBus.Properties"),
        "Set",
        &(iface, name, value),
    )?;
    let _ = reply;
    Ok(())
}

fn player(conn: &Connection, name: &str) -> OwnedValue {
    get(conn, mpris::PATH, mpris::PLAYER_INTERFACE, name).unwrap()
}

fn root(conn: &Connection, name: &str) -> OwnedValue {
    get(conn, mpris::PATH, mpris::ROOT_INTERFACE, name).unwrap()
}

fn string(v: OwnedValue) -> String {
    String::try_from(v).unwrap()
}

fn next_command(rx: &mpsc::Receiver<Command>) -> Command {
    rx.recv_timeout(Duration::from_secs(5)).expect("a command")
}

/// Signals from the service, in order: (member, body signature-checked later).
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

fn track() -> TrackInfo {
    TrackInfo {
        path: PathBuf::from("/music/A B/01.flac"),
        title: "Song".into(),
        album: "Album".into(),
        artist: "Artist".into(),
        track_no: 1,
        duration: 200.0,
        ..Default::default()
    }
}

#[test]
fn mpris_end_to_end() {
    let (server, client) = pair();
    let (tx, rx) = mpsc::channel();
    let tx = std::sync::Mutex::new(tx);
    let service = Mpris::new(Bus::Connection(server), move |c| {
        let _ = tx.lock().unwrap().send(c);
    });
    // Off the bus until enabled.
    assert!(!service.is_registered());
    assert!(get(&client, mpris::PATH, mpris::ROOT_INTERFACE, "Identity").is_err());
    service.set_enabled(true);
    service.flush();
    assert_eq!(service.service_name().as_deref(), Some(""));

    // Root interface.
    assert_eq!(string(root(&client, "Identity")), "Sonkore");
    assert_eq!(string(root(&client, "DesktopEntry")), "com.axiokyn.Sonkore");
    assert_eq!(Vec::<String>::try_from(root(&client, "SupportedUriSchemes")).unwrap(), ["file"]);
    assert_eq!(Vec::<String>::try_from(root(&client, "SupportedMimeTypes")).unwrap().len(), 5);
    assert!(bool::try_from(root(&client, "CanRaise")).unwrap());
    assert!(!bool::try_from(root(&client, "HasTrackList")).unwrap());

    // Nothing loaded.
    assert_eq!(string(player(&client, "PlaybackStatus")), "Stopped");
    assert!(!bool::try_from(player(&client, "CanSeek")).unwrap());
    assert_eq!(f64::try_from(player(&client, "Rate")).unwrap(), 1.0);

    let sig = signals(&client);
    service.update(PlayerState {
        status: PlaybackStatus::Paused,
        repeat: Repeat::All,
        shuffle: true,
        volume: 0.25,
        position: 30.0,
        duration: 200.0,
        has_queue: true,
        track: Some(track()),
    });
    service.flush();

    // The change is announced in one PropertiesChanged.
    let msg = sig.recv_timeout(Duration::from_secs(5)).unwrap();
    let header = msg.header();
    assert_eq!(header.member().unwrap().as_str(), "PropertiesChanged");
    let (iface, changed, _): (String, std::collections::HashMap<String, OwnedValue>, Vec<String>) =
        msg.body().deserialize().unwrap();
    assert_eq!(iface, mpris::PLAYER_INTERFACE);
    for p in ["PlaybackStatus", "LoopStatus", "Shuffle", "Volume", "Metadata", "CanGoNext", "CanSeek"] {
        assert!(changed.contains_key(p), "{p} missing from {changed:?}");
    }
    assert!(!changed.contains_key("Position"));

    assert_eq!(string(player(&client, "PlaybackStatus")), "Paused");
    assert_eq!(string(player(&client, "LoopStatus")), "Playlist");
    assert!(bool::try_from(player(&client, "Shuffle")).unwrap());
    assert_eq!(f64::try_from(player(&client, "Volume")).unwrap(), 0.25);
    assert_eq!(i64::try_from(player(&client, "Position")).unwrap(), 30_000_000);
    assert!(bool::try_from(player(&client, "CanGoNext")).unwrap());
    let meta: std::collections::HashMap<String, OwnedValue> = player(&client, "Metadata").try_into().unwrap();
    let track_id = ObjectPath::try_from(meta["mpris:trackid"].try_clone().unwrap()).unwrap().to_string();
    assert_eq!(track_id, "/org/sonkore/track/1");
    assert_eq!(i64::try_from(meta["mpris:length"].try_clone().unwrap()).unwrap(), 200_000_000);
    assert_eq!(string(meta["xesam:url"].try_clone().unwrap()), "file:///music/A%20B/01.flac");

    // Methods become commands.
    let player_call = |m: &str| call::<_, ()>(&client, mpris::PATH, mpris::PLAYER_INTERFACE, m, &()).unwrap();
    for (m, c) in [
        ("PlayPause", Command::PlayPause),
        ("Play", Command::Play),
        ("Pause", Command::Pause),
        ("Stop", Command::Stop),
        ("Next", Command::Next),
        ("Previous", Command::Previous),
    ] {
        player_call(m);
        assert_eq!(next_command(&rx), c);
    }
    call::<_, ()>(&client, mpris::PATH, mpris::ROOT_INTERFACE, "Raise", &()).unwrap();
    assert_eq!(next_command(&rx), Command::Raise);
    call::<_, ()>(&client, mpris::PATH, mpris::ROOT_INTERFACE, "Quit", &()).unwrap();
    assert_eq!(next_command(&rx), Command::Quit);

    // Paused at 30 s: +5 s, then past the end.
    call::<_, ()>(&client, mpris::PATH, mpris::PLAYER_INTERFACE, "Seek", &(5_000_000i64,)).unwrap();
    assert_eq!(next_command(&rx), Command::SeekTo(35.0));
    call::<_, ()>(&client, mpris::PATH, mpris::PLAYER_INTERFACE, "Seek", &(500_000_000i64,)).unwrap();
    assert_eq!(next_command(&rx), Command::Next);

    // SetPosition for another track is ignored; for this one it seeks.
    let stale = ObjectPath::try_from("/org/sonkore/track/99").unwrap();
    call::<_, ()>(&client, mpris::PATH, mpris::PLAYER_INTERFACE, "SetPosition", &(stale, 1_000_000i64)).unwrap();
    let current = ObjectPath::try_from(track_id.as_str()).unwrap();
    call::<_, ()>(&client, mpris::PATH, mpris::PLAYER_INTERFACE, "SetPosition", &(current, 12_000_000i64)).unwrap();
    assert_eq!(next_command(&rx), Command::SeekTo(12.0));

    call::<_, ()>(&client, mpris::PATH, mpris::PLAYER_INTERFACE, "OpenUri", &("file:///x/a%20b.flac",)).unwrap();
    assert_eq!(next_command(&rx), Command::Open("/x/a b.flac".into()));
    call::<_, ()>(&client, mpris::PATH, mpris::PLAYER_INTERFACE, "OpenUri", &("https://r.org/s.mp3",)).unwrap();
    assert_eq!(next_command(&rx), Command::Open("https://r.org/s.mp3".into()));

    // Writable properties.
    set(&client, mpris::PLAYER_INTERFACE, "LoopStatus", Value::from("Track")).unwrap();
    assert_eq!(next_command(&rx), Command::SetRepeat(Repeat::One));
    assert_eq!(string(player(&client, "LoopStatus")), "Track");
    set(&client, mpris::PLAYER_INTERFACE, "Shuffle", Value::from(false)).unwrap();
    assert_eq!(next_command(&rx), Command::SetShuffle(false));
    set(&client, mpris::PLAYER_INTERFACE, "Volume", Value::from(1.7)).unwrap();
    assert_eq!(next_command(&rx), Command::SetVolume(1.0));
    set(&client, mpris::PLAYER_INTERFACE, "Rate", Value::from(2.0)).unwrap();
    assert_eq!(f64::try_from(player(&client, "Rate")).unwrap(), 1.0);
    assert!(rx.try_recv().is_err(), "Rate is fixed: no command");

    // The single instance: files and quick actions from a second launch.
    let client_bus = Bus::Connection(client.clone());
    assert!(mpris::forward_files(&client_bus, &["rel/song.flac"]));
    let expected = std::env::current_dir().unwrap().join("rel/song.flac");
    assert_eq!(next_command(&rx), Command::Open(expected.to_string_lossy().into_owned()));
    assert_eq!(next_command(&rx), Command::Raise);
    assert!(mpris::send_command(&client_bus, "PlayPause"));
    assert_eq!(next_command(&rx), Command::PlayPause);

    // Seeked, and the position carried forward while playing.
    while sig.try_recv().is_ok() {}
    service.seeked(42.0);
    service.flush();
    let msg = loop {
        let m = sig.recv_timeout(Duration::from_secs(5)).unwrap();
        if m.header().member().map(|n| n.as_str() == "Seeked").unwrap_or(false) {
            break m;
        }
    };
    assert_eq!(msg.body().deserialize::<i64>().unwrap(), 42_000_000);
    let mut playing = service.state();
    playing.status = PlaybackStatus::Playing;
    playing.position = 10.0;
    service.update(playing);
    std::thread::sleep(Duration::from_millis(120));
    let pos = i64::try_from(player(&client, "Position")).unwrap();
    assert!((10_100_000..11_000_000).contains(&pos), "{pos}");

    // The names zbus derived are the MPRIS ones.
    let xml: String = call(&client, mpris::PATH, "org.freedesktop.DBus.Introspectable", "Introspect", &()).unwrap();
    for name in ["PlayPause", "SetPosition", "OpenUri", "Seeked", "CanGoPrevious", "LoopStatus", "SupportedMimeTypes"] {
        assert!(xml.contains(&format!("name=\"{name}\"")), "{name} not in the introspection");
    }

    // Off the bus again.
    service.set_enabled(false);
    service.flush();
    assert!(!service.is_registered());
    assert!(get(&client, mpris::PATH, mpris::ROOT_INTERFACE, "Identity").is_err());
}

#[test]
fn reservation_answers_release_requests() {
    let (server, client) = pair();
    let res = DeviceReservation::new(Bus::Connection(server));
    let asked = Arc::new(AtomicUsize::new(0));
    {
        let asked = Arc::clone(&asked);
        res.set_on_release_requested(move || {
            asked.fetch_add(1, Ordering::SeqCst);
        });
    }
    // A card number no machine has: nothing real is involved anyway (no bus).
    let card = 77;
    res.acquire(Some(card)).unwrap();
    assert_eq!(res.card(), Some(card));
    let path = reservation::object_path(card);
    let prop = |n: &str| get(&client, &path, reservation::INTERFACE, n).unwrap();
    assert_eq!(i32::try_from(prop("Priority")).unwrap(), 10);
    assert_eq!(string(prop("ApplicationName")), "Sonkore");
    assert_eq!(string(prop("ApplicationDeviceName")), "hw:77");

    let request = |p: i32| call::<_, bool>(&client, &path, reservation::INTERFACE, "RequestRelease", &(p,)).unwrap();
    assert!(!request(5));
    assert!(!request(10), "an equal priority does not take it");
    assert_eq!(asked.load(Ordering::SeqCst), 0);
    assert!(res.held());
    assert!(request(20));
    assert_eq!(asked.load(Ordering::SeqCst), 1);
    assert!(!res.held());
    assert!(!request(20), "nothing left to give");

    // Reacquired, then given back: the object goes away.
    res.acquire(Some(card)).unwrap();
    assert!(res.held());
    res.release();
    assert!(!res.held());
    assert!(get(&client, &path, reservation::INTERFACE, "Priority").is_err());

    // A release request answered from inside the callback by release().
    let res = Arc::new(res);
    {
        let inner = Arc::downgrade(&res);
        res.set_on_release_requested(move || {
            if let Some(r) = inner.upgrade() {
                r.release(); // the player's releaseOutput does this
            }
        });
    }
    res.acquire(Some(card)).unwrap();
    assert!(request(20));
    assert!(!res.held());
}
