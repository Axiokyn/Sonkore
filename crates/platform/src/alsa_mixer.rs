//! The playback volume control of an ALSA card: the DAC's
//! own attenuator, so the volume can change while the samples reach the DAC
//! untouched. Picks the card's main playback control and follows changes
//! made elsewhere (alsamixer, another program, a knob the driver reports).

use std::ffi::{CStr, CString};
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;

/// Usual names of a card's main playback control, best first. USB DACs
/// often expose a single control named after the device: any playback
/// volume control is taken when none of these exists.
pub const PREFERRED: [&str; 7] = ["PCM", "Master", "Digital", "Headphone", "Speaker", "Front", "DAC"];

/// The control to use among `(name, usable)` in the mixer's order: the
/// first usable one of [`PREFERRED`], else the first usable one.
pub fn pick_control<'a>(controls: impl IntoIterator<Item = (&'a str, bool)> + Clone) -> Option<usize> {
    for wanted in PREFERRED {
        if let Some(i) = controls.clone().into_iter().position(|(name, usable)| usable && name == wanted) {
            return Some(i);
        }
    }
    controls.into_iter().position(|(_, usable)| usable)
}

/// The top of a control: its maximum, capped at 0 dB (a control that can
/// amplify is never pushed above unity).
pub fn top_db(max_db: f64) -> f64 {
    max_db.min(0.0)
}

/// Linear amplitude of a control set to `db`, 1 at the top.
pub fn db_to_linear(db: f64, top_db: f64) -> f64 {
    10f64.powf((db - top_db) / 20.0).min(1.0)
}

/// The dB to set for a linear amplitude in (0, 1], never below the
/// control's minimum.
pub fn linear_to_db(linear: f64, min_db: f64, top_db: f64) -> f64 {
    (top_db + 20.0 * linear.log10()).max(min_db)
}

/// Linear position of a raw value on a control without a dB scale.
pub fn raw_to_linear(raw: i64, min: i64, max: i64) -> f64 {
    if max > min { (raw - min) as f64 / (max - min) as f64 } else { 1.0 }
}

pub fn linear_to_raw(linear: f64, min: i64, max: i64) -> i64 {
    min + (linear * (max - min) as f64).round() as i64
}

#[derive(Debug)]
pub enum MixerError {
    /// The card could not be opened or loaded (an ALSA error code).
    Alsa(i32),
    /// The card has no usable playback volume control.
    NoControl,
}

impl fmt::Display for MixerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MixerError::Alsa(code) => {
                // SAFETY: snd_strerror returns a static string.
                let text = unsafe { CStr::from_ptr(alsa_sys::snd_strerror(*code)) };
                write!(f, "ALSA mixer: {}", text.to_string_lossy())
            }
            MixerError::NoControl => f.write_str("the card has no playback volume control"),
        }
    }
}

impl std::error::Error for MixerError {}

type Callback = Box<dyn Fn() + Send>;

const CHANNEL: alsa_sys::snd_mixer_selem_channel_id_t = alsa_sys::SND_MIXER_SCHN_FRONT_LEFT;

/// The open mixer and the id of the control picked (looked up on every use:
/// a card going away removes its elements).
struct Handle {
    mixer: *mut alsa_sys::snd_mixer_t,
    id: *mut alsa_sys::snd_mixer_selem_id_t,
}

// SAFETY: alsa-lib mixer handles may move between threads; every use goes
// through the mutex around the Handle.
unsafe impl Send for Handle {}

impl Handle {
    fn elem(&self) -> Option<*mut alsa_sys::snd_mixer_elem_t> {
        // SAFETY: both pointers are live while the Handle is.
        let e = unsafe { alsa_sys::snd_mixer_find_selem(self.mixer, self.id) };
        (!e.is_null()).then_some(e)
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: allocated in `open_handle`, freed once.
        unsafe {
            if !self.id.is_null() {
                alsa_sys::snd_mixer_selem_id_free(self.id);
            }
            alsa_sys::snd_mixer_close(self.mixer);
        }
    }
}

