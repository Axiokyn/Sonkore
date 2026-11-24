//! Damaged Equalizer APO / AutoEQ text, profiles and saved state: the
//! parsers never panic, and whatever curve comes out of them the engine's
//! filters stay finite (fuzz-lite, see tests/fuzz_lite.rs).

#[path = "../../../tests/fuzz_lite.rs"]
mod fuzz_lite;

use std::path::Path;
use std::time::Duration;

use fuzz_lite::{Case, Rng, run, scaled};
use sonkore_core::settings::Settings;
use sonkore_eq::autoeq::Index;
use sonkore_eq::super_eq::{self, Curve};
use sonkore_eq::{EqController, LinearPhaseEq, SuperEq, apo};

#[global_allocator]
static GUARD: fuzz_lite::Guard = fuzz_lite::Guard;

const STACK: usize = 512 << 10;
const LIMIT: Duration = Duration::from_secs(5);

fn data(name: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data").join(name)).unwrap()
}

/// Numbers that break things, in the ways people and programs write them.
const NUMBERS: [&str; 26] = [
    "0",
    "-0",
    "0.0",
    "-1",
    "1e308",
    "-1e308",
    "1e-320",
    "1e999",
    "NaN",
    "nan",
    "inf",
    "-inf",
    "Infinity",
    "99999999999999999999999",
    "4294967296",
    "-24",
    "24",
    "36",
    "30.0001",
    "0.049",
    "24000.1",
    "1.5.5",
    ".",
    "+",
    "1,5",
    "0x10",
];

/// Byte mutations, or numbers and keywords swapped in.
fn mutate_text(rng: &mut Rng, text: &str) -> Vec<u8> {
    if rng.chance(40) {
        return fuzz_lite::mutate(rng, text.as_bytes());
    }
    let mut words: Vec<String> = text.split(' ').map(str::to_owned).collect();
    let keywords = [
        "Preamp:", "Filter", "Filter:", "ON", "OFF", "PK", "PEQ", "LS", "LSC", "LSQ", "HS", "HSC", "HSQ", "LP", "LPQ",
        "HP", "HPQ", "BP", "NO", "AP", "Fc", "Hz", "Gain", "dB", "Q", "BW", "Oct", "\n", "\r\n", "\t",
    ];
    for _ in 0..1 + rng.below(6) {
        let i = rng.below(words.len());
        match rng.below(4) {
            0 | 1 => words[i] = rng.pick(&NUMBERS).to_string(),
            2 => words[i] = rng.pick(&keywords).to_string(),
            _ => {
                let w = words[i].clone();
                let times = 1 + rng.below(40);
                for _ in 0..times {
                    words.insert(i, w.clone());
                }
            }
        }
    }
    words.join(" ").into_bytes()
}

/// JSON that stays JSON: values swapped for odd numbers, other types,
/// copies of themselves; or byte mutations now and then.
fn mutate_json(rng: &mut Rng, text: &str) -> Vec<u8> {
    if rng.chance(15) {
        return fuzz_lite::mutate(rng, text.as_bytes());
    }
    const VALUES: [&str; 18] = [
        "0",
        "-0",
        "-1",
        "1e308",
        "-1e308",
        "1e-320",
        "99999999999999999999",
        "-9223372036854775809",
        "null",
        "true",
        "\"NaN\"",
        "\"inf\"",
        "\"-1e999\"",
        "[]",
        "{}",
        "\"\"",
        "[[[[[[[[[[1]]]]]]]]]]",
        "\"\\u0000\"",
    ];
    let mut s = text.to_owned();
    for _ in 0..1 + rng.below(5) {
        let colons: Vec<usize> = s.match_indices(':').map(|(i, _)| i).collect();
        if colons.is_empty() {
            break;
        }
        let at = *rng.pick(&colons) + 1;
        let rest = &s[at..];
        let mut depth = 0i32;
        let mut end = rest.len();
        for (i, c) in rest.char_indices() {
            match c {
                '[' | '{' => depth += 1,
                ']' | '}' if depth == 0 => {
                    end = i;
                    break;
                }
                ']' | '}' => depth -= 1,
                ',' if depth == 0 => {
                    end = i;
                    break;
                }
                _ => {}
            }
        }
        let value = if rng.chance(15) {
            // The value many times over, in an array.
            let v = rest[..end].to_owned();
            format!("[{}]", vec![v; 1 + rng.below(100)].join(","))
        } else {
            rng.pick(&VALUES).to_string()
        };
        s.replace_range(at..at + end, &value);
    }
    s.into_bytes()
}

