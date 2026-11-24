//! The Equalizer window.
//!
//! The window talks to the equaliser through [`EqModel`]: what it shows
//! (bands, mode, preamp, response, profiles, AutoEQ) and what it changes.
//! `equalizer_model::EqAdapter` implements it over `sonkore_eq::EqController`;
//! any other implementation works the same way.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use crate::i18n::{tr, trf};
use crate::{AutoEqRow, EqBand, EqMark, EqMenuEntry, EqUi, EqualizerWindow, WindowActions};

const CTX: &str = "EqualizerPage";

/// A band as the equaliser keeps it.
#[derive(Clone, Debug, PartialEq)]
pub struct BandValue {
    /// "PK", "LSC", "HSC", "LPQ", "HPQ", "BP", "NO".
    pub kind: String,
    pub freq: f64,
    pub gain: f64,
    pub q: f64,
    pub enabled: bool,
}

/// An AutoEQ search result.
#[derive(Clone, Debug, PartialEq)]
pub struct AutoEqResult {
    pub name: String,
    pub source: String,
    pub rig: String,
}

/// What the Equalizer window needs of the equaliser (EqController's
/// properties and actions, as the page uses them). Every setter is
/// followed by a redraw of the window.
pub trait EqModel {
    fn enabled(&self) -> bool;
    fn set_enabled(&mut self, on: bool);
    fn linear_phase(&self) -> bool;
    fn set_linear_phase(&mut self, on: bool);
    /// Graphic (ten fixed bands) or parametric.
    fn graphic(&self) -> bool;
    fn set_graphic(&mut self, graphic: bool);
    fn preamp(&self) -> f64;
    fn set_preamp(&mut self, db: f64);
    fn auto_preamp(&mut self);
    /// The curve's highest gain, preamp included (the clip warning).
    fn peak_gain(&self) -> f64;
    fn bands(&self) -> Vec<BandValue>;
    fn max_bands(&self) -> usize;
    /// The filter types offered, as type names ("PK", "LSC"…).
    fn filter_types(&self) -> Vec<String>;
    /// "Peak", "Low shelf"… (untranslated; the window translates it).
    fn type_label(&self, kind: &str) -> String;
    /// "31.25", "1k", "2.5k".
    fn frequency_label(&self, hz: f64) -> String;
    /// Standard frequency of graphic band `index` (0 for others).
    fn graphic_frequency(&self, index: usize) -> f64;
    fn set_gain(&mut self, index: usize, db: f64);
    fn set_band(&mut self, index: usize, band: BandValue);
    fn add_band(&mut self);
    fn remove_band(&mut self, index: usize);
    fn reset(&mut self);
    /// The response curve, preamp included: (Hz, dB), 20 Hz..20 kHz.
    fn response(&self) -> Vec<(f64, f64)>;
    /// "" when the curve is no saved profile ("Custom").
    fn current_profile(&self) -> String;
    fn builtin_profiles(&self) -> Vec<String>;
    fn user_profiles(&self) -> Vec<String>;
    fn load_profile(&mut self, name: &str);
    /// Returns the name used.
    fn save_profile(&mut self, name: &str) -> String;
    fn delete_profile(&mut self, name: &str);
    /// A message for the user either way ("Imported 10 filters").
    fn import_file(&mut self, path: &Path) -> Result<String, String>;
    fn export_file(&self, path: &Path) -> bool;
    fn autoeq_search(&mut self, text: &str);
    fn autoeq_refresh_index(&mut self);
    fn autoeq_apply(&mut self, index: usize);
    fn autoeq_results(&self) -> Vec<AutoEqResult>;
    fn autoeq_busy(&self) -> bool;
    /// Translated.
    fn autoeq_status(&self) -> String;
    /// Collects background work (AutoEQ downloads); true when something
    /// changed. Called every 100 ms while `autoeq_busy()`.
    fn poll(&mut self) -> bool {
        false
    }
}

/// What a file dialog is asked for (`filter` as "Name (*.txt);;…").
#[allow(dead_code)]
pub enum FileRequest {
    /// "Import parametric EQ profile": an existing *.txt.
    Import { title: String, filter: String },
    /// "Export EQ profile": where to write, `suggested` as the name.
    Export { title: String, filter: String, suggested: String },
}

