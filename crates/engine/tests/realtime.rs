//! Real-time safety: the device thread's `render()` never allocates or frees
//! memory once playing (malloc can take a lock, page in, or wait on another
//! thread: an underrun). Checked with a counting global allocator that only
//! counts on the thread under test, while the engine plays bit-perfect,
//! through the processed path (volume, EQ, crossfeed, limiter, resampler),
//! and DSD (native and DoP), with pauses, seeks and the visualizer's tap in
//! between. The DSP chain's per-block processing is checked the same way.
//!
//! `SONKORE_ALLOC_TRACE=1` prints where the first counted allocation came
//! from (a backtrace taken inside the allocator).

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sonkore_engine::Engine;
use sonkore_engine::dsp::{Chain, Dsp};
use sonkore_engine::output::{DeviceFormat, Output, Render, Request};

// --- The counting allocator --------------------------------------------------------

thread_local! {
    /// Count this thread's allocations now.
    static ARMED: Cell<bool> = const { Cell::new(false) };
}

static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static FREES: AtomicUsize = AtomicUsize::new(0);
static FIRST: Mutex<Option<String>> = Mutex::new(None);
/// SONKORE_ALLOC_TRACE, read once (reading it allocates).
static TRACE: AtomicBool = AtomicBool::new(false);
/// Only one measurement at a time (the counters are global).
static SERIAL: Mutex<()> = Mutex::new(());

struct Counting;

fn counted(what: &str, size: usize) {
    let armed = ARMED.try_with(Cell::get).unwrap_or(false);
    if !armed {
        return;
    }
    let first =
        if what == "free" { FREES.fetch_add(1, Ordering::Relaxed) } else { ALLOCS.fetch_add(1, Ordering::Relaxed) };
    if first == 0 && TRACE.load(Ordering::Relaxed) {
        // Disarmed while the backtrace (which allocates) is taken.
        let _ = ARMED.try_with(|a| a.set(false));
        let text = format!(
            "first {what} of {size} bytes on the measured thread:\n{}",
            std::backtrace::Backtrace::force_capture()
        );
        if let Ok(mut f) = FIRST.lock()
            && f.is_none()
        {
            *f = Some(text);
        }
        // (Re-armed only once the backtrace is freed.)
        let _ = ARMED.try_with(|a| a.set(true));
    }
}

