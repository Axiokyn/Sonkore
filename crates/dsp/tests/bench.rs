//! Throughput, as multiples of real time on one core (run with
//! `cargo test --release -p sonkore-dsp --test bench -- --ignored --nocapture`).

mod common;

use std::path::PathBuf;
use std::time::Instant;

use common::noise;
use sonkore_dsp::convolver::{Convolver, ImpulseResponse};
use sonkore_dsp::crossfeed::{Crossfeed, Preset};
use sonkore_dsp::limiter::Limiter;
use sonkore_dsp::resampler::{Filter, Resampler};

const SECONDS: usize = 20;

fn report(name: &str, rate: u32, start: Instant) {
    let t = start.elapsed().as_secs_f64();
    println!(
        "{name}: {:.0}× real time ({:.1} ms per second of audio)",
        SECONDS as f64 / t,
        t * 1000.0 / SECONDS as f64
    );
    let _ = rate;
}

#[test]
#[ignore]
fn throughput() {
    let rate = 48000u32;
    let block = 1024;
    let x = noise(2 * rate as usize * SECONDS, 1);

    let mut l = Limiter::new(rate, 2, -1.0);
    let mut out = vec![0.0; block * 2];
    let loud: Vec<f64> = x.iter().map(|v| v * 1.5).collect();
    let t = Instant::now();
    for c in loud.chunks(block * 2) {
        l.process(c, &mut out);
    }
    report("limiter 48 kHz stereo (limiting)", rate, t);

    let mut y = x.clone();
    let mut cf = Crossfeed::new(Preset::Default, rate as f64);
    let t = Instant::now();
    for c in y.chunks_mut(block * 2) {
        cf.process(c);
    }
    report("crossfeed 48 kHz stereo", rate, t);

    for taps in [4096usize, 65536, 1 << 20] {
        let ir = ImpulseResponse {
            path: PathBuf::new(),
            sample_rate: rate,
            channels: 2,
            taps: vec![noise(taps, 2), noise(taps, 3)],
        };
        let mut conv = Convolver::new(&ir, rate, 2).unwrap();
        let mut y = x[..x.len() / if taps > 100_000 { 10 } else { 1 }].to_vec();
        let secs = y.len() / 2 / rate as usize;
        let t = Instant::now();
        for c in y.chunks_mut(block * 2) {
            conv.process(c);
        }
        let el = t.elapsed().as_secs_f64();
        println!("convolver 48 kHz stereo, {taps} taps: {:.0}× real time", secs as f64 / el);
    }

    for (i, o, f) in [
        (44100u32, 48000u32, Filter::Linear),
        (44100, 48000, Filter::LinearSteep),
        (48000, 44100, Filter::Linear),
        (44100, 192000, Filter::Linear),
        (44100, 705600, Filter::LinearSteep),
        (192000, 44100, Filter::LinearSteep),
    ] {
        let x = noise(2 * i as usize * SECONDS, 4);
        let mut r = Resampler::new(i, o, 2, f).unwrap();
        let mut out = Vec::with_capacity(r.max_output_frames(block) * 2);
        let t = Instant::now();
        for c in x.chunks(block * 2) {
            r.process(c, &mut out);
        }
        report(&format!("resampler {i} -> {o} stereo {f:?}"), i, t);
    }
    let x = noise(2 * 352800 * SECONDS, 5);
    let mut r = Resampler::new_lowpass(352800, 88200, 2, 30000.0 / 44100.0, 1.0).unwrap();
    let mut out = Vec::new();
    let t = Instant::now();
    for c in x.chunks(4096 * 2) {
        r.process(c, &mut out);
    }
    report("low-pass 352.8 k -> 88.2 k stereo (DSD64 stage 2)", 352800, t);
}