/// Asks for a file and calls back with it (None: cancelled).
pub type FilePicker = Box<dyn Fn(FileRequest, Box<dyn FnOnce(Option<PathBuf>)>)>;

/// The desktop's own dialog (the XDG portal, zenity or kdialog; Windows'
/// common dialog), for *.txt profiles. It runs on a thread of its own: the
/// window keeps drawing and the media keys keep working while it is open
/// (zenity used to freeze the whole app).
pub fn system_file_picker() -> FilePicker {
    Box::new(|request, done| {
        let (title, save_name) = match request {
            FileRequest::Import { title, .. } => (title, None),
            FileRequest::Export { title, suggested, .. } => (title, Some(suggested)),
        };
        let request = crate::file_dialog::Request {
            title,
            save_name,
            filters: vec![("Text (*.txt)".into(), vec!["*.txt".into()]), ("*".into(), vec!["*".into()])],
            ..Default::default()
        };
        crate::file_dialog::ask(request, move |paths| done(paths.into_iter().next().map(PathBuf::from)));
    })
}

/// The window and what it shows.
pub struct EqualizerUi {
    pub window: EqualizerWindow,
    model: Rc<RefCell<dyn EqModel>>,
    notify: Rc<dyn Fn(&str)>,
    /// File dialogs (Import / Export); [`system_file_picker`] by default.
    pub pick_file: RefCell<FilePicker>,
    /// Called after every change made in the window (the player bar's EQ
    /// button, the signal path…).
    pub on_change: RefCell<Option<Box<dyn Fn()>>>,
    bands: Rc<VecModel<EqBand>>,
    menu_names: RefCell<Vec<Option<String>>>,
    /// The dragged point's frequency limits and the graph's size.
    limits: Cell<(f64, f64)>,
    graph: Cell<(f64, f64)>,
    poll: slint::Timer,
    me: std::rc::Weak<EqualizerUi>,
}

/// The graph's height (the card's 300 px less its margins).
const GRAPH_HEIGHT: f64 = 268.0;
const K_LEFT: f64 = 34.0;
const K_TOP: f64 = 8.0;
const K_BOTTOM: f64 = 20.0;

/// Peaks and shelves have a gain; passes, band pass and notch do not.
fn has_gain(kind: &str) -> bool {
    matches!(kind, "PK" | "LSC" | "HSC")
}

/// `v` with `n` fixed decimals.
fn fixed(v: f64, n: usize) -> String {
    format!("{v:.n$}")
}

fn signed(v: f64, n: usize) -> String {
    (if v > 0.0 { "+" } else { "" }).to_owned() + &fixed(v, n)
}

/// Where `hz` sits on the 20 Hz..20 kHz log axis (0..1).
fn axis(hz: f64) -> f64 {
    (hz.log10() - 20f64.log10()) / 3.0
}

impl EqualizerUi {
    pub fn new(model: Rc<RefCell<dyn EqModel>>, notify: Rc<dyn Fn(&str)>) -> Result<Rc<Self>, slint::PlatformError> {
        let window = EqualizerWindow::new()?;
        install_metrics(&window);
        let ui = Rc::new_cyclic(|me| EqualizerUi {
            me: me.clone(),
            window,
            model,
            notify,
            pick_file: RefCell::new(system_file_picker()),
            on_change: RefCell::new(None),
            bands: Rc::new(VecModel::default()),
            menu_names: RefCell::new(Vec::new()),
            limits: Cell::new((20.0, 20000.0)),
            graph: Cell::new((900.0, GRAPH_HEIGHT)),
            poll: slint::Timer::default(),
        });
        let eq = ui.window.global::<EqUi>();
        eq.set_bands(ModelRc::from(ui.bands.clone()));
        let marks: Vec<EqMark> = [20, 50, 100, 200, 500, 1000, 2000, 5000, 10000, 20000]
            .iter()
            .map(|&m| EqMark {
                t: axis(m as f64) as f32,
                label: if m >= 1000 { format!("{}k", m / 1000) } else { m.to_string() }.into(),
            })
            .collect();
        eq.set_freq_marks(ModelRc::new(VecModel::from(marks)));
        let risk = trf(CTX, "CLIP RISK +{0} dB", &["99.9"]);
        let none = tr(CTX, "NO CLIPPING");
        eq.set_clip_reserve(if risk.chars().count() >= none.chars().count() { risk } else { none }.into());
        ui.connect();
        connect_frame(&ui.window);
        ui.refresh();
        Ok(ui)
    }