fn check(code: libc::c_int) -> Result<(), MixerError> {
    if code < 0 { Err(MixerError::Alsa(code)) } else { Ok(()) }
}

/// Opens, attaches and loads the mixer of `card`.
fn open_handle(card: &CStr) -> Result<Handle, MixerError> {
    let mut mixer = std::ptr::null_mut();
    // SAFETY: valid out pointer.
    check(unsafe { alsa_sys::snd_mixer_open(&mut mixer, 0) })?;
    let handle = Handle { mixer, id: std::ptr::null_mut() };
    // SAFETY: live mixer, valid C string.
    unsafe {
        check(alsa_sys::snd_mixer_attach(mixer, card.as_ptr()))?;
        check(alsa_sys::snd_mixer_selem_register(mixer, std::ptr::null_mut(), std::ptr::null_mut()))?;
        check(alsa_sys::snd_mixer_load(mixer))?;
    }
    Ok(handle)
}

fn elem_name(e: *mut alsa_sys::snd_mixer_elem_t) -> String {
    // SAFETY: a live element; the name is a C string owned by it.
    unsafe {
        let n = alsa_sys::snd_mixer_selem_get_name(e);
        if n.is_null() { String::new() } else { CStr::from_ptr(n).to_string_lossy().into_owned() }
    }
}

/// An open mixer control. A thread waits on the mixer's descriptors and
/// calls the callback set with [`AlsaMixer::set_on_change`] whenever the
/// control changes (including through [`AlsaMixer::set_volume`]).
pub struct AlsaMixer {
    handle: Arc<Mutex<Handle>>,
    on_change: Arc<Mutex<Option<Callback>>>,
    card: String,
    name: String,
    has_db: bool,
    min_db: f64,
    top_db: f64,
    min_raw: i64,
    max_raw: i64,
    stop_fd: libc::c_int,
    watcher: Option<JoinHandle<()>>,
}

impl AlsaMixer {
    /// Opens the playback control of `card` (`hw:CARD=DX1`, `hw:1`).
    pub fn open(card: &str) -> Result<AlsaMixer, MixerError> {
        let c_card = CString::new(card).map_err(|_| MixerError::NoControl)?;
        let mut handle = open_handle(&c_card)?;
        let hardware = HardwareControls::open(&c_card);

        let mut elems = Vec::new();
        // SAFETY: walking the live mixer's element list.
        unsafe {
            let mut e = alsa_sys::snd_mixer_first_elem(handle.mixer);
            while !e.is_null() {
                let name = elem_name(e);
                let usable = alsa_sys::snd_mixer_selem_is_active(e) != 0
                    && alsa_sys::snd_mixer_selem_has_playback_volume(e) != 0
                    && hardware.as_ref().is_none_or(|h| h.is_hardware(&name, alsa_sys::snd_mixer_selem_get_index(e)));
                elems.push((name, usable, e));
                e = alsa_sys::snd_mixer_elem_next(e);
            }
        }
        drop(hardware);
        let i = pick_control(elems.iter().map(|(n, u, _)| (n.as_str(), *u))).ok_or(MixerError::NoControl)?;
        let (name, _, elem) = elems.swap_remove(i);

        let (mut min_raw, mut max_raw): (libc::c_long, libc::c_long) = (0, 0);
        let (mut min_mb, mut max_mb): (libc::c_long, libc::c_long) = (0, 0);
        // SAFETY: a live element, valid out pointers.
        let has_db = unsafe {
            check(alsa_sys::snd_mixer_selem_id_malloc(&mut handle.id))?;
            alsa_sys::snd_mixer_selem_get_id(elem, handle.id);
            alsa_sys::snd_mixer_selem_get_playback_volume_range(elem, &mut min_raw, &mut max_raw);
            alsa_sys::snd_mixer_selem_get_playback_dB_range(elem, &mut min_mb, &mut max_mb) == 0 && max_mb > min_mb
        };
        let (min_db, top) = if has_db { (min_mb as f64 / 100.0, top_db(max_mb as f64 / 100.0)) } else { (0.0, 0.0) };

        // Changes made elsewhere arrive on the mixer's file descriptors.
        // SAFETY: the array has room for `count` descriptors.
        let fds = unsafe {
            let count = alsa_sys::snd_mixer_poll_descriptors_count(handle.mixer).max(0) as usize;
            let mut fds = vec![libc::pollfd { fd: -1, events: 0, revents: 0 }; count];
            let n = if count > 0 {
                alsa_sys::snd_mixer_poll_descriptors(handle.mixer, fds.as_mut_ptr(), count as libc::c_uint)
            } else {
                0
            };
            fds.truncate(n.max(0) as usize);
            fds
        };
        // SAFETY: eventfd has no preconditions.
        let stop_fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
        let handle = Arc::new(Mutex::new(handle));
        let on_change: Arc<Mutex<Option<Callback>>> = Arc::new(Mutex::new(None));
        let watcher = if stop_fd >= 0 && !fds.is_empty() {
            let handle = Arc::clone(&handle);
            let on_change = Arc::clone(&on_change);
            std::thread::Builder::new()
                .name("sonkore-mixer".into())
                .spawn(move || watch(fds, stop_fd, &handle, &on_change))
                .ok()
        } else {
            None
        };
        Ok(AlsaMixer {
            handle,
            on_change,
            card: card.to_string(),
            name,
            has_db,
            min_db,
            top_db: top,
            min_raw,
            max_raw,
            stop_fd,
            watcher,
        })
    }

