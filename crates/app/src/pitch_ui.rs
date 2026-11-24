//! The Pitch & tempo window.
//!
//! [`PitchSettings`] is what is kept for SoundTouch, read from
//! and written to the same settings keys (`pitch/…`, `soundtouch/…`). The
//! engine is told through [`PitchUi::on_change`] after every change.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use sonkore_core::settings::Settings;

use crate::i18n::{tr, trf};
use crate::{PitchUi as PitchGlobal, PitchWindow};

const CTX: &str = "EditSoundtouch";
/// Decimals of the precise fields.
const PRECISION: usize = 7;
/// The − / + steps offered.
const STEPS: [f64; 7] = [0.000001, 0.00001, 0.0001, 0.001, 0.01, 0.1, 1.0];
const SEMITONES: (f64, f64) = (-48.0, 48.0);
const RATIO: (f64, f64) = (0.05, 20.0);

/// SoundTouch as the engine runs it (TimeStretch + its SoundTouchSettings).
#[derive(Clone, Debug, PartialEq)]
pub struct PitchSettings {
    pub enabled: bool,
    pub semitones: f64,
    pub tempo: f64,
    pub rate: f64,
    pub aa_filter: bool,
    /// 8..128, a multiple of 8.
    pub aa_length: i32,
    pub quick_seek: bool,
    /// 0 = automatic.
    pub sequence_ms: i32,
    /// 0 = automatic.
    pub seek_window_ms: i32,
    pub overlap_ms: i32,
}

impl Default for PitchSettings {
    fn default() -> Self {
        PitchSettings {
            enabled: false, // a fresh install starts bit-perfect
            semitones: 0.0,
            tempo: 1.0,
            rate: 1.0,
            aa_filter: true,
            aa_length: 64,
            quick_seek: false,
            sequence_ms: 0,
            seek_window_ms: 0,
            overlap_ms: 8,
        }
    }
}

/// The 'g' format at 17 significant digits: the exact double comes back from the text.
fn exact_text(v: f64) -> String {
    if v == 0.0 {
        return "0".into();
    }
    let e = v.abs().log10().floor() as i32;
    if (-4..17).contains(&e) {
        let decimals = (16 - e).max(0) as usize;
        let s = format!("{v:.decimals$}");
        let s = if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_owned() } else { s };
        return s;
    }
    let s = format!("{v:.16e}");
    let (m, x) = s.split_once('e').unwrap_or((&s, "0"));
    let m = if m.contains('.') { m.trim_end_matches('0').trim_end_matches('.') } else { m };
    let x: i32 = x.parse().unwrap_or(0);
    format!("{m}e{}{:02}", if x < 0 { '-' } else { '+' }, x.abs())
}

/// The stored text (or number) read as a double, else the fallback.
fn exact_value(s: &Settings, key: &str, fallback: f64) -> f64 {
    s.value(key).and_then(|v| v.as_text().trim().parse::<f64>().ok()).filter(|v| v.is_finite()).unwrap_or(fallback)
}

impl PitchSettings {
    /// The `soundtouch/` group first, the
    /// ratios from `pitch/` (older versions kept them under `soundtouch/`).
    pub fn load(s: &Settings) -> PitchSettings {
        let d = PitchSettings::default();
        let legacy = |k: &str, fallback: f64| exact_value(s, &format!("soundtouch/{k}"), fallback);
        let mut p = PitchSettings {
            aa_filter: s.flag("soundtouch/aaFilter", d.aa_filter),
            aa_length: s.int("soundtouch/aaFilterLength", d.aa_length as i64) as i32,
            quick_seek: s.flag("soundtouch/quickSeek", d.quick_seek),
            sequence_ms: s.int("soundtouch/sequenceMs", d.sequence_ms as i64) as i32,
            seek_window_ms: s.int("soundtouch/seekWindowMs", d.seek_window_ms as i64) as i32,
            overlap_ms: s.int("soundtouch/overlapMs", d.overlap_ms as i64) as i32,
            semitones: exact_value(s, "pitch/pitch", legacy("pitch", 0.0)),
            tempo: exact_value(s, "pitch/tempo", legacy("tempo", 1.0)),
            rate: exact_value(s, "pitch/rate", legacy("rate", 1.0)),
            enabled: s.flag("pitch/enabled", s.flag("soundtouch/enabled", false)),
        };
        p.clamp();
        p
    }

