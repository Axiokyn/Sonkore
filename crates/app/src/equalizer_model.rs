//! [`EqModel`] over the real equaliser (`sonkore_eq::EqController`): every
//! change written to the `equalizer/` settings, the
//! AutoEQ downloads run on a thread and collected by [`EqModel::poll`], the
//! controller's status lines translated (EqController context).

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::sync::mpsc;

use sonkore_core::settings::Settings;
use sonkore_eq::SuperEq;
use sonkore_eq::autoeq::{self, DownloadError, Entry};
use sonkore_eq::controller::{self, AutoEqNext, EqController, FILTER_TYPES, ImportError, Mode};

use crate::equalizer_ui::{AutoEqResult, BandValue, EqModel};
use crate::i18n::{tr, trf};

const CTX: &str = "EqController";

enum Done {
    Index(Result<String, DownloadError>),
    Profile(Entry, Result<String, DownloadError>),
}

pub struct EqAdapter {
    pub eq: Rc<RefCell<EqController>>,
    settings: Rc<RefCell<Settings>>,
    done: Option<mpsc::Receiver<Done>>,
}

impl EqAdapter {
    /// A controller of its own (a `SuperEq` no engine runs): for the
    /// screenshot and until the engine's is wired.
    pub fn new(settings: Rc<RefCell<Settings>>) -> Self {
        let eq = EqController::with_default_paths(SuperEq::new(), &settings.borrow());
        Self::with_controller(Rc::new(RefCell::new(eq)), settings)
    }

    /// Over the app's controller (Ctx::equalizer).
    pub fn with_controller(eq: Rc<RefCell<EqController>>, settings: Rc<RefCell<Settings>>) -> Self {
        EqAdapter { eq, settings, done: None }
    }

    /// Writes the `equalizer/` keys when they changed.
    fn save(&mut self) {
        let dirty = self.eq.borrow().state_dirty();
        if dirty {
            self.eq.borrow_mut().save_state(&mut self.settings.borrow_mut());
        }
    }

    fn run(&mut self, job: impl FnOnce() -> Done + Send + 'static) {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(job());
        });
        self.done = Some(rx);
    }

    fn next(&mut self, next: AutoEqNext) {
        if next == AutoEqNext::DownloadIndex {
            self.run(|| Done::Index(autoeq::download_index()));
        }
    }
}

/// The controller's English status line in the window's language.
fn translate_status(s: &str) -> String {
    let arg =
        |prefix: &str, suffix: &str| s.strip_prefix(prefix).and_then(|r| r.strip_suffix(suffix)).map(str::to_owned);
    if s == "Downloading AutoEQ database…" {
        return tr(CTX, s);
    }
    if let Some(n) = arg("", " headphone profiles available") {
        return trf(CTX, "{0} headphone profiles available", &[&n]);
    }
    if let Some(e) = arg("Could not reach AutoEQ: ", "") {
        return trf(CTX, "Could not reach AutoEQ: {0}", &[&e]);
    }
    if let Some(e) = arg("Download failed: ", "") {
        return trf(CTX, "Download failed: {0}", &[&e]);
    }
    if let Some(n) = arg("No parametric profile published for ", "") {
        return trf(CTX, "No parametric profile published for {0}", &[&n]);
    }
    if let Some(n) = arg("Applied and saved as “", "”") {
        return trf(CTX, "Applied and saved as “{0}”", &[&n]);
    }
    if let Some(n) = arg("Downloading ", "…") {
        return trf(CTX, "Downloading {0}…", &[&n]);
    }
    s.to_owned()
}

