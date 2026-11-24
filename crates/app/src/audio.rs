//! The playback settings applied to the engine (the stored settings, the
//! headroom, a track's gain): the keys and defaults earlier Sonkore versions wrote.

use std::sync::Arc;

use sonkore_core::settings::Settings;
use sonkore_engine::Engine;
use sonkore_engine::quantize::DitherMode;

use crate::lib_shim::Track;

thread_local! {
    /// The player whose output a release request closes (set by the controller).
    static PLAYER: std::cell::RefCell<std::rc::Weak<std::cell::RefCell<sonkore_engine::Player>>> =
        const { std::cell::RefCell::new(std::rc::Weak::new()) };
}

/// The engine, opening its outputs this way: a card played to
/// directly (`alsa|hw:…`) is reserved first (org.freedesktop.ReserveDevice1),
/// so the sound server lets go of it, and given back when another output is
/// opened (or Sonkore quits).
pub fn engine() -> Engine {
    let reservation = RESERVATION
        .get_or_init(|| Arc::new(sonkore_platform::DeviceReservation::new(sonkore_platform::Bus::Session)))
        .clone();
    // Another program wants the card: the device closes before the answer.
    reservation.set_on_release_requested(|| {
        let (done, wait) = std::sync::mpsc::channel();
        let asked = slint::invoke_from_event_loop(move || {
            PLAYER.with(|p| {
                if let Some(player) = p.borrow().upgrade() {
                    player.borrow_mut().release_output();
                }
            });
            let _ = done.send(());
        });
        if asked.is_ok() {
            let _ = wait.recv_timeout(std::time::Duration::from_secs(2));
        }
    });
    let engine = Engine::with_outputs(Box::new(move |key, request, render| {
        let card = key.strip_prefix("alsa|").and_then(sonkore_platform::reservation::card_index_of);
        let given_back = card.is_none() && reservation.held();
        if let Err(busy) = reservation.acquire_with_retry(card) {
            return Err(sonkore_engine::output::OutputError(busy.to_string()));
        }
        if given_back {
            wait_for_sink(key);
        }
        let (render, tap) = crate::tap::wrap(render);
        let opened = sonkore_engine::output::open(key, request, render);
        if opened.is_err() && card.is_some() {
            reservation.release();
        }
        if let (Ok(out), Some(tap)) = (&opened, tap) {
            // ALSA's null device (the tests' DAC) takes samples faster than it plays.
            crate::tap::opened(&tap, out.key(), &out.format(), !key.starts_with("pipewire|") && !key.is_empty());
        }
        opened
    }));
    // The device threads get real-time priority through rtkit (its portal
    // in a Flatpak), asked for off the audio threads; refused: as before.
    #[cfg(target_os = "linux")]
    let engine = {
        let mut engine = engine;
        engine.set_realtime_hook(Box::new(sonkore_platform::rtkit::make_realtime));
        engine
    };
    engine
}

/// No sound server to wait for (no card is reserved off Linux).
#[cfg(not(target_os = "linux"))]
fn wait_for_sink(_key: &str) {}

