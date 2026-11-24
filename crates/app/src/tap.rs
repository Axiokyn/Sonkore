//! For the live tests (tools/ui-live.py, `sonido`): with SONKORE_OUTPUT_TAP
//! set to a folder, every output opened writes what the engine hands the
//! device there, so a test can measure what each setting does to the sound.
//! Output n gives `n.raw` (the device's interleaved s32, little endian, as
//! `Render::render` filled them) and `n.json` (its key and format).
//!
//! It also keeps time: ALSA's null device (the tests' stand-in for a DAC: in
//! `tests/asound` every `hw:` is the null device) takes samples as fast as
//! they come, so the render waits for the clock there; PipeWire already plays
//! in real time.

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use sonkore_engine::output::{DeviceFormat, Render};

fn folder() -> Option<&'static PathBuf> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = PathBuf::from(std::env::var_os("SONKORE_OUTPUT_TAP")?);
        std::fs::create_dir_all(&dir).ok()?;
        Some(dir)
    })
    .as_ref()
}

static COUNT: AtomicUsize = AtomicUsize::new(0);

pub struct Tap {
    inner: Arc<dyn Render>,
    index: usize,
    pending: Mutex<Vec<i32>>,
    /// Frames rendered, and the rate and channels once the device is open.
    frames: AtomicU64,
    rate: AtomicU32,
    channels: AtomicU32,
    start: OnceLock<Instant>,
}

impl Render for Tap {
    fn render(&self, out: &mut [i32]) {
        self.inner.render(out);
        if let Ok(mut p) = self.pending.lock() {
            p.extend_from_slice(out);
        }
        let (rate, channels) = (self.rate.load(Ordering::Relaxed), self.channels.load(Ordering::Relaxed));
        if rate == 0 || channels == 0 {
            return;
        }
        let frames = self.frames.fetch_add((out.len() / channels as usize) as u64, Ordering::Relaxed);
        let start = *self.start.get_or_init(Instant::now);
        // Up to 0.2 s ahead of the clock, like a device's buffer.
        let due = Duration::from_secs_f64(frames as f64 / rate as f64);
        let ahead = due.saturating_sub(start.elapsed());
        if ahead > Duration::from_millis(200) {
            std::thread::sleep(ahead - Duration::from_millis(150));
        }
    }
}

/// `render` with a tap when the tests asked for one.
pub fn wrap(render: Arc<dyn Render>) -> (Arc<dyn Render>, Option<Arc<Tap>>) {
    let Some(dir) = folder() else { return (render, None) };
    let index = COUNT.fetch_add(1, Ordering::Relaxed);
    let tap = Arc::new(Tap {
        inner: render,
        index,
        pending: Mutex::new(Vec::new()),
        frames: AtomicU64::new(0),
        rate: AtomicU32::new(0),
        channels: AtomicU32::new(0),
        start: OnceLock::new(),
    });
    let Ok(mut file) = std::fs::File::create(dir.join(format!("{index}.raw"))) else {
        return (tap.inner.clone(), None);
    };
    // Written out every 50 ms until the output is closed (the last Arc is this one's).
    let weak = Arc::downgrade(&tap);
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_millis(50));
            let Some(tap) = weak.upgrade() else { break };
            let samples = std::mem::take(&mut *tap.pending.lock().unwrap_or_else(|e| e.into_inner()));
            let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
            if file.write_all(&bytes).is_err() {
                break;
            }
        }
    });
    (tap.clone(), Some(tap))
}

/// The output opened as `format`: its description next to the samples.
pub fn opened(tap: &Tap, key: &str, format: &DeviceFormat, paced: bool) {
    let Some(dir) = folder() else { return };
    let json = serde_json::json!({
        "key": key,
        "rate": format.rate,
        "channels": format.channels,
        "bits": format.bits,
        "float": format.float,
        "quantizer": format.quantizer_bits(),
        "exclusive": format.exclusive,
        "dsd": format.dsd.map(|d| format!("{d:?}")),
        "period": format.period_frames,
        "buffer": format.buffer_frames,
    });
    let _ = std::fs::write(dir.join(format!("{}.json", tap.index)), json.to_string());
    if paced {
        tap.channels.store(format.channels as u32, Ordering::Relaxed);
        tap.rate.store(format.rate, Ordering::Relaxed);
    }
}
