//! The equaliser as the interface sees it: a 10-band graphic or
//! up-to-32-band parametric curve with a preamp, its response, built-in and
//! saved profiles,
//! Equalizer APO import / export, AutoEQ profiles, and the `equalizer/`
//! settings, all kept compatible with the files earlier versions wrote.
//!
//! Every change is pushed to the engine's [`SuperEq`] at once. The settings
//! are written into a [`Settings`] by [`EqController::save_state`] (they
//! are due after every change: call it after each action, or when
//! [`EqController::state_dirty`] says so); saved profiles go to
//! `eq-profiles.json` immediately.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value, json};
use sonkore_core::settings::Settings;

use crate::apo;
use crate::autoeq::{self, DownloadError, Entry, Index, SearchResult};
use crate::biquad::{self, Coeffs, FilterType};
use crate::numfmt;
use crate::super_eq::{self, Curve, GRAPHIC_CENTERS, GRAPHIC_Q, LOG10_20, LOG10_20000, MAX_BANDS, SuperEq};

/// The rate the response curve is drawn at.
const RESPONSE_RATE: f64 = 48000.0;
/// Points of the response curve.
pub const RESPONSE_POINTS: usize = 192;
/// The profiles file in the data folder.
pub const PROFILES_FILE: &str = "eq-profiles.json";

pub const PREAMP_RANGE: (f64, f64) = (-30.0, 12.0);
pub const GAIN_RANGE: (f64, f64) = (-24.0, 24.0);
pub const FREQ_RANGE: (f64, f64) = (10.0, 24000.0);
pub const Q_RANGE: (f64, f64) = (0.05, 30.0);

/// The filter types the interface offers.
pub const FILTER_TYPES: [&str; 7] = ["PK", "LSC", "HSC", "LPQ", "HPQ", "BP", "NO"];