// SAFETY: the system allocator, with counters.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        counted("allocation", layout.size());
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        counted("allocation", layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        counted("reallocation", new_size);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        counted("free", layout.size());
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static COUNTING: Counting = Counting;

/// Allocations and frees made by `f` on this thread.
fn count<R>(f: impl FnOnce() -> R) -> (R, usize, usize) {
    let (a, d) = (ALLOCS.load(Ordering::Relaxed), FREES.load(Ordering::Relaxed));
    ARMED.with(|x| x.set(true));
    let r = f();
    ARMED.with(|x| x.set(false));
    (r, ALLOCS.load(Ordering::Relaxed) - a, FREES.load(Ordering::Relaxed) - d)
}

fn reset_counters() {
    TRACE.store(std::env::var_os("SONKORE_ALLOC_TRACE").is_some(), Ordering::Relaxed);
    ALLOCS.store(0, Ordering::Relaxed);
    FREES.store(0, Ordering::Relaxed);
    *FIRST.lock().unwrap() = None;
}

fn first_trace() -> String {
    FIRST.lock().unwrap().clone().unwrap_or_else(|| "(SONKORE_ALLOC_TRACE=1 shows where)".into())
}

#[test]
fn the_counter_counts() {
    // The test would pass with a broken counter: it has to see a Vec.
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    reset_counters();
    TRACE.store(false, Ordering::Relaxed); // (the backtrace would count)
    let (_, allocs, frees) = count(|| drop(std::hint::black_box(vec![1u8; 100])));
    assert_eq!((allocs, frees), (1, 1));
}

// --- A device thread that measures its render() calls ------------------------------

#[derive(Default)]
struct Measure {
    /// Count render()'s allocations from now on.
    on: AtomicBool,
    renders: AtomicUsize,
    /// Renders (while measuring) that had music in them.
    music: AtomicUsize,
    allocs: AtomicUsize,
    frees: AtomicUsize,
}

/// A device that renders on its own thread, `SPEED` times real time.
struct Device {
    format: DeviceFormat,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

const SPEED: f64 = 4.0;

impl Device {
    fn open(format: DeviceFormat, render: Arc<dyn Render>, m: Arc<Measure>) -> Device {
        let stop = Arc::new(AtomicBool::new(false));
        let s = stop.clone();
        let thread = std::thread::Builder::new()
            .name("measured-device".into())
            .spawn(move || {
                let mut block = vec![0i32; format.period_frames * format.channels];
                let period = Duration::from_secs_f64(format.period_frames as f64 / format.rate as f64 / SPEED);
                let idle = format.dsd.map(|p| sonkore_dsd::silence(p, false));
                while !s.load(Ordering::Acquire) {
                    if m.on.load(Ordering::Acquire) {
                        let ((), allocs, frees) = count(|| render.render(&mut block));
                        m.allocs.fetch_add(allocs, Ordering::Relaxed);
                        m.frees.fetch_add(frees, Ordering::Relaxed);
                        m.renders.fetch_add(1, Ordering::Relaxed);
                        let music = match idle {
                            None => block.iter().any(|&v| v != 0),
                            // DSD: not the idle pattern (DoP: its marker byte aside).
                            Some(idle) => block.iter().any(|&v| v & 0x00FF_FFFF != idle & 0x00FF_FFFF),
                        };
                        if music {
                            m.music.fetch_add(1, Ordering::Relaxed);
                        }
                    } else {
                        render.render(&mut block);
                    }
                    std::thread::sleep(period);
                }
            })
            .unwrap();
        Device { format, stop, thread: Some(thread) }
    }
}

impl Output for Device {
    fn format(&self) -> DeviceFormat {
        self.format
    }
    fn latency_frames(&self) -> u64 {
        self.format.period_frames as u64
    }
    fn key(&self) -> &str {
        "measured"
    }
    fn speed(&self) -> f64 {
        SPEED
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn engine(m: Arc<Measure>, bits: u32) -> Engine {
    Engine::with_outputs(Box::new(move |_key: &str, req: Request, render: Arc<dyn Render>| {
        let mono = req.channels == 1;
        let format = DeviceFormat {
            rate: req.rate,
            channels: if mono { 2 } else { req.channels },
            bits,
            float: false,
            mono_to_stereo: mono,
            exclusive: true,
            period_frames: 512,
            buffer_frames: 2048,
            dsd: req.dsd,
        };
        Ok(Box::new(Device::open(format, render, m.clone())) as Box<dyn Output>)
    }))
}

// --- Test files ----------------------------------------------------------------------

fn tmp(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name)
}

fn write_wav(path: &Path, rate: u32, channels: u16, frames: usize) {
    let mut data = Vec::with_capacity(frames * channels as usize * 2);
    for f in 0..frames {
        for c in 0..channels as usize {
            let v = ((f as f64 * (0.05 + 0.01 * c as f64)).sin() * 8000.0) as i16;
            data.extend_from_slice(&v.to_le_bytes());
        }
    }
    let block = channels as u32 * 2;
    let mut w = b"RIFF".to_vec();
    w.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&channels.to_le_bytes());
    w.extend_from_slice(&rate.to_le_bytes());
    w.extend_from_slice(&(rate * block).to_le_bytes());
    w.extend_from_slice(&(block as u16).to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&(data.len() as u32).to_le_bytes());
    w.extend_from_slice(&data);
    std::fs::write(path, w).unwrap();
}

/// A stereo DSD64 DSF of `seconds`, never the idle byte.
fn write_dsf(path: &Path, seconds: f64) {
    const BLOCK: usize = 4096;
    let channels = 2usize;
    let samples = (2_822_400.0 * seconds) as u64;
    let bytes_per_ch = samples.div_ceil(8) as usize;
    let blocks = bytes_per_ch.div_ceil(BLOCK);
    let mut data = Vec::with_capacity(blocks * BLOCK * channels);
    for b in 0..blocks {
        for c in 0..channels {
            for i in 0..BLOCK {
                let v = (((b * BLOCK + i) * 7 + c * 13) % 251) as u8 ^ 0x5A;
                data.push(if v == 0x69 { 0x6A } else { v });
            }
        }
    }
    let data_size = 12 + data.len() as u64;
    let mut f = b"DSD ".to_vec();
    f.extend_from_slice(&28u64.to_le_bytes());
    f.extend_from_slice(&(28 + 52 + data_size).to_le_bytes());
    f.extend_from_slice(&0u64.to_le_bytes());
    f.extend_from_slice(b"fmt ");
    f.extend_from_slice(&52u64.to_le_bytes());
    for v in [1u32, 0, 2, channels as u32, 2_822_400, 1] {
        f.extend_from_slice(&v.to_le_bytes());
    }
    f.extend_from_slice(&samples.to_le_bytes());
    f.extend_from_slice(&(BLOCK as u32).to_le_bytes());
    f.extend_from_slice(&0u32.to_le_bytes());
    f.extend_from_slice(b"data");
    f.extend_from_slice(&data_size.to_le_bytes());
    f.extend_from_slice(&data);
    std::fs::write(path, f).unwrap();
}

// --- The scenarios -------------------------------------------------------------------

/// Plays `path` (after `setup`), lets the device warm up, then measures its
/// render() calls for a while, with `during` run on the control side in the
/// middle (pauses, seeks…). Returns (renders, renders with music, allocs, frees).
fn measure(
    path: &Path,
    bits: u32,
    setup: impl FnOnce(&mut Engine),
    during: impl FnOnce(&mut Engine),
) -> (usize, usize, usize, usize) {
    let m = Arc::new(Measure::default());
    let mut e = engine(m.clone(), bits);
    setup(&mut e);
    e.load(path, 0.0).unwrap();
    e.play();
    // Warm-up: the first periods (preroll, fade-in, the ring filling).
    std::thread::sleep(Duration::from_millis(200));
    m.on.store(true, Ordering::Release);
    std::thread::sleep(Duration::from_millis(150));
    during(&mut e);
    std::thread::sleep(Duration::from_millis(150));
    m.on.store(false, Ordering::Release);
    e.stop();
    drop(e);
    (
        m.renders.load(Ordering::Relaxed),
        m.music.load(Ordering::Relaxed),
        m.allocs.load(Ordering::Relaxed),
        m.frees.load(Ordering::Relaxed),
    )
}

fn check(what: &str, (renders, music, allocs, frees): (usize, usize, usize, usize)) {
    eprintln!("{what}: {renders} render() calls ({music} with music), {allocs} allocations, {frees} frees");
    assert!(renders > 50, "{what}: the device rendered only {renders} times");
    assert!(music > 20, "{what}: only {music} periods had music: not the playing path");
    assert_eq!((allocs, frees), (0, 0), "{what}: render() allocated\n{}", first_trace());
}

fn pause_seek_resume(e: &mut Engine) {
    e.set_analysis(true);
    std::thread::sleep(Duration::from_millis(100));
    e.pause();
    std::thread::sleep(Duration::from_millis(60));
    e.play();
    std::thread::sleep(Duration::from_millis(100));
    let _ = e.seek(1.0);
    std::thread::sleep(Duration::from_millis(100));
    e.set_volume(0.7);
}

#[test]
fn render_does_not_allocate_bit_perfect() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    reset_counters();
    let path = tmp("rt-bitperfect.wav");
    write_wav(&path, 44100, 2, 44100 * 10);
    let r = measure(
        &path,
        32,
        |_| {},
        |e| {
            assert!(e.bit_perfect(), "bit-perfect");
            e.set_analysis(true);
            std::thread::sleep(Duration::from_millis(100));
            e.pause();
            std::thread::sleep(Duration::from_millis(60));
            e.play();
            std::thread::sleep(Duration::from_millis(100));
            let _ = e.seek(2.0);
        },
    );
    check("bit-perfect", r);
}

#[test]
fn render_does_not_allocate_mono_on_a_stereo_device() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    reset_counters();
    let path = tmp("rt-mono.wav");
    write_wav(&path, 48000, 1, 48000 * 10);
    check("mono to stereo", measure(&path, 24, |_| {}, pause_seek_resume));
}