impl EqModel for EqAdapter {
    fn enabled(&self) -> bool {
        self.eq.borrow().enabled()
    }
    fn set_enabled(&mut self, on: bool) {
        self.eq.borrow_mut().set_enabled(on);
        self.save();
    }
    fn linear_phase(&self) -> bool {
        self.eq.borrow().linear_phase()
    }
    fn set_linear_phase(&mut self, on: bool) {
        self.eq.borrow_mut().set_linear_phase(on);
        self.save();
    }
    fn graphic(&self) -> bool {
        self.eq.borrow().mode() == Mode::Graphic
    }
    fn set_graphic(&mut self, graphic: bool) {
        self.eq.borrow_mut().set_mode(if graphic { Mode::Graphic } else { Mode::Parametric });
        self.save();
    }
    fn preamp(&self) -> f64 {
        self.eq.borrow().preamp()
    }
    fn set_preamp(&mut self, db: f64) {
        self.eq.borrow_mut().set_preamp(db);
        self.save();
    }
    fn auto_preamp(&mut self) {
        self.eq.borrow_mut().auto_preamp();
        self.save();
    }
    fn peak_gain(&self) -> f64 {
        self.eq.borrow().peak_gain()
    }
    fn bands(&self) -> Vec<BandValue> {
        self.eq
            .borrow()
            .bands()
            .iter()
            .map(|b| BandValue { kind: b.kind.clone(), freq: b.freq, gain: b.gain, q: b.q, enabled: b.enabled })
            .collect()
    }
    fn max_bands(&self) -> usize {
        self.eq.borrow().max_bands()
    }
    fn filter_types(&self) -> Vec<String> {
        FILTER_TYPES.iter().map(|s| (*s).to_owned()).collect()
    }
    fn type_label(&self, kind: &str) -> String {
        controller::type_label(kind).to_owned()
    }
    fn frequency_label(&self, hz: f64) -> String {
        controller::frequency_label(hz)
    }
    fn graphic_frequency(&self, index: usize) -> f64 {
        controller::graphic_frequency(index)
    }
    fn set_gain(&mut self, index: usize, db: f64) {
        self.eq.borrow_mut().set_gain(index, db);
        self.save();
    }
    fn set_band(&mut self, index: usize, b: BandValue) {
        self.eq.borrow_mut().set_band(index, &b.kind, b.freq, b.gain, b.q, b.enabled);
        self.save();
    }
    fn add_band(&mut self) {
        self.eq.borrow_mut().add_band();
        self.save();
    }
    fn remove_band(&mut self, index: usize) {
        self.eq.borrow_mut().remove_band(index);
        self.save();
    }
    fn reset(&mut self) {
        self.eq.borrow_mut().reset();
        self.save();
    }
    fn response(&self) -> Vec<(f64, f64)> {
        self.eq.borrow().response().iter().map(|p| (p.f, p.db)).collect()
    }
    fn current_profile(&self) -> String {
        self.eq.borrow().current_profile().to_owned()
    }
    fn builtin_profiles(&self) -> Vec<String> {
        self.eq.borrow().builtin_profiles().into_iter().map(str::to_owned).collect()
    }
    fn user_profiles(&self) -> Vec<String> {
        self.eq.borrow().user_profiles().into_iter().map(str::to_owned).collect()
    }
    fn load_profile(&mut self, name: &str) {
        self.eq.borrow_mut().load_profile(name);
        self.save();
    }
    fn save_profile(&mut self, name: &str) -> String {
        // " (custom)" and "My profile" in the window's language.
        let mut wanted = name.trim().to_owned();
        if wanted.is_empty() {
            wanted = tr(CTX, "My profile");
        }
        let builtin = self.eq.borrow().builtin_profiles().contains(&wanted.as_str());
        if builtin {
            wanted += &tr(CTX, " (custom)");
        }
        let saved = self.eq.borrow_mut().save_profile(&wanted);
        self.save();
        saved
    }
    fn delete_profile(&mut self, name: &str) {
        self.eq.borrow_mut().delete_profile(name);
        self.save();
    }
    fn import_file(&mut self, path: &Path) -> Result<String, String> {
        let result = self.eq.borrow_mut().import_file(path);
        self.save();
        match result {
            Ok(done) => Ok(trf(CTX, "Imported {0} filters", &[&done.count.to_string()])),
            Err(ImportError::NoFilters) => Err(tr(CTX, "No parametric EQ filters found in that file")),
            Err(ImportError::Io(e)) => Err(e.to_string()),
        }
    }
    fn export_file(&self, path: &Path) -> bool {
        self.eq.borrow().export_file(path).is_ok()
    }
    fn autoeq_search(&mut self, text: &str) {
        let next = self.eq.borrow_mut().autoeq_search(text);
        self.next(next);
    }
    fn autoeq_refresh_index(&mut self) {
        let next = self.eq.borrow_mut().autoeq_refresh_index();
        self.next(next);
    }
    fn autoeq_apply(&mut self, index: usize) {
        let entry = self.eq.borrow_mut().autoeq_apply(index);
        if let Some(entry) = entry {
            self.run(move || {
                let result = autoeq::download_profile(&entry);
                Done::Profile(entry, result)
            });
        }
    }
    fn autoeq_results(&self) -> Vec<AutoEqResult> {
        self.eq
            .borrow()
            .autoeq_results()
            .iter()
            .map(|r| AutoEqResult { name: r.name.clone(), source: r.source.clone(), rig: r.rig.clone() })
            .collect()
    }
    fn autoeq_busy(&self) -> bool {
        self.eq.borrow().autoeq_busy()
    }
    fn autoeq_status(&self) -> String {
        translate_status(self.eq.borrow().autoeq_status())
    }
    fn poll(&mut self) -> bool {
        let Some(rx) = &self.done else { return false };
        let Ok(done) = rx.try_recv() else { return false };
        self.done = None;
        match done {
            Done::Index(result) => self.eq.borrow_mut().autoeq_index_downloaded(result),
            Done::Profile(entry, result) => self.eq.borrow_mut().autoeq_profile_downloaded(&entry, result),
        }
        self.save();
        true
    }
}