/// The built-in profiles: ten graphic gains each.
pub const BUILTIN_PROFILES: [(&str, [f64; 10]); 13] = [
    ("Flat", [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
    ("Bass Boost", [6.0, 5.0, 4.0, 2.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0]),
    ("Bass Reducer", [-6.0, -5.0, -4.0, -2.0, -0.5, 0.0, 0.0, 0.0, 0.0, 0.0]),
    ("Treble Boost", [0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 2.0, 4.0, 5.0, 6.0]),
    ("Treble Reducer", [0.0, 0.0, 0.0, 0.0, 0.0, -1.0, -2.0, -4.0, -5.0, -6.0]),
    ("Vocal", [-2.0, -2.0, -1.0, 1.0, 3.0, 3.0, 2.0, 1.0, 0.0, -1.0]),
    ("Rock", [4.0, 3.0, 2.0, 0.0, -1.0, -1.0, 1.0, 2.0, 3.0, 4.0]),
    ("Pop", [-1.0, 1.0, 3.0, 4.0, 3.0, 0.0, -1.0, -1.0, 0.0, 1.0]),
    ("Jazz", [3.0, 2.0, 1.0, 2.0, -1.0, -1.0, 0.0, 1.0, 2.0, 3.0]),
    ("Classical", [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, -3.0, -3.0, -3.0, -5.0]),
    ("Electronic", [4.0, 3.0, 1.0, 0.0, -2.0, 2.0, 1.0, 1.0, 3.0, 4.0]),
    ("Acoustic", [4.0, 4.0, 3.0, 1.0, 2.0, 1.0, 3.0, 3.0, 3.0, 2.0]),
    ("Loudness", [5.0, 4.0, 1.0, 0.0, -1.0, 0.0, 0.0, 1.0, 4.0, 5.0]),
];

/// The filter type of a type name, aliases and any case accepted
/// (LS / LSQ, HS / HSQ, LP, HP, NOTCH); anything else is a peak.
pub fn filter_type(name: &str) -> FilterType {
    match name.to_uppercase().as_str() {
        "LSC" | "LS" | "LSQ" => FilterType::LowShelf,
        "HSC" | "HS" | "HSQ" => FilterType::HighShelf,
        "LPQ" | "LP" => FilterType::LowPass,
        "HPQ" | "HP" => FilterType::HighPass,
        "BP" => FilterType::BandPass,
        "NO" | "NOTCH" => FilterType::Notch,
        _ => FilterType::Peaking,
    }
}

/// "Low shelf", "Peak"… for a type name.
pub fn type_label(name: &str) -> &'static str {
    match filter_type(name) {
        FilterType::LowShelf => "Low shelf",
        FilterType::HighShelf => "High shelf",
        FilterType::LowPass => "Low pass",
        FilterType::HighPass => "High pass",
        FilterType::BandPass => "Band pass",
        FilterType::Notch => "Notch",
        FilterType::Peaking => "Peak",
    }
}

/// "31.25", "500", "1k", "2.5k", "16k".
pub fn frequency_label(hz: f64) -> String {
    if hz >= 1000.0 {
        let k = hz / 1000.0;
        let n = if k % 1.0 == 0.0 { numfmt::number_f(k, 0) } else { numfmt::number_g(k, 3) };
        return n + "k";
    }
    numfmt::number_g(hz, 4)
}

/// Standard frequency of graphic band `index` (31.25 Hz .. 16 kHz), 0 for
/// others.
pub fn graphic_frequency(index: usize) -> f64 {
    GRAPHIC_CENTERS.get(index).copied().unwrap_or(0.0)
}

/// A band as the interface keeps it. `kind` is the type's name as given
/// ("PK", "LSC"…), saved as is.
#[derive(Clone, Debug, PartialEq)]
pub struct Band {
    pub kind: String,
    pub freq: f64,
    pub gain: f64,
    pub q: f64,
    pub enabled: bool,
}

impl Default for Band {
    fn default() -> Self {
        Band { kind: "PK".into(), freq: 1000.0, gain: 0.0, q: GRAPHIC_Q, enabled: true }
    }
}

impl Band {
    pub fn filter_type(&self) -> FilterType {
        filter_type(&self.kind)
    }

    /// `{"enabled", "freq", "gain", "q", "type"}`.
    pub fn to_json(&self) -> Value {
        json!({"type": self.kind, "freq": self.freq, "gain": self.gain, "q": self.q, "enabled": self.enabled})
    }

    /// Within the editor's ranges ([`FREQ_RANGE`], [`GAIN_RANGE`],
    /// [`Q_RANGE`]); a value that is not a finite number takes the
    /// default's. Every band that comes from outside (a file, a profile, the
    /// settings, a snapshot) goes through here: an infinite or NaN value made
    /// NaN coefficients and a NaN output.
    pub fn sanitised(self) -> Band {
        let d = Band::default();
        let fix = |v: f64, default: f64, range: (f64, f64)| {
            biquad::clamp(if v.is_finite() { v } else { default }, range.0, range.1)
        };
        Band {
            freq: fix(self.freq, d.freq, FREQ_RANGE),
            gain: fix(self.gain, d.gain, GAIN_RANGE),
            q: fix(self.q, d.q, Q_RANGE),
            ..self
        }
    }

    /// Missing keys take the defaults (PK, 1 kHz, 0 dB, Q 1.41421356237, on).
    pub fn from_json(v: &Value) -> Band {
        let d = Band::default();
        let Some(m) = v.as_object() else {
            return d;
        };
        Band {
            kind: m.get("type").map_or(d.kind, numfmt::to_string),
            freq: m.get("freq").map_or(d.freq, numfmt::to_double),
            gain: m.get("gain").map_or(d.gain, numfmt::to_double),
            q: m.get("q").map_or(d.q, numfmt::to_double),
            enabled: m.get("enabled").map_or(d.enabled, numfmt::to_bool),
        }
    }
}

fn bands_to_json(bands: &[Band]) -> Value {
    Value::Array(bands.iter().map(Band::to_json).collect())
}

fn bands_from_json(v: Option<&Value>) -> Vec<Band> {
    v.and_then(Value::as_array).map(|l| l.iter().map(Band::from_json).collect()).unwrap_or_default()
}

/// Bands from outside: at most [`MAX_BANDS`], each [`Band::sanitised`].
fn sanitised_bands(mut bands: Vec<Band>) -> Vec<Band> {
    bands.truncate(MAX_BANDS);
    bands.into_iter().map(Band::sanitised).collect()
}

/// A preamp from outside: within [`PREAMP_RANGE`], 0 dB when not a finite number.
fn sanitised_preamp(db: f64) -> f64 {
    if db.is_finite() { biquad::clamp(db, PREAMP_RANGE.0, PREAMP_RANGE.1) } else { 0.0 }
}

/// Graphic (ten fixed bands, gains only) or parametric.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Graphic,
    Parametric,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Graphic => "graphic",
            Mode::Parametric => "parametric",
        }
    }

    /// "graphic" or "parametric"; None otherwise.
    pub fn parse(s: &str) -> Option<Mode> {
        match s {
            "graphic" => Some(Mode::Graphic),
            "parametric" => Some(Mode::Parametric),
            _ => None,
        }
    }

    /// A stored mode: anything but "graphic" behaves as parametric.
    fn from_stored(s: &str) -> Mode {
        Mode::parse(s).unwrap_or(Mode::Parametric)
    }
}