    /// Called on the mixer's own thread after every change.
    pub fn set_on_change(&self, f: impl Fn() + Send + 'static) {
        *lock(&self.on_change) = Some(Box::new(f));
    }

    pub fn card(&self) -> &str {
        &self.card
    }

    pub fn control_name(&self) -> &str {
        &self.name
    }

    pub fn has_db_scale(&self) -> bool {
        self.has_db
    }

    pub fn min_db(&self) -> f64 {
        self.min_db
    }

    pub fn top_db(&self) -> f64 {
        self.top_db
    }

    /// Linear amplitude 0..1, where 1 is the top of the control.
    pub fn volume(&self) -> f64 {
        let handle = lock(&self.handle);
        let Some(e) = handle.elem() else {
            return 1.0;
        };
        // SAFETY: a live element (under the lock), valid out pointers.
        unsafe {
            if alsa_sys::snd_mixer_selem_has_playback_switch(e) != 0 {
                let mut on: libc::c_int = 1;
                if alsa_sys::snd_mixer_selem_get_playback_switch(e, CHANNEL, &mut on) == 0 && on == 0 {
                    return 0.0;
                }
            }
            if self.has_db {
                let mut mb: libc::c_long = 0;
                if alsa_sys::snd_mixer_selem_get_playback_dB(e, CHANNEL, &mut mb) == 0 {
                    return db_to_linear(mb as f64 / 100.0, self.top_db);
                }
            }
            let mut raw: libc::c_long = 0;
            alsa_sys::snd_mixer_selem_get_playback_volume(e, CHANNEL, &mut raw);
            raw_to_linear(raw as i64, self.min_raw, self.max_raw)
        }
    }

    /// Sets the linear amplitude. 0 mutes when the control has a switch,
    /// otherwise sets its minimum.
    pub fn set_volume(&self, linear: f64) {
        let linear = if linear.is_nan() { 0.0 } else { linear.clamp(0.0, 1.0) };
        let handle = lock(&self.handle);
        let Some(e) = handle.elem() else {
            return;
        };
        // SAFETY: a live element (under the lock).
        unsafe {
            let has_switch = alsa_sys::snd_mixer_selem_has_playback_switch(e) != 0;
            if linear <= 0.0 {
                if has_switch {
                    alsa_sys::snd_mixer_selem_set_playback_switch_all(e, 0);
                } else {
                    alsa_sys::snd_mixer_selem_set_playback_volume_all(e, self.min_raw as libc::c_long);
                }
                return;
            }
            if has_switch {
                alsa_sys::snd_mixer_selem_set_playback_switch_all(e, 1);
            }
            if self.has_db {
                // Towards the quieter step when the control cannot hit the value.
                let mb = (linear_to_db(linear, self.min_db, self.top_db) * 100.0).round() as libc::c_long;
                if alsa_sys::snd_mixer_selem_set_playback_dB_all(e, mb, -1) < 0 {
                    alsa_sys::snd_mixer_selem_set_playback_dB_all(e, mb, 1);
                }
                return;
            }
            let raw = linear_to_raw(linear, self.min_raw, self.max_raw);
            alsa_sys::snd_mixer_selem_set_playback_volume_all(e, raw as libc::c_long);
        }
    }
}