#[test]
fn render_does_not_allocate_processed() {
    use sonkore_eq::super_eq::Band;
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    reset_counters();
    let path = tmp("rt-processed.wav");
    write_wav(&path, 44100, 2, 44100 * 10);
    let setup = |e: &mut Engine| {
        e.set_volume(0.5);
        e.dsp().eq.set_bands(vec![Band { freq_hz: 1000.0, gain_db: 6.0, q: 1.0, enabled: true, ..Default::default() }]);
        e.dsp().eq.set_enabled(true);
        e.dsp().update(|s| {
            s.crossfeed = 1;
            s.limiter = true;
            s.headroom_db = -3.0;
        });
    };
    let r = measure(&path, 24, setup, |e| {
        assert!(!e.bit_perfect(), "processed");
        pause_seek_resume(e);
    });
    check("processed (volume, EQ, crossfeed, limiter)", r);
}

#[test]
fn render_does_not_allocate_resampled() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    reset_counters();
    let path = tmp("rt-resampled.wav");
    write_wav(&path, 44100, 2, 44100 * 10);
    // A device fixed at 96 kHz: the resampler runs.
    let r = measure(&path, 32, |e| e.set_rate_mode(0, 96000), pause_seek_resume);
    check("resampled 44.1 to 96 kHz", r);
}

