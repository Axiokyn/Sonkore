//! Takes a sound card away from the desktop sound server so Sonkore can open
//! its raw hw: device, using the session-bus device
//! reservation protocol (`org.freedesktop.ReserveDevice1.Audio<card>`) that
//! PipeWire's session manager and other sound servers honour: they hold
//! every card at a low priority, release it when asked by a higher one and
//! take it back as soon as the name is free again.
//!
//! Without D-Bus every call is a successful no-op.

use std::fmt;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use zbus::blocking::Connection;
use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::names::WellKnownName;

use crate::bus::{Bus, get_property};

pub const INTERFACE: &str = "org.freedesktop.ReserveDevice1";
/// Sonkore's own name in `ApplicationName`.
pub const APPLICATION_NAME: &str = "Sonkore";

pub fn service_name(card: u32) -> String {
    format!("org.freedesktop.ReserveDevice1.Audio{card}")
}

pub fn object_path(card: u32) -> String {
    format!("/org/freedesktop/ReserveDevice1/Audio{card}")
}

/// The card is held by another program that would not let it go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Busy {
    /// Its `ApplicationName`, or "another application".
    pub holder: String,
}

impl fmt::Display for Busy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the sound card is in use by {}", self.holder)
    }
}

impl std::error::Error for Busy {}

/// ALSA card index of a hw: device (`hw:CARD=DX1,DEV=0`, `hw:0,0`,
/// `plughw:1`), looking a card id up in `/proc/asound`.
pub fn card_index_of(alsa_device: &str) -> Option<u32> {
    card_index_with(alsa_device, |id| std::fs::read_link(Path::new("/proc/asound").join(id)).ok())
}