    /// Saved as doubles with 17 significant digits.
    pub fn save(&self, s: &mut Settings) {
        for k in ["pitch", "tempo", "rate", "enabled"] {
            s.remove(&format!("soundtouch/{k}"));
        }
        s.set_text("pitch/pitch", exact_text(self.semitones));
        s.set_text("pitch/tempo", exact_text(self.tempo));
        s.set_text("pitch/rate", exact_text(self.rate));
        s.set_flag("pitch/enabled", self.enabled);
        s.set_flag("soundtouch/aaFilter", self.aa_filter);
        s.set_int("soundtouch/aaFilterLength", self.aa_length as i64);
        s.set_flag("soundtouch/quickSeek", self.quick_seek);
        s.set_int("soundtouch/sequenceMs", self.sequence_ms as i64);
        s.set_int("soundtouch/seekWindowMs", self.seek_window_ms as i64);
        s.set_int("soundtouch/overlapMs", self.overlap_ms as i64);
    }

    /// TimeStretch's ranges.
    fn clamp(&mut self) {
        self.semitones = self.semitones.clamp(SEMITONES.0, SEMITONES.1);
        self.tempo = self.tempo.clamp(RATIO.0, RATIO.1);
        self.rate = self.rate.clamp(RATIO.0, RATIO.1);
        self.aa_length = ((self.aa_length + 4) / 8 * 8).clamp(8, 128);
        self.sequence_ms = self.sequence_ms.clamp(0, 500);
        self.seek_window_ms = self.seek_window_ms.clamp(0, 250);
        self.overlap_ms = self.overlap_ms.clamp(1, 100);
    }

    /// Pitch, tempo and rate neutral, the advanced settings at their
    /// defaults (on / off kept).
    pub fn reset(&mut self) {
        *self = PitchSettings { enabled: self.enabled, ..PitchSettings::default() };
    }
}

/// Fixed decimals, never "-0.000…".
pub fn format_decimal(value: f64, decimals: usize) -> String {
    let s = format!("{value:.prec$}", prec = decimals.min(15));
    match s.strip_prefix('-') {
        Some(rest) if rest.parse::<f64>() == Ok(0.0) => rest.to_owned(),
        _ => s,
    }
}

/// A decimal number, with a point or the language's comma.
pub fn parse_decimal(text: &str) -> Option<f64> {
    crate::i18n::parse_number(text)
}

/// One step up or down, through the decimal text
/// the user would see, so the result is the double typing it would give.
pub fn step_value(value: f64, step: f64, direction: i32, decimals: usize, minimum: f64, maximum: f64) -> f64 {
    let raw = value + direction as f64 * step;
    let snapped: f64 = format!("{raw:.prec$}", prec = decimals.min(15)).parse().unwrap_or(raw);
    (if snapped == 0.0 { 0.0 } else { snapped }).clamp(minimum, maximum)
}

/// Acceptable input: from..to, 7 decimals, standard notation, C locale.
fn acceptable(text: &str, from: f64, to: f64) -> bool {
    let t = text.trim();
    let body = t.strip_prefix(['-', '+']).unwrap_or(t);
    let (int, frac) = body.split_once('.').unwrap_or((body, ""));
    if int.is_empty() && frac.is_empty() {
        return false;
    }
    if !int.chars().all(|c| c.is_ascii_digit()) || !frac.chars().all(|c| c.is_ascii_digit()) || frac.len() > PRECISION {
        return false;
    }
    parse_decimal(t).is_some_and(|v| v >= from && v <= to)
}

fn range_of(key: &str) -> (f64, f64) {
    if key == "pitch" { SEMITONES } else { RATIO }
}