#[test]
fn render_does_not_allocate_dsd() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let path = tmp("rt-dsd.dsf");
    write_dsf(&path, 4.0);
    for (mode, name) in [(1, "DSD native"), (2, "DSD over PCM (DoP)")] {
        reset_counters();
        let setup = move |e: &mut Engine| {
            e.set_device("alsa|hw:CARD=Test,DEV=0");
            e.set_dsd_output(mode);
        };
        let r = measure(&path, 32, setup, |e| {
            std::thread::sleep(Duration::from_millis(100));
            e.pause();
            std::thread::sleep(Duration::from_millis(60));
            e.play();
            std::thread::sleep(Duration::from_millis(100));
            let _ = e.seek(1.0);
        });
        check(name, r);
    }
}

// --- The DSP chain, block by block -------------------------------------------------

/// Blocks of 4096 stereo frames through a chain (as the worker runs it):
/// warm-up until `ready(chain)` (a background FIR design, SoundTouch filling
/// up), then `blocks` more counted, `change` applied before the counted
/// block `at` (an EQ switched off: a crossfade).
fn chain_allocations(
    dsp: Arc<Dsp>,
    in_rate: u32,
    out_rate: u32,
    gain: f64,
    ready: impl Fn(&Chain) -> bool,
    change: Option<(usize, &dyn Fn(&Dsp))>,
) -> (usize, usize) {
    let mut chain = Chain::new(dsp.clone(), in_rate, out_rate, 2);
    let source: Vec<f64> = (0..4096 * 2).map(|i| ((i as f64) * 0.013).sin() * 0.5).collect();
    let mut block = source.clone();
    let mut kept = Vec::with_capacity(1 << 16);
    let mut step = |chain: &mut Chain, block: &mut Vec<f64>| {
        chain.sync();
        kept.clear();
        chain.process(block, gain, false, &mut |out: &[f64]| {
            // The worker's staging buffer (its capacity kept).
            let room = kept.capacity() - kept.len();
            kept.extend_from_slice(&out[..out.len().min(room)]);
        });
        block.copy_from_slice(&source);
    };
    let started = Instant::now();
    let mut warm = 0;
    while warm < 20 || !ready(&chain) {
        assert!(started.elapsed() < Duration::from_secs(10), "the chain never got ready");
        step(&mut chain, &mut block);
        warm += 1;
        if warm > 20 {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    let (mut allocs, mut frees) = (0, 0);
    for b in 0..40 {
        if let Some((at, f)) = &change
            && *at == b
        {
            f(&dsp);
        }
        let ((), a, f) = count(|| step(&mut chain, &mut block));
        allocs += a;
        frees += f;
    }
    (allocs, frees)
}

fn chain_case(what: &str, setup: impl FnOnce(&Dsp), in_rate: u32, out_rate: u32, gain: f64) -> (usize, usize) {
    chain_case_with(what, setup, in_rate, out_rate, gain, |_| true, None)
}

fn chain_case_with(
    what: &str,
    setup: impl FnOnce(&Dsp),
    in_rate: u32,
    out_rate: u32,
    gain: f64,
    ready: impl Fn(&Chain) -> bool,
    change: Option<(usize, &dyn Fn(&Dsp))>,
) -> (usize, usize) {
    let dsp = Arc::new(Dsp::default());
    setup(&dsp);
    let r = chain_allocations(dsp, in_rate, out_rate, gain, ready, change);
    eprintln!("chain, {what}: {} allocations, {} frees in 40 blocks", r.0, r.1);
    r
}

fn eq_on(d: &Dsp) {
    use sonkore_eq::super_eq::Band;
    d.eq.set_bands(vec![
        Band { freq_hz: 100.0, gain_db: 4.0, q: 0.7, enabled: true, ..Default::default() },
        Band { freq_hz: 3000.0, gain_db: -3.0, q: 2.0, enabled: true, ..Default::default() },
    ]);
    d.eq.set_enabled(true);
}

#[test]
fn the_chain_does_not_allocate_per_block() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    reset_counters();
    let cases: Vec<(&str, (usize, usize))> = vec![
        ("volume", chain_case("volume", |_| {}, 44100, 44100, 0.5)),
        ("biquad EQ", chain_case("biquad EQ", eq_on, 44100, 44100, 1.0)),
        (
            "EQ + crossfeed + limiter + headroom",
            chain_case(
                "EQ + crossfeed + limiter + headroom",
                |d| {
                    eq_on(d);
                    d.update(|s| {
                        s.crossfeed = 2;
                        s.limiter = true;
                        s.headroom_db = -6.0;
                    });
                },
                44100,
                44100,
                1.5,
            ),
        ),
        ("resampler 44.1 to 96 kHz", chain_case("resampler 44.1 to 96 kHz", |_| {}, 44100, 96000, 0.8)),
        (
            "resampler 96 to 44.1 kHz + limiter",
            chain_case("resampler 96 to 44.1 kHz + limiter", |d| d.update(|s| s.limiter = true), 96000, 44100, 1.0),
        ),
    ];
    let bad: Vec<String> = cases
        .iter()
        .filter(|(_, r)| *r != (0, 0))
        .map(|(w, (a, f))| format!("{w}: {a} allocations, {f} frees"))
        .collect();
    assert!(bad.is_empty(), "the chain allocated per block:\n{}\n{}", bad.join("\n"), first_trace());
}

#[test]
fn the_linear_phase_chain_does_not_allocate_per_block() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    reset_counters();
    let linear = |d: &Dsp| {
        eq_on(d);
        d.eq.set_linear_phase(true);
    };
    // Running (its latency shows once the FIR is designed).
    let active = |c: &Chain| c.pending_out_frames() > 0;
    let r = chain_case_with("linear-phase EQ", linear, 44100, 44100, 1.0, active, None);
    assert_eq!(r, (0, 0), "{}", first_trace());
}