/// A saved profile.
#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    pub name: String,
    pub mode: Mode,
    pub preamp: f64,
    pub bands: Vec<Band>,
}

/// The ten graphic bands, at `gains` (missing ones 0 dB).
fn graphic_layout(gains: &[f64]) -> Vec<Band> {
    GRAPHIC_CENTERS
        .iter()
        .enumerate()
        .map(|(i, &f)| Band { freq: f, gain: gains.get(i).copied().unwrap_or(0.0), ..Band::default() })
        .collect()
}

fn sections_db(sections: &[Coeffs], hz: f64) -> f64 {
    let z1 = biquad::Complex::polar1(-2.0 * std::f64::consts::PI * hz / RESPONSE_RATE);
    let z2 = z1.mul(z1);
    let mut db = 0.0;
    for c in sections {
        db += 20.0 * biquad::max(c.response_at(z1, z2).abs(), 1e-9).log10();
    }
    db
}

/// A point of the response curve.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResponsePoint {
    pub f: f64,
    pub db: f64,
}

/// Why an import failed.
#[derive(Debug)]
pub enum ImportError {
    Io(io::Error),
    /// "No parametric EQ filters found in that file".
    NoFilters,
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportError::Io(e) => e.fmt(f),
            ImportError::NoFilters => f.write_str("No parametric EQ filters found in that file"),
        }
    }
}

impl std::error::Error for ImportError {}

/// A successful import: "Imported <count> filters", saved as `profile`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Imported {
    pub count: usize,
    pub profile: String,
}

/// What the interface should do after [`EqController::autoeq_search`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AutoEqNext {
    /// The results are ready ([`EqController::autoeq_results`]).
    Results,
    /// The index has to be downloaded first: run
    /// [`autoeq::download_index`] off the UI thread and hand the result to
    /// [`EqController::autoeq_index_downloaded`] (which then searches).
    DownloadIndex,
    /// A download is already running.
    Busy,
}

pub struct EqController {
    eq: SuperEq,
    data_dir: PathBuf,
    enabled: bool,
    linear_phase: bool,
    mode: Mode,
    preamp: f64,
    bands: Vec<Band>,
    /// What the response curve is drawn from (the bands as the engine runs
    /// them, at 48 kHz).
    sections: Vec<Coeffs>,
    current_profile: String,
    user_profiles: Vec<Profile>,
    state_dirty: bool,

    autoeq_index: Index,
    autoeq_matches: Vec<usize>,
    autoeq_results: Vec<SearchResult>,
    pending_search: String,
    autoeq_busy: bool,
    autoeq_status: String,
}

impl EqController {
    /// The controller for the engine's `eq`, its state read from `settings`
    /// (`equalizer/…`) and its profiles from `data_dir`/eq-profiles.json;
    /// the curve is pushed to the engine.
    pub fn new(eq: SuperEq, settings: &Settings, data_dir: impl Into<PathBuf>) -> Self {
        let mut c = EqController {
            eq,
            data_dir: data_dir.into(),
            enabled: true,
            linear_phase: false,
            mode: Mode::Graphic,
            preamp: 0.0,
            bands: graphic_layout(&[]),
            sections: Vec::new(),
            current_profile: String::new(),
            user_profiles: Vec::new(),
            state_dirty: false,
            autoeq_index: Index::default(),
            autoeq_matches: Vec::new(),
            autoeq_results: Vec::new(),
            pending_search: String::new(),
            autoeq_busy: false,
            autoeq_status: String::new(),
        };
        c.load_user_profiles();
        c.load_state(settings);
        c.apply(false);
        c.state_dirty = false;
        c
    }