    /// Opens the window 1000 wide, never taller than its
    /// content (nor than `available` less 80 px).
    pub fn open(&self, available: f32) -> Result<(), slint::PlatformError> {
        let full = self.window.get_full_height();
        let h = full.min((available - 80.0).max(480.0));
        self.window.window().set_size(slint::LogicalSize::new(1000.0, h));
        self.window.show()
    }

    fn with<R>(&self, f: impl FnOnce(&mut dyn EqModel) -> R) -> R {
        let r = f(&mut *self.model.borrow_mut());
        self.refresh();
        if let Some(f) = &*self.on_change.borrow() {
            f();
        }
        r
    }

    fn connect(self: &Rc<Self>) {
        let eq = self.window.global::<EqUi>();
        macro_rules! on {
            ($name:ident, |$ui:ident $(, $arg:ident)*| $body:expr) => {{
                let weak = Rc::downgrade(self);
                eq.$name(move |$($arg),*| {
                    if let Some($ui) = weak.upgrade() {
                        $body
                    }
                });
            }};
        }
        on!(on_set_enabled, |ui, v| ui.with(|m| m.set_enabled(v)));
        on!(on_set_mode, |ui, graphic| ui.with(|m| m.set_graphic(graphic)));
        on!(on_set_preamp, |ui, v| ui.with(|m| m.set_preamp(v as f64)));
        on!(on_preamp_edited, |ui, text| {
            if let Some(v) = crate::i18n::parse_number(&text) {
                ui.with(|m| m.set_preamp(v.clamp(-30.0, 12.0)));
            } else {
                ui.refresh();
            }
        });
        on!(on_auto_preamp, |ui| ui.with(|m| m.auto_preamp()));
        on!(on_set_linear_phase, |ui, v| ui.with(|m| m.set_linear_phase(v)));
        on!(on_set_gain, |ui, i, v| ui.with(|m| m.set_gain(i as usize, v as f64)));
        on!(on_band_toggled, |ui, i, v| ui.edit_band(i as usize, |b| b.enabled = v));
        on!(on_band_type, |ui, i, t| {
            let kind = ui.model.borrow().filter_types().get(t as usize).cloned();
            if let Some(kind) = kind {
                ui.edit_band(i as usize, |b| b.kind = kind);
            }
        });
        on!(on_band_edited, |ui, i, what, text| {
            let (lo, hi) = match what.as_str() {
                "freq" => (10.0, 24000.0),
                "gain" => (-24.0, 24.0),
                _ => (0.05, 30.0),
            };
            match crate::i18n::parse_number(&text) {
                Some(v) => ui.edit_band(i as usize, |b| {
                    let v = v.clamp(lo, hi);
                    match what.as_str() {
                        "freq" => b.freq = v,
                        "gain" => b.gain = v,
                        _ => b.q = v,
                    }
                }),
                _ => ui.refresh(),
            }
        });
        on!(on_add_band, |ui| ui.with(|m| m.add_band()));
        on!(on_remove_band, |ui, i| ui.with(|m| m.remove_band(i as usize)));
        on!(on_reset, |ui| ui.with(|m| m.reset()));
        on!(on_load_profile, |ui, i| {
            let name = ui.menu_names.borrow().get(i as usize).cloned().flatten();
            if let Some(name) = name {
                ui.with(|m| m.load_profile(&name));
            }
        });
        on!(on_save_profile, |ui, name| {
            let saved = ui.with(|m| m.save_profile(name.trim()));
            (ui.notify)(&trf(CTX, "Saved “{0}”", &[&saved]));
        });
        on!(on_delete_profile, |ui| ui.with(|m| {
            let name = m.current_profile();
            m.delete_profile(&name)
        }));
        on!(on_import_profile, |ui| {
            let request = FileRequest::Import {
                title: tr(CTX, "Import parametric EQ profile"),
                filter: format!("{};;{}", tr(CTX, "Parametric EQ (*.txt)"), tr(CTX, "All files (*)")),
            };
            let weak = Rc::downgrade(&ui);
            (ui.pick_file.borrow())(
                request,
                Box::new(move |file| {
                    if let (Some(file), Some(ui)) = (file, weak.upgrade()) {
                        let message = ui.with(|m| m.import_file(&file));
                        (ui.notify)(&message.unwrap_or_else(|e| e));
                    }
                }),
            );
        });
        on!(on_export_profile, |ui| {
            let request = FileRequest::Export {
                title: tr(CTX, "Export EQ profile"),
                filter: tr(CTX, "Parametric EQ (*.txt)"),
                suggested: ui.model.borrow().current_profile() + ".txt",
            };
            let weak = Rc::downgrade(&ui);
            (ui.pick_file.borrow())(
                request,
                Box::new(move |file| {
                    if let (Some(file), Some(ui)) = (file, weak.upgrade()) {
                        let ok = ui.model.borrow().export_file(&file);
                        (ui.notify)(&tr(CTX, if ok { "Profile exported" } else { "Export failed" }));
                    }
                }),
            );
        });
        on!(on_point_pressed, |ui, i, w, h| {
            ui.graph.set((w as f64, h as f64));
            ui.limits.set(ui.freq_limits(i as usize));
        });
        on!(on_point_dragged, |ui, i, x, y| ui.drag(i as usize, x as f64, y as f64));
        on!(on_point_reset, |ui, i| {
            let i = i as usize;
            let standard = {
                let m = ui.model.borrow();
                if m.graphic() { m.graphic_frequency(i) } else { 0.0 }
            };
            ui.edit_band(i, |b| {
                if standard > 0.0 {
                    b.freq = standard;
                }
                b.gain = 0.0;
            });
        });
        on!(on_point_standard, |ui, i| {
            let i = i as usize;
            let standard = {
                let m = ui.model.borrow();
                if m.graphic() { m.graphic_frequency(i) } else { 0.0 }
            };
            if standard > 0.0 {
                ui.edit_band(i, |b| b.freq = standard);
            }
        });
        on!(on_point_wheel, |ui, i, up| ui.edit_band(i as usize, |b| {
            let q = (b.q * if up { 1.1 } else { 1.0 / 1.1 }).clamp(0.1, 20.0);
            b.q = (q * 100.0).round() / 100.0;
        }));
        on!(on_autoeq_search, |ui, text| ui.with(|m| m.autoeq_search(&text)));
        on!(on_autoeq_refresh, |ui| ui.with(|m| m.autoeq_refresh_index()));
        on!(on_autoeq_apply, |ui, i| ui.with(|m| m.autoeq_apply(i as usize)));
    }

