//! The controller's behaviour: clamps, modes, profiles, persistence, import /
//! export and the AutoEQ flow (fed offline).

#![allow(clippy::approx_constant)]

use std::fs;
use std::path::PathBuf;

use serde_json::json;
use sonkore_core::settings::{Settings, Value};
use sonkore_eq::autoeq::{self, DownloadError};
use sonkore_eq::{AutoEqNext, EqController, FilterType, Mode, SuperEq};

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> TempDir {
        let p = std::env::temp_dir().join(format!("sonkore-eq-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn fresh(dir: &TempDir) -> (SuperEq, EqController, Settings) {
    let settings = Settings::open(dir.0.join("sonkore.conf"));
    let eq = SuperEq::new();
    let c = EqController::new(eq.clone(), &settings, &dir.0);
    (eq, c, settings)
}

#[test]
fn defaults_and_clamps() {
    let dir = TempDir::new("clamps");
    let (eq, mut c, _) = fresh(&dir);
    assert!(!c.enabled() && !eq.is_enabled(), "a fresh install starts bit-perfect");
    assert_eq!(c.mode(), Mode::Graphic);
    assert_eq!(c.current_profile(), "Flat");
    assert_eq!(c.bands().len(), 10);
    assert_eq!(c.builtin_profiles().len(), 13);

    c.set_preamp(20.0);
    assert_eq!(c.preamp(), 12.0);
    c.set_preamp(-99.0);
    assert_eq!(c.preamp(), -30.0);
    assert_eq!(c.current_profile(), "", "an edit makes the curve custom");
    c.set_gain(0, 30.0);
    assert_eq!(c.bands()[0].gain, 24.0);
    c.set_gain(42, 1.0); // no such band: nothing
    c.set_band(1, "HSC", 5.0, -40.0, 100.0, false);
    let b = &c.bands()[1];
    assert_eq!((b.kind.as_str(), b.freq, b.gain, b.q, b.enabled), ("HSC", 10.0, -24.0, 30.0, false));
    c.set_band(1, "LSC", 30000.0, 3.0, 0.0, true);
    assert_eq!((c.bands()[1].freq, c.bands()[1].q), (24000.0, 0.05));
    assert_eq!(eq.preamp_db(), -30.0);
    assert_eq!(eq.bands()[1].kind, FilterType::LowShelf);

    c.add_band();
    assert_eq!(c.mode(), Mode::Parametric);
    let added = c.bands().last().unwrap().clone();
    assert_eq!((added.kind.as_str(), added.freq, added.gain, added.q), ("PK", 1000.0, 0.0, 1.0));
    while c.bands().len() < 32 {
        c.add_band();
    }
    c.add_band();
    assert_eq!(c.bands().len(), 32);
    c.remove_band(0);
    assert_eq!(c.bands().len(), 31);

    c.reset();
    assert_eq!((c.mode(), c.preamp(), c.current_profile(), c.bands().len()), (Mode::Graphic, 0.0, "Flat", 10));
    assert!(c.bands().iter().all(|b| b.gain == 0.0));
}

#[test]
fn graphic_mode_runs_solved_gains() {
    let dir = TempDir::new("graphic");
    let (eq, mut c, _) = fresh(&dir);
    for i in 0..10 {
        c.set_gain(i, 6.0);
    }
    // Ten sliders at +6 dB: the cascade passes through +6 dB at the centres,
    // so each band's own gain is lower, rounded to 0.001 dB.
    let gains: Vec<f64> = eq.bands().iter().map(|b| b.gain_db).collect();
    assert!(gains.iter().all(|&g| g < 6.0 && (g * 1000.0).round() / 1000.0 == g), "{gains:?}");
    for f in [62.5, 1000.0, 8000.0] {
        assert!((c.response_at(f) - 6.0).abs() < 0.02, "{f}: {}", c.response_at(f));
    }
    // Parametric runs the bands as they are.
    c.set_mode(Mode::Parametric);
    assert!(eq.bands().iter().all(|b| b.gain_db == 6.0));
    // And back: the curve fitted at the centres, 0.1 dB steps.
    c.set_mode(Mode::Graphic);
    for b in c.bands() {
        assert_eq!(b.gain, (b.gain * 10.0).round() / 10.0);
    }
}

#[test]
fn builtin_profiles_get_headroom() {
    let dir = TempDir::new("builtin");
    let (_, mut c, _) = fresh(&dir);
    c.load_profile("Bass Boost");
    assert_eq!(c.current_profile(), "Bass Boost");
    let preamp = c.preamp();
    assert!(preamp < 0.0 && (preamp * 10.0).round() / 10.0 == preamp, "{preamp}");
    // The preamp keeps the peak at or just under 0 dB.
    let peak = c.peak_gain();
    assert!(peak <= 0.0 && peak > -0.1, "{peak}");
    // Loading it again gives the same (it does not depend on the previous curve).
    c.load_profile("Flat");
    assert_eq!(c.preamp(), 0.0);
    c.load_profile("Bass Boost");
    assert_eq!(c.preamp(), preamp);
    c.set_preamp(0.0);
    c.auto_preamp();
    assert_eq!(c.preamp(), preamp);
}

#[test]
fn user_profiles_persist() {
    let dir = TempDir::new("profiles");
    let (_, mut c, _) = fresh(&dir);
    c.set_mode(Mode::Parametric);
    c.set_band(0, "LSC", 80.0, 4.0, 0.7, true);
    c.set_preamp(-4.0);
    assert_eq!(c.save_profile("  Mine  "), "Mine");
    assert_eq!(c.save_profile(""), "My profile");
    assert_eq!(c.save_profile("Rock"), "Rock (custom)");
    assert_eq!(c.current_profile(), "Rock (custom)");
    c.set_preamp(-5.0);
    assert_eq!(c.save_profile("Mine"), "Mine"); // replaced
    assert_eq!(c.user_profiles(), ["Mine", "My profile", "Rock (custom)"]);

    let text = fs::read_to_string(dir.0.join("eq-profiles.json")).unwrap();
    assert!(
        text.starts_with("[\n    {\n        \"bands\": [\n            {\n                \"enabled\": true,"),
        "{text}"
    );
    let saved: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(saved[0]["name"], "Mine");
    assert_eq!(saved[0]["mode"], "parametric");
    assert_eq!(saved[0]["preamp"], -5.0);
    assert_eq!(saved[0]["bands"][0], json!({"enabled": true, "freq": 80, "gain": 4, "q": 0.7, "type": "LSC"}));

    let settings = Settings::open(dir.0.join("sonkore.conf"));
    let mut again = EqController::new(SuperEq::new(), &settings, &dir.0);
    assert_eq!(again.user_profiles(), ["Mine", "My profile", "Rock (custom)"]);
    again.load_profile("Mine");
    assert_eq!((again.mode(), again.preamp(), again.current_profile()), (Mode::Parametric, -5.0, "Mine"));
    assert_eq!(again.bands()[0].kind, "LSC");
    again.delete_profile("Mine");
    assert_eq!(again.current_profile(), "");
    assert_eq!(again.user_profiles(), ["My profile", "Rock (custom)"]);
}

#[test]
fn settings_round_trip() {
    let dir = TempDir::new("settings");
    let (_, mut c, mut s) = fresh(&dir);
    c.set_enabled(true);
    c.set_linear_phase(true);
    c.load_profile("Vocal");
    c.save_state(&mut s);
    s.save().unwrap();
    let file = fs::read_to_string(dir.0.join("sonkore.conf")).unwrap();
    assert!(
        file.contains("[equalizer]\nbands=\"@ByteArray([{\\\"enabled\\\":true,\\\"freq\\\":31.25,\\\"gain\\\":-2,"),
        "{file}"
    );
    assert!(file.contains("enabled=true\nlinearPhase=true\nmode=graphic\npreamp="), "{file}");
    assert!(file.contains("profile=Vocal"), "{file}");

    let s2 = Settings::open(dir.0.join("sonkore.conf"));
    let eq = SuperEq::new();
    let back = EqController::new(eq.clone(), &s2, &dir.0);
    assert_eq!(back.snapshot(), c.snapshot());
    assert!(eq.is_enabled() && eq.linear_phase());
    assert_eq!(eq.preamp_db(), c.preamp());
}

#[test]
fn stored_settings_files_are_read() {
    let dir = TempDir::new("storedsettings");
    let mut s = Settings::open(dir.0.join("sonkore.conf"));
    s.set_text("equalizer/enabled", "true");
    s.set_text("equalizer/mode", "parametric");
    s.set_text("equalizer/preamp", "-3.5");
    s.set(
        "equalizer/bands",
        Value::Bytes(
            br#"[{"enabled":true,"freq":31.25,"q":1.41421356237,"type":"PK"},{"type":"NO","freq":60}]"#.to_vec(),
        ),
    );
    let c = EqController::new(SuperEq::new(), &s, &dir.0);
    assert!(c.enabled());
    assert_eq!((c.mode(), c.preamp(), c.current_profile()), (Mode::Parametric, -3.5, "Flat"));
    assert_eq!(c.bands().len(), 2);
    assert_eq!((c.bands()[0].gain, c.bands()[1].q, c.bands()[1].enabled), (0.0, 1.41421356237, true));
}

#[test]
fn snapshot_restore() {
    let dir = TempDir::new("snapshot");
    let (eq, mut c, _) = fresh(&dir);
    c.set_enabled(true);
    c.load_profile("Jazz");
    let snap = c.snapshot();
    assert_eq!(snap["profile"], "Jazz");
    assert_eq!(snap["mode"], "graphic");
    let keys: Vec<&String> = snap.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["bands", "enabled", "linearPhase", "mode", "preamp", "profile"]);

    c.reset();
    c.set_enabled(false);
    c.restore(&json!({}));
    assert!(!c.enabled(), "an empty state changes nothing");
    c.restore(&snap);
    assert_eq!(c.snapshot(), snap);
    assert!(eq.is_enabled());
    // Missing keys keep what is there; a missing profile is custom.
    c.restore(&json!({"preamp": -1.0}));
    assert_eq!((c.preamp(), c.current_profile(), c.mode()), (-1.0, "", Mode::Graphic));
}

#[test]
fn import_and_export_files() {
    let dir = TempDir::new("apo");
    let (eq, mut c, _) = fresh(&dir);
    let path = dir.0.join("HD 650 ParametricEQ.txt");
    fs::write(&path, "Preamp: -6.2 dB\nFilter 1: ON LSC Fc 105 Hz Gain 5.6 dB Q 0.70\nFilter 2: ON PK Fc 2363 Hz Gain -2.4 dB Q 1.43\n")
        .unwrap();
    let imported = c.import_file(&path).unwrap();
    assert_eq!(imported.count, 2);
    assert_eq!(imported.profile, "HD 650 ParametricEQ");
    assert_eq!((c.mode(), c.preamp(), c.current_profile()), (Mode::Parametric, -6.2, "HD 650 ParametricEQ"));
    assert_eq!(eq.bands().len(), 2);

    let out = dir.0.join("out.txt");
    c.export_file(&out).unwrap();
    assert_eq!(
        fs::read_to_string(&out).unwrap(),
        "Preamp: -6.2 dB\nFilter 1: ON LSC Fc 105 Hz Gain 5.6 dB Q 0.70\nFilter 2: ON PK Fc 2363 Hz Gain -2.4 dB Q 1.43\n"
    );
    let empty = dir.0.join("empty.txt");
    fs::write(&empty, "nothing here").unwrap();
    assert!(matches!(c.import_file(&empty), Err(sonkore_eq::ImportError::NoFilters)));
    assert!(c.import_file(&dir.0.join("missing.txt")).is_err());
}

#[test]
fn autoeq_flow_offline() {
    let dir = TempDir::new("autoeq");
    let (eq, mut c, _) = fresh(&dir);
    // No cache: the index must be downloaded first.
    assert_eq!(c.autoeq_search("hd 650"), AutoEqNext::DownloadIndex);
    assert!(c.autoeq_busy());
    assert_eq!(c.autoeq_status(), "Downloading AutoEQ database…");
    assert_eq!(c.autoeq_search("600"), AutoEqNext::Busy);
    let index = "- [Sennheiser HD 650](./oratory1990/over-ear/Sennheiser%20HD%20650) by oratory1990\n\
                 - [Sennheiser HD 600](./crinacle/GRAS%2043AG-7%20over-ear/Sennheiser%20HD%20600) by crinacle on GRAS 43AG-7\n";
    c.autoeq_index_downloaded(Ok(index.into()));
    assert!(!c.autoeq_busy());
    assert_eq!(c.autoeq_status(), "2 headphone profiles available");
    assert_eq!(c.autoeq_results().len(), 1, "the pending search ran");
    assert_eq!(c.autoeq_results()[0].name, "Sennheiser HD 600"); // "600" was the last text
    assert_eq!(fs::read_to_string(autoeq::cache_path(&dir.0)).unwrap(), index);

    assert_eq!(c.autoeq_search("sennheiser"), AutoEqNext::Results);
    assert_eq!(c.autoeq_results().len(), 2);
    let entry = c.autoeq_apply(0).unwrap();
    assert_eq!(c.autoeq_status(), "Downloading Sennheiser HD 650…");
    assert!(c.autoeq_apply(1).is_none(), "busy");
    c.autoeq_profile_downloaded(&entry, Err(DownloadError("timed out".into())));
    assert_eq!(c.autoeq_status(), "Download failed: timed out");
    let entry = c.autoeq_apply(0).unwrap();
    c.autoeq_profile_downloaded(&entry, Ok("Preamp: -6.4 dB\nFilter 1: ON LSC Fc 105 Hz Gain 6.0 dB Q 0.70\n".into()));
    assert!(c.enabled() && eq.is_enabled());
    assert_eq!(c.current_profile(), "AutoEQ · Sennheiser HD 650 (oratory1990)");
    assert_eq!(c.autoeq_status(), "Applied and saved as “AutoEQ · Sennheiser HD 650 (oratory1990)”");
    let entry = c.autoeq_apply(1).unwrap();
    c.autoeq_profile_downloaded(&entry, Ok("<html>404</html>".into()));
    assert_eq!(c.autoeq_status(), "No parametric profile published for Sennheiser HD 600");

    // A new controller finds the fresh cache.
    let settings = Settings::open(dir.0.join("sonkore.conf"));
    let mut again = EqController::new(SuperEq::new(), &settings, &dir.0);
    assert_eq!(again.autoeq_search("650"), AutoEqNext::Results);
    assert_eq!(again.autoeq_results().len(), 1);
    assert!(again.autoeq_apply(5).is_none());
}

/// Every band and the preamp within the controller's ranges, and the
/// engine's curve finite: what any import or restore must leave.
fn assert_sane(c: &EqController, eq: &SuperEq, what: &str) {
    use sonkore_eq::controller::{FREQ_RANGE, GAIN_RANGE, PREAMP_RANGE, Q_RANGE};
    let inside = |v: f64, r: (f64, f64)| v.is_finite() && v >= r.0 && v <= r.1;
    assert!(inside(c.preamp(), PREAMP_RANGE), "{what}: preamp {}", c.preamp());
    assert!(c.bands().len() <= 32, "{what}: {} bands", c.bands().len());
    for b in c.bands() {
        assert!(inside(b.freq, FREQ_RANGE) && inside(b.gain, GAIN_RANGE) && inside(b.q, Q_RANGE), "{what}: {b:?}");
    }
    assert!(c.response().iter().all(|p| p.db.is_finite()), "{what}: response");
    assert!(eq.peak_gain_db().is_finite(), "{what}: peak");
    let mut p = eq.processor();
    p.prepare(48000.0, 2);
    let mut x: Vec<f64> = (0..4800).map(|i| (i as f64 * 0.05).sin() * 0.1).collect();
    p.process(&mut x);
    assert!(x.iter().all(|v| v.is_finite()), "{what}: the output is not finite");
}

#[test]
fn imported_and_restored_values_are_sanitised() {
    let dir = TempDir::new("sanitise");
    let (eq, mut c, _) = fresh(&dir);
    c.set_enabled(true);
    let huge = "9".repeat(309); // parses to infinity
    let path = dir.0.join("bad.txt");
    fs::write(
        &path,
        format!(
            "Preamp: -{huge} dB\nFilter 1: ON PK Fc 100 Hz Gain {huge} dB Q 1\n\
             Filter 2: ON PK Fc 1000000 Hz Gain -99 dB Q 0\nFilter 3: ON LSC Fc 0 Hz Gain 3 dB Q {huge}\n"
        ),
    )
    .unwrap();
    c.import_file(&path).unwrap();
    assert_sane(&c, &eq, "APO import");
    // Digits past f64's range read as 0; finite values are clamped.
    assert_eq!((c.preamp(), c.bands()[0].gain), (0.0, 0.0));
    assert_eq!((c.bands()[1].freq, c.bands()[1].gain, c.bands()[1].q), (24000.0, -24.0, 0.05));
    // The profile saved from it is sane too.
    let saved = c.current_profile().to_owned();
    c.reset();
    c.load_profile(&saved);
    assert_sane(&c, &eq, "saved import");

    // Snapshots (sound profiles of outputs) with numbers as text.
    c.restore(&json!({
        "preamp": "inf",
        "bands": [{"type": "PK", "freq": "nan", "gain": "-inf", "q": "1e999", "enabled": true},
                  {"type": "HSC", "freq": 8000, "gain": 1e300, "q": -5}]
    }));
    assert_sane(&c, &eq, "restore");
    c.restore(&json!({"preamp": 500.0}));
    assert_sane(&c, &eq, "restore preamp");

    // AutoEQ.
    c.autoeq_search("x"); // needs the index first
    c.autoeq_index_downloaded(Ok("- [A B](./src/over-ear/A%20B) by src\n".into()));
    c.autoeq_search("a");
    let entry = c.autoeq_apply(0).unwrap();
    c.autoeq_profile_downloaded(
        &entry,
        Ok(format!("Preamp: {huge} dB\nFilter 1: ON PK Fc 50 Hz Gain {huge} dB Q 1\n")),
    );
    assert_sane(&c, &eq, "AutoEQ");

    // Settings and saved profiles written by hand.
    let mut s = Settings::open(dir.0.join("sonkore.conf"));
    s.set_text("equalizer/enabled", "true");
    s.set_text("equalizer/mode", "parametric");
    s.set_text("equalizer/preamp", "nan");
    s.set(
        "equalizer/bands",
        Value::Bytes(
            br#"[{"type":"PK","freq":"inf","gain":"nan","q":"0"},{"type":"PK","freq":10,"gain":"1e400","q":1}]"#
                .to_vec(),
        ),
    );
    fs::write(
        dir.0.join("eq-profiles.json"),
        r#"[{"name":"Odd","mode":"parametric","preamp":"-inf","bands":[{"type":"PK","freq":-3,"gain":"inf","q":"nan"}]}]"#,
    )
    .unwrap();
    let eq = SuperEq::new();
    let mut c = EqController::new(eq.clone(), &s, &dir.0);
    assert_sane(&c, &eq, "stored settings");
    c.load_profile("Odd");
    assert_eq!(c.current_profile(), "Odd");
    assert_sane(&c, &eq, "stored profile");
}
