//! MPRIS 2 (`org.mpris.MediaPlayer2`) on the session bus:
//! GNOME's media widget and lock screen, KDE's task-manager tooltips,
//! Dash-to-Panel/Dock, media keys and `playerctl` all control Sonkore
//! through it. Also the single instance: a second launch hands its files
//! (or a quick action) to the running one.
//!
//! The app owns an [`Mpris`] handle: it pushes the player's state with
//! [`Mpris::update`] (a plain struct, compared with the last one to send the
//! desktop only what changed) and receives what the desktop asks for as
//! [`Command`]s through a callback. Nothing on the D-Bus side runs on the
//! caller's thread: a worker thread owns the connection, and zbus answers
//! the desktop on its own executor thread.

use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use zbus::blocking::Connection;
use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::names::WellKnownName;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

use crate::bus::{Bus, name_has_owner};
use crate::portal;
use crate::url::{absolute, file_url, is_local_file};

pub const SERVICE: &str = "org.mpris.MediaPlayer2.sonkore";
pub const PATH: &str = "/org/mpris/MediaPlayer2";
pub const ROOT_INTERFACE: &str = "org.mpris.MediaPlayer2";
pub const PLAYER_INTERFACE: &str = "org.mpris.MediaPlayer2.Player";
pub const IDENTITY: &str = "Sonkore";
pub const DESKTOP_ENTRY: &str = "com.axiokyn.Sonkore";
pub const URI_SCHEMES: [&str; 1] = ["file"];
pub const MIME_TYPES: [&str; 5] = ["audio/flac", "audio/x-flac", "audio/mpeg", "audio/wav", "audio/x-wav"];
/// `mpris:trackid` with nothing loaded.
pub const NO_TRACK: &str = "/org/mpris/MediaPlayer2/TrackList/NoTrack";

/// The name taken when [`SERVICE`] belongs to another process.
pub fn instance_name(pid: u32) -> String {
    format!("{SERVICE}.instance{pid}")
}

/// `mpris:trackid` of the n-th track loaded.
pub fn track_id_path(n: u64) -> String {
    format!("/org/sonkore/track/{n}")
}

pub use crate::media::{
    Command, PlaybackStatus, PlayerState, Repeat, TrackInfo, loop_status, player_command, repeat_from_loop_status,
    seek_command, set_position_command,
};

/// The `Metadata` map for a track (`track_id` numbers `mpris:trackid`).
pub fn build_metadata(track: Option<&TrackInfo>, track_id: u64) -> HashMap<String, OwnedValue> {
    let mut map: HashMap<String, Value<'static>> = HashMap::new();
    let path = |p: String| Value::from(ObjectPath::try_from(p).expect("valid object path"));
    let Some(t) = track else {
        map.insert("mpris:trackid".into(), path(NO_TRACK.to_string()));
        return owned(map);
    };
    map.insert("mpris:trackid".into(), path(track_id_path(track_id)));
    map.insert("mpris:length".into(), Value::from((t.duration * 1e6) as i64));
    map.insert("xesam:title".into(), Value::from(t.title.clone()));
    map.insert("xesam:album".into(), Value::from(t.album.clone()));
    map.insert("xesam:artist".into(), Value::from(vec![t.artist.clone()]));
    if !t.album_artist.is_empty() {
        map.insert("xesam:albumArtist".into(), Value::from(vec![t.album_artist.clone()]));
    }
    if t.track_no > 0 {
        map.insert("xesam:trackNumber".into(), Value::from(t.track_no as i32));
    }
    if !t.genre.is_empty() {
        map.insert("xesam:genre".into(), Value::from(vec![t.genre.clone()]));
    }
    map.insert("xesam:url".into(), Value::from(file_url(&t.path)));
    if let Some(art) = t.art_url.as_ref().filter(|a| !a.is_empty()) {
        map.insert("mpris:artUrl".into(), Value::from(art.clone()));
    }
    owned(map)
}

fn owned(map: HashMap<String, Value<'static>>) -> HashMap<String, OwnedValue> {
    map.into_iter().map(|(k, v)| (k, OwnedValue::try_from(v).expect("no file descriptors"))).collect()
}