    fn edit_band(&self, index: usize, f: impl FnOnce(&mut BandValue)) {
        let Some(mut b) = self.model.borrow().bands().get(index).cloned() else { return };
        f(&mut b);
        self.with(|m| m.set_band(index, b));
    }

    /// A band never gets closer than 1/6 octave to its neighbours.
    fn freq_limits(&self, index: usize) -> (f64, f64) {
        let bands = self.model.borrow().bands();
        let Some(f) = bands.get(index).map(|b| b.freq) else { return (20.0, 20000.0) };
        let (mut below, mut above) = (0.0f64, f64::INFINITY);
        for (i, b) in bands.iter().enumerate() {
            if i == index {
                continue;
            }
            if b.freq < f {
                below = below.max(b.freq);
            } else if b.freq > f {
                above = above.min(b.freq);
            }
        }
        let gap = 2f64.powf(1.0 / 6.0);
        let lo = if below > 0.0 { f.min(below * gap) } else { 20.0 };
        let hi = if above.is_finite() { f.max(above / gap) } else { 20000.0 };
        (lo.max(20.0), hi.min(20000.0))
    }

    /// A point dragged to (x, y) of the graph: its frequency and gain.
    fn drag(&self, index: usize, x: f64, y: f64) {
        let (w, h) = self.graph.get();
        let (lo, hi) = self.limits.get();
        let (b, preamp, r) = {
            let m = self.model.borrow();
            let Some(b) = m.bands().get(index).cloned() else { return };
            (b, m.preamp(), self.range(&*m))
        };
        let db = r - 2.0 * r * (y - K_TOP) / (h - K_BOTTOM - K_TOP);
        let target = db - preamp;
        let gain = if has_gain(&b.kind) { (target.clamp(-24.0, 24.0) * 10.0).round() / 10.0 } else { b.gain };
        let f = 10f64.powf(20f64.log10() + (x - K_LEFT) / (w - K_LEFT) * 3.0).clamp(lo, hi);
        let freq = (if f < 1000.0 { f.round() } else { (f / 10.0).round() * 10.0 }).clamp(lo, hi);
        if freq != b.freq || gain != b.gain {
            self.with(|m| m.set_band(index, BandValue { freq, gain, ..b }));
        }
    }