impl Drop for AlsaMixer {
    fn drop(&mut self) {
        if self.stop_fd >= 0 {
            let one: u64 = 1;
            // SAFETY: writing 8 bytes from a valid u64 to our eventfd.
            unsafe { libc::write(self.stop_fd, (&one as *const u64).cast(), 8) };
        }
        if let Some(t) = self.watcher.take() {
            let _ = t.join();
        }
        if self.stop_fd >= 0 {
            // SAFETY: our own descriptor, closed once.
            unsafe { libc::close(self.stop_fd) };
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn watch(
    mut fds: Vec<libc::pollfd>,
    stop_fd: libc::c_int,
    handle: &Mutex<Handle>,
    on_change: &Mutex<Option<Callback>>,
) {
    fds.push(libc::pollfd { fd: stop_fd, events: libc::POLLIN, revents: 0 });
    loop {
        for fd in fds.iter_mut() {
            fd.revents = 0;
        }
        // SAFETY: `fds` is a valid array of pollfd.
        let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, -1) };
        if n < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return;
        }
        let (stop, mixer_fds) = fds.split_last_mut().expect("the stop descriptor is there");
        if stop.revents != 0 {
            return;
        }
        let gone = mixer_fds.iter().any(|f| f.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0);
        let handled = {
            let h = lock(handle);
            let mut revents: libc::c_ushort = 0;
            // SAFETY: the live mixer and the descriptors it gave.
            unsafe {
                alsa_sys::snd_mixer_poll_descriptors_revents(
                    h.mixer,
                    mixer_fds.as_mut_ptr(),
                    mixer_fds.len() as libc::c_uint,
                    &mut revents,
                );
                revents != 0 && alsa_sys::snd_mixer_handle_events(h.mixer) >= 0
            }
        };
        if handled && let Some(f) = lock(on_change).as_ref() {
            f();
        }
        if gone {
            return; // the card went away
        }
    }
}

/// The card's control list, to tell real controls from alsa-lib's softvol
/// ones: the softvol plugin adds "user" controls (the "PCM" of most onboard
/// HD-Audio chips) that only act on the "default" chain; on a direct hw:
/// output they change nothing.
struct HardwareControls {
    ctl: *mut alsa_sys::snd_ctl_t,
}

impl HardwareControls {
    fn open(name: &CStr) -> Option<HardwareControls> {
        let mut ctl = std::ptr::null_mut();
        // SAFETY: valid out pointer and C string.
        let r = unsafe { alsa_sys::snd_ctl_open(&mut ctl, name.as_ptr(), alsa_sys::SND_CTL_READONLY as i32) };
        (r >= 0 && !ctl.is_null()).then_some(HardwareControls { ctl })
    }