/// A card just given back to the sound server: its sink takes a moment to
/// come back; up to 2 s for the one asked
/// for ("" or a name).
#[cfg(target_os = "linux")]
fn wait_for_sink(key: &str) {
    use sonkore_engine::output::pipewire;
    if !pipewire::running() {
        return;
    }
    let name = key.strip_prefix("pipewire|").or_else(|| key.strip_prefix("pulse|"));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        let sinks = pipewire::sinks();
        let back = match name {
            Some(n) => sinks.iter().any(|s| pipewire::base_sink_name(&s.name) == pipewire::base_sink_name(n)),
            None => sinks.iter().any(|s| s.is_default),
        };
        if back {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// `db` up or down from `now`, whole tenths of a
/// dB; the top is exactly 1, below -60 dB is silence.
pub fn stepped_volume(now: f64, db: f64) -> f64 {
    let next = if now > 0.0 {
        20.0 * now.log10() + db
    } else if db > 0.0 {
        -60.0 + db
    } else {
        -1e9
    };
    let next = (next * 10.0).round() / 10.0;
    if next >= -0.05 {
        1.0
    } else if next < -60.0 {
        0.0
    } else {
        10f64.powf(next / 20.0)
    }
}

/// The card reserved for a direct output (see [`engine`]).
static RESERVATION: std::sync::OnceLock<Arc<sonkore_platform::DeviceReservation>> = std::sync::OnceLock::new();

/// A card is held for Sonkore alone.
pub fn output_reserved() -> bool {
    RESERVATION.get().is_some_and(|r| r.held())
}

/// The outputs as they are listed: "System
/// default" heading the sound server's group, then every output; the card
/// Sonkore holds has no sink in PipeWire meanwhile, so its sinks are shown as
/// they were (after the other PipeWire outputs), the way back where it was.
pub fn output_list(devices: &[sonkore_engine::output::DeviceInfo]) -> Vec<sonkore_engine::output::DeviceInfo> {
    use sonkore_engine::output::DeviceInfo;
    thread_local! {
        static CARD_SINKS: std::cell::RefCell<std::collections::HashMap<i32, Vec<DeviceInfo>>> = Default::default();
    }
    let default_group = devices
        .iter()
        .find(|d| d.key.starts_with("pipewire|") || d.key.starts_with("pulse|"))
        .map(|d| d.backend.clone())
        .unwrap_or_else(|| crate::i18n::tr("DspController", "System"));
    let mut all = vec![DeviceInfo {
        key: String::new(),
        backend: default_group,
        name: crate::i18n::tr("DspController", "System default"),
        detail: crate::i18n::tr("DspController", "Follows the desktop's sound settings"),
        is_default: true,
        direct: false,
        alsa_card: -1,
    }];
    all.extend(devices.iter().cloned());
    let mut now: std::collections::HashMap<i32, Vec<DeviceInfo>> = Default::default();
    for d in devices.iter().filter(|d| d.alsa_card >= 0 && !d.direct) {
        now.entry(d.alsa_card).or_default().push(d.clone());
    }
    let held = RESERVATION.get().filter(|r| r.held()).and_then(|r| r.card()).map_or(-1, |c| c as i32);
    CARD_SINKS.with(|cards| {
        let mut cards = cards.borrow_mut();
        let missing = held >= 0 && !now.contains_key(&held);
        cards.extend(now);
        if missing {
            if let Some(sinks) = cards.get(&held) {
                let mut at = 1;
                while at < all.len() && all[at].key.starts_with("pipewire|") {
                    at += 1;
                }
                for d in sinks {
                    let mut d = d.clone();
                    d.detail = crate::i18n::tr("DspController", "Given back to PipeWire when chosen");
                    d.is_default = false;
                    all.insert(at, d);
                    at += 1;
                }
            }
        }
    });
    all
}

/// "Release DAC to the system": paused, the
/// device closed and the card given back; the next play takes it again.
pub fn release_output(player: &mut sonkore_engine::Player) {
    player.release_output();
    if let Some(r) = RESERVATION.get() {
        r.release();
    }
}

/// The player a release request pauses and closes.
pub fn set_player(player: &std::rc::Rc<std::cell::RefCell<sonkore_engine::Player>>) {
    PLAYER.with(|p| *p.borrow_mut() = std::rc::Rc::downgrade(player));
}

/// The settings that shape the output path and the processing.
pub fn apply(settings: &Settings, engine: &mut Engine) {
    let p = |k: &str| format!("playback/{k}");
    engine.set_follow_rate(settings.flag(&p("followTrackRate"), true));
    // Windows: WASAPI exclusive mode (on by default).
    engine.set_exclusive(settings.flag(&p("exclusive"), true));
    engine.set_hdcd(settings.flag(&p("hdcd"), false));
    sonkore_engine::decode::set_from_memory(settings.flag(&p("playFromMemory"), true));
    engine.set_buffer_ms(settings.int(&p("bufferMs"), 150).clamp(60, 2000) as u32);
    // Silence after the DAC changes rate, so it locks before the music.
    engine.set_resync_delay_ms(settings.int(&p("resyncDelayMs"), 250).clamp(0, 3000) as u32);
    let dither_default = if settings.flag(&p("dither"), true) { 1 } else { 0 };
    engine.set_dither(DitherMode::from_index(settings.int(&p("ditherMode"), dither_default).clamp(0, 2)));
    engine.set_rate_mode(
        settings.int(&p("upsampling"), 0).clamp(0, 3),
        settings.int(&p("fixedRate"), 0).clamp(0, 768_000) as u32,
    );
    // DSD: 0 as PCM, 1 native (the default), 2 DoP.
    engine.set_dsd_output(settings.int(&p("dsdOutput"), 1));
    // PCM to DSD through that DSD output: 0 off, 1 DSD64, 2 DSD128, 3 DSD256,
    // 4 the highest the DAC takes (up to DSD256), 5 DSD512.
    engine.set_pcm_to_dsd(settings.int(&p("pcmToDsd"), 0));
    let crossfeed = settings.int(&p("crossfeed"), 0).clamp(0, 3);
    let limiter = settings.flag(&p("limiter"), false);
    let ceiling = settings.number(&p("limiterCeilingDb"), -1.0).clamp(-6.0, 0.0);
    let filter = settings.int(&p("resampleFilter"), 0).clamp(0, 3);
    engine.dsp().update(|s| {
        s.crossfeed = crossfeed;
        s.limiter = limiter;
        s.limiter_ceiling_db = ceiling;
        s.resample_filter = filter;
    });
    apply_convolution(settings, engine);
    apply_pitch(settings, engine);
    apply_headroom(settings, engine);
}

/// The impulse response when convolution is on (playback/convolution, /convolutionFile).
pub fn apply_convolution(settings: &Settings, engine: &Engine) {
    let on = settings.flag("playback/convolution", false);
    let file =
        sonkore_platform::portal::host_path(std::path::Path::new(&settings.text("playback/convolutionFile", "")));
    // Every playback/* change comes through here: the file is decoded again
    // only when it is another one or changed on disk (it used to be decoded,
    // on the interface's thread, for each limiter toggle or preamp step).
    thread_local! {
        static LOADED: std::cell::RefCell<Option<(std::path::PathBuf, Option<std::time::SystemTime>)>> =
            const { std::cell::RefCell::new(None) };
    }
    let wanted = (on && !file.as_os_str().is_empty()).then(|| {
        let modified = std::fs::metadata(&file).and_then(|m| m.modified()).ok();
        (file.clone(), modified)
    });
    let in_use = engine.dsp().snapshot().impulse.is_some();
    if wanted.is_some() && in_use && LOADED.with(|l| *l.borrow() == wanted) {
        return;
    }
    let ir = match &wanted {
        Some((file, _)) => sonkore_dsp::convolver::ImpulseResponse::load(file).ok().map(Arc::new),
        None => None,
    };
    LOADED.with(|l| *l.borrow_mut() = if ir.is_some() { wanted } else { None });
    engine.dsp().update(|s| {
        // Loaded means new (another file, or the same one changed on disk);
        // None only matters when one was in use.
        let changed = ir.is_some() || s.impulse.is_some();
        if changed {
            s.impulse = ir;
            s.impulse_generation += 1;
        }
    });
}

/// Pitch, tempo, rate (pitch/…) and SoundTouch's settings (soundtouch/…).
pub fn apply_pitch(settings: &Settings, engine: &Engine) {
    let st = &engine.dsp().stretch;
    st.set_use_aa_filter(settings.flag("soundtouch/aaFilter", true));
    st.set_aa_filter_length(settings.int("soundtouch/aaFilterLength", 64) as i32);
    st.set_use_quick_seek(settings.flag("soundtouch/quickSeek", false));
    st.set_sequence_ms(settings.int("soundtouch/sequenceMs", 0) as i32);
    st.set_seek_window_ms(settings.int("soundtouch/seekWindowMs", 0) as i32);
    st.set_overlap_ms(settings.int("soundtouch/overlapMs", 8) as i32);
    // Older versions kept the ratios in the soundtouch group.
    let legacy = |k: &str, d: f64| settings.number(&format!("soundtouch/{k}"), d);
    st.set_pitch_semitones(settings.number("pitch/pitch", legacy("pitch", 0.0)));
    st.set_tempo(settings.number("pitch/tempo", legacy("tempo", 1.0)));
    st.set_rate(settings.number("pitch/rate", legacy("rate", 1.0)));
    st.set_enabled(settings.flag("pitch/enabled", settings.flag("soundtouch/enabled", false)));
}

/// Headroom: manual, or 3 dB above the EQ's
/// highest boost; whole tenths of a dB.
pub fn apply_headroom(settings: &Settings, engine: &Engine) -> f64 {
    let db = if settings.flag("playback/headroomAuto", false) {
        (-(3.0 + engine.dsp().eq.peak_gain_db().max(0.0))).clamp(-24.0, 0.0)
    } else {
        settings.number("playback/headroomDb", 0.0).clamp(-24.0, 0.0)
    };
    let rounded = (db * 10.0).round() / 10.0;
    engine.dsp().update(|s| s.headroom_db = rounded);
    rounded
}

/// ReplayGain for a track: track or album gain
/// with the preamp, its peak landing at -1 dBTP when clipping is prevented;
/// 1 when off or unknown.
pub fn track_gain(settings: &Settings, track: Option<&Track>) -> f64 {
    let mode = settings.int("playback/replayGain", 0).clamp(0, 2);
    let Some(t) = track else { return 1.0 };
    if mode == 0 {
        return 1.0;
    }
    let preamp = settings.number("playback/replayGainPreamp", 0.0).clamp(-15.0, 15.0);
    let (mut gain, mut peak) = (t.rg_track_gain, t.rg_track_peak.unwrap_or(0.0));
    if mode == 2 && t.rg_album_gain.is_some() {
        gain = t.rg_album_gain;
        peak = t.rg_album_peak.unwrap_or(0.0);
    }
    let Some(gain) = gain else { return 1.0 };
    let mut linear = 10f64.powf((gain + preamp) / 20.0);
    const CEILING: f64 = 0.8912509381337456; // -1 dB
    if settings.flag("playback/replayGainPreventClipping", true) && peak > 0.0 {
        linear = linear.min(CEILING / peak);
    }
    linear
}
