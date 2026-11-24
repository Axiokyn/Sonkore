//! Shared by the reference tests: the cases of `data/cases.txt`, the test
//! signals and the two drivers, each a line-for-line twin of the harness
//! that recorded the reference, so both sides see the same calls in the
//! same order.

#![allow(dead_code)]

use std::collections::HashMap;

use sonkore_soundtouch::{CHUNK_FRAMES, Interpolation, SoundTouch, TimeStretch};

pub struct Event {
    pub block: usize,
    pub key: String,
    pub value: f64,
}

pub struct Case {
    kv: HashMap<String, String>,
    pub events: Vec<Event>,
}

impl Case {
    pub fn name(&self) -> &str {
        self.kv.get("name").map_or("?", String::as_str)
    }
    pub fn get<'a>(&'a self, k: &str, d: &'a str) -> &'a str {
        self.kv.get(k).map_or(d, String::as_str)
    }
    pub fn num(&self, k: &str, d: f64) -> f64 {
        self.kv.get(k).map_or(d, |v| v.parse().expect("number"))
    }
    pub fn has(&self, k: &str) -> bool {
        self.kv.contains_key(k)
    }
}

pub fn cases() -> Vec<Case> {
    let text = include_str!("../data/cases.txt");
    let mut out = Vec::new();
    for line in text.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let kv: HashMap<String, String> = line
            .split_whitespace()
            .filter_map(|t| t.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
            .collect();
        if kv.is_empty() {
            continue;
        }
        let events = kv
            .get("ev")
            .map(|ev| {
                ev.split(';')
                    .map(|item| {
                        let mut p = item.splitn(3, ':');
                        Event {
                            block: p.next().unwrap().parse().unwrap(),
                            key: p.next().unwrap().to_string(),
                            value: p.next().unwrap().parse().unwrap(),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.push(Case { kv, events });
    }
    out
}

/// Deterministic sine without libm (the harness has the same one).
fn dsin(x: f64) -> f64 {
    let twopi = std::f64::consts::TAU; // == the harness's 6.283185307179586
    let k = (x / twopi + 0.5).floor();
    let r = x - k * twopi;
    let r2 = r * r;
    let mut t = r;
    let mut s = r;
    for i in 1..=8 {
        t = -t * r2 / ((2 * i) * (2 * i + 1)) as f64;
        s += t;
    }
    s
}

struct Rng(u32);

impl Rng {
    fn next(&mut self) -> f64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x as i32 as f64 / 2147483648.0 * 0.5
    }
}

pub fn make_signal(sig: &str, sr: u32, ch: usize, frames: usize) -> Vec<f32> {
    let twopi = std::f64::consts::TAU; // == the harness's 6.283185307179586
    let srf = sr as f64;
    let mut out = vec![0.0f32; frames * ch];
    let mut rng = Rng(0x9E3779B9);
    let mut phase = 0.0f64;
    for n in 0..frames {
        if sig == "sweep" {
            let f = 20.0 + (0.45 * srf - 20.0) * n as f64 / frames as f64;
            phase += twopi * f / srf;
        }
        for c in 0..ch {
            let cf = c as f64;
            let f1 = 220.0 * (1.0 + 0.5 * cf);
            let f2 = 3150.0 + 410.0 * cf;
            let sine = 0.45 * dsin(twopi * f1 / srf * n as f64) + 0.25 * dsin(twopi * f2 / srf * n as f64 + 0.7 * cf);
            let v = match sig {
                "sine" => sine,
                "noise" => rng.next(),
                "mix" => 0.7 * sine + 0.15 * rng.next(),
                "sweep" => 0.6 * dsin(phase + 0.3 * cf),
                "burst" => {
                    let r = rng.next();
                    if (n / (sr as usize / 10)).is_multiple_of(2) { r } else { 0.0 }
                }
                _ => 0.0,
            };
            out[n * ch + c] = v as f32;
        }
    }
    out
}

pub struct Run {
    pub channels: usize,
    pub output: Vec<f32>,
    /// Wrapper: held source frames after each block (and after the flush).
    /// Raw: frames waiting in the stretcher's input.
    pub held: Vec<f64>,
}

fn apply_wrapper(ts: &TimeStretch, k: &str, v: f64) {
    let c = ts.control();
    match k {
        "pitch" => c.set_pitch_semitones(v),
        "tempo" => c.set_tempo(v),
        "rate" => c.set_rate(v),
        "aa" => c.set_use_aa_filter(v != 0.0),
        "aalen" => c.set_aa_filter_length(v as i32),
        "quick" => c.set_use_quick_seek(v != 0.0),
        "seq" => c.set_sequence_ms(v as i32),
        "seek" => c.set_seek_window_ms(v as i32),
        "ovl" => c.set_overlap_ms(v as i32),
        "enabled" => c.set_enabled(v != 0.0),
        _ => {}
    }
}

fn apply_raw(st: &mut SoundTouch, k: &str, v: f64) {
    match k {
        "pitch" => st.set_pitch_semitones(v),
        "tempo" => st.set_tempo(v),
        "rate" => st.set_rate(v),
        "aa" => st.set_aa_filter(v != 0.0),
        "aalen" => {
            st.set_aa_filter_length(v as usize);
        }
        "quick" => st.set_quick_seek(v != 0.0),
        "seq" => st.set_sequence_ms(v as i32),
        "seek" => st.set_seek_window_ms(v as i32),
        "ovl" => st.set_overlap_ms(v as i32),
        "flush" => st.flush(),
        _ => {}
    }
}

const KEYS: [&str; 9] = ["pitch", "tempo", "rate", "aa", "aalen", "quick", "seq", "seek", "ovl"];

pub fn run(case: &Case) -> Run {
    let sr = case.num("sr", 44100.0) as u32;
    let ch = case.num("ch", 2.0) as usize;
    let frames = (case.num("secs", 1.5) * sr as f64) as usize;
    let block = case.num("block", 1024.0) as usize;
    let input = make_signal(case.get("sig", "mix"), sr, ch, frames);
    let n_blocks = frames.div_ceil(block);
    let mut output = Vec::new();
    let mut held = Vec::new();

    if case.get("api", "wrapper") == "wrapper" {
        let mut ts = TimeStretch::new(44100, 2);
        ts.control().set_enabled(true);
        for k in KEYS {
            if case.has(k) {
                apply_wrapper(&ts, k, case.num(k, 0.0));
            }
        }
        ts.prepare(sr, ch, 1024);
        let mut was = false;
        for b in 0..n_blocks {
            for e in case.events.iter().filter(|e| e.block == b) {
                if e.key == "clear" {
                    ts.clear();
                } else {
                    apply_wrapper(&ts, &e.key, e.value);
                }
            }
            ts.apply_pending_changes();
            let active = !ts.is_neutral();
            if active != was {
                if active {
                    ts.clear();
                } else {
                    ts.release();
                }
                was = active;
            }
            let n = block.min(frames - b * block);
            let src = &input[b * block * ch..(b * block + n) * ch];
            if active {
                ts.process(src, &mut output);
            } else {
                output.extend_from_slice(src);
            }
            held.push(ts.held_source_frames());
        }
        if was {
            ts.flush();
            ts.drain_into(&mut output);
        }
        held.push(ts.held_source_frames());
    } else {
        let algo = match case.get("algo", "cubic") {
            "linear" => Interpolation::Linear,
            "shannon" => Interpolation::Shannon,
            _ => Interpolation::Cubic,
        };
        let mut st = SoundTouch::with_interpolation(sr, ch, algo).expect("supported format");
        st.set_pitch_semitones(case.num("pitch", 0.0));
        st.set_tempo(case.num("tempo", 1.0));
        st.set_rate(case.num("rate", 1.0));
        for k in ["aa", "aalen", "quick", "seq", "seek", "ovl"] {
            if case.has(k) {
                apply_raw(&mut st, k, case.num(k, 0.0));
            }
        }
        let mut chunk = vec![0.0f32; CHUNK_FRAMES * ch];
        let drain = |st: &mut SoundTouch, output: &mut Vec<f32>, chunk: &mut [f32]| loop {
            let got = st.receive_samples(chunk);
            if got == 0 {
                break;
            }
            output.extend_from_slice(&chunk[..got * ch]);
        };
        for b in 0..n_blocks {
            for e in case.events.iter().filter(|e| e.block == b) {
                apply_raw(&mut st, &e.key, e.value);
            }
            let n = block.min(frames - b * block);
            st.put_samples(&input[b * block * ch..(b * block + n) * ch]);
            drain(&mut st, &mut output, &mut chunk);
            held.push(st.num_unprocessed_samples() as f64);
        }
        st.flush();
        drain(&mut st, &mut output, &mut chunk);
        held.push(st.num_unprocessed_samples() as f64);
    }
    Run { channels: ch, output, held }
}

pub fn fnv(bytes: impl IntoIterator<Item = u8>) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

pub fn hash_f32(v: &[f32]) -> u64 {
    fnv(v.iter().flat_map(|x| x.to_le_bytes()))
}

pub fn hash_f64(v: &[f64]) -> u64 {
    fnv(v.iter().flat_map(|x| x.to_le_bytes()))
}
