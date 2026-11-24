//! Windows: the volume control of the output device itself:
//! IAudioEndpointVolume on an endpoint that
//! implements it in hardware, so the volume can change while WASAPI's
//! exclusive mode hands the DAC untouched samples. The same interface as
//! the ALSA mixer the Linux build uses; follows changes made elsewhere (the
//! Windows volume flyout, the DAC's knob when its driver reports it).

use std::sync::{Arc, Mutex};

use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Media::Audio::Endpoints::{
    IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl,
};
use windows::Win32::Media::Audio::{
    AUDIO_VOLUME_NOTIFICATION_DATA, DEVICE_STATE_ACTIVE, ENDPOINT_HARDWARE_SUPPORT_VOLUME, IMMDevice,
    IMMDeviceEnumerator, MMDeviceEnumerator, eConsole, eRender,
};
use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, STGM_READ};
use windows::core::{GUID, implement};

type OnChange = Arc<Mutex<Option<Box<dyn Fn() + Send>>>>;

/// Set by Sonkore itself: its own changes are not reported back as changes
/// made elsewhere.
const OURS: GUID = GUID::from_u128(0x5f0a_3c71_9d2e_4b8a_a1c4_53bb_70e2_d901);

#[implement(IAudioEndpointVolumeCallback)]
struct Watcher {
    on_change: OnChange,
}

impl IAudioEndpointVolumeCallback_Impl for Watcher_Impl {
    fn OnNotify(&self, data: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> windows::core::Result<()> {
        // SAFETY: Windows hands a valid notification for the call's length.
        let ours = !data.is_null() && unsafe { (*data).guidEventContext } == OURS;
        if !ours {
            if let Some(f) = self.on_change.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                f();
            }
        }
        Ok(())
    }
}

/// The endpoint's own volume, while open.
pub struct EndpointVolume {
    volume: IAudioEndpointVolume,
    watcher: IAudioEndpointVolumeCallback,
    on_change: OnChange,
    name: String,
    min_db: f64,
    top_db: f64,
}

impl EndpointVolume {
    /// The endpoint called `name` (its friendly name, as the output's key
    /// "wasapi|<name>" holds it; "" = the default output). Fails when it is
    /// not there or has no volume control of its own: Windows would then
    /// scale the samples in software, which exclusive mode bypasses anyway.
    pub fn open(name: &str) -> Result<EndpointVolume, String> {
        let err = |e: windows::core::Error| e.message();
        // SAFETY: COM calls on interfaces we hold; the property value is read
        // before it is dropped.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let devices: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(err)?;
            let device: IMMDevice = if name.is_empty() {
                devices.GetDefaultAudioEndpoint(eRender, eConsole).map_err(err)?
            } else {
                let all = devices.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE).map_err(err)?;
                let mut found = None;
                for i in 0..all.GetCount().map_err(err)? {
                    let d = all.Item(i).map_err(err)?;
                    let friendly = d
                        .OpenPropertyStore(STGM_READ)
                        .and_then(|p| p.GetValue(&PKEY_Device_FriendlyName))
                        .map(|v| v.to_string());
                    if friendly.is_ok_and(|f| f == name) {
                        found = Some(d);
                        break;
                    }
                }
                found.ok_or_else(|| format!("{name}: not connected"))?
            };
            let volume: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None).map_err(err)?;
            if volume.QueryHardwareSupport().map_err(err)? & ENDPOINT_HARDWARE_SUPPORT_VOLUME == 0 {
                return Err(format!("{name}: no volume control of its own"));
            }
            let (mut min, mut max, mut step) = (0f32, 0f32, 0f32);
            volume.GetVolumeRange(&mut min, &mut max, &mut step).map_err(err)?;
            let on_change: OnChange = Arc::new(Mutex::new(None));
            let watcher: IAudioEndpointVolumeCallback = Watcher { on_change: on_change.clone() }.into();
            volume.RegisterControlChangeNotify(&watcher).map_err(err)?;
            let shown = if name.is_empty() {
                device
                    .OpenPropertyStore(STGM_READ)
                    .and_then(|p| p.GetValue(&PKEY_Device_FriendlyName))
                    .map(|v| v.to_string())
                    .unwrap_or_default()
            } else {
                name.to_owned()
            };
            Ok(EndpointVolume {
                volume,
                watcher,
                on_change,
                name: shown,
                min_db: f64::from(min),
                top_db: f64::from(max).min(0.0),
            })
        }
    }

    /// Runs (on a Windows thread) when the volume changes elsewhere.
    pub fn set_on_change(&self, f: impl Fn() + Send + 'static) {
        *self.on_change.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(f));
    }

    /// The control's name in Settings: the device's.
    pub fn control_name(&self) -> &str {
        &self.name
    }

    pub fn has_db_scale(&self) -> bool {
        self.top_db > self.min_db
    }

    pub fn min_db(&self) -> f64 {
        self.min_db
    }

    pub fn top_db(&self) -> f64 {
        self.top_db
    }

    /// Linear amplitude 0..1, 1 at the top of the control (capped at 0 dB).
    pub fn volume(&self) -> f64 {
        // SAFETY: a COM call on an interface we hold.
        let db = unsafe { self.volume.GetMasterVolumeLevel() }.map_or(self.top_db, f64::from);
        // SAFETY: as above.
        let muted = unsafe { self.volume.GetMute() }.is_ok_and(|m| m.as_bool());
        if muted { 0.0 } else { curve::db_to_linear(db, self.min_db, self.top_db) }
    }

    /// Sets it (0 mutes, leaving the level); Sonkore's own change is not
    /// reported back.
    pub fn set_volume(&self, linear: f64) {
        if !linear.is_finite() {
            return;
        }
        let linear = linear.clamp(0.0, 1.0);
        // SAFETY: COM calls on an interface we hold; OURS outlives them.
        unsafe {
            if linear <= 0.0 {
                let _ = self.volume.SetMute(true, &OURS);
                return;
            }
            let _ =
                self.volume.SetMasterVolumeLevel(curve::linear_to_db(linear, self.min_db, self.top_db) as f32, &OURS);
            if self.volume.GetMute().is_ok_and(|m| m.as_bool()) {
                let _ = self.volume.SetMute(false, &OURS);
            }
        }
    }
}

impl Drop for EndpointVolume {
    fn drop(&mut self) {
        // SAFETY: unregisters the callback registered in open.
        let _ = unsafe { self.volume.UnregisterControlChangeNotify(&self.watcher) };
    }
}

/// Amplitude to dB on the control's own range.
mod curve {
    pub fn db_to_linear(db: f64, min_db: f64, top_db: f64) -> f64 {
        if db <= min_db { 0.0 } else { 10f64.powf((db - top_db) / 20.0).clamp(0.0, 1.0) }
    }

    pub fn linear_to_db(linear: f64, min_db: f64, top_db: f64) -> f64 {
        if linear <= 0.0 { min_db } else { (top_db + 20.0 * linear.log10()).max(min_db) }
    }
}