    /// ±dB shown: at least 12, in steps of 6, room for the curve and the gains.
    fn range(&self, m: &dyn EqModel) -> f64 {
        let mut top: f64 = 12.0;
        for (_, db) in m.response() {
            top = top.max(db.abs());
        }
        for b in m.bands() {
            top = top.max(b.gain.abs());
        }
        ((top + 1.0) / 6.0).ceil() * 6.0
    }

    /// Everything from the model into the window.
    pub fn refresh(&self) {
        let m = self.model.borrow();
        let eq = self.window.global::<EqUi>();
        let graphic = m.graphic();
        let enabled = m.enabled();
        let bands = m.bands();
        let preamp = m.preamp();
        let profile = m.current_profile();
        let users = m.user_profiles();
        eq.set_enabled(enabled);
        eq.set_graphic(graphic);
        eq.set_linear_phase(m.linear_phase());
        eq.set_preamp(preamp as f32);
        eq.set_preamp_text(fixed(preamp, 1).into());
        eq.set_profile(if profile.is_empty() { tr(CTX, "Custom") } else { profile.clone() }.into());
        eq.set_user_profile(!profile.is_empty() && users.contains(&profile));
        let kind = if graphic {
            tr(CTX, "10-band graphic")
        } else {
            trf(CTX, "{0}-band parametric", &[&bands.len().to_string()])
        };
        let phase = if m.linear_phase() { tr(CTX, "linear phase (FIR)") } else { tr(CTX, "64-bit biquads") };
        eq.set_subtitle(format!("{kind}  ·  {phase}").into());
        let peak = m.peak_gain();
        let clipping = enabled && peak > 0.05;
        eq.set_clipping(clipping);
        eq.set_clip_text(
            if clipping { trf(CTX, "CLIP RISK +{0} dB", &[&fixed(peak, 1)]) } else { tr(CTX, "NO CLIPPING") }.into(),
        );
        eq.set_can_add(bands.len() < m.max_bands());

        // The bands.
        let types = m.filter_types();
        let labels: Vec<SharedString> = types.iter().map(|k| tr("EqController", &m.type_label(k)).into()).collect();
        eq.set_filter_types(ModelRc::new(VecModel::from(labels)));
        let rows: Vec<EqBand> = bands
            .iter()
            .map(|b| {
                let with_gain = has_gain(&b.kind);
                let type_label = tr("EqController", &m.type_label(&b.kind));
                EqBand {
                    kind: b.kind.clone().into(),
                    type_index: types.iter().position(|k| k.eq_ignore_ascii_case(&b.kind)).unwrap_or(0) as i32,
                    enabled: b.enabled,
                    has_gain: with_gain,
                    gain: b.gain as f32,
                    t: axis(b.freq) as f32,
                    y_db: ((if with_gain { b.gain } else { 0.0 }) + preamp) as f32,
                    freq_text: fixed(b.freq, 1).into(),
                    gain_text: fixed(b.gain, 1).into(),
                    q_text: fixed(b.q, 2).into(),
                    freq_label: m.frequency_label(b.freq).into(),
                    gain_label: signed(b.gain, 1).into(),
                    tip: format!(
                        "{type_label} · {} Hz · {} dB · Q {}",
                        m.frequency_label(b.freq),
                        signed(b.gain, 1),
                        fixed(b.q, 2)
                    )
                    .into(),
                }
            })
            .collect();
        // Rows kept (an edit keeps its focus); only the changed ones set.
        if self.bands.row_count() != rows.len() {
            self.bands.set_vec(rows);
        } else {
            for (i, row) in rows.into_iter().enumerate() {
                if self.bands.row_data(i).as_ref() != Some(&row) {
                    self.bands.set_row_data(i, row);
                }
            }
        }

        // The response.
        let r = self.range(&*m);
        eq.set_range(r as f32);
        let mut marks = Vec::new();
        let mut db = -r;
        while db <= r + 1e-9 {
            marks.push(db as f32);
            db += 6.0;
        }
        eq.set_db_marks(ModelRc::new(VecModel::from(marks)));
        let plot = GRAPH_HEIGHT - K_BOTTOM - K_TOP;
        let y_of = |db: f64| K_TOP + plot * (r - db) / (2.0 * r);
        let points = m.response();
        let (mut curve, mut area) = (String::new(), String::new());
        if points.len() >= 2 {
            let zero = y_of(0.0);
            let x0 = axis(points[0].0) * 1000.0;
            area += &format!("M {x0:.3} {zero:.3}");
            for (i, (f, db)) in points.iter().enumerate() {
                let (x, y) = (axis(*f) * 1000.0, y_of(*db));
                curve += &format!("{} {x:.3} {y:.3} ", if i == 0 { "M" } else { "L" });
                area += &format!(" L {x:.3} {y:.3}");
            }
            let xn = axis(points[points.len() - 1].0) * 1000.0;
            area += &format!(" L {xn:.3} {zero:.3} Z");
        }
        eq.set_curve(curve.into());
        eq.set_area(area.into());

        // The profile menu.
        let mut entries = vec![EqMenuEntry { kind: 0, text: tr(CTX, "Built-in").into(), checked: false }];
        let mut names = vec![None];
        for name in m.builtin_profiles() {
            entries.push(EqMenuEntry { kind: 1, text: format!("    {name}").into(), checked: name == profile });
            names.push(Some(name));
        }
        entries.push(EqMenuEntry { kind: 2, text: SharedString::new(), checked: false });
        names.push(None);
        let mine = if users.is_empty() { tr(CTX, "My profiles — none saved yet") } else { tr(CTX, "My profiles") };
        entries.push(EqMenuEntry { kind: 0, text: mine.into(), checked: false });
        names.push(None);
        for name in users {
            entries.push(EqMenuEntry { kind: 1, text: format!("    {name}").into(), checked: name == profile });
            names.push(Some(name));
        }
        eq.set_menu(ModelRc::new(VecModel::from(entries)));
        *self.menu_names.borrow_mut() = names;

        // AutoEQ.
        let results: Vec<AutoEqRow> = m
            .autoeq_results()
            .into_iter()
            .map(|r| AutoEqRow {
                name: r.name.into(),
                by: (trf(CTX, "by {0}", &[&r.source])
                    + &if r.rig.is_empty() { String::new() } else { format!(" · {}", r.rig) })
                    .into(),
            })
            .collect();
        eq.set_autoeq_results(ModelRc::new(VecModel::from(results)));
        let busy = m.autoeq_busy();
        eq.set_autoeq_busy(busy);
        eq.set_autoeq_status(m.autoeq_status().into());
        drop(m);
        self.watch(busy);
    }

