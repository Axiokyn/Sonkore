//! The equalizer against reference values recorded earlier: tests/data holds
//! what the reference equalizer computed for the same cases.
//! The cases below are walked in the order they were recorded in.
//!
//! Tolerances: the biquad path (coefficients, the graphic solve, peak gain,
//! the response curve, processing) comes out bit for bit the reference's (the
//! printed figures, `--nocapture`, count the values that are not); it is
//! checked to 1e-12 so another libm stays within the test. The linear-phase
//! FIR uses another FFT (realfft vs pocketfft): its taps differ by < 1e-16
//! and its output by < 1e-15, checked to 1e-13.

#![allow(clippy::approx_constant)]

use std::path::PathBuf;

use sonkore_core::settings::Settings;
use sonkore_eq::biquad::{self, FilterType};
use sonkore_eq::super_eq::{self, Band, Curve, GRAPHIC_CENTERS, GRAPHIC_Q};
use sonkore_eq::{EqController, LinearPhaseEq, Mode, SuperEq, linear_phase};

struct Data {
    values: Vec<f64>,
    at: usize,
}

impl Data {
    fn open(name: &str) -> Data {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data").join(name);
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let values = bytes.as_chunks::<8>().0.iter().map(|b| f64::from_le_bytes(*b)).collect();
        Data { values, at: 0 }
    }

    fn next(&mut self) -> f64 {
        self.at += 1;
        self.values[self.at - 1]
    }

    fn take(&mut self, n: usize) -> Vec<f64> {
        self.at += n;
        self.values[self.at - n..self.at].to_vec()
    }

    fn done(&self) {
        assert_eq!(self.at, self.values.len(), "unread reference data");
    }
}

/// Largest absolute difference; also counts the values not bit-identical.
fn compare(what: &str, got: &[f64], want: &[f64]) -> (f64, usize) {
    assert_eq!(got.len(), want.len(), "{what}: length");
    let mut worst = 0.0f64;
    let mut differ = 0;
    for (g, w) in got.iter().zip(want) {
        if g.to_bits() != w.to_bits() {
            differ += 1;
        }
        worst = worst.max((g - w).abs());
    }
    (worst, differ)
}

fn check(what: &str, got: &[f64], want: &[f64], tolerance: f64) {
    let (worst, differ) = compare(what, got, want);
    println!("{what}: {} values, {differ} not bit-identical, max |diff| {worst:e}", got.len());
    assert!(worst <= tolerance, "{what}: max |diff| {worst:e} > {tolerance:e}");
}

/// The harness's xorshift64 signal.
struct Noise(u64);

impl Noise {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 * (1.0 / 9007199254740992.0) * 2.0 - 1.0
    }
}

fn signal(frames: usize, channels: usize, seed: u64) -> Vec<f64> {
    let mut n = Noise(seed);
    let mut v: Vec<f64> = (0..frames * channels).map(|_| 0.5 * n.next()).collect();
    for f in (0..frames).step_by(2000) {
        v[f * channels] = 0.9;
    }
    v
}

fn band(kind: FilterType, freq_hz: f64, gain_db: f64, q: f64) -> Band {
    Band { kind, freq_hz, gain_db, q, enabled: true }
}

fn off(b: Band) -> Band {
    Band { enabled: false, ..b }
}

