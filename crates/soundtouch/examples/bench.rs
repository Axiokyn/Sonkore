//! Throughput of the stage on 30 s of noisy stereo / mono test audio:
//! `cargo run --release -p sonkore-soundtouch --example bench [FILTER]`.

use std::time::Instant;

use sonkore_soundtouch::TimeStretch;

fn signal(sr: u32, ch: usize, frames: usize) -> Vec<f32> {
    let mut x = 0x9E3779B9u32;
    let mut out = Vec::with_capacity(frames * ch);
    for n in 0..frames {
        for c in 0..ch {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            let t = n as f64 / sr as f64;
            let v = 0.5 * (std::f64::consts::TAU * (220.0 + 110.0 * c as f64) * t).sin()
                + 0.1 * (x as i32 as f64 / 2147483648.0);
            out.push(v as f32);
        }
    }
    out
}

fn main() {
    let cases: [(&str, u32, usize, f64, f64, f64, bool); 7] = [
        ("44.1k st pitch +3", 44100, 2, 3.0, 1.0, 1.0, false),
        ("44.1k st pitch -3", 44100, 2, -3.0, 1.0, 1.0, false),
        ("44.1k st tempo 1.25", 44100, 2, 0.0, 1.25, 1.0, false),
        ("44.1k st tempo 1.25 quick", 44100, 2, 0.0, 1.25, 1.0, true),
        ("48k mono rate 1.1", 48000, 1, 0.0, 1.0, 1.1, false),
        ("96k st pitch +2 tempo 0.9", 96000, 2, 2.0, 0.9, 1.0, false),
        ("48k 6ch pitch +1", 48000, 6, 1.0, 1.0, 1.0, false),
    ];
    let filter = std::env::args().nth(1).unwrap_or_default();
    for (name, sr, ch, pitch, tempo, rate, quick) in cases {
        if !name.contains(filter.as_str()) {
            continue;
        }
        let secs = 30;
        let frames = sr as usize * secs;
        let input = signal(sr, ch, frames);
        let mut ts = TimeStretch::new(sr, ch);
        let c = ts.control().clone();
        c.set_enabled(true);
        c.set_pitch_semitones(pitch);
        c.set_tempo(tempo);
        c.set_rate(rate);
        c.set_use_quick_seek(quick);
        let mut out = Vec::with_capacity(frames * ch * 2);
        let start = Instant::now();
        ts.apply_pending_changes();
        for block in input.chunks(1024 * ch) {
            ts.process(block, &mut out);
        }
        ts.flush();
        ts.drain_into(&mut out);
        let dt = start.elapsed().as_secs_f64();
        println!("{name:<28} {:>7.2} Mframes/s  {:>6.0}x real time", frames as f64 / dt / 1e6, secs as f64 / dt);
        std::hint::black_box(&out);
    }
}