    /// While a download runs, polls the model for its end.
    fn watch(&self, busy: bool) {
        if !busy {
            self.poll.stop();
            return;
        }
        if self.poll.running() {
            return;
        }
        let me = self.me.clone();
        self.poll.start(slint::TimerMode::Repeated, std::time::Duration::from_millis(100), move || {
            let Some(ui) = me.upgrade() else { return };
            let changed = ui.model.borrow_mut().poll();
            if changed {
                ui.refresh();
            }
        });
    }
}

/// A text's hinted width in the interface font (fonts.rs): each glyph's
/// advance rounded to whole pixels, plus `spacing` per character.
pub fn hinted_width(text: &str, px: f32, weight: u16, spacing: f32) -> f32 {
    let Some(face) = crate::fonts::face(weight) else { return text.chars().count() as f32 * px * 0.55 };
    let scale = px / face.units_per_em() as f32;
    // Inter's advances are hinted: a few glyphs are a pixel off the rounded
    // design advance (tools/avances-hinteados.py).
    let inter = crate::fonts::is_inter();
    let w = [400u16, 500, 600, 700].into_iter().min_by_key(|w| w.abs_diff(weight)).unwrap_or(400);
    let hinted = |c: char| {
        (inter && px.fract() == 0.0)
            .then(|| {
                crate::hinted_advances::HINTED.iter().find(|(hw, hp, hc, _)| *hw == w && *hp as f32 == px && *hc == c)
            })
            .flatten()
            .map(|(_, _, _, a)| *a as f32)
    };
    text.chars()
        .map(|c| {
            hinted(c).unwrap_or_else(|| {
                face.glyph_index(c)
                    .and_then(|g| face.glyph_hor_advance(g))
                    .map_or(px * 0.55, |a| (a as f32 * scale).round())
            }) + spacing
        })
        .sum()
}