/// Lighter / darker by a factor (HSV value scaled; past white the
/// saturation gives way).
fn color_lighter(c: slint::Color, factor: f64) -> slint::Color {
    let (r, g, b) = (c.red() as f64 / 255.0, c.green() as f64 / 255.0, c.blue() as f64 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let mut s = if max > 0.0 { delta / max } else { 0.0 };
    let h = if delta == 0.0 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / delta).rem_euclid(6.0))
    } else if max == g {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    let mut v = max * factor;
    if v > 1.0 {
        s = (s - (v - 1.0)).max(0.0);
        v = 1.0;
    }
    let c2 = v * s;
    let x = c2 * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
    let m = v - c2;
    let (r1, g1, b1) = match (h / 60.0) as i32 {
        0 => (c2, x, 0.0),
        1 => (x, c2, 0.0),
        2 => (0.0, c2, x),
        3 => (0.0, x, c2),
        4 => (x, 0.0, c2),
        _ => (c2, 0.0, x),
    };
    let to = |f: f64| ((f + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    slint::Color::from_argb_u8(c.alpha(), to(r1), to(g1), to(b1))
}

pub struct PitchUi {
    pub window: PitchWindow,
    settings: Rc<RefCell<Settings>>,
    state: RefCell<PitchSettings>,
    step: Cell<usize>,
    loaded: RefCell<Option<String>>,
    on_change: RefCell<Option<Box<dyn Fn(&PitchSettings)>>>,
}

impl PitchUi {
    pub fn new(settings: Rc<RefCell<Settings>>) -> Result<Rc<Self>, slint::PlatformError> {
        let window = PitchWindow::new()?;
        crate::equalizer_ui::install_metrics(&window);
        let state = PitchSettings::load(&settings.borrow());
        let ui = Rc::new(PitchUi {
            window,
            settings,
            state: RefCell::new(state),
            step: Cell::new(0),
            loaded: RefCell::new(None),
            on_change: RefCell::new(None),
        });
        let g = ui.window.global::<PitchGlobal>();
        let steps: Vec<SharedString> = STEPS.iter().map(|s| format_decimal(*s, 6).into()).collect();
        g.set_steps(ModelRc::new(VecModel::from(steps)));
        ui.connect();
        crate::equalizer_ui::connect_frame(&ui.window);
        // Not maximizable: minimize and close only.
        ui.window.global::<crate::WindowActions>().set_show_maximize(false);
        ui.refresh();
        Ok(ui)
    }

    /// Called after every change with the new settings (the engine's
    /// TimeStretch is set from them).
    pub fn on_change(&self, f: impl Fn(&PitchSettings) + 'static) {
        *self.on_change.borrow_mut() = Some(Box::new(f));
    }

    /// The line under "SoundTouch": the output's description while a track
    /// is loaded, else "No track loaded".
    pub fn set_output(&self, description: Option<&str>) {
        *self.loaded.borrow_mut() = description.map(str::to_owned);
        self.refresh();
    }

    /// Opens the window 580 wide, never taller than its
    /// content (nor than `available` less 60 px).
    pub fn open(&self, available: f32) -> Result<(), slint::PlatformError> {
        let full = self.window.get_full_height();
        let h = full.min((available - 60.0).max(360.0));
        self.window.window().set_size(slint::LogicalSize::new(580.0, h));
        self.window.show()
    }

    fn change(&self, f: impl FnOnce(&mut PitchSettings)) {
        let before = self.state.borrow().clone();
        {
            let mut s = self.state.borrow_mut();
            f(&mut s);
            s.clamp();
        }
        let now = self.state.borrow().clone();
        if now != before {
            now.save(&mut self.settings.borrow_mut());
            if let Some(f) = &*self.on_change.borrow() {
                f(&now);
            }
        }
        self.refresh();
    }

    fn set_value(&self, key: &str, v: f64) {
        self.change(|s| match key {
            "pitch" => s.semitones = v,
            "tempo" => s.tempo = v,
            _ => s.rate = v,
        });
    }

    fn value(&self, key: &str) -> f64 {
        let s = self.state.borrow();
        match key {
            "pitch" => s.semitones,
            "tempo" => s.tempo,
            _ => s.rate,
        }
    }

    fn connect(self: &Rc<Self>) {
        let g = self.window.global::<PitchGlobal>();
        macro_rules! on {
            ($name:ident, |$ui:ident $(, $arg:ident)*| $body:expr) => {{
                let weak = Rc::downgrade(self);
                g.$name(move |$($arg),*| {
                    if let Some($ui) = weak.upgrade() {
                        $body
                    }
                });
            }};
        }
        on!(on_set_enabled, |ui, v| ui.change(|s| s.enabled = v));
        g.on_valid(|key, text| {
            let (from, to) = range_of(&key);
            acceptable(&text, from, to)
        });
        on!(on_typed, |ui, key, text| {
            let (from, to) = range_of(&key);
            if acceptable(&text, from, to) {
                if let Some(v) = parse_decimal(&text) {
                    ui.set_value(&key, v);
                }
            }
        });
        on!(on_finished, |ui, key, text| {
            let (from, to) = range_of(&key);
            if let Some(v) = parse_decimal(&text).filter(|v| *v >= from && *v <= to) {
                ui.set_value(&key, v);
            } else {
                ui.refresh();
            }
        });
        on!(on_nudge, |ui, key, direction| {
            let (from, to) = range_of(&key);
            let v = step_value(ui.value(&key), STEPS[ui.step.get()], direction, PRECISION, from, to);
            ui.set_value(&key, v);
        });
        on!(on_set_step, |ui, i| {
            ui.step.set((i.max(0) as usize).min(STEPS.len() - 1));
            ui.refresh();
        });
        on!(on_set_aa_filter, |ui, v| ui.change(|s| s.aa_filter = v));
        on!(on_set_aa_length, |ui, v| ui.change(|s| s.aa_length = v));
        on!(on_set_quick_seek, |ui, v| ui.change(|s| s.quick_seek = v));
        on!(on_set_sequence_ms, |ui, v| ui.change(|s| s.sequence_ms = v));
        on!(on_set_seek_window_ms, |ui, v| ui.change(|s| s.seek_window_ms = v));
        on!(on_set_overlap_ms, |ui, v| ui.change(|s| s.overlap_ms = v));
        on!(on_reset, |ui| ui.change(|s| s.reset()));
        on!(on_done, |ui| {
            let _ = ui.window.hide();
        });
    }

    pub fn refresh(&self) {
        let s = self.state.borrow().clone();
        let g = self.window.global::<PitchGlobal>();
        g.set_enabled(s.enabled);
        g.set_subtitle(match &*self.loaded.borrow() {
            Some(d) => d.clone().into(),
            None => tr(CTX, "No track loaded").into(),
        });
        g.set_pitch_text(format_decimal(s.semitones, PRECISION).into());
        g.set_tempo_text(format_decimal(s.tempo, PRECISION).into());
        g.set_rate_text(format_decimal(s.rate, PRECISION).into());
        g.set_pitch_down(s.semitones > SEMITONES.0);
        g.set_pitch_up(s.semitones < SEMITONES.1);
        g.set_tempo_down(s.tempo > RATIO.0);
        g.set_tempo_up(s.tempo < RATIO.1);
        g.set_rate_down(s.rate > RATIO.0);
        g.set_rate_up(s.rate < RATIO.1);
        // Reference A (standard tuning 440 Hz) after the pitch shift.
        let ratio = (s.semitones / 12.0).exp2();
        g.set_pitch_hint(
            trf(
                CTX,
                "A = {0} Hz · ratio {1} · {2} cents",
                &[
                    &format_decimal(440.0 * ratio, 3),
                    &format_decimal(ratio, 12),
                    &format_decimal(s.semitones * 100.0, 5),
                ],
            )
            .into(),
        );
        g.set_tempo_hint(
            trf(CTX, "{0} % speed, pitch preserved", &[&format_decimal((s.tempo - 1.0) * 100.0, 5)]).into(),
        );
        g.set_rate_hint(
            trf(CTX, "{0} % speed and pitch (resampling)", &[&format_decimal((s.rate - 1.0) * 100.0, 5)]).into(),
        );
        g.set_step_index(self.step.get() as i32);
        g.set_step_text(format_decimal(STEPS[self.step.get()], PRECISION).into());
        g.set_aa_filter(s.aa_filter);
        g.set_aa_length(s.aa_length);
        g.set_quick_seek(s.quick_seek);
        g.set_sequence_ms(s.sequence_ms);
        g.set_seek_window_ms(s.seek_window_ms);
        g.set_overlap_ms(s.overlap_ms);
        // The spin box's colours, from the theme's.
        let theme = self.window.global::<crate::Theme>();
        let window = theme.get_bg();
        let outline = color_lighter(window, 1.0 / 1.4);
        let button = color_lighter(theme.get_surface_hi(), 1.24);
        g.set_spin_outline(outline);
        g.set_spin_strip_top(color_lighter(button, 1.24));
        g.set_spin_strip_bottom(color_lighter(button, 1.02));
        g.set_spin_pressed(color_lighter(theme.get_surface_hi(), 1.0 / 1.1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimals_as_stored() {
        assert_eq!(format_decimal(-0.00000001, 7), "0.0000000");
        assert_eq!(format_decimal(-0.317666, 7), "-0.3176660");
        assert_eq!(step_value(1.0, 0.000001, 1, 7, 0.05, 20.0), 1.000001);
        assert_eq!(exact_text(0.1), "0.10000000000000001");
        assert_eq!(exact_text(1.25), "1.25");
        assert_eq!(exact_text(-0.317666).parse::<f64>(), Ok(-0.317666));
        assert_eq!(exact_text(1e-7).parse::<f64>(), Ok(1e-7));
        assert!(acceptable("-0.3", -48.0, 48.0));
        assert!(!acceptable("-", -48.0, 48.0));
        assert!(!acceptable("1.00000001", 0.05, 20.0));
        assert!(!acceptable("30", 0.05, 20.0));
    }
}