#[test]
fn the_linear_phase_chain_does_not_allocate_when_switched_off() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    reset_counters();
    let linear = |d: &Dsp| {
        eq_on(d);
        d.eq.set_linear_phase(true);
    };
    let active = |c: &Chain| c.pending_out_frames() > 0;
    let off: &dyn Fn(&Dsp) = &|d: &Dsp| d.eq.set_enabled(false);
    let r = chain_case_with("linear-phase EQ switched off", linear, 44100, 44100, 1.0, active, Some((5, off)));
    assert_eq!(r, (0, 0), "{}", first_trace());
}

#[test]
fn the_time_stretch_chain_does_not_allocate_per_block() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    reset_counters();
    let r = chain_case("tempo 1.25", |d| d.stretch.set_tempo(1.25), 44100, 44100, 1.0);
    assert_eq!(r, (0, 0), "{}", first_trace());
}

/// The decoders per block: reported, not required (the worker is not the
/// real-time thread; the ring absorbs its pauses).
#[test]
fn decoder_allocations_per_block_are_reported() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    reset_counters();
    let path = tmp("rt-decoder.wav");
    write_wav(&path, 44100, 2, 44100 * 5);
    let mut files = vec![path];
    if let Some(c) = std::env::var_os("SONKORE_TEST_CORPUS").map(PathBuf::from) {
        files.extend(["album/01.flac", "formats/mp3.mp3", "formats/aac.m4a", "formats/vorbis.ogg"].map(|f| c.join(f)));
    }
    for f in files.iter().filter(|f| f.exists()) {
        let mut d = sonkore_engine::decode::open(f).unwrap();
        let ch = d.info().channels;
        let mut buf = vec![0f64; 4096 * ch];
        for _ in 0..5 {
            let _ = d.read_f64(&mut buf);
        }
        let t = Instant::now();
        let (_, a, fr) = count(|| {
            for _ in 0..20 {
                let _ = d.read_f64(&mut buf);
            }
        });
        eprintln!(
            "decoder {}: {:.1} allocations, {:.1} frees per 4096-frame block ({:?} for 20)",
            f.file_name().unwrap().to_string_lossy(),
            a as f64 / 20.0,
            fr as f64 / 20.0,
            t.elapsed()
        );
    }
}
