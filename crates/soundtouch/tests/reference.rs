//! The port against upstream SoundTouch 2.3.3 (plain float build, strict
//! IEEE) and Sonkore's TimeStretch, on the cases of `data/cases.txt`.
//!
//! `reference_hashes` always runs: every output and every held-frames trace
//! must hash to the recorded reference (`data/reference.txt`), i.e. match bit
//! for bit.
//!
//! The references were recorded earlier from upstream SoundTouch's sources
//! and Sonkore's TimeStretch stage of the time.
//!
//! `reference_dumps` compares against raw dumps and prints the error per
//! case. It runs when `SONKORE_SOUNDTOUCH_REF` names a directory of
//! `<case>.f32` / `<case>.held` files written by the reference harness; with
//! `SONKORE_SOUNDTOUCH_REPORT_ONLY=1` it only reports (for the SSE / -Ofast
//! build, which is not expected to match exactly).

mod common;

use std::collections::HashMap;

#[test]
fn reference_hashes() {
    let text = include_str!("data/reference.txt");
    let refs: HashMap<&str, (usize, &str, &str)> = text
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            (f[0], (f[1].parse().unwrap(), f[2], f[3]))
        })
        .collect();
    let mut failures = Vec::new();
    let cases = common::cases();
    for case in &cases {
        let run = common::run(case);
        let Some(&(frames, out_hash, held_hash)) = refs.get(case.name()) else {
            failures.push(format!("{}: no reference", case.name()));
            continue;
        };
        let got_frames = run.output.len() / run.channels;
        let got_out = format!("{:016x}", common::hash_f32(&run.output));
        let got_held = format!("{:016x}", common::hash_f64(&run.held));
        if got_frames != frames || got_out != out_hash || got_held != held_hash {
            failures.push(format!(
                "{}: frames {got_frames} (want {frames}), output {got_out} (want {out_hash}), held {got_held} (want {held_hash})",
                case.name()
            ));
        }
    }
    assert!(failures.is_empty(), "{} of {} cases differ:\n{}", failures.len(), cases.len(), failures.join("\n"));
}

fn read_f32(path: &str) -> Vec<f32> {
    std::fs::read(path).unwrap().as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect()
}

fn read_f64(path: &str) -> Vec<f64> {
    std::fs::read(path).unwrap().as_chunks::<8>().0.iter().map(|b| f64::from_le_bytes(*b)).collect()
}

#[test]
fn reference_dumps() {
    let Ok(dir) = std::env::var("SONKORE_SOUNDTOUCH_REF") else {
        eprintln!("SONKORE_SOUNDTOUCH_REF not set: skipped");
        return;
    };
    let report_only = std::env::var("SONKORE_SOUNDTOUCH_REPORT_ONLY").is_ok_and(|v| v == "1");
    let mut bad = 0;
    println!(
        "{:<24} {:>8} {:>8} {:>11} {:>11} {:>9} {:>9}",
        "case", "frames", "ref", "max |err|", "rel", "differ", "held err"
    );
    for case in common::cases() {
        let run = common::run(&case);
        let want = read_f32(&format!("{dir}/{}.f32", case.name()));
        let want_held = read_f64(&format!("{dir}/{}.held", case.name()));
        let n = run.output.len().min(want.len());
        let (mut max_err, mut peak, mut differ) = (0.0f64, 0.0f64, 0usize);
        for (a, b) in run.output[..n].iter().zip(&want[..n]) {
            let e = (*a as f64 - *b as f64).abs();
            max_err = max_err.max(e);
            peak = peak.max((*b as f64).abs());
            differ += (a.to_bits() != b.to_bits()) as usize;
        }
        let held_err = run.held.iter().zip(&want_held).map(|(a, b)| (a - b).abs()).fold(0.0, f64::max);
        let exact = run.output.len() == want.len() && differ == 0 && run.held == want_held;
        bad += !exact as usize;
        println!(
            "{:<24} {:>8} {:>8} {:>11.3e} {:>11.3e} {:>9} {:>9.2e}",
            case.name(),
            run.output.len() / run.channels,
            want.len() / run.channels,
            max_err,
            if peak > 0.0 { max_err / peak } else { 0.0 },
            differ,
            held_err
        );
    }
    assert!(report_only || bad == 0, "{bad} cases differ from the reference dumps");
}
