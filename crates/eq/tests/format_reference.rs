//! Text handling as earlier Sonkore versions did it, so their files read and
//! write the same: tests/data/formats.txt is the recorded reference (Equalizer
//! APO parsing and export, JSON output, %g / %f numbers, frequency labels)
//! for the inputs in tests/data/apo_cases.txt.

#![allow(clippy::approx_constant)]

use std::path::PathBuf;

use serde_json::{Value, json};
use sonkore_eq::controller::frequency_label;
use sonkore_eq::{Band, apo, numfmt};

fn data(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data").join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// C's "%a" text back to the double.
fn hex_float(s: &str) -> f64 {
    let (negative, s) = s.strip_prefix('-').map_or((false, s), |r| (true, r));
    let s = s.strip_prefix("0x").expect("0x");
    let (mantissa, exponent) = s.split_once('p').expect("p");
    let (int, frac) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let mut bits: u64 = u64::from_str_radix(int, 16).unwrap();
    for c in frac.chars() {
        bits = bits * 16 + c.to_digit(16).unwrap() as u64;
    }
    let exponent: i32 = exponent.parse().unwrap();
    let e = exponent - 4 * frac.len() as i32;
    let v = bits as f64 * 2f64.powi(e / 2) * 2f64.powi(e - e / 2); // in two steps: subnormals

    if negative { -v } else { v }
}

#[test]
fn hex_floats() {
    assert_eq!(hex_float("0x1.4p+0"), 1.25);
    assert_eq!(hex_float("-0x1.1p+2"), -4.25);
    assert_eq!(hex_float("0x1.999999999999ap-5"), 0.05);
    assert_eq!(hex_float("0x0p+0"), 0.0);
}

#[test]
fn apo_import_export_json_and_numbers_match_reference() {
    let inputs = data("apo_cases.txt");
    let cases: Vec<&str> = inputs.split("\n=====\n").collect();
    let reference = data("formats.txt");
    let mut lines = reference.split('\n');
    let mut next = || lines.next().expect("formats.txt ended early");
    let mut profiles = Vec::new();

    for (index, case) in cases.iter().enumerate() {
        let parsed = apo::parse(case);
        // "case <i> ok|none preamp <a> bands <n>"
        let head: Vec<&str> = next().split(' ').collect();
        // Without filters the reference still reports the preamp it read (its
        // callers drop it); here there is nothing to report.
        let (preamp, bands) = parsed.clone().unwrap_or((hex_float(head[4]), Vec::new()));
        assert_eq!(head[1], index.to_string());
        assert_eq!(head[2] == "ok", parsed.is_some(), "case {index}: found");
        assert_eq!(preamp.to_bits(), hex_float(head[4]).to_bits(), "case {index}: preamp");
        assert_eq!(bands.len().to_string(), head[6], "case {index}: band count");
        for (i, b) in bands.iter().enumerate() {
            let f: Vec<&str> = next().split(' ').collect();
            let want = Band {
                kind: f[1].to_owned(),
                freq: hex_float(f[2]),
                gain: hex_float(f[3]),
                q: hex_float(f[4]),
                enabled: f[5] == "1",
            };
            assert_eq!(*b, want, "case {index}, band {i}");
        }
        assert_eq!(next(), "export");
        let mut export = String::new();
        loop {
            let l = next();
            if l == "end" {
                break;
            }
            export.push_str(l);
            export.push('\n');
        }
        assert_eq!(apo::export(preamp, &bands), export, "case {index}: export");
        let list = Value::Array(bands.iter().map(Band::to_json).collect());
        assert_eq!(format!("json {}", numfmt::to_json(&list, true)), next(), "case {index}: compact JSON");
        // Round trip: the exported text reads back as what it says (case 2
        // is not: its odd values export as "1.23457e+08", which the format's
        // own pattern does not read back, in the reference too).
        if !bands.is_empty() && index != 2 {
            let (p2, b2) = apo::parse(&export).unwrap();
            assert_eq!(apo::export(p2, &b2), export, "case {index}: re-export");
        }
        profiles.push(json!({
            "name": format!("Case \"{index}\" · é"),
            "mode": if index % 2 == 1 { "graphic" } else { "parametric" },
            "preamp": preamp,
            "bands": list,
        }));
    }
    profiles.push(json!({"name": "Empty", "mode": "parametric", "preamp": -0.5, "bands": []}));
    assert_eq!(next(), "profiles");
    let mut indented = String::new();
    loop {
        let l = next();
        if l == "end" {
            break;
        }
        indented.push_str(l);
        indented.push('\n');
    }
    assert_eq!(numfmt::to_json(&Value::Array(profiles), false), indented, "indented profiles");

    let mut numbers = 0;
    let mut labels = 0;
    for line in lines.by_ref() {
        let f: Vec<&str> = line.split(' ').collect();
        match f[0] {
            "number" => {
                let v = hex_float(f[1]);
                let got = [
                    numfmt::number_f(v, 0),
                    numfmt::number_f(v, 1),
                    numfmt::number_f(v, 2),
                    numfmt::number_g(v, 3),
                    numfmt::number_g(v, 4),
                    numfmt::number_g(v, 6),
                ];
                assert_eq!(got.to_vec(), f[2..].to_vec(), "number formatting of {v}");
                numbers += 1;
            }
            "label" => {
                let v = hex_float(f[1]);
                assert_eq!(frequency_label(v), f[2], "frequencyLabel({v})");
                labels += 1;
            }
            "" => {}
            other => panic!("unexpected line {other}"),
        }
    }
    assert_eq!((numbers, labels), (33, 17));
}
