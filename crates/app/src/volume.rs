//! Where the volume is set.
//!
//! DAC: Sonkore moves the DAC's own control (the card's ALSA mixer on a
//! direct output); where there is none the volume stays fixed at 100 %. DAC
//! never falls back to digital volume. Automatic on a direct output: fixed
//! at 100 %, set on the DAC or amplifier. Either way the engine stays at
//! unity gain, so the volume never costs bit-perfect. A PipeWire output (the
//! desktop's default or a sink by name) in DAC or Automatic: the sink's own
//! volume, the DAC's control on a device's sink. Digital, or Automatic on a
//! shared output: Sonkore's own, in the engine.
//!
//! On Windows a
//! direct output is WASAPI in exclusive mode (playback/exclusive), and the
//! DAC's control is the endpoint's own volume (IAudioEndpointVolume, when
//! the device implements it in hardware). There is no sound server volume.

use std::cell::RefCell;

use sonkore_core::settings::Settings;
use sonkore_engine::Engine;
#[cfg(target_os = "linux")]
use sonkore_engine::output::pipewire::SinkVolume;

#[cfg(target_os = "linux")]
type Mixer = sonkore_platform::AlsaMixer;
#[cfg(windows)]
type Mixer = crate::win_volume::EndpointVolume;

/// Windows has no sound server whose volume Sonkore follows.
#[cfg(windows)]
struct SinkVolume;

#[cfg(windows)]
impl SinkVolume {
    fn open(_sink: &str) -> Option<SinkVolume> {
        None
    }
    fn set_on_change(&self, _f: impl Fn() + Send + 'static) {}
    fn volume(&self) -> f64 {
        1.0
    }
    fn set_volume(&self, _v: f64) {}
    fn hardware(&self) -> bool {
        false
    }
    fn sink_description(&self) -> String {
        String::new()
    }
}

pub const DIGITAL: i64 = 0;
pub const HARDWARE: i64 = 1;
pub const AUTO: i64 = 2;

#[derive(Default)]
struct State {
    mixer: Option<Mixer>,
    /// The card the mixer is open on ("hw:CARD=DX1"; on Windows the
    /// endpoint's name, "" for the default).
    mixer_card: String,
    sink: Option<SinkVolume>,
    /// The sink followed ("" = the default).
    sink_name: Option<String>,
    fixed: bool,
    /// The engine held at 1 (the digital volume kept aside).
    at_unity: bool,
    digital: f64,
    /// Runs when the volume changes elsewhere (the desktop, the DAC's knob).
    on_change: Option<std::rc::Rc<dyn Fn()>>,
    /// Windows: WASAPI exclusive mode (playback/exclusive), what makes an
    /// output direct there.
    exclusive: bool,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State { digital: 1.0, ..Default::default() });
}

/// Played to without a sound server between.
#[cfg(not(windows))]
pub fn direct_output(key: &str) -> bool {
    key.starts_with("alsa|hw:") || key.starts_with("alsa|plughw:")
}

/// Played to without the Windows mixer between: WASAPI in exclusive mode,
/// or an ASIO driver.
#[cfg(windows)]
pub fn direct_output(key: &str) -> bool {
    key.starts_with("asio|") || ((key.is_empty() || key.starts_with("wasapi|")) && STATE.with(|s| s.borrow().exclusive))
}

/// The digital volume the engine starts with (playback/volume).
pub fn set_digital(v: f64) {
    STATE.with(|s| s.borrow_mut().digital = v);
}

/// What to run when the volume changes elsewhere (on the interface's thread).
pub fn on_change(f: impl Fn() + 'static) {
    STATE.with(|s| s.borrow_mut().on_change = Some(std::rc::Rc::new(f)));
}

fn changed_elsewhere() {
    let _ = slint::invoke_from_event_loop(|| {
        let f = STATE.with(|s| s.borrow().on_change.clone());
        if let Some(f) = f {
            f();
        }
    });
}