    /// With the default data folder (sonkore_core::paths::data_dir()).
    pub fn with_default_paths(eq: SuperEq, settings: &Settings) -> Self {
        Self::new(eq, settings, sonkore_core::paths::data_dir())
    }

    pub fn equalizer(&self) -> &SuperEq {
        &self.eq
    }

    // --- Properties ------------------------------------------------------------

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn linear_phase(&self) -> bool {
        self.linear_phase
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn preamp(&self) -> f64 {
        self.preamp
    }

    pub fn bands(&self) -> &[Band] {
        &self.bands
    }

    pub fn max_bands(&self) -> usize {
        MAX_BANDS
    }

    pub fn current_profile(&self) -> &str {
        &self.current_profile
    }

    pub fn builtin_profiles(&self) -> Vec<&'static str> {
        BUILTIN_PROFILES.iter().map(|(n, _)| *n).collect()
    }

    pub fn user_profiles(&self) -> Vec<&str> {
        self.user_profiles.iter().map(|p| p.name.as_str()).collect()
    }

    pub fn user_profile(&self, name: &str) -> Option<&Profile> {
        self.user_profiles.iter().find(|p| p.name == name)
    }

    /// The settings changed since the last [`save_state`](Self::save_state).
    pub fn state_dirty(&self) -> bool {
        self.state_dirty
    }

    // --- Switches and bands -------------------------------------------------------

    pub fn set_enabled(&mut self, on: bool) {
        if on == self.enabled {
            return;
        }
        self.enabled = on;
        self.eq.set_enabled(on);
        self.state_dirty = true;
    }

    pub fn set_linear_phase(&mut self, on: bool) {
        if on == self.linear_phase {
            return;
        }
        self.linear_phase = on;
        self.eq.set_linear_phase(on);
        self.state_dirty = true;
    }

    /// Switching to graphic approximates the current curve on the ten
    /// graphic bands (its response at their centres, preamp aside, rounded
    /// to 0.1 dB).
    pub fn set_mode(&mut self, mode: Mode) {
        if mode == self.mode {
            return;
        }
        if mode == Mode::Graphic {
            let gains: Vec<f64> =
                GRAPHIC_CENTERS.iter().map(|&f| ((self.response_at(f) - self.preamp) * 10.0).round() / 10.0).collect();
            self.bands = graphic_layout(&gains);
        }
        self.mode = mode;
        self.apply(true);
    }

    /// Clamped to -30..12 dB.
    pub fn set_preamp(&mut self, db: f64) {
        let db = biquad::clamp(db, PREAMP_RANGE.0, PREAMP_RANGE.1);
        if db == self.preamp {
            return;
        }
        self.preamp = db;
        self.apply(true);
    }

    /// Clamped to ±24 dB.
    pub fn set_gain(&mut self, index: usize, db: f64) {
        let db = biquad::clamp(db, GAIN_RANGE.0, GAIN_RANGE.1);
        let Some(b) = self.bands.get_mut(index) else {
            return;
        };
        if b.gain == db {
            return;
        }
        b.gain = db;
        self.apply(true);
    }

    /// Frequency clamped to 10..24000 Hz, gain to ±24 dB, Q to 0.05..30.
    pub fn set_band(&mut self, index: usize, kind: &str, freq: f64, gain: f64, q: f64, enabled: bool) {
        let Some(b) = self.bands.get_mut(index) else {
            return;
        };
        *b = Band {
            kind: kind.to_owned(),
            freq: biquad::clamp(freq, FREQ_RANGE.0, FREQ_RANGE.1),
            gain: biquad::clamp(gain, GAIN_RANGE.0, GAIN_RANGE.1),
            q: biquad::clamp(q, Q_RANGE.0, Q_RANGE.1),
            enabled,
        };
        self.apply(true);
    }

    /// A 1 kHz peak, Q 1, 0 dB (switches to parametric).
    pub fn add_band(&mut self) {
        if self.bands.len() >= MAX_BANDS {
            return;
        }
        self.mode = Mode::Parametric;
        self.bands.push(Band { q: 1.0, ..Band::default() });
        self.apply(true);
    }

