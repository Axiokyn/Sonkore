//! Throughput of the biquad EQ and the linear-phase FIR:
//! cargo run --release -p sonkore-eq --example bench

use std::time::Instant;

use sonkore_eq::super_eq::{self, Band, Curve};
use sonkore_eq::{FilterType, LinearPhaseEq, SuperEq};

fn noise(n: usize) -> Vec<f64> {
    let mut s: u64 = 1;
    (0..n)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            ((s >> 11) as f64 / 9007199254740992.0 - 0.5) * 0.5
        })
        .collect()
}

fn main() {
    let rate = 48000.0;
    let seconds = 60;
    let frames = rate as usize * seconds;
    let input = noise(frames * 2);
    let graphic = super_eq::graphic_gains(&[4.0, 3.0, 2.0, 0.0, -1.0, -1.0, 1.0, 2.0, 3.0, 4.0]);
    let ten: Vec<Band> =
        super_eq::graphic_10_band().into_iter().zip(graphic).map(|(b, g)| Band { gain_db: g, ..b }).collect();
    let many: Vec<Band> = (0..32)
        .map(|i| Band {
            kind: FilterType::Peaking,
            freq_hz: 20.0 * 1.24f64.powi(i),
            gain_db: if i % 2 == 0 { 2.0 } else { -2.0 },
            q: 2.0,
            enabled: true,
        })
        .collect();
    for (name, bands) in [("10 bands", ten.clone()), ("32 bands", many)] {
        let eq = SuperEq::new();
        eq.set_curve(Curve { bands, preamp_db: -3.0 });
        let mut p = eq.processor();
        p.prepare(rate, 2);
        let mut buf = input.clone();
        let t = Instant::now();
        for block in buf.chunks_mut(2048) {
            p.process(block);
        }
        let s = t.elapsed().as_secs_f64();
        println!(
            "biquad {name}, stereo 48 kHz: {:.2} ns/frame, {:.0}x real time (checksum {:.6})",
            s * 1e9 / frames as f64,
            seconds as f64 / s,
            buf.iter().sum::<f64>()
        );
    }
    let eq = SuperEq::new();
    eq.set_curve(Curve { bands: ten, preamp_db: -3.0 });
    let mut lin = LinearPhaseEq::new();
    lin.prepare(48000, 2);
    let t = Instant::now();
    lin.update(true, &eq);
    let engage = t.elapsed().as_secs_f64();
    let t = Instant::now();
    let mut sum = 0.0;
    for block in input.chunks(2048) {
        lin.update(true, &eq);
        lin.remember(block);
        lin.process(block);
        sum += lin.filtered().iter().sum::<f64>();
    }
    let s = t.elapsed().as_secs_f64();
    println!(
        "linear phase ({} taps), stereo 48 kHz: engage {:.1} ms, {:.2} ns/frame, {:.0}x real time (checksum {sum:.6})",
        lin.taps(),
        engage * 1e3,
        s * 1e9 / frames as f64,
        seconds as f64 / s
    );
}