/// `mpris:artUrl` for an embedded cover: written once to
/// `<cache_dir>/mpris/<sha1 prefix>.png|jpg` (the names earlier versions
/// used) so the desktop can read it.
pub fn cached_cover_url(image: &[u8], cache_dir: &Path) -> Option<String> {
    if image.is_empty() {
        return None;
    }
    let dir = cache_dir.join("mpris");
    std::fs::create_dir_all(&dir).ok()?;
    let ext = if image.starts_with(b"\x89PNG") { "png" } else { "jpg" };
    let hash = sha1_smol::Sha1::from(image).digest().to_string();
    let file = dir.join(format!("{}.{ext}", &hash[..16]));
    if !file.exists() {
        std::fs::write(&file, image).ok()?;
    }
    Some(file_url(&file))
}

/// Sends files to an already running instance (OpenUri each, then Raise);
/// true if one answered.
pub fn forward_files<P: AsRef<Path>>(bus: &Bus, paths: &[P]) -> bool {
    let Some(conn) = running_instance(bus) else {
        return false;
    };
    for p in paths {
        let uri = file_url(&absolute(p.as_ref()));
        let _ = conn.call_method(Some(SERVICE), PATH, Some(PLAYER_INTERFACE), "OpenUri", &(uri,));
    }
    let _ = conn.call_method(Some(SERVICE), PATH, Some(ROOT_INTERFACE), "Raise", &());
    true
}

/// Calls a Player method (PlayPause, Next…) on a running instance; true if
/// one answered.
pub fn send_command(bus: &Bus, method: &str) -> bool {
    let Some(conn) = running_instance(bus) else {
        return false;
    };
    conn.call_method(Some(SERVICE), PATH, Some(PLAYER_INTERFACE), method, &()).is_ok()
}

fn running_instance(bus: &Bus) -> Option<Connection> {
    let conn = bus.connect(Duration::from_secs(5)).ok()?;
    name_has_owner(&conn, SERVICE).then_some(conn)
}

// --- The service -------------------------------------------------------------

type CommandFn = Box<dyn Fn(Command) + Send + Sync>;

struct Shared {
    state: PlayerState,
    /// When `state.position` was true.
    stamp: Instant,
    track_id: u64,
}

impl Shared {
    fn position(&self) -> f64 {
        let s = &self.state;
        let mut pos = s.position;
        if s.status == PlaybackStatus::Playing {
            pos += self.stamp.elapsed().as_secs_f64();
            if s.duration > 0.0 {
                pos = pos.min(s.duration);
            }
        }
        pos.max(0.0)
    }

    fn metadata(&self) -> HashMap<String, OwnedValue> {
        build_metadata(self.state.track.as_ref(), self.track_id)
    }

    fn current_track_id(&self) -> String {
        if self.state.track.is_some() { track_id_path(self.track_id) } else { NO_TRACK.to_string() }
    }
}

struct Ctx {
    shared: Mutex<Shared>,
    on_command: CommandFn,
}

impl Ctx {
    fn shared(&self) -> MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn send(&self, command: Command) {
        (self.on_command)(command);
    }
}

/// The Player properties that changed from `old` to `new` (Position is
/// never announced: clients read it, and follow Seeked).
pub fn changed_properties(old: &PlayerState, new: &PlayerState) -> Vec<&'static str> {
    let mut out = Vec::new();
    let mut add = |name: &'static str| {
        if !out.contains(&name) {
            out.push(name);
        }
    };
    if old.status != new.status {
        add("PlaybackStatus");
    }
    if old.repeat != new.repeat {
        add("LoopStatus");
    }
    if old.shuffle != new.shuffle {
        add("Shuffle");
    }
    if old.volume != new.volume {
        add("Volume");
    }
    if old.track != new.track {
        for p in ["Metadata", "CanGoNext", "CanGoPrevious", "CanPlay", "CanSeek"] {
            add(p);
        }
    }
    if old.has_queue != new.has_queue {
        for p in ["CanGoNext", "CanGoPrevious", "CanPlay"] {
            add(p);
        }
    }
    if (old.duration > 0.0) != (new.duration > 0.0) {
        add("CanSeek");
    }
    out
}