    /// Switches to parametric.
    pub fn remove_band(&mut self, index: usize) {
        if index >= self.bands.len() {
            return;
        }
        self.mode = Mode::Parametric;
        self.bands.remove(index);
        self.apply(true);
    }

    /// Flat graphic, no preamp; the profile becomes "Flat".
    pub fn reset(&mut self) {
        self.mode = Mode::Graphic;
        self.bands = graphic_layout(&[]);
        self.preamp = 0.0;
        self.apply(false);
        self.set_current_profile("Flat");
    }

    /// The preamp that keeps the curve's peak at 0 dB: -ceil(peak·10)/10,
    /// or 0 when the curve never goes above 0 dB.
    pub fn auto_preamp(&mut self) {
        let peak = self.peak_gain() - self.preamp; // curve without the current preamp
        self.preamp = if peak > 0.0 { -(peak * 10.0).ceil() / 10.0 } else { 0.0 };
        self.apply(true);
    }

    // --- Response --------------------------------------------------------------------

    /// The bands the engine runs: in graphic mode each gain is solved so
    /// the cascade passes through the slider values at the band centres
    /// (neighbouring one-octave bands overlap: ten sliders at +6 dB would
    /// otherwise give +8.8 dB), rounded to 0.001 dB.
    pub fn engine_bands(&self) -> Vec<Band> {
        if self.mode != Mode::Graphic || self.bands.len() != 10 {
            return self.bands.clone();
        }
        let mut sliders = [0.0; 10];
        for (s, b) in sliders.iter_mut().zip(&self.bands) {
            *s = if b.enabled { b.gain } else { 0.0 };
        }
        let gains = super_eq::graphic_gains(&sliders);
        let mut bands = self.bands.clone();
        for (b, g) in bands.iter_mut().zip(gains) {
            b.gain = (g * 1000.0).round() / 1000.0;
        }
        bands
    }

    fn update_sections(&mut self) {
        self.sections = self
            .engine_bands()
            .iter()
            .filter(|b| b.enabled)
            .map(|b| biquad::design(b.filter_type(), RESPONSE_RATE, b.freq, b.q, b.gain))
            .collect();
    }

    /// The curve's gain (dB, preamp included) at `hz`, at 48 kHz.
    pub fn response_at(&self, hz: f64) -> f64 {
        self.preamp + sections_db(&self.sections, hz)
    }

    /// 192 points from 20 Hz to 20 kHz, log spaced.
    pub fn response(&self) -> Vec<ResponsePoint> {
        (0..RESPONSE_POINTS)
            .map(|i| {
                let f = biquad::pow10(LOG10_20 + (LOG10_20000 - LOG10_20) * i as f64 / (RESPONSE_POINTS - 1) as f64);
                ResponsePoint { f, db: self.response_at(f) }
            })
            .collect()
    }

    /// The curve's highest gain over 400 log points, 20 Hz - 20 kHz.
    pub fn peak_gain(&self) -> f64 {
        let mut peak = -1e9;
        for i in 0..400 {
            let f = biquad::pow10(LOG10_20 + (LOG10_20000 - LOG10_20) * f64::from(i) / 399.0);
            peak = biquad::max(peak, self.response_at(f));
        }
        peak
    }

    // --- Engine sync -------------------------------------------------------------------

    fn apply(&mut self, mark_custom: bool) {
        let bands = self
            .engine_bands()
            .iter()
            .map(|b| super_eq::Band {
                kind: b.filter_type(),
                freq_hz: b.freq,
                gain_db: b.gain,
                q: b.q,
                enabled: b.enabled,
            })
            .collect();
        self.eq.set_curve(Curve { bands, preamp_db: self.preamp });
        self.eq.set_enabled(self.enabled);
        self.update_sections();
        self.eq.set_linear_phase(self.linear_phase);
        if mark_custom {
            self.set_current_profile("");
        }
        self.state_dirty = true;
    }

    fn set_current_profile(&mut self, name: &str) {
        if name == self.current_profile {
            return;
        }
        self.current_profile = name.to_owned();
        self.state_dirty = true;
    }

