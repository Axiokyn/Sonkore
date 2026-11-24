//! The playing track's waveform for the "wave" seek bar:
//! the file decoded once on a worker thread into BUCKETS levels (0..1,
//! against the track's own loudest, squared so the quiet parts stand out).
//! Nothing is read while no bar shows the waveform. The last few tracks are
//! kept.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub const BUCKETS: usize = 600;
const CACHE: usize = 8;

#[derive(Default)]
struct State {
    path: String,
    active: bool,
    peaks: Vec<f32>,
    cache: VecDeque<(String, Vec<f32>)>, // oldest first
    generation: u64,
    cancel: Option<Arc<AtomicBool>>,
    listener: Option<Box<dyn Fn()>>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

/// Decodes the whole file: the RMS (all channels) of every chunk of 4096
/// frames, each bucket the loudest of its chunks. RMS rather than peaks: on
/// loud masters the peaks all touch full scale.
fn measure(path: &str, cancel: &AtomicBool) -> Vec<f32> {
    if sonkore_dsd::is_dsd_extension(Path::new(path)) {
        return Vec::new(); // DSD is sent as it is: no waveform
    }
    let Ok(mut decoder) = sonkore_engine::decode::open(Path::new(path)) else { return Vec::new() };
    let channels = decoder.info().channels;
    if channels == 0 {
        return Vec::new();
    }
    const CHUNK: usize = 4096;
    let mut buffer = vec![0i32; CHUNK * channels];
    let mut levels = Vec::new();
    while !cancel.load(Ordering::Relaxed) {
        let got = decoder.read_i32(&mut buffer).unwrap_or(0);
        if got == 0 {
            break;
        }
        let samples = &buffer[..got * channels];
        let sum: f64 = samples.iter().map(|&v| (v as f64) * (v as f64)).sum();
        levels.push((sum / samples.len() as f64).sqrt());
        if got < CHUNK {
            break;
        }
    }
    if cancel.load(Ordering::Relaxed) || levels.is_empty() {
        return Vec::new();
    }
    let mut loudest = vec![0f64; BUCKETS];
    for (c, &l) in levels.iter().enumerate() {
        let b = (c * BUCKETS / levels.len()).min(BUCKETS - 1);
        loudest[b] = loudest[b].max(l);
    }
    // Fewer chunks than buckets (a very short file): fill the gaps.
    if levels.len() < BUCKETS {
        for b in 1..BUCKETS {
            if loudest[b] == 0.0 {
                loudest[b] = loudest[b - 1];
            }
        }
    }
    let top = loudest.iter().copied().fold(1.0, f64::max);
    loudest.iter().map(|&v| ((v / top) * (v / top)) as f32).collect()
}

/// Called when the peaks change (on the UI thread).
pub fn on_change(f: impl Fn() + 'static) {
    STATE.with(|s| s.borrow_mut().listener = Some(Box::new(f)));
}

/// The peaks of the track shown (empty while it is read, or none).
pub fn peaks() -> Vec<f32> {
    STATE.with(|s| s.borrow().peaks.clone())
}

/// Whether a bar shows the waveform (the seek bar's style is "wave").
pub fn set_active(on: bool) {
    let changed = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let was = s.active;
        s.active = on;
        was != on
    });
    if changed {
        load();
    }
}

/// The track to show; "" clears it.
pub fn set_path(path: &str) {
    let changed = STATE.with(|s| {
        let mut s = s.borrow_mut();
        if s.path == path {
            return false;
        }
        s.path = path.to_owned();
        true
    });
    if changed {
        load();
    }
}

fn notify() {
    let listener = STATE.with(|s| s.borrow_mut().listener.take());
    if let Some(f) = listener {
        f();
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            if s.listener.is_none() {
                s.listener = Some(f);
            }
        });
    }
}

fn load() {
    let start = STATE.with(|s| {
        let mut s = s.borrow_mut();
        if let Some(c) = s.cancel.take() {
            c.store(true, Ordering::Relaxed);
        }
        s.generation += 1;
        let path = if s.active { s.path.clone() } else { String::new() };
        if path.is_empty() {
            s.peaks.clear();
            return None;
        }
        if let Some((_, p)) = s.cache.iter().find(|(k, _)| *k == path) {
            s.peaks = p.clone();
            return None;
        }
        s.peaks.clear();
        let cancel = Arc::new(AtomicBool::new(false));
        s.cancel = Some(cancel.clone());
        Some((path, cancel, s.generation))
    });
    notify();
    let Some((path, cancel, generation)) = start else { return };
    std::thread::spawn(move || {
        let peaks = measure(&path, &cancel);
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let _ = slint::invoke_from_event_loop(move || finished(generation, path, peaks));
    });
}

fn finished(generation: u64, path: String, peaks: Vec<f32>) {
    let current = STATE.with(|s| {
        let mut s = s.borrow_mut();
        if s.generation != generation {
            return false;
        }
        if !peaks.is_empty() {
            s.cache.retain(|(k, _)| *k != path);
            s.cache.push_back((path, peaks.clone()));
            while s.cache.len() > CACHE {
                s.cache.pop_front();
            }
        }
        s.peaks = peaks;
        s.cancel = None;
        true
    });
    if current {
        notify();
    }
}