fn property_value(name: &str, s: &Shared) -> Option<Value<'static>> {
    let st = &s.state;
    Some(match name {
        "PlaybackStatus" => Value::from(st.status.as_str()),
        "LoopStatus" => Value::from(loop_status(st.repeat)),
        "Shuffle" => Value::from(st.shuffle),
        "Volume" => Value::from(st.volume),
        "Metadata" => Value::from(s.metadata()),
        "CanGoNext" | "CanGoPrevious" | "CanPlay" => Value::from(st.has_queue),
        "CanSeek" => Value::from(st.duration > 0.0),
        _ => return None,
    })
}

enum Msg {
    Enable(bool),
    Notify(Vec<&'static str>),
    Seeked(i64),
    Flush(mpsc::Sender<()>),
    Stop,
}

/// The MPRIS service. Created off the bus; [`Mpris::set_enabled`] puts it
/// on (Settings › System: off, the desktop no longer sees the player and a
/// second launch cannot hand files over).
pub struct Mpris {
    ctx: Arc<Ctx>,
    tx: mpsc::Sender<Msg>,
    status: Arc<Mutex<Option<String>>>,
    worker: Option<JoinHandle<()>>,
    /// What set_enabled last asked for (on the bus or not, whether one is there).
    wanted: std::sync::atomic::AtomicBool,
}

impl Mpris {
    pub fn new(bus: Bus, on_command: impl Fn(Command) + Send + Sync + 'static) -> Mpris {
        let ctx = Arc::new(Ctx {
            shared: Mutex::new(Shared { state: PlayerState::default(), stamp: Instant::now(), track_id: 0 }),
            on_command: Box::new(on_command),
        });
        let (tx, rx) = mpsc::channel();
        let status = Arc::new(Mutex::new(None));
        let worker = {
            let ctx = Arc::clone(&ctx);
            let status = Arc::clone(&status);
            std::thread::Builder::new()
                .name("sonkore-mpris".into())
                .spawn(move || Worker { bus, ctx, status, conn: None }.run(rx))
                .ok()
        };
        Mpris { ctx, tx, status, worker, wanted: std::sync::atomic::AtomicBool::new(false) }
    }

    /// On the session bus or not. Returns at once; [`Mpris::flush`] waits.
    pub fn set_enabled(&self, on: bool) {
        self.wanted.store(on, std::sync::atomic::Ordering::Relaxed);
        let _ = self.tx.send(Msg::Enable(on));
    }

    /// What [`Mpris::set_enabled`] last asked for.
    pub fn enabled(&self) -> bool {
        self.wanted.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Waits until everything sent before has reached the bus.
    pub fn flush(&self) {
        let (tx, rx) = mpsc::channel();
        if self.tx.send(Msg::Flush(tx)).is_ok() {
            let _ = rx.recv();
        }
    }

    /// The bus name held (`org.mpris.MediaPlayer2.sonkore` or
    /// `….instance<pid>`; empty on a peer-to-peer connection), None when off
    /// the bus.
    pub fn service_name(&self) -> Option<String> {
        self.status.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn is_registered(&self) -> bool {
        self.service_name().is_some()
    }

    /// The player's state now. A new track (any change in `track`) gets a
    /// new `mpris:trackid`; the desktop hears what changed.
    pub fn update(&self, state: PlayerState) {
        let changed = {
            let mut s = self.ctx.shared();
            let changed = changed_properties(&s.state, &state);
            if s.state.track != state.track && state.track.is_some() {
                s.track_id += 1;
            }
            s.state = state;
            s.stamp = Instant::now();
            changed
        };
        if !changed.is_empty() {
            let _ = self.tx.send(Msg::Notify(changed));
        }
    }

    /// The player jumped to `position` (seconds): sends Seeked.
    pub fn seeked(&self, position: f64) {
        {
            let mut s = self.ctx.shared();
            s.state.position = position;
            s.stamp = Instant::now();
        }
        let _ = self.tx.send(Msg::Seeked((position * 1e6) as i64));
    }

    pub fn state(&self) -> PlayerState {
        self.ctx.shared().state.clone()
    }

    /// The `Metadata` map as the desktop reads it now.
    pub fn metadata(&self) -> HashMap<String, OwnedValue> {
        self.ctx.shared().metadata()
    }

    /// `mpris:trackid` now.
    pub fn track_id(&self) -> String {
        self.ctx.shared().current_track_id()
    }
}

impl Drop for Mpris {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::Stop);
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}

struct Worker {
    bus: Bus,
    ctx: Arc<Ctx>,
    status: Arc<Mutex<Option<String>>>,
    conn: Option<Connection>,
}

impl Worker {
    fn run(mut self, rx: mpsc::Receiver<Msg>) {
        while let Ok(msg) = rx.recv() {
            match msg {
                Msg::Enable(true) => self.register(),
                Msg::Enable(false) => self.unregister(),
                Msg::Notify(names) => self.notify(&names),
                Msg::Seeked(us) => self.emit_seeked(us),
                Msg::Flush(done) => {
                    let _ = done.send(());
                }
                Msg::Stop => break,
            }
        }
        self.unregister();
    }