    /// The whole equaliser state (on/off, phase, mode, preamp, profile,
    /// bands) as one JSON object: the sound profile of an output device.
    pub fn snapshot(&self) -> Value {
        json!({
            "enabled": self.enabled,
            "linearPhase": self.linear_phase,
            "mode": self.mode.as_str(),
            "preamp": self.preamp,
            "profile": self.current_profile,
            "bands": bands_to_json(&self.bands),
        })
    }

    /// Back from a [`snapshot`](Self::snapshot): missing keys keep the
    /// current values; an empty object changes nothing.
    pub fn restore(&mut self, state: &Value) {
        let Some(m) = state.as_object().filter(|m| !m.is_empty()) else {
            return;
        };
        self.enabled = m.get("enabled").map_or(self.enabled, numfmt::to_bool);
        self.linear_phase = m.get("linearPhase").map_or(self.linear_phase, numfmt::to_bool);
        if let Some(mode) = m.get("mode") {
            self.mode = Mode::from_stored(&numfmt::to_string(mode));
        }
        self.preamp = sanitised_preamp(m.get("preamp").map_or(self.preamp, numfmt::to_double));
        let bands = bands_from_json(m.get("bands"));
        if !bands.is_empty() {
            self.bands = sanitised_bands(bands);
        }
        self.apply(false);
        let profile = m.get("profile").map(numfmt::to_string).unwrap_or_default();
        self.set_current_profile(&profile);
    }

    // --- Profiles ------------------------------------------------------------------------

    /// A built-in profile (with the preamp that keeps it from clipping) or a
    /// saved one; unknown names change nothing.
    pub fn load_profile(&mut self, name: &str) {
        if let Some((_, gains)) = BUILTIN_PROFILES.iter().find(|(n, _)| *n == name) {
            self.mode = Mode::Graphic;
            self.bands = graphic_layout(gains);
            self.preamp = 0.0;
            // Headroom against clipping, from this profile's own curve (not
            // measured before redrawing the curve, i.e. on the previous
            // profile's).
            self.update_sections();
            let peak = self.peak_gain();
            if peak > 0.0 {
                self.preamp = -(peak * 10.0).ceil() / 10.0;
            }
            self.apply(false);
            self.set_current_profile(name);
            return;
        }
        if let Some(p) = self.user_profiles.iter().find(|p| p.name == name).cloned() {
            self.mode = p.mode;
            self.preamp = p.preamp;
            self.bands = p.bands;
            self.apply(false);
            self.set_current_profile(name);
        }
    }

    /// Saves the current curve as `wanted` (trimmed; "My profile" when
    /// empty; " (custom)" appended to a built-in's name), replacing a saved
    /// profile of that name. Returns the name used.
    pub fn save_profile(&mut self, wanted: &str) -> String {
        let mut name = wanted.trim().to_owned();
        if name.is_empty() {
            name = "My profile".into();
        }
        if BUILTIN_PROFILES.iter().any(|(n, _)| *n == name) {
            name += " (custom)";
        }
        let p = Profile { name: name.clone(), mode: self.mode, preamp: self.preamp, bands: self.bands.clone() };
        let mut replaced = false;
        for existing in self.user_profiles.iter_mut().filter(|e| e.name == name) {
            *existing = p.clone();
            replaced = true;
        }
        if !replaced {
            self.user_profiles.push(p);
        }
        let _ = self.save_user_profiles();
        self.set_current_profile(&name);
        name
    }

    pub fn delete_profile(&mut self, name: &str) {
        let Some(i) = self.user_profiles.iter().position(|p| p.name == name) else {
            return;
        };
        self.user_profiles.remove(i);
        let _ = self.save_user_profiles();
        if self.current_profile == name {
            self.set_current_profile("");
        }
    }

    /// Imports an Equalizer APO / AutoEQ text file as a parametric curve,
    /// saved as a profile named after the file.
    pub fn import_file(&mut self, path: &Path) -> Result<Imported, ImportError> {
        let bytes = fs::read(path).map_err(ImportError::Io)?;
        let (preamp, bands) = apo::parse(&String::from_utf8_lossy(&bytes)).ok_or(ImportError::NoFilters)?;
        let count = bands.len();
        self.mode = Mode::Parametric;
        self.preamp = sanitised_preamp(preamp);
        self.bands = sanitised_bands(bands);
        self.apply(false);
        let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        // The base name: up to the last dot.
        let base = file_name.rfind('.').map_or(file_name.as_str(), |i| &file_name[..i]);
        let profile = self.save_profile(base);
        Ok(Imported { count, profile })
    }

