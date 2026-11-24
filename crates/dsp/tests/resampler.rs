//! The resampler's timing and bookkeeping: the output lines up with the
//! input (no delay left), has exactly round(n · out / in) frames, does not
//! depend on how the input is cut into blocks, and reports soxr's delay.

mod common;

use common::{channel, noise, resample_all, sine};
use sonkore_dsp::resampler::{Filter, Resampler};

/// Worst difference from the ideal (analytic) sine, away from the edges.
fn alignment_error(r: &mut Resampler, in_rate: u32, out_rate: u32, hz: f64) -> f64 {
    let n = in_rate as usize; // 1 s
    let x = sine(hz, in_rate as f64, n, r.channels(), 0.5);
    let y = resample_all(r, &x, 1000);
    let ch = r.channels();
    let frames = y.len() / ch;
    let edge = out_rate as usize / 10;
    let mut worst: f64 = 0.0;
    for c in 0..ch {
        let y = channel(&y, ch, c);
        for (i, v) in y.iter().enumerate().take(frames - edge).skip(edge) {
            let ideal = 0.5 * (2.0 * std::f64::consts::PI * hz * i as f64 / out_rate as f64).sin();
            worst = worst.max((v - ideal).abs());
        }
    }
    worst
}

const PAIRS: [(u32, u32); 10] = [
    (44100, 48000),
    (48000, 44100),
    (44100, 96000),
    (96000, 44100),
    (44100, 88200),
    (88200, 44100),
    (48000, 192000),
    (192000, 48000),
    (44100, 705600),
    (32000, 44100),
];

#[test]
fn output_lines_up_with_input() {
    for (i, o) in PAIRS {
        for filter in [Filter::Linear, Filter::LinearSteep] {
            let mut r = Resampler::new(i, o, 2, filter).unwrap();
            let e = alignment_error(&mut r, i, o, 997.0);
            println!("{i} -> {o} {filter:?}: worst error {e:.2e}");
            // A one-sample offset at 1 kHz would be ~0.07; 1e-9 is a timing
            // error under 1e-8 samples.
            assert!(e < 1e-9, "{i} -> {o}: {e}");
        }
    }
}

#[test]
fn lowpass_lines_up_with_input() {
    // DSD→PCM's second stages, and a few non-integer ratios.
    for (i, o, pass, stop) in [
        (352800u32, 88200u32, 30000.0 / 44100.0, 1.0),
        (352800, 176400, 50000.0 / 88200.0, 70000.0 / 88200.0),
        (705600, 176400, 70000.0 / 88200.0, 1.0),
        (44100, 48000, 0.9, 1.0),
        (48000, 44100, 0.8, 0.95),
        (96000, 96000, 0.5, 0.6),
    ] {
        let mut r = Resampler::new_lowpass(i, o, 2, pass, stop).unwrap();
        let e = alignment_error(&mut r, i, o, 997.0);
        println!("low-pass {i} -> {o} ({pass:.3}..{stop:.3}): worst error {e:.2e}");
        assert!(e < 1e-8, "{i} -> {o}: {e}");
    }
}

#[test]
fn exact_output_length_and_delay() {
    for (i, o) in PAIRS {
        for n in [0usize, 1, 7, 1000, 12345, 100_000] {
            let mut r = Resampler::new(i, o, 1, Filter::Linear).unwrap();
            let x = noise(n, 3);
            let mut out = Vec::new();
            let got = r.process(&x, &mut out);
            let held = r.delay_frames();
            let expect_delay = n as f64 * o as f64 / i as f64 - got as f64;
            assert!((held - expect_delay).abs() < 1e-6);
            let mut total = got;
            loop {
                let d = r.drain(&mut out);
                if d == 0 {
                    break;
                }
                total += d;
            }
            let want = ((n as u128 * o as u128 * 2 + i as u128) / (2 * i as u128)) as usize;
            assert_eq!(total, want, "{i} -> {o}, {n} frames");
            assert_eq!(r.delay_frames(), 0.0);
        }
    }
}

#[test]
fn blocks_do_not_matter() {
    let x = noise(3 * 30_000, 11);
    for (i, o) in [(44100u32, 48000u32), (96000, 44100)] {
        let reference = resample_all(&mut Resampler::new(i, o, 3, Filter::Linear).unwrap(), &x, 30_000);
        for block in [1usize, 17, 1024, 4099] {
            let y = resample_all(&mut Resampler::new(i, o, 3, Filter::Linear).unwrap(), &x, block);
            assert_eq!(y, reference, "block {block}");
        }
        // And after clear() it starts over identically.
        let mut r = Resampler::new(i, o, 3, Filter::Linear).unwrap();
        let _ = resample_all(&mut r, &x[..3000], 512);
        r.clear();
        assert_eq!(resample_all(&mut r, &x, 777), reference);
    }
}

#[test]
fn reserved_output_never_grows() {
    let mut r = Resampler::new(44100, 192000, 8, Filter::LinearSteep).unwrap();
    let block = 1024;
    let mut out = Vec::with_capacity(r.max_output_frames(block) * 8);
    let cap = out.capacity();
    let x = noise(block * 8, 5);
    for _ in 0..50 {
        r.process(&x, &mut out);
        assert_eq!(out.capacity(), cap);
    }
    r.drain(&mut out);
    assert_eq!(out.capacity(), cap);
}
