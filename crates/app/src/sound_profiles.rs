//! Each output keeps its own sound (playback/soundPerOutput, on by default):
//! headphones and speakers each come back with their equalizer, convolution,
//! crossfeed, headroom, dither, oversampling and DSD settings. They live in
//! `soundProfiles/<output>` as compact JSON (the output's key, "" as
//! "default", '/' and '\' as '|'), stored when an output is left, when the
//! switch is turned on and at quit; an output not seen before starts from
//! the sound in use.

use std::cell::RefCell;

use serde_json::{Map, Value};
use sonkore_core::settings::Settings;
use sonkore_eq::controller::EqController;

const SWITCH: &str = "playback/soundPerOutput";
const GROUP: &str = "soundProfiles";

#[derive(Clone, Copy)]
enum Kind {
    Int(i64),
    Number(f64),
    Flag(bool),
    Text,
}

/// The `playback/` settings a profile carries, with their defaults.
const KEYS: [(&str, Kind); 12] = [
    ("ditherMode", Kind::Int(1)),
    ("headroomDb", Kind::Number(0.0)),
    ("headroomAuto", Kind::Flag(false)),
    ("crossfeed", Kind::Int(0)),
    ("upsampling", Kind::Int(0)),
    ("convolutionFile", Kind::Text),
    ("limiter", Kind::Flag(false)),
    ("limiterCeilingDb", Kind::Number(-1.0)),
    ("convolution", Kind::Flag(false)),
    ("resampleFilter", Kind::Int(0)),
    ("dsdOutput", Kind::Int(1)),
    ("pcmToDsd", Kind::Int(0)),
];

thread_local! {
    /// Runs after a profile was put back (the equalizer window shows it).
    static ON_RESTORED: RefCell<Vec<Box<dyn Fn()>>> = const { RefCell::new(Vec::new()) };
}

/// Whether each output keeps its own sound.
pub fn enabled(settings: &Settings) -> bool {
    settings.flag(SWITCH, true)
}

/// `f` runs after a profile was put back.
pub fn on_restored(f: impl Fn() + 'static) {
    ON_RESTORED.with(|l| l.borrow_mut().push(Box::new(f)));
}

/// The settings key of `device`'s profile.
fn key_for(device: &str) -> String {
    let name = if device.is_empty() { "default" } else { device };
    format!("{GROUP}/{}", name.replace(['/', '\\'], "|"))
}

/// The sound in use, as a profile.
pub fn snapshot(settings: &Settings, eq: &EqController) -> Value {
    let mut m = Map::new();
    for (key, kind) in KEYS {
        let k = format!("playback/{key}");
        let v = match kind {
            // (ditherMode falls back on the older on/off key, as audio::apply.)
            Kind::Int(_) if key == "ditherMode" => {
                Value::from(settings.int(&k, settings.flag("playback/dither", true) as i64))
            }
            Kind::Int(d) => Value::from(settings.int(&k, d)),
            Kind::Number(d) => Value::from(settings.number(&k, d)),
            Kind::Flag(d) => Value::from(settings.flag(&k, d)),
            Kind::Text => Value::from(settings.text(&k, "")),
        };
        m.insert(key.to_owned(), v);
    }
    m.insert("equalizer".to_owned(), eq.snapshot());
    Value::Object(m)
}

/// Keeps the sound in use as `device`'s.
pub fn store(settings: &mut Settings, eq: &EqController, device: &str) {
    let json = serde_json::to_string(&snapshot(settings, eq)).unwrap_or_default();
    settings.set_bytes(&key_for(device), json.into_bytes());
}

/// Puts `device`'s sound back; false (and nothing changes) when it has none.
pub fn restore(settings: &mut Settings, eq: &mut EqController, device: &str) -> bool {
    let key = key_for(device);
    let raw = settings.bytes(&key).map(<[u8]>::to_vec).unwrap_or_else(|| settings.text(&key, "").into_bytes());
    let Ok(Value::Object(m)) = serde_json::from_slice::<Value>(&raw) else { return false };
    if m.is_empty() {
        return false;
    }
    for (key, kind) in KEYS {
        let Some(v) = m.get(key) else { continue };
        let k = format!("playback/{key}");
        match kind {
            Kind::Int(_) => {
                if let Some(n) = v.as_i64().or_else(|| v.as_f64().map(|f| f.round() as i64)) {
                    settings.set_int(&k, n);
                }
            }
            Kind::Number(_) => {
                if let Some(n) = v.as_f64() {
                    settings.set_number(&k, n);
                }
            }
            Kind::Flag(_) => {
                if let Some(b) = v.as_bool().or_else(|| v.as_f64().map(|f| f != 0.0)) {
                    settings.set_flag(&k, b);
                }
            }
            Kind::Text => {
                if let Some(s) = v.as_str() {
                    settings.set_text(&k, s);
                }
            }
        }
    }
    if let Some(state) = m.get("equalizer") {
        eq.restore(state);
        eq.save_state(settings);
    }
    // On the next turn of the event loop: the caller still holds the
    // settings (and the windows read them). Run at once, Settings › Audio
    // open, choosing an output with a profile panicked (RefCell already
    // mutably borrowed). Without an event loop (the tests) nobody listens.
    let _ = slint::invoke_from_event_loop(|| ON_RESTORED.with(|l| l.borrow().iter().for_each(|f| f())));
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_keep_the_settings_groups_apart() {
        assert_eq!(key_for(""), "soundProfiles/default");
        assert_eq!(key_for("alsa|hw:CARD=DX1,DEV=0"), "soundProfiles/alsa|hw:CARD=DX1,DEV=0");
        assert_eq!(key_for("pipewire|a/b\\c"), "soundProfiles/pipewire|a|b|c");
    }

    #[test]
    fn a_profile_survives_the_settings_file() {
        let dir = std::env::temp_dir().join(format!("sonkore-profiles-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let conf = dir.join("sonkore.conf");
        let dac = "alsa|hw:CARD=DX1,DEV=0";
        {
            let mut s = Settings::open(&conf);
            let mut eq = EqController::new(sonkore_eq::SuperEq::new(), &s, &dir);
            s.set_int("playback/crossfeed", 2);
            s.set_number("playback/headroomDb", -3.5);
            s.set_text("playback/convolutionFile", "/m/a, b=c.wav");
            eq.set_enabled(true);
            store(&mut s, &eq, dac);
            s.save().unwrap();
        }
        let mut s = Settings::open(&conf);
        let mut eq = EqController::new(sonkore_eq::SuperEq::new(), &s, &dir);
        s.set_int("playback/crossfeed", 0);
        s.set_number("playback/headroomDb", 0.0);
        s.set_text("playback/convolutionFile", "");
        eq.set_enabled(false);
        assert!(restore(&mut s, &mut eq, dac));
        assert_eq!(s.int("playback/crossfeed", 0), 2);
        assert_eq!(s.number("playback/headroomDb", 0.0), -3.5);
        assert_eq!(s.text("playback/convolutionFile", ""), "/m/a, b=c.wav");
        assert!(eq.enabled());
        // An output never seen keeps the sound in use.
        assert!(!restore(&mut s, &mut eq, "pipewire|other"));
        assert_eq!(s.int("playback/crossfeed", 0), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