    fn registered(&self) -> Option<&Connection> {
        let on = self.status.lock().unwrap_or_else(|e| e.into_inner()).is_some();
        if on { self.conn.as_ref() } else { None }
    }

    fn set_status(&self, name: Option<String>) {
        *self.status.lock().unwrap_or_else(|e| e.into_inner()) = name;
    }

    fn register(&mut self) {
        if self.registered().is_some() {
            return;
        }
        if self.conn.is_none() {
            self.conn = self.bus.connect(Duration::from_secs(5)).ok();
            // Calls arriving while off then get an error instead of no answer.
            if let Some(conn) = &self.conn {
                let _ = conn.object_server();
            }
        }
        let Some(conn) = self.conn.as_ref() else {
            return;
        };
        let server = conn.object_server();
        let root = server.at(PATH, Root { ctx: Arc::clone(&self.ctx) });
        let player = server.at(PATH, Player { ctx: Arc::clone(&self.ctx) });
        if root.is_err() || player.is_err() {
            self.remove_objects();
            return;
        }
        drop(server);
        let name = if conn.is_bus() {
            let Ok(dbus) = zbus::blocking::fdo::DBusProxy::new(conn) else {
                self.remove_objects();
                return;
            };
            let take = |name: &str| {
                WellKnownName::try_from(name).is_ok_and(|n| {
                    matches!(
                        dbus.request_name(n, RequestNameFlags::DoNotQueue.into()),
                        Ok(RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner)
                    )
                })
            };
            let instance = instance_name(std::process::id());
            if take(SERVICE) {
                SERVICE.to_string()
            } else if take(&instance) {
                instance
            } else {
                self.remove_objects();
                return;
            }
        } else {
            String::new()
        };
        self.set_status(Some(name));
    }

    fn remove_objects(&self) {
        if let Some(conn) = self.conn.as_ref() {
            let server = conn.object_server();
            let _ = server.remove::<Root, _>(PATH);
            let _ = server.remove::<Player, _>(PATH);
        }
    }

    fn unregister(&mut self) {
        let name = self.status.lock().unwrap_or_else(|e| e.into_inner()).take();
        let Some(name) = name else {
            return;
        };
        if let Some(conn) = self.conn.as_ref()
            && conn.is_bus()
            && let (Ok(dbus), Ok(n)) = (zbus::blocking::fdo::DBusProxy::new(conn), WellKnownName::try_from(name))
        {
            let _ = dbus.release_name(n);
        }
        self.remove_objects();
    }

    fn notify(&self, names: &[&'static str]) {
        let Some(conn) = self.registered() else {
            return;
        };
        let changed: HashMap<&str, Value<'static>> = {
            let s = self.ctx.shared();
            names.iter().filter_map(|&n| property_value(n, &s).map(|v| (n, v))).collect()
        };
        if changed.is_empty() {
            return;
        }
        let _ = conn.emit_signal(
            None::<zbus::names::BusName<'_>>,
            PATH,
            "org.freedesktop.DBus.Properties",
            "PropertiesChanged",
            &(PLAYER_INTERFACE, changed, Vec::<&str>::new()),
        );
    }

    fn emit_seeked(&self, us: i64) {
        if let Some(conn) = self.registered() {
            let _ = conn.emit_signal(None::<zbus::names::BusName<'_>>, PATH, PLAYER_INTERFACE, "Seeked", &(us,));
        }
    }
}

// --- org.mpris.MediaPlayer2 ---------------------------------------------------

struct Root {
    ctx: Arc<Ctx>,
}

#[zbus::interface(name = "org.mpris.MediaPlayer2")]
impl Root {
    #[zbus(name = "Raise")]
    fn raise(&self) {
        self.ctx.send(Command::Raise);
    }