const BUILTINS: [[f64; 10]; 13] = [
    [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
    [6.0, 5.0, 4.0, 2.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0],
    [-6.0, -5.0, -4.0, -2.0, -0.5, 0.0, 0.0, 0.0, 0.0, 0.0],
    [0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 2.0, 4.0, 5.0, 6.0],
    [0.0, 0.0, 0.0, 0.0, 0.0, -1.0, -2.0, -4.0, -5.0, -6.0],
    [-2.0, -2.0, -1.0, 1.0, 3.0, 3.0, 2.0, 1.0, 0.0, -1.0],
    [4.0, 3.0, 2.0, 0.0, -1.0, -1.0, 1.0, 2.0, 3.0, 4.0],
    [-1.0, 1.0, 3.0, 4.0, 3.0, 0.0, -1.0, -1.0, 0.0, 1.0],
    [3.0, 2.0, 1.0, 2.0, -1.0, -1.0, 0.0, 1.0, 2.0, 3.0],
    [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, -3.0, -3.0, -3.0, -5.0],
    [4.0, 3.0, 1.0, 0.0, -2.0, 2.0, 1.0, 1.0, 3.0, 4.0],
    [4.0, 4.0, 3.0, 1.0, 2.0, 1.0, 3.0, 3.0, 3.0, 2.0],
    [5.0, 4.0, 1.0, 0.0, -1.0, 0.0, 0.0, 1.0, 4.0, 5.0],
];

fn graphic_bands(sliders: &[f64; 10]) -> Vec<Band> {
    super_eq::graphic_gains(sliders)
        .iter()
        .zip(GRAPHIC_CENTERS)
        .map(|(g, f)| band(FilterType::Peaking, f, (g * 1000.0).round() / 1000.0, GRAPHIC_Q))
        .collect()
}

fn all_types() -> Vec<Band> {
    vec![
        band(FilterType::Peaking, 100.0, 6.0, 1.0),
        band(FilterType::LowShelf, 80.0, -4.0, 0.7),
        band(FilterType::HighShelf, 9000.0, 3.0, 0.7071),
        band(FilterType::LowPass, 18000.0, 0.0, 0.7071),
        band(FilterType::HighPass, 25.0, 0.0, 0.5),
        band(FilterType::BandPass, 2000.0, 0.0, 2.0),
        band(FilterType::Notch, 60.0, 0.0, 8.0),
    ]
}

#[test]
fn coefficients() {
    let mut d = Data::open("coeffs.bin");
    let types = [
        FilterType::Peaking,
        FilterType::LowShelf,
        FilterType::HighShelf,
        FilterType::LowPass,
        FilterType::HighPass,
        FilterType::BandPass,
        FilterType::Notch,
    ];
    let (mut got, mut want) = (Vec::new(), Vec::new());
    for t in types {
        for r in [44100.0, 48000.0, 192000.0, 8000.0] {
            for f in [20.0, 100.0, 1000.0, 5000.0, 15000.0, 21000.0] {
                for q in [0.3, 0.7071, 1.41421356237, 5.0, 20.0] {
                    for g in [-12.0, -3.0, 0.0, 2.5, 9.0] {
                        let c = biquad::design(t, r, f, q, g);
                        got.extend([c.b0, c.b1, c.b2, c.a1, c.a2]);
                        want.extend(d.take(5));
                    }
                }
            }
        }
    }
    d.done();
    check("biquad coefficients (4200 designs)", &got, &want, 1e-12);
}

fn slider_sets() -> Vec<[f64; 10]> {
    let mut sets = BUILTINS.to_vec();
    sets.push([12.0; 10]);
    sets.push([-12.0; 10]);
    sets.push([24.0, -24.0, 24.0, -24.0, 24.0, -24.0, 24.0, -24.0, 24.0, -24.0]);
    sets.push([24.0; 10]);
    sets.push([0.0, 0.0, 0.0, 0.0, 0.0, 6.0, 0.0, 0.0, 0.0, 0.0]);
    let mut n = Noise(12345);
    for _ in 0..12 {
        let mut s = [0.0; 10];
        for v in s.iter_mut() {
            *v = (n.next() * 240.0).round() / 10.0;
        }
        sets.push(s);
    }
    sets
}

#[test]
fn graphic_solve() {
    let mut d = Data::open("graphic.bin");
    let (mut got, mut want) = (Vec::new(), Vec::new());
    for s in slider_sets() {
        let g = super_eq::graphic_gains(&s);
        got.extend(g);
        got.extend(super_eq::graphic_response(&g));
        want.extend(d.take(20));
    }
    d.done();
    check("graphic Newton gains + response (30 slider sets)", &got, &want, 1e-12);
}

#[test]
fn peak_gain() {
    let mut d = Data::open("peak.bin");
    let mut curves: Vec<Curve> = BUILTINS.iter().map(|s| Curve { bands: graphic_bands(s), preamp_db: 0.0 }).collect();
    curves.push(Curve { bands: all_types(), preamp_db: -3.0 });
    curves.push(Curve {
        bands: vec![band(FilterType::Peaking, 15000.0, 12.0, 0.5), band(FilterType::HighShelf, 18000.0, 9.0, 0.7)],
        preamp_db: 2.0,
    });
    curves.push(Curve { bands: vec![band(FilterType::Peaking, 1000.0, 0.0, 1.0)], preamp_db: -4.5 });
    curves.push(Curve { bands: vec![off(band(FilterType::Peaking, 1000.0, 5.0, 1.0))], preamp_db: 0.0 });
    let (mut got, mut want) = (Vec::new(), Vec::new());
    for c in curves {
        let eq = SuperEq::new();
        eq.set_curve(c);
        got.push(eq.peak_gain_db());
        want.push(d.next());
        assert_eq!(eq.is_neutral(), d.next() == 1.0);
    }
    d.done();
    check("SuperEQ peakGainDb (17 curves)", &got, &want, 1e-12);
}

struct Scenario {
    rate: f64,
    channels: usize,
    frames: usize,
    a: Curve,
    b: Curve,
    change_at: i32,
    off_at: i32,
    on_at: i32,
    reset_at: i32,
}

fn scenarios() -> Vec<Scenario> {
    let mut bass = graphic_bands(&BUILTINS[1]);
    bass[2].enabled = false;
    let none = Curve::default();
    vec![
        // Off at block 8 and on again at 12. The recorded reference kept the
        // filters' memory through the pause and started again from it; the
        // processor now clears it once the fade out ends. The data from block
        // 12 on (values 10468..12000 of process.bin, 2026-11-24) was
        // recomputed with the recorded arithmetic plus a reset before block
        // 12, which is what the processor does by itself; blocks 0-11 are
        // the recording's.
        Scenario {
            rate: 48000.0,
            channels: 2,
            frames: 6000,
            a: Curve { bands: all_types(), preamp_db: -3.0 },
            b: Curve { bands: graphic_bands(&BUILTINS[6]), preamp_db: -2.5 },
            change_at: 4,
            off_at: 8,
            on_at: 12,
            reset_at: -1,
        },
        Scenario {
            rate: 44100.0,
            channels: 2,
            frames: 4000,
            a: Curve { bands: bass, preamp_db: -6.0 },
            b: none.clone(),
            change_at: -1,
            off_at: -1,
            on_at: -1,
            reset_at: -1,
        },
        Scenario {
            rate: 96000.0,
            channels: 6,
            frames: 2000,
            a: Curve {
                bands: vec![
                    band(FilterType::Peaking, 30.0, 12.0, 20.0),
                    band(FilterType::Peaking, 15000.0, -9.0, 0.3),
                    band(FilterType::LowShelf, 200.0, 6.0, 2.0),
                    band(FilterType::HighShelf, 20000.0, -12.0, 0.5),
                    band(FilterType::Peaking, 1000.0, 0.0, 1.0),
                    off(band(FilterType::Peaking, 5000.0, 3.0, 1.0)),
                ],
                preamp_db: 0.0,
            },
            b: Curve { bands: vec![band(FilterType::Peaking, 440.0, -24.0, 30.0)], preamp_db: 0.0 },
            change_at: 5,
            off_at: -1,
            on_at: -1,
            reset_at: 3,
        },
        Scenario {
            rate: 192000.0,
            channels: 1,
            frames: 3000,
            a: Curve {
                bands: vec![
                    band(FilterType::HighShelf, 90000.0, 6.0, 0.7),
                    band(FilterType::LowPass, 30000.0, 0.0, 0.7071),
                    band(FilterType::Notch, 50.0, 0.0, 30.0),
                    band(FilterType::Peaking, 23000.0, 4.0, 1.41),
                ],
                preamp_db: 2.0,
            },
            b: none.clone(),
            change_at: -1,
            off_at: -1,
            on_at: -1,
            reset_at: -1,
        },
        Scenario {
            rate: 8000.0,
            channels: 2,
            frames: 3000,
            a: Curve {
                bands: vec![
                    band(FilterType::Peaking, 10000.0, 6.0, 1.0),
                    band(FilterType::HighShelf, 3000.0, 5.0, 0.7),
                    band(FilterType::BandPass, 500.0, 0.0, 0.05),
                    band(FilterType::HighPass, 10.0, 0.0, 30.0),
                ],
                preamp_db: -1.0,
            },
            b: none.clone(),
            change_at: -1,
            off_at: -1,
            on_at: -1,
            reset_at: -1,
        },
        Scenario {
            rate: 48000.0,
            channels: 8,
            frames: 1200,
            a: Curve { bands: vec![], preamp_db: -6.0 },
            b: none,
            change_at: 3,
            off_at: -1,
            on_at: -1,
            reset_at: -1,
        },
    ]
}

#[test]
fn biquad_processing() {
    let mut d = Data::open("process.bin");
    let sizes = [512, 300, 1024, 17, 700, 64];
    for (i, sc) in scenarios().into_iter().enumerate() {
        let eq = SuperEq::new();
        eq.set_curve(sc.a.clone());
        let mut p = eq.processor();
        p.prepare(sc.rate, sc.channels);
        let mut x = signal(sc.frames, sc.channels, i as u64 + 1);
        let mut at = 0;
        let mut k = 0;
        while at < sc.frames {
            if k == sc.change_at {
                eq.set_curve(sc.b.clone());
            }
            if k == sc.off_at {
                eq.set_enabled(false);
            }
            if k == sc.on_at {
                eq.set_enabled(true);
            }
            if k == sc.reset_at {
                p.reset();
            }
            let n = sizes[k as usize % 6].min(sc.frames - at);
            p.process(&mut x[at * sc.channels..(at + n) * sc.channels]);
            at += n;
            k += 1;
        }
        let want = d.take(x.len());
        check(&format!("SuperEQ process, scenario {} ({} Hz, {} ch)", i + 1, sc.rate, sc.channels), &x, &want, 1e-12);
        assert_eq!(eq.is_neutral(), d.next() == 1.0, "scenario {}: neutral", i + 1);
    }
    d.done();
}

#[test]
fn linear_phase() {
    let mut d = Data::open("linear.bin");
    let designs = [
        (graphic_bands(&BUILTINS[6]), -2.0, 48000.0),
        (vec![band(FilterType::Notch, 40.0, 0.0, 10.0), band(FilterType::Peaking, 100.0, -6.0, 4.0)], 0.0, 44100.0),
        (vec![band(FilterType::LowPass, 20000.0, 0.0, 0.7071)], -6.0, 96000.0),
    ];
    for (i, (bands, preamp, rate)) in designs.iter().enumerate() {
        let taps = linear_phase::taps_for(*rate, bands);
        assert_eq!(taps as f64, d.next(), "design {}: taps", i + 1);
        let fir = linear_phase::design(bands, *preamp, *rate, taps);
        let got: Vec<f64> = fir.iter().step_by(7).copied().collect();
        let want = d.take(got.len());
        check(&format!("linear-phase FIR {} ({taps} taps at {rate} Hz, every 7th)", i + 1), &got, &want, 1e-13);
    }
    for r in [8000.0, 44100.0, 48000.0, 96000.0, 192000.0, 384000.0] {
        assert_eq!(linear_phase::taps_for_rate(r) as f64, d.next(), "{r} Hz");
    }

    let eq = SuperEq::new();
    eq.set_linear_phase(true);
    let mut lin = LinearPhaseEq::new();
    lin.prepare(48000, 2);
    let (block, blocks) = (1000, 150);
    let x = signal(block * blocks, 2, 99);
    let (mut stream, mut sizes) = (Vec::new(), Vec::new());
    let a = Curve { bands: graphic_bands(&BUILTINS[6]), preamp_db: -2.0 };
    let b = Curve { bands: vec![band(FilterType::Peaking, 300.0, 4.0, 2.0)], preamp_db: -1.0 };
    let flat = Curve { bands: super_eq::graphic_10_band(), preamp_db: 0.0 };
    for k in 0..blocks {
        if k == 10 || k == 100 {
            eq.set_curve(a.clone());
        }
        if k == 40 {
            eq.set_curve(b.clone());
        }
        if k == 70 {
            eq.set_curve(flat.clone());
        }
        let s = &x[k * block * 2..(k + 1) * block * 2];
        lin.update(!eq.is_neutral(), &eq);
        lin.remember(s);
        if !lin.active() {
            stream.extend_from_slice(s);
            sizes.extend([-1.0, -1.0]);
            continue;
        }
        lin.process(s);
        stream.extend_from_slice(lin.filtered());
        stream.extend_from_slice(lin.passed());
        sizes.extend([(lin.filtered().len() / 2) as f64, (lin.passed().len() / 2) as f64]);
    }
    assert_eq!(stream.len() as f64, d.next(), "stream length");
    assert_eq!(sizes, d.take(sizes.len()), "per-block filtered / passed frames");
    let got: Vec<f64> = stream.iter().step_by(11).copied().collect();
    let want = d.take(got.len());
    d.done();
    check("linear-phase engine stream (engage, swap, drain, re-engage; every 11th)", &got, &want, 1e-13);
}

#[test]
fn controller_response() {
    let mut d = Data::open("response.bin");
    let dir = std::env::temp_dir().join(format!("sonkore-eq-response-{}", std::process::id()));
    let settings = Settings::open(dir.join("none.conf"));
    let mut cases: Vec<EqController> = Vec::new();
    for gains in BUILTINS {
        let mut c = EqController::new(SuperEq::new(), &settings, &dir);
        for (i, g) in gains.iter().enumerate() {
            c.set_gain(i, *g);
        }
        cases.push(c);
    }
    {
        let mut c = EqController::new(SuperEq::new(), &settings, &dir);
        c.set_mode(Mode::Parametric);
        while !c.bands().is_empty() {
            c.remove_band(0);
        }
        let names = ["PK", "LSC", "HSC", "LPQ", "HPQ", "BP", "NO"];
        for (b, name) in all_types().iter().zip(names) {
            c.add_band();
            let i = c.bands().len() - 1;
            c.set_band(i, name, b.freq_hz, b.gain_db, b.q, b.enabled);
        }
        c.add_band();
        c.set_band(7, "PK", 3000.0, 4.0, 2.0, false);
        c.set_preamp(-2.0);
        cases.push(c);
    }
    {
        let mut c = EqController::new(SuperEq::new(), &settings, &dir);
        for (i, g) in BUILTINS[10].iter().enumerate() {
            c.set_gain(i, g * 1.7);
            if i == 4 {
                let b = c.bands()[4].clone();
                c.set_band(4, &b.kind, b.freq, b.gain, b.q, false);
            }
        }
        c.set_preamp(1.5);
        cases.push(c);
    }
    let (mut got, mut want) = (Vec::new(), Vec::new());
    let (mut fits_got, mut fits_want) = (Vec::new(), Vec::new());
    for c in cases.iter_mut() {
        assert_eq!(c.bands().len(), if c.mode() == Mode::Graphic { 10 } else { 8 });
        for p in c.response() {
            got.extend([p.f, p.db]);
        }
        want.extend(d.take(384));
        got.push(c.peak_gain());
        want.push(d.next());
        // autoPreamp
        let mut probe = EqController::new(SuperEq::new(), &settings, &dir);
        probe.restore(&c.snapshot());
        probe.auto_preamp();
        got.push(probe.preamp());
        want.push(d.next());
        // Switching to graphic fits the ten centres.
        let mut fit = EqController::new(SuperEq::new(), &settings, &dir);
        fit.restore(&c.snapshot());
        let want_fit = d.take(10);
        if fit.mode() == Mode::Parametric {
            fit.set_mode(Mode::Graphic);
            fits_got.extend(fit.bands().iter().map(|b| b.gain));
            fits_want.extend(want_fit);
        }
    }
    d.done();
    check("controller response (192 points) + peakGain + autoPreamp, 15 curves", &got, &want, 1e-12);
    check("controller graphic fit", &fits_got, &fits_want, 0.0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn engine_bands_are_the_solved_gains() {
    let dir = std::env::temp_dir().join(format!("sonkore-eq-engine-{}", std::process::id()));
    let settings = Settings::open(dir.join("none.conf"));
    let eq = SuperEq::new();
    let mut c = EqController::new(eq.clone(), &settings, &dir);
    c.load_profile("Rock");
    let want = graphic_bands(&BUILTINS[6]);
    let got = eq.bands();
    assert_eq!(got, want);
    assert_eq!(eq.preamp_db(), c.preamp());
    let _ = std::fs::remove_dir_all(&dir);
}