/// [`card_index_of`] with the `/proc/asound/<id>` link reader given.
pub fn card_index_with(alsa_device: &str, read_link: impl FnOnce(&str) -> Option<std::path::PathBuf>) -> Option<u32> {
    let leading_number = |s: &str| {
        let end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
        s[..end].parse::<u32>().ok()
    };
    let by_index = alsa_device.strip_prefix("plughw:").or_else(|| alsa_device.strip_prefix("hw:"));
    if let Some(n) = by_index.and_then(leading_number) {
        return Some(n);
    }
    let start = alsa_device.find("CARD=")? + 5;
    let rest = &alsa_device[start..];
    let card = &rest[..rest.find(',').unwrap_or(rest.len())];
    if card.is_empty() {
        return None;
    }
    if let Ok(n) = card.parse::<u32>() {
        return Some(n);
    }
    // /proc/asound/<id> links to card<N>.
    let target = read_link(card)?;
    let digits = target.to_str()?.rsplit_once("card")?.1;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

type Callback = Arc<dyn Fn() + Send + Sync>;

thread_local! {
    /// Set while the release callback runs on the D-Bus thread: a release()
    /// from there must not wait on the bus that is waiting for it.
    static IN_RELEASE_REQUEST: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// What the exported object and the owner share.
struct Shared {
    held: Mutex<Option<u32>>,
    on_release: Mutex<Option<Callback>>,
}

/// The app's side: the connection and the object exported.
#[derive(Default)]
struct Local {
    /// None: not tried yet; Some(None): no bus.
    conn: Option<Option<Connection>>,
    exported: Option<u32>,
}

/// One reserved card at a time. Every method takes `&self`; acquiring and
/// releasing block for the D-Bus round trips (call them where the device is
/// opened).
pub struct DeviceReservation {
    bus: Bus,
    local: Mutex<Local>,
    shared: Arc<Shared>,
}

impl DeviceReservation {
    /// Sound servers hold cards at a priority below 0.
    pub const PRIORITY: i32 = 10;

    pub fn new(bus: Bus) -> Self {
        DeviceReservation {
            bus,
            local: Mutex::new(Local::default()),
            shared: Arc::new(Shared { held: Mutex::new(None), on_release: Mutex::new(None) }),
        }
    }

    /// Another program with a higher priority wants the card. Called on the
    /// D-Bus thread; it must close the device before returning (the answer
    /// waits for it), after which the reservation is gone: [`Self::held`]
    /// is already false while it runs, so a [`Self::release`] from it does
    /// nothing. It must not acquire again from inside.
    pub fn set_on_release_requested(&self, f: impl Fn() + Send + Sync + 'static) {
        *lock(&self.shared.on_release) = Some(Arc::new(f));
    }

    pub fn card(&self) -> Option<u32> {
        *lock(&self.shared.held)
    }

    pub fn held(&self) -> bool {
        self.card().is_some()
    }

    /// Reserves `card` (None: just releases), asking the current holder to
    /// let go.
    pub fn acquire(&self, card: Option<u32>) -> Result<(), Busy> {
        let mut local = lock(&self.local);
        if card == self.card() {
            return Ok(());
        }
        self.release_locked(&mut local);
        let Some(card) = card else {
            return Ok(());
        };
        let bus = &self.bus;
        let conn = local.conn.get_or_insert_with(|| bus.connect(Duration::from_secs(3)).ok()).clone();
        let Some(conn) = conn else {
            *lock(&self.shared.held) = Some(card);
            return Ok(());
        };

        let path = object_path(card);
        let _ = conn.object_server().remove::<ReserveObject, _>(path.as_str());
        let object = ReserveObject { card, shared: Arc::clone(&self.shared) };
        if conn.object_server().at(path.as_str(), object).is_ok() {
            local.exported = Some(card);
        }

        match take_name(&conn, card) {
            Ok(()) => {
                *lock(&self.shared.held) = Some(card);
                Ok(())
            }
            Err(busy) => {
                let _ = conn.object_server().remove::<ReserveObject, _>(path.as_str());
                local.exported = None;
                Err(busy)
            }
        }
    }

    /// [`Self::acquire`] as the player does it: held by an earlier Sonkore
    /// that is still closing (it answered the release request but has not
    /// dropped the name yet), it waits for it a little (8 × 250 ms) rather
    /// than telling the user that Sonkore is in its own way.
    pub fn acquire_with_retry(&self, card: Option<u32>) -> Result<(), Busy> {
        let mut result = self.acquire(card);
        for _ in 0..8 {
            match &result {
                Err(busy) if busy.holder == APPLICATION_NAME => {
                    std::thread::sleep(Duration::from_millis(250));
                    result = self.acquire(card);
                }
                _ => break,
            }
        }
        result
    }

    /// Gives the card back (the sound server picks it up again by itself).
    pub fn release(&self) {
        if IN_RELEASE_REQUEST.with(|f| f.get()) {
            return; // already given up; the object goes on the next acquire/release
        }
        let mut local = lock(&self.local);
        self.release_locked(&mut local);
    }

    fn release_locked(&self, local: &mut Local) {
        let card = lock(&self.shared.held).take();
        let conn = local.conn.clone().flatten();
        if let (Some(card), Some(conn)) = (card, &conn)
            && conn.is_bus()
            && let (Ok(dbus), Ok(name)) =
                (zbus::blocking::fdo::DBusProxy::new(conn), WellKnownName::try_from(service_name(card)))
        {
            let _ = dbus.release_name(name);
        }
        // Also an object left behind by a release the other side asked for.
        if let (Some(card), Some(conn)) = (local.exported.take(), &conn) {
            let _ = conn.object_server().remove::<ReserveObject, _>(object_path(card).as_str());
        }
    }
}

impl Drop for DeviceReservation {
    fn drop(&mut self) {
        self.release();
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Requests the card's name, asking its holder to release it when taken.
fn take_name(conn: &Connection, card: u32) -> Result<(), Busy> {
    if !conn.is_bus() {
        return Ok(()); // a peer-to-peer connection has no names
    }
    let name = service_name(card);
    let path = object_path(card);
    let busy =
        |holder: String| Busy { holder: if holder.is_empty() { "another application".to_string() } else { holder } };
    let Ok(dbus) = zbus::blocking::fdo::DBusProxy::new(conn) else {
        return Err(busy(String::new()));
    };
    let Ok(well_known) = WellKnownName::try_from(name.as_str()) else {
        return Err(busy(String::new()));
    };
    let request = |flags| {
        matches!(
            dbus.request_name(well_known.clone(), flags),
            Ok(RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner)
        )
    };
    if request(RequestNameFlags::DoNotQueue | RequestNameFlags::AllowReplacement) {
        return Ok(());
    }
    let holder = get_property(conn, &name, &path, INTERFACE, "ApplicationName")
        .ok()
        .and_then(|v| String::try_from(v).ok())
        .unwrap_or_default();
    let agreed = conn
        .call_method(
            Some(name.as_str()),
            path.as_str(),
            Some(INTERFACE),
            "RequestRelease",
            &(DeviceReservation::PRIORITY,),
        )
        .ok()
        .and_then(|reply| reply.body().deserialize::<bool>().ok())
        .unwrap_or(false);
    if agreed {
        // The holder may drop the name a moment after answering.
        let flags =
            RequestNameFlags::DoNotQueue | RequestNameFlags::ReplaceExisting | RequestNameFlags::AllowReplacement;
        for _ in 0..20 {
            if request(flags) {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
    Err(busy(holder))
}

/// The object other programs talk to while Sonkore holds a card.
struct ReserveObject {
    card: u32,
    shared: Arc<Shared>,
}

#[zbus::interface(name = "org.freedesktop.ReserveDevice1")]
impl ReserveObject {
    #[zbus(name = "RequestRelease")]
    async fn request_release(&self, priority: i32, #[zbus(connection)] conn: &zbus::Connection) -> bool {
        {
            let mut held = lock(&self.shared.held);
            if *held != Some(self.card) || priority <= DeviceReservation::PRIORITY {
                return false;
            }
            *held = None;
        }
        let callback = lock(&self.shared.on_release).clone();
        if let Some(f) = callback {
            IN_RELEASE_REQUEST.with(|flag| flag.set(true));
            f(); // the receiver closes the device
            IN_RELEASE_REQUEST.with(|flag| flag.set(false));
        }
        if conn.is_bus()
            && let (Ok(dbus), Ok(name)) =
                (zbus::fdo::DBusProxy::new(conn).await, WellKnownName::try_from(service_name(self.card)))
        {
            let _ = dbus.release_name(name).await;
        }
        true
    }

    #[zbus(property, name = "Priority")]
    fn priority(&self) -> i32 {
        DeviceReservation::PRIORITY
    }

    #[zbus(property, name = "ApplicationName")]
    fn application_name(&self) -> String {
        APPLICATION_NAME.to_string()
    }

    #[zbus(property, name = "ApplicationDeviceName")]
    fn application_device_name(&self) -> String {
        format!("hw:{}", self.card)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn card_index_from_device_names() {
        let none = |_: &str| None;
        assert_eq!(card_index_with("hw:0,0", none), Some(0));
        assert_eq!(card_index_with("hw:12", none), Some(12));
        assert_eq!(card_index_with("plughw:3,1", none), Some(3));
        assert_eq!(card_index_with("hw:CARD=2,DEV=0", none), Some(2));
        assert_eq!(card_index_with("hw:CARD=DX1,DEV=0", |id| (id == "DX1").then(|| PathBuf::from("card4"))), Some(4));
        assert_eq!(card_index_with("hw:CARD=DX1", |_| Some(PathBuf::from("/x/card"))), None);
        assert_eq!(card_index_with("hw:CARD=DX1,DEV=0", none), None);
        assert_eq!(card_index_with("default", none), None);
        assert_eq!(card_index_with("pipewire", none), None);
    }

    #[test]
    fn names() {
        assert_eq!(service_name(1), "org.freedesktop.ReserveDevice1.Audio1");
        assert_eq!(object_path(1), "/org/freedesktop/ReserveDevice1/Audio1");
    }
}