    #[zbus(name = "Quit")]
    fn quit(&self) {
        self.ctx.send(Command::Quit);
    }

    #[zbus(property, name = "CanQuit")]
    fn can_quit(&self) -> bool {
        true
    }

    #[zbus(property, name = "CanRaise")]
    fn can_raise(&self) -> bool {
        true
    }

    #[zbus(property, name = "CanSetFullscreen")]
    fn can_set_fullscreen(&self) -> bool {
        false
    }

    #[zbus(property, name = "Fullscreen")]
    fn fullscreen(&self) -> bool {
        false
    }

    #[zbus(property, name = "HasTrackList")]
    fn has_track_list(&self) -> bool {
        false
    }

    #[zbus(property, name = "Identity")]
    fn identity(&self) -> &str {
        IDENTITY
    }

    #[zbus(property, name = "DesktopEntry")]
    fn desktop_entry(&self) -> &str {
        DESKTOP_ENTRY
    }

    #[zbus(property, name = "SupportedUriSchemes")]
    fn supported_uri_schemes(&self) -> Vec<&str> {
        URI_SCHEMES.to_vec()
    }

    #[zbus(property, name = "SupportedMimeTypes")]
    fn supported_mime_types(&self) -> Vec<&str> {
        MIME_TYPES.to_vec()
    }
}

// --- org.mpris.MediaPlayer2.Player --------------------------------------------

struct Player {
    ctx: Arc<Ctx>,
}

#[zbus::interface(name = "org.mpris.MediaPlayer2.Player")]
impl Player {
    #[zbus(name = "Next")]
    fn next(&self) {
        self.ctx.send(Command::Next);
    }

    #[zbus(name = "Previous")]
    fn previous(&self) {
        self.ctx.send(Command::Previous);
    }

    #[zbus(name = "Pause")]
    fn pause(&self) {
        self.ctx.send(Command::Pause);
    }

    #[zbus(name = "PlayPause")]
    fn play_pause(&self) {
        self.ctx.send(Command::PlayPause);
    }

    #[zbus(name = "Stop")]
    fn stop(&self) {
        self.ctx.send(Command::Stop);
    }

    #[zbus(name = "Play")]
    fn play(&self) {
        self.ctx.send(Command::Play);
    }

    #[zbus(name = "Seek")]
    fn seek(&self, offset: i64) {
        let command = {
            let s = self.ctx.shared();
            seek_command(s.position(), s.state.duration, offset)
        };
        self.ctx.send(command);
    }

    #[zbus(name = "SetPosition")]
    fn set_position(&self, track_id: ObjectPath<'_>, position: i64) {
        let command = {
            let s = self.ctx.shared();
            if track_id.as_str() != s.current_track_id() {
                return; // stale request for a previous track
            }
            set_position_command(position as f64 / 1e6, s.state.duration)
        };
        if let Some(command) = command {
            self.ctx.send(command);
        }
    }

    #[zbus(name = "OpenUri")]
    fn open_uri(&self, uri: &str) {
        let path = if is_local_file(uri) { portal::local_path(uri) } else { uri.to_string() };
        self.ctx.send(Command::Open(path));
    }