    /// False only for a control the card's list marks as a user control.
    fn is_hardware(&self, name: &str, index: u32) -> bool {
        let wanted = format!("{name} Playback Volume");
        // SAFETY: alsa-lib list/info objects allocated and freed here, used
        // with the open ctl handle only.
        unsafe {
            let mut list = std::ptr::null_mut();
            if alsa_sys::snd_ctl_elem_list_malloc(&mut list) < 0 {
                return true;
            }
            let mut hardware = true;
            'done: {
                if alsa_sys::snd_ctl_elem_list(self.ctl, list) < 0 {
                    break 'done;
                }
                let count = alsa_sys::snd_ctl_elem_list_get_count(list);
                if alsa_sys::snd_ctl_elem_list_alloc_space(list, count) < 0 {
                    break 'done;
                }
                if alsa_sys::snd_ctl_elem_list(self.ctl, list) == 0 {
                    for i in 0..alsa_sys::snd_ctl_elem_list_get_used(list) {
                        let n = alsa_sys::snd_ctl_elem_list_get_name(list, i);
                        if n.is_null()
                            || std::ffi::CStr::from_ptr(n).to_bytes() != wanted.as_bytes()
                            || alsa_sys::snd_ctl_elem_list_get_index(list, i) != index
                        {
                            continue;
                        }
                        let mut id = std::ptr::null_mut();
                        let mut info = std::ptr::null_mut();
                        if alsa_sys::snd_ctl_elem_id_malloc(&mut id) == 0 {
                            if alsa_sys::snd_ctl_elem_info_malloc(&mut info) == 0 {
                                alsa_sys::snd_ctl_elem_list_get_id(list, i, id);
                                alsa_sys::snd_ctl_elem_info_set_id(info, id);
                                if alsa_sys::snd_ctl_elem_info(self.ctl, info) == 0
                                    && alsa_sys::snd_ctl_elem_info_is_user(info) != 0
                                {
                                    hardware = false;
                                }
                                alsa_sys::snd_ctl_elem_info_free(info);
                            }
                            alsa_sys::snd_ctl_elem_id_free(id);
                        }
                        break;
                    }
                }
                alsa_sys::snd_ctl_elem_list_free_space(list);
            }
            alsa_sys::snd_ctl_elem_list_free(list);
            hardware
        }
    }
}

impl Drop for HardwareControls {
    fn drop(&mut self) {
        // SAFETY: opened in `open`, closed once.
        unsafe { alsa_sys::snd_ctl_close(self.ctl) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_by_preference_then_any() {
        let c = [("Line", true), ("Master", true), ("PCM", true)];
        assert_eq!(pick_control(c), Some(2));
        let c = [("Line", true), ("Master", true), ("PCM", false)];
        assert_eq!(pick_control(c), Some(1));
        let c = [("Line", false), ("DX1 Volume", true)];
        assert_eq!(pick_control(c), Some(1));
        assert_eq!(pick_control([("PCM", false)]), None);
    }

    #[test]
    fn db_against_the_top() {
        // A control reaching +6 dB is treated as ending at 0 dB.
        assert_eq!(top_db(6.0), 0.0);
        assert_eq!(top_db(-3.0), -3.0);
        assert!((db_to_linear(-6.0, 0.0) - 0.501187).abs() < 1e-6);
        assert_eq!(db_to_linear(3.0, 0.0), 1.0);
        assert!((db_to_linear(-9.0, -3.0) - 0.501187).abs() < 1e-6);
        assert!((linear_to_db(0.5, -100.0, 0.0) + 6.0206).abs() < 1e-4);
        assert!((linear_to_db(0.5, -100.0, -3.0) + 9.0206).abs() < 1e-4);
        assert_eq!(linear_to_db(1e-9, -60.0, 0.0), -60.0);
        for v in [0.01, 0.1, 0.5, 0.9, 1.0] {
            assert!((db_to_linear(linear_to_db(v, -120.0, -2.0), -2.0) - v).abs() < 1e-12);
        }
    }

    #[test]
    fn raw_scale() {
        assert_eq!(raw_to_linear(50, 0, 100), 0.5);
        assert_eq!(raw_to_linear(3, 5, 5), 1.0);
        assert_eq!(linear_to_raw(0.5, 0, 255), 128);
        assert_eq!(linear_to_raw(1.0, -10, 10), 10);
    }

    // Software devices only: never a hw: mixer (that is the user's DAC).
    #[test]
    fn a_card_without_a_mixer_does_not_open() {
        assert!(AlsaMixer::open("sonkore_no_such_device").is_err());
        assert!(AlsaMixer::open("null").is_err());
    }
}
