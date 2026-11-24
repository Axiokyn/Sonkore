//! Which D-Bus a service talks on.

use std::time::Duration;

use zbus::blocking::Connection;

/// The bus a service connects to. The app uses [`Bus::Session`]; tests and
/// tools can point a service at a private bus or an already open connection
/// (a peer-to-peer one too: names are then skipped).
#[derive(Clone, Debug, Default)]
pub enum Bus {
    #[default]
    Session,
    System,
    /// A D-Bus address, `unix:path=/run/…`.
    Address(String),
    Connection(Connection),
}

impl Bus {
    /// Opens the bus. `timeout` bounds every method call made on a new
    /// connection (an open [`Bus::Connection`] keeps its own).
    pub fn connect(&self, timeout: Duration) -> zbus::Result<Connection> {
        use zbus::blocking::connection::Builder;
        let builder = match self {
            Bus::Session => Builder::session()?,
            Bus::System => Builder::system()?,
            Bus::Address(address) => Builder::address(address.as_str())?,
            Bus::Connection(conn) => return Ok(conn.clone()),
        };
        let conn = builder.method_timeout(timeout).build()?;
        // zbus starts dispatching method calls on a task of its own when the
        // object server is first used, and calls arriving before that task
        // listens are dropped: start it now, well before any name is taken.
        let _ = conn.object_server();
        Ok(conn)
    }
}

/// Calls `org.freedesktop.DBus.Properties.Get` and returns the value.
pub(crate) fn get_property(
    conn: &Connection,
    destination: &str,
    path: &str,
    interface: &str,
    name: &str,
) -> zbus::Result<zbus::zvariant::OwnedValue> {
    let reply =
        conn.call_method(Some(destination), path, Some("org.freedesktop.DBus.Properties"), "Get", &(interface, name))?;
    reply.body().deserialize()
}

/// Whether `name` has an owner on the bus (always true on a peer-to-peer
/// connection: the peer is the only one there).
pub(crate) fn name_has_owner(conn: &Connection, name: &str) -> bool {
    if !conn.is_bus() {
        return true;
    }
    let Ok(name) = zbus::names::BusName::try_from(name) else {
        return false;
    };
    zbus::blocking::fdo::DBusProxy::new(conn).and_then(|dbus| Ok(dbus.name_has_owner(name)?)).unwrap_or(false)
}

/// An integer property value of any D-Bus integer type.
pub(crate) fn value_to_i64(value: &zbus::zvariant::Value<'_>) -> Option<i64> {
    use zbus::zvariant::Value;
    match value {
        Value::U8(v) => Some(i64::from(*v)),
        Value::I16(v) => Some(i64::from(*v)),
        Value::U16(v) => Some(i64::from(*v)),
        Value::I32(v) => Some(i64::from(*v)),
        Value::U32(v) => Some(i64::from(*v)),
        Value::I64(v) => Some(*v),
        Value::U64(v) => i64::try_from(*v).ok(),
        Value::Value(inner) => value_to_i64(inner),
        _ => None,
    }
}