/// The curve through the real-time cascade and the linear-phase FIR at a
/// few rates: every output sample finite.
fn run_curve(preamp: f64, bands: &[sonkore_eq::Band], linear: bool) {
    let engine_bands: Vec<super_eq::Band> = bands
        .iter()
        .map(|b| super_eq::Band { kind: b.filter_type(), freq_hz: b.freq, gain_db: b.gain, q: b.q, enabled: b.enabled })
        .collect();
    let eq = SuperEq::new();
    eq.set_curve(Curve { bands: engine_bands, preamp_db: preamp });
    eq.set_enabled(true);
    let _ = eq.peak_gain_db();
    let _ = eq.is_neutral();
    let mut rng = Rng::new(7);
    let input: Vec<f64> = (0..512 * 2).map(|_| (rng.next() as i64 as f64) / 9.3e18).collect();
    for rate in [8000.0, 44100.0, 384_000.0] {
        let mut p = eq.processor();
        p.prepare(rate, 2);
        let mut block = input.clone();
        for _ in 0..4 {
            p.process(&mut block);
        }
        if let Some(bad) = block.iter().position(|s| !s.is_finite()) {
            panic!("the biquads gave {} at {rate} Hz (sample {bad}) for {bands:?}, preamp {preamp}", block[bad]);
        }
    }
    if linear {
        eq.set_linear_phase(true);
        let mut l = LinearPhaseEq::new();
        l.prepare(48000, 2);
        l.update(true, &eq);
        for _ in 0..3 {
            l.remember(&input);
            l.process(&input);
            if let Some(bad) = l.filtered().iter().position(|s| !s.is_finite()) {
                panic!("the FIR gave {} (sample {bad}) for {bands:?}, preamp {preamp}", l.filtered()[bad]);
            }
        }
    }
}

#[test]
fn apo_text_survives_damage() {
    let seeds: Vec<String> = data("apo_cases.txt").split("=====").map(|s| s.trim().to_owned()).collect();
    let make = move |n: usize| {
        let mut rng = Rng::for_case("apo", n);
        let seed = &seeds[n % seeds.len()];
        Case {
            label: format!("APO text {} (case {n})", n % seeds.len()),
            ext: "txt".into(),
            bytes: mutate_text(&mut rng, seed),
        }
    };
    let mut n = 0usize;
    run("apo", scaled(2500), STACK, LIMIT, make, move |case: &Case| {
        n += 1;
        let text = String::from_utf8_lossy(&case.bytes);
        if let Some((preamp, bands)) = apo::parse(&text) {
            let _ = apo::export(preamp, &bands);
            // What import_file keeps, then the raw parse as well (the engine
            // sanitises again).
            let kept: Vec<_> = bands.iter().cloned().map(sonkore_eq::Band::sanitised).collect();
            run_curve(preamp, &kept, n % 50 == 0);
            run_curve(preamp, &bands, false);
        }
    });
}

#[test]
fn autoeq_index_survives_damage() {
    let index = "# AutoEq\n\
        - [Sennheiser HD 650](./oratory1990/over-ear/Sennheiser%20HD%20650) by oratory1990\n\
        - [Sony WH-1000XM4 (ANC on)](./rtings/over-ear/Sony%20WH-1000XM4) by rtings on Bose QC\n\
        - [Moondrop Aria](./crinacle/GRAS%2043AG-7/in-ear/Moondrop%20Aria) by crinacle on GRAS 43AG-7\n\
        - [](./x) by \n\
        - [a] by b\n";
    let make = move |n: usize| {
        let mut rng = Rng::for_case("autoeq", n);
        let mut bytes = fuzz_lite::mutate(&mut rng, index.as_bytes());
        if rng.chance(10) {
            // A long line of brackets.
            let n = rng.below(20000);
            bytes.extend(std::iter::repeat_n(b'[', n));
        }
        Case { label: format!("INDEX.md (case {n})"), ext: "md".into(), bytes }
    };
    run("autoeq", scaled(4000), STACK, LIMIT, make, |case: &Case| {
        let text = String::from_utf8_lossy(&case.bytes);
        let idx = Index::parse(&text);
        for q in ["", " ", "sennheiser", "hd 650 oratory", "[", "\u{0130}"] {
            let _ = idx.search(q);
        }
        for e in idx.entries() {
            let _ = e.profile_url();
        }
    });
}

#[test]
fn saved_state_and_profiles_survive_damage() {
    let dir = fuzz_lite::scratch("eq-state");
    let state = r#"{"enabled":true,"linearPhase":false,"mode":"parametric","preamp":-3.5,"profile":"Mine","bands":[{"type":"PK","freq":1000,"gain":3,"q":1.41,"enabled":true},{"type":"LSC","freq":105,"gain":5.6,"q":0.7,"enabled":true},{"type":"HPQ","freq":20,"gain":0,"q":0.5,"enabled":false}]}"#;
    let profiles = r#"[{"name":"Mine","mode":"parametric","preamp":-2,"bands":[{"type":"PK","freq":1000,"gain":3,"q":1.41,"enabled":true}]},{"name":"Graphic","mode":"graphic","preamp":0,"bands":[{"gain":1},{"gain":2},{"gain":-3}]}]"#;
    let make = move |n: usize| {
        let mut rng = Rng::for_case("eq-state", n);
        let (what, seed) = if n % 2 == 0 { ("state", state) } else { ("profiles", profiles) };
        Case { label: format!("{what} JSON (case {n})"), ext: what.into(), bytes: mutate_json(&mut rng, seed) }
    };
    let settings = Settings::open(dir.join("sonkore.conf"));
    let mut controller = EqController::new(SuperEq::new(), &settings, &dir);
    run("eq-state", scaled(2000), STACK, LIMIT, make, move |case: &Case| {
        if case.ext == "state" {
            if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&case.bytes) {
                controller.restore(&v);
                let _ = controller.response();
                let _ = controller.peak_gain();
                let _ = controller.export_text();
                let _ = controller.snapshot();
            }
        } else {
            std::fs::write(dir.join(sonkore_eq::controller::PROFILES_FILE), &case.bytes).unwrap();
            let c = EqController::new(SuperEq::new(), &settings, &dir);
            for name in c.user_profiles() {
                let _ = c.user_profile(name);
            }
        }
    });
}