    #[zbus(signal, name = "Seeked")]
    async fn seeked(emitter: &SignalEmitter<'_>, position: i64) -> zbus::Result<()>;

    #[zbus(property, name = "PlaybackStatus")]
    fn playback_status(&self) -> &str {
        self.ctx.shared().state.status.as_str()
    }

    // The writable properties take the value at once, so the answer (and
    // the change zbus announces) shows it; the app's next update agrees.
    #[zbus(property, name = "LoopStatus")]
    fn loop_status(&self) -> &str {
        loop_status(self.ctx.shared().state.repeat)
    }

    #[zbus(property, name = "LoopStatus")]
    fn set_loop_status(&self, status: String) {
        let repeat = repeat_from_loop_status(&status);
        self.ctx.shared().state.repeat = repeat;
        self.ctx.send(Command::SetRepeat(repeat));
    }

    #[zbus(property, name = "Rate")]
    fn rate(&self) -> f64 {
        1.0
    }

    #[zbus(property, name = "Rate")]
    fn set_rate(&self, _rate: f64) {}

    #[zbus(property, name = "Shuffle")]
    fn shuffle(&self) -> bool {
        self.ctx.shared().state.shuffle
    }

    #[zbus(property, name = "Shuffle")]
    fn set_shuffle(&self, on: bool) {
        self.ctx.shared().state.shuffle = on;
        self.ctx.send(Command::SetShuffle(on));
    }

    #[zbus(property, name = "Metadata")]
    fn metadata(&self) -> HashMap<String, OwnedValue> {
        self.ctx.shared().metadata()
    }

    #[zbus(property, name = "Volume")]
    fn volume(&self) -> f64 {
        self.ctx.shared().state.volume
    }

    #[zbus(property, name = "Volume")]
    fn set_volume(&self, volume: f64) {
        let volume = if volume.is_nan() { 0.0 } else { volume.clamp(0.0, 1.0) };
        self.ctx.shared().state.volume = volume;
        self.ctx.send(Command::SetVolume(volume));
    }

    #[zbus(property(emits_changed_signal = "false"), name = "Position")]
    fn position(&self) -> i64 {
        (self.ctx.shared().position() * 1e6) as i64
    }

    #[zbus(property, name = "MinimumRate")]
    fn minimum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property, name = "MaximumRate")]
    fn maximum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property, name = "CanGoNext")]
    fn can_go_next(&self) -> bool {
        self.ctx.shared().state.has_queue
    }

    #[zbus(property, name = "CanGoPrevious")]
    fn can_go_previous(&self) -> bool {
        self.ctx.shared().state.has_queue
    }

    #[zbus(property, name = "CanPlay")]
    fn can_play(&self) -> bool {
        self.ctx.shared().state.has_queue
    }

    #[zbus(property, name = "CanPause")]
    fn can_pause(&self) -> bool {
        true
    }

    #[zbus(property, name = "CanSeek")]
    fn can_seek(&self) -> bool {
        self.ctx.shared().state.duration > 0.0
    }

