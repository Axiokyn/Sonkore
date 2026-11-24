//! Windows has no device reservation (org.freedesktop.ReserveDevice1):
//! WASAPI's exclusive mode is the lock. Every call succeeds and holds
//! nothing, as the Linux one does without a bus.

use std::fmt;

use crate::bus::Bus;

/// Another application holds the card (never on Windows).
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

/// The ALSA card of a device: none on Windows.
pub fn card_index_of(_alsa_device: &str) -> Option<u32> {
    None
}

#[derive(Default)]
pub struct DeviceReservation;

impl DeviceReservation {
    pub fn new(_bus: Bus) -> Self {
        DeviceReservation
    }

    pub fn set_on_release_requested(&self, _f: impl Fn() + Send + Sync + 'static) {}

    pub fn card(&self) -> Option<u32> {
        None
    }

    pub fn held(&self) -> bool {
        false
    }

    pub fn acquire(&self, _card: Option<u32>) -> Result<(), Busy> {
        Ok(())
    }

    pub fn acquire_with_retry(&self, _card: Option<u32>) -> Result<(), Busy> {
        Ok(())
    }

    pub fn release(&self) {}
}