/// The mode or the output changed: the controls opened again.
pub fn update(settings: &Settings, engine: &Engine) {
    let mode = settings.int("playback/volumeMode", AUTO).clamp(0, 2);
    let key = engine.device_key().to_owned();
    STATE.with(|s| s.borrow_mut().exclusive = settings.flag("playback/exclusive", true));
    let direct = direct_output(&key);
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        // The card's own control (DAC mode on an ALSA output; on Windows on
        // an exclusive WASAPI one).
        let card = if cfg!(windows) {
            // (An ASIO driver has no Windows volume control of its own.)
            (direct && !key.starts_with("asio|")).then(|| key.strip_prefix("wasapi|").unwrap_or("").to_owned())
        } else {
            key.strip_prefix("alsa|").map(|d| d.split(',').next().unwrap_or(d).to_owned())
        };
        let want_mixer = if mode == HARDWARE { card } else { None };
        match want_mixer {
            Some(card) if s.mixer.is_some() && s.mixer_card == card => {}
            Some(card) => {
                s.mixer = Mixer::open(&card).ok();
                if let Some(m) = &s.mixer {
                    m.set_on_change(changed_elsewhere);
                }
                s.mixer_card = card;
            }
            None => {
                s.mixer = None;
                s.mixer_card.clear();
            }
        }
        // The sink's volume (DAC or Automatic on a PipeWire output).
        let pipewire = key.is_empty() || key.starts_with("pipewire|") || key.starts_with("pulse|");
        let want_sink = (mode != DIGITAL && pipewire)
            .then(|| key.strip_prefix("pipewire|").or_else(|| key.strip_prefix("pulse|")).unwrap_or("").to_owned());
        if want_sink != s.sink_name || (want_sink.is_some() && s.sink.is_none()) {
            s.sink = want_sink.as_deref().and_then(SinkVolume::open);
            if let Some(v) = &s.sink {
                v.set_on_change(changed_elsewhere);
            }
            s.sink_name = if s.sink.is_some() { want_sink } else { None };
        }
        let pw = s.sink.is_some();
        let active = s.mixer.is_some();
        s.fixed = !pw && ((mode == AUTO && direct) || (mode == HARDWARE && !active));
        let unity = active || pw || s.fixed;
        if unity != s.at_unity {
            if unity {
                s.digital = engine.volume();
                engine.set_volume(1.0);
            } else {
                engine.set_volume(s.digital);
            }
            s.at_unity = unity;
        }
    });
}

/// At 100 %, set on the DAC or the amplifier.
pub fn fixed() -> bool {
    STATE.with(|s| s.borrow().fixed)
}

/// The DAC's own control moves.
pub fn hardware() -> bool {
    STATE.with(|s| s.borrow().mixer.is_some() && s.borrow().sink.is_none())
}

/// The sound server's volume for the sink.
pub fn system() -> bool {
    STATE.with(|s| s.borrow().sink.is_some())
}

/// …and the sink carries it on the DAC's control.
pub fn system_in_hardware() -> bool {
    STATE.with(|s| s.borrow().sink.as_ref().is_some_and(|v| v.hardware()))
}

/// The DAC control's name and level, while it carries the volume.
pub fn hardware_control() -> Option<(String, f64)> {
    STATE.with(|s| {
        let s = s.borrow();
        match (&s.mixer, &s.sink) {
            (Some(m), None) => Some((m.control_name().to_owned(), m.volume())),
            _ => None,
        }
    })
}

/// The volume: 1 when fixed, else wherever it is set.
pub fn get(engine: &Engine) -> f64 {
    STATE.with(|s| {
        let s = s.borrow();
        if s.fixed {
            1.0
        } else if let (Some(m), None) = (&s.mixer, &s.sink) {
            m.volume()
        } else if let Some(v) = &s.sink {
            v.volume()
        } else {
            engine.volume()
        }
    })
}