/// As `hinted_width`, with tabular figures ("tnum": the Inter Tnum faces).
pub fn hinted_tnum_width(text: &str, px: f32, weight: u16) -> f32 {
    use std::sync::OnceLock;
    const TNUM: [(u16, &[u8]); 3] = [
        (400, include_bytes!("../../../fonts/inter/tnum-400.ttf")),
        (500, include_bytes!("../../../fonts/inter/tnum-500.ttf")),
        (600, include_bytes!("../../../fonts/inter/tnum-600.ttf")),
    ];
    static FACES: OnceLock<Vec<(u16, ttf_parser::Face<'static>)>> = OnceLock::new();
    let faces = FACES
        .get_or_init(|| TNUM.iter().filter_map(|(w, d)| ttf_parser::Face::parse(d, 0).ok().map(|f| (*w, f))).collect());
    let Some((_, face)) = faces.iter().filter(|_| crate::fonts::is_inter()).min_by_key(|(w, _)| w.abs_diff(weight))
    else {
        return hinted_width(text, px, weight, 0.0);
    };
    let scale = px / face.units_per_em() as f32;
    text.chars()
        .map(|c| {
            face.glyph_index(c)
                .and_then(|g| face.glyph_hor_advance(g))
                .map_or(px * 0.55, |a| (a as f32 * scale).round())
        })
        .sum()
}

/// Gives a window's `TextMetrics` global its widths. Each reads
/// `Theme.font-family`, so what was measured is measured again when the
/// font changes (Slint keeps a pure callback's result until a property it
/// read changes).
pub fn install_metrics<C>(window: &C)
where
    C: ComponentHandle + 'static,
    for<'a> crate::TextMetrics<'a>: slint::Global<'a, C>,
    for<'a> crate::Theme<'a>: slint::Global<'a, C>,
{
    let g = window.global::<crate::TextMetrics>();
    let font = {
        let weak = window.as_weak();
        move || {
            if let Some(w) = weak.upgrade() {
                let _ = w.global::<crate::Theme>().get_font_family();
            }
        }
    };
    let f = font.clone();
    g.on_width(move |text, px, weight| {
        f();
        hinted_width(&text, px, weight.clamp(100, 900) as u16, 0.0)
    });
    let f = font.clone();
    g.on_spaced_width(move |text, px, weight, spacing| {
        f();
        hinted_width(&text, px, weight.clamp(100, 900) as u16, spacing)
    });
    g.on_tnum_width(move |text, px, weight| {
        font();
        hinted_tnum_width(&text, px, weight.clamp(100, 900) as u16)
    });
}

/// The frame's buttons and edges, through winit, for a window
/// with its own `WindowActions` global: close hides it.
pub fn connect_frame<C>(window: &C)
where
    C: ComponentHandle + 'static,
    for<'a> WindowActions<'a>: slint::Global<'a, C>,
{
    let actions = window.global::<WindowActions>();
    let weak = window.as_weak();
    actions.on_minimize(move || {
        if let Some(w) = weak.upgrade() {
            crate::window_drag::minimize(&w);
        }
    });
    let weak = window.as_weak();
    actions.on_toggle_maximize(move || {
        if let Some(w) = weak.upgrade() {
            let on = !w.window().is_maximized();
            w.window().set_maximized(on);
            w.global::<WindowActions>().set_maximized(on);
        }
    });
    let weak = window.as_weak();
    actions.on_close(move || {
        if let Some(w) = weak.upgrade() {
            let _ = w.hide();
        }
    });
    let weak = window.as_weak();
    actions.on_start_move(move || {
        if let Some(w) = weak.upgrade() {
            if crate::window_drag::start_move(&w) {
                if w.global::<crate::WindowActions>().get_show_maximize() {
                    w.global::<crate::WindowActions>().invoke_toggle_maximize();
                }
            }
        }
    });
    let weak = window.as_weak();
    actions.on_start_resize(move |edge| {
        if let Some(w) = weak.upgrade() {
            crate::window_drag::start_resize(&w, edge.as_str());
        }
    });
    crate::window_drag::follow_maximized(window, |w, on| w.global::<WindowActions>().set_maximized(on));
}

/// SONKORE_SCREENSHOT_WINDOW=equalizer|pitch with SONKORE_SCREENSHOT=file.png:
/// that window alone, offscreen, at its default size (1000 × 720, 580 × 695
/// unless SONKORE_SCREENSHOT_SIZE says otherwise).
pub fn screenshot(
    which: &str,
    settings: sonkore_core::settings::Settings,
    off: &slint::platform::software_renderer::MinimalSoftwareWindow,
    path: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    use crate::{Skins, theme};
    let settings = Rc::new(RefCell::new(settings));
    let notify: Rc<dyn Fn(&str)> = Rc::new(|_: &str| {});
    // Kept alive until the frame is saved.
    let (_eq, _pitch);
    match which {
        "pitch" | "soundtouch" => {
            let ui = crate::pitch_ui::PitchUi::new(settings.clone())?;
            crate::i18n::select_slint();
            let look = theme::Look::load(&settings.borrow(), ui.window.global::<Skins>().get_list().iter().count());
            theme::apply(&ui.window, &look);
            ui.refresh();
            ui.window.show()?;
            if std::env::var_os("SONKORE_SCREENSHOT_SIZE").is_none() {
                // SAFETY: before any other thread starts.
                unsafe { std::env::set_var("SONKORE_SCREENSHOT_SIZE", "580x695") };
            }
            _pitch = Some(ui);
            _eq = None;
        }
        _ => {
            let model = crate::equalizer_model::EqAdapter::new(settings.clone());
            let model: Rc<RefCell<dyn EqModel>> = Rc::new(RefCell::new(model));
            let ui = EqualizerUi::new(model, notify)?;
            crate::i18n::select_slint();
            let look = theme::Look::load(&settings.borrow(), ui.window.global::<Skins>().get_list().iter().count());
            theme::apply(&ui.window, &look);
            ui.window.show()?;
            if std::env::var_os("SONKORE_SCREENSHOT_SIZE").is_none() {
                // SAFETY: before any other thread starts.
                unsafe { std::env::set_var("SONKORE_SCREENSHOT_SIZE", "1000x720") };
            }
            _eq = Some(ui);
            _pitch = None;
        }
    }
    crate::screenshot::capture(off, path);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::hinted_width;

    // Measured: the player bar's "System" label, in Spanish.
    #[test]
    fn text_widths_are_hinted_ones() {
        assert_eq!(hinted_width("Sistema", 10.0, 600, 0.0), 41.0);
        assert_eq!(hinted_width("DAC", 10.0, 600, 0.0), 21.0);
    }
}
