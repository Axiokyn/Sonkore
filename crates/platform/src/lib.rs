//! Desktop integration.
//!
//! On Linux: MPRIS and the single instance, the tray icon, the ALSA mixer,
//! the device reservation, rtkit and the document portal. Everything on
//! D-Bus goes through zbus. Without a bus (or with the service missing)
//! every call degrades quietly: MPRIS stays off, the
//! reservation is a successful no-op, rtkit grants nothing.
//!
//! On Windows the same modules, with the same interface, do it the Windows
//! way: the single instance on a named pipe and the system's media controls
//! (`mpris`), the notification-area icon (`tray`), the endpoint's own volume
//! (`endpoint_volume`); the reservation and the portal have nothing to do.
//!
//! See docs/engine.md section 6.

pub mod media;
pub mod portal;
// The Windows modules' pure logic, built everywhere for its tests.
pub mod url;
#[path = "win/shared.rs"]
mod win_shared;

#[cfg(target_os = "linux")]
pub mod alsa_mixer;
#[cfg(target_os = "linux")]
pub mod bus;
#[cfg(target_os = "linux")]
pub mod mpris;
#[cfg(target_os = "linux")]
pub mod reservation;
#[cfg(target_os = "linux")]
pub mod rtkit;
#[cfg(target_os = "linux")]
pub mod tray;

#[cfg(windows)]
#[path = "win/bus.rs"]
pub mod bus;
#[cfg(windows)]
#[path = "win/mpris.rs"]
pub mod mpris;
#[cfg(windows)]
#[path = "win/reservation.rs"]
pub mod reservation;
#[cfg(windows)]
#[path = "win/tray.rs"]
pub mod tray;
#[cfg(windows)]
#[path = "win/window.rs"]
mod window;

#[cfg(target_os = "linux")]
pub use alsa_mixer::{AlsaMixer, MixerError};
pub use bus::Bus;
pub use media::{Command, PlaybackStatus, PlayerState, Repeat, TrackInfo, TrayCommand, TrayLabels};
pub use mpris::Mpris;
pub use reservation::{Busy, DeviceReservation};
pub use tray::Tray;

/// zbus, for callers that build their own [`Bus::Connection`].
#[cfg(target_os = "linux")]
pub use zbus;