    /// The current curve as Equalizer APO text.
    pub fn export_text(&self) -> String {
        apo::export(self.preamp, &self.bands)
    }

    /// Writes [`export_text`](Self::export_text) to `path`. Inside the
    /// Flatpak document portal, a name typed without its extension is
    /// granted without it: when `…/doc/<id>/<name>.<ext>` cannot be created,
    /// `…/doc/<id>/<name>` is tried.
    pub fn export_file(&self, path: &Path) -> io::Result<()> {
        static SUFFIXED: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"^(/run/(?:user/[0-9]+|flatpak)/doc/[^/]+/[^/]+)\.[^./]+$").unwrap());
        let text = self.export_text();
        match fs::write(path, &text) {
            Ok(()) => Ok(()),
            Err(e) => {
                let p = path.to_string_lossy();
                match SUFFIXED.captures(&p) {
                    Some(m) => fs::write(&m[1], &text),
                    None => Err(e),
                }
            }
        }
    }

    // --- AutoEQ ----------------------------------------------------------------------------

    pub fn autoeq_results(&self) -> &[SearchResult] {
        &self.autoeq_results
    }

    pub fn autoeq_busy(&self) -> bool {
        self.autoeq_busy
    }

    pub fn autoeq_status(&self) -> &str {
        &self.autoeq_status
    }

    fn set_autoeq_state(&mut self, busy: bool, status: String) {
        self.autoeq_busy = busy;
        self.autoeq_status = status;
    }

    fn index_loaded_status(&mut self) {
        let status = format!("{} headphone profiles available", self.autoeq_index.len());
        self.set_autoeq_state(false, status);
    }

    /// Searches the index (loading it from a cache younger than 14 days the
    /// first time). [`AutoEqNext::DownloadIndex`]: the index must be
    /// downloaded first (the search runs once it arrives).
    pub fn autoeq_search(&mut self, text: &str) -> AutoEqNext {
        self.pending_search = text.to_owned();
        if self.autoeq_index.is_empty() {
            match autoeq::read_fresh_cache(&self.data_dir) {
                Some(markdown) => {
                    self.autoeq_index = Index::parse(&markdown);
                    self.index_loaded_status();
                }
                None => return self.autoeq_refresh_index(),
            }
        }
        self.autoeq_matches = self.autoeq_index.search(text);
        self.autoeq_results = self
            .autoeq_matches
            .iter()
            .map(|&i| {
                let e = &self.autoeq_index.entries()[i];
                SearchResult { name: e.name.clone(), source: e.source.clone(), rig: e.rig.clone() }
            })
            .collect();
        AutoEqNext::Results
    }

    /// Asks for a fresh index: [`AutoEqNext::DownloadIndex`] (busy, "Downloading
    /// AutoEQ database…"), or [`AutoEqNext::Busy`].
    pub fn autoeq_refresh_index(&mut self) -> AutoEqNext {
        if self.autoeq_busy {
            return AutoEqNext::Busy;
        }
        self.set_autoeq_state(true, "Downloading AutoEQ database…".into());
        AutoEqNext::DownloadIndex
    }

    /// The index download's outcome: parsed, cached and searched with the
    /// last search text.
    pub fn autoeq_index_downloaded(&mut self, result: Result<String, DownloadError>) {
        match result {
            Err(e) => self.set_autoeq_state(false, format!("Could not reach AutoEQ: {e}")),
            Ok(markdown) => {
                self.autoeq_index = Index::parse(&markdown);
                let _ = autoeq::write_cache(&self.data_dir, &markdown);
                self.index_loaded_status();
                let text = self.pending_search.clone();
                self.autoeq_search(&text);
            }
        }
    }

    /// Starts applying search result `index`: the entry whose profile to
    /// download ([`autoeq::download_profile`], off the UI thread) and hand
    /// to [`autoeq_profile_downloaded`](Self::autoeq_profile_downloaded).
    /// None when there is no such result or a download is running.
    pub fn autoeq_apply(&mut self, index: usize) -> Option<Entry> {
        if self.autoeq_busy {
            return None;
        }
        let entry = self.autoeq_index.entries().get(*self.autoeq_matches.get(index)?)?.clone();
        self.set_autoeq_state(true, format!("Downloading {}…", entry.name));
        Some(entry)
    }

    /// The profile download's outcome: applied as a parametric curve (the
    /// EQ switched on) and saved as "AutoEQ · <name> (<source>)".
    pub fn autoeq_profile_downloaded(&mut self, entry: &Entry, result: Result<String, DownloadError>) {
        let text = match result {
            Ok(t) => t,
            Err(e) => return self.set_autoeq_state(false, format!("Download failed: {e}")),
        };
        let Some((preamp, bands)) = apo::parse(&text) else {
            return self.set_autoeq_state(false, format!("No parametric profile published for {}", entry.name));
        };
        self.mode = Mode::Parametric;
        self.preamp = sanitised_preamp(preamp);
        self.bands = sanitised_bands(bands);
        self.enabled = true;
        self.apply(false);
        let saved = self.save_profile(&format!("AutoEQ · {} ({})", entry.name, entry.source));
        self.set_autoeq_state(false, format!("Applied and saved as “{saved}”"));
    }

    // --- Persistence -------------------------------------------------------------------

    fn load_state(&mut self, s: &Settings) {
        self.enabled = s.flag("equalizer/enabled", false); // a fresh install starts bit-perfect
        self.linear_phase = s.flag("equalizer/linearPhase", false);
        self.mode = Mode::from_stored(&s.text("equalizer/mode", "graphic"));
        self.preamp = sanitised_preamp(s.number("equalizer/preamp", 0.0));
        self.current_profile = s.text("equalizer/profile", "Flat");
        let json = s.value("equalizer/bands").map(|v| match v.as_bytes() {
            Some(b) => b.to_vec(),
            None => v.as_text().into_bytes(),
        });
        let bands = json.and_then(|b| serde_json::from_slice::<Value>(&b).ok());
        let bands = bands_from_json(bands.as_ref());
        if !bands.is_empty() {
            self.bands = sanitised_bands(bands);
        }
    }

    /// Writes the `equalizer/` keys into `s` (enabled, linearPhase, mode,
    /// preamp, profile, and bands as a compact JSON array in a byte array).
    /// The file itself is written by `s.save()`.
    pub fn save_state(&mut self, s: &mut Settings) {
        s.set_flag("equalizer/enabled", self.enabled);
        s.set_flag("equalizer/linearPhase", self.linear_phase);
        s.set_text("equalizer/mode", self.mode.as_str());
        s.set_number("equalizer/preamp", self.preamp);
        s.set_text("equalizer/profile", self.current_profile.clone());
        s.set_bytes("equalizer/bands", numfmt::to_json(&bands_to_json(&self.bands), true).into_bytes());
        self.state_dirty = false;
    }

    fn profiles_path(&self) -> PathBuf {
        self.data_dir.join(PROFILES_FILE)
    }

    fn load_user_profiles(&mut self) {
        let Ok(bytes) = fs::read(self.profiles_path()) else {
            return;
        };
        let Ok(Value::Array(list)) = serde_json::from_slice::<Value>(&bytes) else {
            return;
        };
        let empty = Map::new();
        for v in &list {
            let m = v.as_object().unwrap_or(&empty);
            let p = Profile {
                name: m.get("name").map(numfmt::to_string).unwrap_or_default(),
                mode: Mode::from_stored(&m.get("mode").map_or("parametric".into(), numfmt::to_string)),
                preamp: sanitised_preamp(m.get("preamp").map_or(0.0, numfmt::to_double)),
                bands: sanitised_bands(bands_from_json(m.get("bands"))),
            };
            if !p.name.is_empty() {
                self.user_profiles.push(p);
            }
        }
    }

    /// eq-profiles.json: `[{name, mode, preamp, bands}]`, indented.
    fn save_user_profiles(&self) -> io::Result<()> {
        let list: Vec<Value> = self
            .user_profiles
            .iter()
            .map(|p| json!({"name": p.name, "mode": p.mode.as_str(), "preamp": p.preamp, "bands": bands_to_json(&p.bands)}))
            .collect();
        fs::create_dir_all(&self.data_dir)?;
        fs::write(self.profiles_path(), numfmt::to_json(&Value::Array(list), false))
    }
}