/// Sets the volume; true when the digital volume changed (it is
/// saved as playback/volume; the DAC and the sound server keep theirs).
pub fn set(engine: &Engine, v: f64) -> bool {
    if !v.is_finite() {
        return false;
    }
    let v = v.clamp(0.0, 1.0);
    STATE.with(|s| {
        let s = s.borrow();
        if s.fixed {
            false
        } else if let (Some(m), None) = (&s.mixer, &s.sink) {
            m.set_volume(v);
            false
        } else if let Some(sink) = &s.sink {
            sink.set_volume(v);
            false
        } else {
            engine.set_volume(v);
            true
        }
    })
}

/// [`set`], the digital volume saved as playback/volume.
pub fn change(settings: &RefCell<Settings>, engine: &Engine, v: f64) {
    if set(engine, v) {
        settings.borrow_mut().set_number("playback/volume", v.clamp(0.0, 1.0));
    }
}

/// The line under Settings › Audio ›
/// Volume for the mode in use ("" for Digital).
pub fn info(mode: i64, key: &str) -> String {
    use crate::i18n::{tr, trf};
    const C: &str = "DspController";
    if mode == DIGITAL {
        return String::new();
    }
    let automatic = mode == AUTO;
    let direct = direct_output(key);
    STATE.with(|s| {
        let s = s.borrow();
        if s.fixed && automatic {
            return tr(C, "Automatic, direct output: Sonkore sends the signal at 100 % and the volume is set on the DAC or the amplifier, with its own knob. The audio stays bit-perfect. Choose Digital to set it from Sonkore, or DAC to move the DAC's control from here.");
        }
        if s.fixed {
            return if direct {
                tr(C, "This DAC has no volume control of its own: the volume stays fixed at 100 %, set it on the DAC or the amplifier. Choose Digital to set it from Sonkore.")
            } else {
                tr(C, "Only for direct (hw:) outputs: with this one the volume stays fixed at 100 %. Choose Digital to set it from Sonkore.")
            };
        }
        if let Some(sink) = &s.sink {
            let name = sink.sink_description();
            let control = if sink.hardware() {
                trf(C, "Using the volume of \"{0}\", the DAC's own control through PipeWire: the desktop's volume keys and Sonkore's slider move the same control, and the audio stays bit-perfect at any volume.", &[&name])
            } else {
                trf(C, "Using the volume of \"{0}\" through PipeWire. It is a virtual output, so PipeWire scales the samples in software below 100 %.", &[&name])
            };
            return if automatic { trf(C, "Automatic: {0}", &[&control]) } else { control };
        }
        if let Some(m) = &s.mixer {
            let control = if m.has_db_scale() {
                let n = |v: f64| crate::i18n::locale_number(v, 1);
                trf(C, "Using the DAC's \"{0}\" control ({1} to {2} dB).", &[m.control_name(), &n(m.min_db().max(-150.0)), &n(m.top_db())])
            } else {
                trf(C, "Using the DAC's \"{0}\" control.", &[m.control_name()])
            };
            return if automatic { trf(C, "Automatic, direct output: {0} The audio stays bit-perfect at any volume.", &[&control]) } else { control };
        }
        if automatic && !direct {
            return tr(C, "Automatic, shared output: Sonkore's digital volume is used. With a direct (hw:) output, the DAC's own volume is used instead.");
        }
        if key.starts_with("alsa|") {
            return tr(C, "Automatic: this DAC has no volume control of its own, so Sonkore's digital volume is used.");
        }
        tr(C, "Automatic: Sonkore's digital volume is used with this output.")
    })
}

/// The sink's volume while it carries Sonkore's: (linear, on the DAC's control).
pub fn system_volume() -> Option<(f64, bool)> {
    STATE.with(|s| s.borrow().sink.as_ref().map(|v| (v.volume(), v.hardware())))
}