    #[zbus(property, name = "CanControl")]
    fn can_control(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn track() -> TrackInfo {
        TrackInfo {
            path: PathBuf::from("/music/A B/01 x.flac"),
            title: "Title".into(),
            album: "Album".into(),
            artist: "Artist".into(),
            album_artist: String::new(),
            track_no: 3,
            genre: "Jazz".into(),
            duration: 61.5,
            art_url: Some("file:///c/a.jpg".into()),
        }
    }

    #[test]
    fn loop_status_maps_both_ways() {
        for r in [Repeat::Off, Repeat::All, Repeat::One] {
            assert_eq!(repeat_from_loop_status(loop_status(r)), r);
        }
        assert_eq!(loop_status(Repeat::All), "Playlist");
        assert_eq!(loop_status(Repeat::One), "Track");
        assert_eq!(loop_status(Repeat::Off), "None");
        assert_eq!(repeat_from_loop_status("bogus"), Repeat::Off);
    }

    #[test]
    fn status_strings() {
        assert_eq!(PlaybackStatus::from_player(true, true).as_str(), "Playing");
        assert_eq!(PlaybackStatus::from_player(false, true).as_str(), "Paused");
        assert_eq!(PlaybackStatus::from_player(false, false).as_str(), "Stopped");
    }

    #[test]
    fn metadata_of_a_track() {
        let m = build_metadata(Some(&track()), 7);
        let get = |k: &str| m.get(k).map(|v| Value::from(v.try_clone().unwrap()));
        assert_eq!(get("mpris:trackid"), Some(Value::from(ObjectPath::try_from("/org/sonkore/track/7").unwrap())));
        assert_eq!(get("mpris:length"), Some(Value::from(61_500_000i64)));
        assert_eq!(get("xesam:title"), Some(Value::from("Title")));
        assert_eq!(get("xesam:album"), Some(Value::from("Album")));
        assert_eq!(get("xesam:artist"), Some(Value::from(vec!["Artist".to_string()])));
        assert_eq!(get("xesam:albumArtist"), None);
        assert_eq!(get("xesam:trackNumber"), Some(Value::from(3i32)));
        assert_eq!(get("xesam:genre"), Some(Value::from(vec!["Jazz".to_string()])));
        assert_eq!(get("xesam:url"), Some(Value::from("file:///music/A%20B/01%20x.flac")));
        assert_eq!(get("mpris:artUrl"), Some(Value::from("file:///c/a.jpg")));
        assert_eq!(m.len(), 9);

        let mut bare = TrackInfo { path: "/a.mp3".into(), ..Default::default() };
        bare.album_artist = "VA".into();
        let m = build_metadata(Some(&bare), 1);
        assert!(
            !m.contains_key("xesam:trackNumber") && !m.contains_key("xesam:genre") && !m.contains_key("mpris:artUrl")
        );
        assert!(m.contains_key("xesam:albumArtist"));

        let none = build_metadata(None, 9);
        assert_eq!(none.len(), 1);
        assert_eq!(
            Value::from(none["mpris:trackid"].try_clone().unwrap()),
            Value::from(ObjectPath::try_from(NO_TRACK).unwrap())
        );
    }

    #[test]
    fn seek_past_the_end_goes_next() {
        assert_eq!(seek_command(10.0, 60.0, 5_000_000), Command::SeekTo(15.0));
        assert_eq!(seek_command(10.0, 60.0, -20_000_000), Command::SeekTo(0.0));
        assert_eq!(seek_command(58.0, 60.0, 2_000_000), Command::Next);
        // Length unknown (0): a seek, never the next track.
        assert_eq!(seek_command(10.0, 0.0, 5_000_000), Command::SeekTo(15.0));
        assert_eq!(set_position_command(30.0, 60.0), Some(Command::SeekTo(30.0)));
        assert_eq!(set_position_command(-1.0, 60.0), None);
        assert_eq!(set_position_command(61.0, 60.0), None);
        assert_eq!(set_position_command(30.0, 0.0), Some(Command::SeekTo(30.0)));
    }

    #[test]
    fn changes_announced() {
        let a = PlayerState::default();
        let mut b = a.clone();
        b.position = 12.0;
        assert!(changed_properties(&a, &b).is_empty());
        b.status = PlaybackStatus::Playing;
        b.volume = 0.5;
        assert_eq!(changed_properties(&a, &b), ["PlaybackStatus", "Volume"]);
        let mut c = a.clone();
        c.track = Some(track());
        c.has_queue = true;
        assert_eq!(changed_properties(&a, &c), ["Metadata", "CanGoNext", "CanGoPrevious", "CanPlay", "CanSeek"]);
        let mut d = a.clone();
        d.duration = 3.0;
        assert_eq!(changed_properties(&a, &d), ["CanSeek"]);
    }

    #[test]
    fn quick_actions() {
        assert_eq!(player_command(&["--play-pause"]), Some("PlayPause"));
        assert_eq!(player_command(&["x.flac", "--next"]), Some("Next"));
        assert_eq!(player_command(&["--previous"]), Some("Previous"));
        assert_eq!(player_command(&["x.flac"]), None);
    }

    #[test]
    fn track_ids_follow_new_tracks() {
        let m = Mpris::new(Bus::Address("unix:path=/nonexistent/sonkore-test".into()), |_| {});
        assert_eq!(m.track_id(), NO_TRACK);
        let mut s = PlayerState { track: Some(track()), ..Default::default() };
        m.update(s.clone());
        assert_eq!(m.track_id(), "/org/sonkore/track/1");
        s.position = 3.0;
        m.update(s.clone());
        assert_eq!(m.track_id(), "/org/sonkore/track/1");
        s.track.as_mut().unwrap().title = "Other".into();
        m.update(s);
        assert_eq!(m.track_id(), "/org/sonkore/track/2");
        // No bus there: enabling fails quietly.
        m.set_enabled(true);
        m.flush();
        assert!(!m.is_registered());
    }

    #[test]
    fn cover_cache_names() {
        let dir = std::env::temp_dir().join(format!("sonkore-platform-test-{}", std::process::id()));
        let png = b"\x89PNG\r\n\x1a\nrest";
        let url = cached_cover_url(png, &dir).unwrap();
        let hash = &sha1_smol::Sha1::from(&png[..]).digest().to_string()[..16];
        assert!(url.ends_with(&format!("/mpris/{hash}.png")), "{url}");
        assert!(cached_cover_url(b"\xff\xd8jpeg", &dir).unwrap().ends_with(".jpg"));
        assert_eq!(cached_cover_url(b"", &dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
