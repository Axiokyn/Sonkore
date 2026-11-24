//! Outputs: the devices to play to and the thread that feeds each one.
//!
//! Devices are keyed as earlier versions keyed them ("<backend>|<id>"), so a
//! saved choice carries over: on Linux `alsa|hw:CARD=DX1,DEV=0` is a sound
//! card's own endpoint (exclusive, bit-perfect), `pipewire|<node>` a PipeWire
//! sink; on Windows `wasapi|<name>` an endpoint, shared or exclusive as
//! playback/exclusive says; "" the system's default. A device thread asks the engine for interleaved
//! left-justified s32 (`Render::render`) and writes them in the device's own
//! format.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

pub use sonkore_dsd::DsdPacking;

#[cfg(target_os = "linux")]
pub mod pipewire;
#[cfg(windows)]
pub mod wasapi;

/// The engine side of a device thread: fills `out` (interleaved s32, the
/// device's channels) without blocking or allocating.
pub trait Render: Send + Sync {
    fn render(&self, out: &mut [i32]);
}

#[derive(Debug)]
pub struct OutputError(pub String);

impl std::fmt::Display for OutputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for OutputError {}

pub type Result<T> = std::result::Result<T, OutputError>;

/// A device as the Settings list shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceInfo {
    pub key: String,
    pub backend: String,
    pub name: String,
    pub detail: String,
    pub is_default: bool,
    /// A card's own endpoint: nothing mixes in, bit-perfect possible.
    pub direct: bool,
    /// The ALSA card behind a sound server's output, -1 for none.
    pub alsa_card: i32,
}

/// What the device was opened with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceFormat {
    pub rate: u32,
    pub channels: usize,
    /// The device's sample width (16, 24, 32); the quantizer's grid.
    pub bits: u32,
    pub float: bool,
    /// Opened in stereo for a mono track (the device refused one channel).
    pub mono_to_stereo: bool,
    /// Nothing else can mix into it (a hw: endpoint).
    pub exclusive: bool,
    pub period_frames: usize,
    pub buffer_frames: usize,
    /// Opened for DSD: DoP words through PCM, or a native DSD_U32 format.
    pub dsd: Option<DsdPacking>,
}

impl DeviceFormat {
    /// The depth bit-perfect output needs at least.
    pub fn depth(&self) -> u32 {
        if self.float { 24 } else { self.bits }
    }

    /// The quantizer's grid (float ? 24 : min(bits, 24)).
    pub fn quantizer_bits(&self) -> u32 {
        if self.float { 24 } else { self.bits.min(24) }
    }
}

/// What the engine asks a device for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request {
    pub rate: u32,
    pub channels: usize,
    /// PipeWire: hold the graph at the track's rate (playback/followTrackRate).
    pub follow_rate: bool,
    /// WASAPI: exclusive mode, past the Windows mixer (playback/exclusive).
    pub exclusive: bool,
    /// DSD to the device: DoP (a PCM format of 24+ bits at exactly `rate`)
    /// or native (DSD_U32, big or little endian, whichever the card takes).
    pub dsd: Option<DsdPacking>,
}

/// An open device with its thread.
pub trait Output: Send {
    fn format(&self) -> DeviceFormat;
    /// Frames between render() and the speakers (the device's buffer).
    fn latency_frames(&self) -> u64;
    /// The key it was opened for.
    fn key(&self) -> &str;
    /// The device stopped for good (unplugged, the sound server gone): its
    /// thread no longer renders. The engine closes it and reports an error.
    fn failed(&self) -> bool {
        false
    }
    /// How many times real time the device plays its rate: 1 for a sound
    /// card; a test device may run faster (the worker paces itself by it).
    fn speed(&self) -> f64 {
        1.0
    }
}

/// A key DSD can reach: a card's own endpoint (ALSA hw:), or on Windows a
/// WASAPI endpoint, which is opened exclusively for DSD.
pub fn is_direct_key(key: &str) -> bool {
    let key = resolve_key(key);
    key.starts_with("alsa|") || (cfg!(windows) && (key.is_empty() || key.starts_with("wasapi|")))
}

/// The backend and the device name the default output shows (Settings,
/// the signal path) once opened with `format`.
pub fn default_label(devices: &[DeviceInfo], format: &DeviceFormat) -> (String, String) {
    if cfg!(windows) {
        let name = devices.iter().find(|d| d.is_default).map_or_else(|| "Default".to_owned(), |d| d.name.clone());
        return ("WASAPI".into(), name);
    }
    // PipeWire's default sink (it carries floats), else ALSA's.
    if format.float {
        let name = devices
            .iter()
            .find(|d| d.backend == "PipeWire" && d.is_default)
            .map_or_else(|| "Default".to_owned(), |d| d.name.clone());
        ("PipeWire".into(), name)
    } else {
        ("ALSA".into(), "default".into())
    }
}

/// Every device to play to: the system's endpoints (WASAPI).
#[cfg(windows)]
pub fn devices() -> Vec<DeviceInfo> {
    wasapi::devices()
}

/// Opens the device `key` ("" = the default) for `request`.
#[cfg(windows)]
pub fn open(key: &str, request: Request, render: Arc<dyn Render>) -> Result<Box<dyn Output>> {
    let key = resolve_key(key);
    if key == "null" {
        return Ok(Box::new(MemoryOutput::open(
            DeviceFormat {
                rate: request.rate,
                channels: request.channels,
                bits: 32,
                float: false,
                mono_to_stereo: false,
                exclusive: false,
                period_frames: (request.rate / 25) as usize,
                buffer_frames: (request.rate / 25) as usize * 3,
                dsd: None,
            },
            render,
            1.0,
            0,
        )));
    }
    let name = key.strip_prefix("wasapi|").unwrap_or("");
    if !key.is_empty() && name.is_empty() {
        return Err(OutputError(format!("{key}: not a Windows output")));
    }
    // DSD goes out exclusively or not at all (the mixer would convert it).
    let exclusive = request.exclusive || request.dsd.is_some();
    Ok(Box::new(wasapi::WasapiOutput::open(&key, name, exclusive, request, render)?))
}

/// Every device to play to: PipeWire's sinks, then each card's own endpoints.
#[cfg(target_os = "linux")]
pub fn devices() -> Vec<DeviceInfo> {
    let mut list: Vec<DeviceInfo> = Vec::new();
    // PipeWire's sinks, keyed without WirePlumber's ".N" suffix (it changes
    // when the node is made again); a second sink of the same name as it is.
    for sink in pipewire::sinks() {
        if sink.name == "auto_null" {
            continue; // WirePlumber's stand-in while no card is free
        }
        let mut key = format!("pipewire|{}", pipewire::base_sink_name(&sink.name));
        if list.iter().any(|d| d.key == key) {
            key = format!("pipewire|{}", sink.name);
        }
        list.push(DeviceInfo {
            key,
            backend: "PipeWire".into(),
            name: sink.description,
            detail: sink.name,
            is_default: sink.is_default,
            direct: false,
            alsa_card: sink.alsa_card,
        });
    }
    for pcm in alsa_hardware() {
        let hw = format!("hw:CARD={},DEV={}", pcm.card_id, pcm.device);
        let name = if pcm.pcm_name.is_empty() {
            pcm.card_name.clone()
        } else {
            format!("{} · {}", pcm.card_name, pcm.pcm_name)
        };
        list.push(DeviceInfo {
            key: format!("alsa|{hw}"),
            backend: "ALSA".into(),
            name,
            detail: hw,
            is_default: false,
            direct: true,
            alsa_card: -1,
        });
    }
    list
}

/// A saved key as it is used now: PulseAudio's names are PipeWire's.
pub fn resolve_key(key: &str) -> String {
    match key.strip_prefix("pulse|") {
        Some(rest) => format!("pipewire|{rest}"),
        None => key.to_owned(),
    }
}

/// Opens the device `key` ("" = the default) for `request`.
#[cfg(target_os = "linux")]
pub fn open(key: &str, request: Request, render: Arc<dyn Render>) -> Result<Box<dyn Output>> {
    let key = resolve_key(key);
    if request.dsd.is_some() && !key.starts_with("alsa|") {
        // A sound server mixes and converts: DSD reaches a card's own endpoint only.
        return Err(OutputError(format!("{key}: DSD needs a direct ALSA device")));
    }
    if let Some(hw) = key.strip_prefix("alsa|") {
        return Ok(Box::new(alsa::AlsaOutput::open(&key, hw, true, request, render)?));
    }
    if key == "null" {
        return Ok(Box::new(alsa::AlsaOutput::open(&key, "null", false, request, render)?));
    }
    // PipeWire: mono goes as stereo (both sides the same).
    let pipewire_request = |r: Request| Request { channels: if r.channels == 1 { 2 } else { r.channels }, ..r };
    let mono = request.channels == 1;
    let opened = |mut out: pipewire::PipeWireOutput| -> Box<dyn Output> {
        out.set_mono_to_stereo(mono);
        Box::new(out)
    };
    if let Some(name) = key.strip_prefix("pipewire|") {
        // The node as it is called now (its ".N" suffix may have changed).
        let target = pipewire::sinks()
            .into_iter()
            .map(|s| s.name)
            .find(|n| n == name || pipewire::base_sink_name(n) == pipewire::base_sink_name(name))
            .unwrap_or_else(|| name.to_owned());
        return Ok(opened(pipewire::PipeWireOutput::open(&key, &target, pipewire_request(request), render)?));
    }
    // The system's default: PipeWire's default sink, else ALSA's "default".
    if pipewire::running() {
        if let Ok(out) = pipewire::PipeWireOutput::open(&key, "", pipewire_request(request), render.clone()) {
            return Ok(opened(out));
        }
    }
    Ok(Box::new(alsa::AlsaOutput::open(&key, "default", false, request, render)?))
}

/// The rate to open a device at when it refuses the track's: the lowest
/// whole multiple above it, else the
/// lowest rate above it, else the highest whole divisor, else the highest.
pub fn pick_rate(mut rates: Vec<u32>, wanted: u32) -> Option<u32> {
    if rates.is_empty() || wanted == 0 {
        return None;
    }
    rates.sort_unstable();
    rates.dedup();
    if let Some(&r) = rates.iter().find(|&&r| r > wanted && r % wanted == 0) {
        return Some(r);
    }
    if let Some(&r) = rates.iter().find(|&&r| r >= wanted) {
        return Some(r);
    }
    if let Some(&r) = rates.iter().rev().find(|&&r| wanted % r == 0) {
        return Some(r);
    }
    rates.last().copied()
}

/// The rates sound cards take, to probe a device with.
pub const STANDARD_RATES: [u32; 15] =
    [8000, 11025, 16000, 22050, 32000, 44100, 48000, 88200, 96000, 176400, 192000, 352800, 384000, 705600, 768000];

// --- procfs ---------------------------------------------------------------------

#[cfg(target_os = "linux")]
struct AlsaPcm {
    card_id: String,
    card_name: String,
    device: u32,
    pcm_name: String,
}

/// Every playback PCM of every card, from /proc/asound.
/// SONKORE_ASOUND_ROOT stands in for /proc/asound in tests (tests/asound).
#[cfg(target_os = "linux")]
fn alsa_hardware() -> Vec<AlsaPcm> {
    let root = std::env::var_os("SONKORE_ASOUND_ROOT")
        .map_or_else(|| std::path::PathBuf::from("/proc/asound"), std::path::PathBuf::from);
    let Ok(cards) = std::fs::read_to_string(root.join("cards")) else { return Vec::new() };
    let mut result = Vec::new();
    let mut lines = cards.lines();
    while let Some(line) = lines.next() {
        // " 2 [DX1            ]: USB-Audio - DX1"
        let trimmed = line.trim_start();
        let Some((index, rest)) = trimmed.split_once(' ') else { continue };
        if index.parse::<u32>().is_err() {
            continue;
        }
        let Some(open) = rest.find('[') else { continue };
        let Some(close) = rest.find(']') else { continue };
        let card_id = rest[open + 1..close].trim().to_owned();
        let after = rest[close + 1..].trim_start_matches(':').trim();
        let mut card_name =
            after.split_once(" - ").map(|(_, n)| n.trim().to_owned()).unwrap_or_else(|| after.to_owned());
        if let Some(long) = lines.next() {
            let long = long.trim();
            let long = long.split(" at ").next().unwrap_or(long).trim();
            if !long.is_empty() {
                card_name = long.to_owned();
            }
        }
        let dir = root.join(format!("card{index}"));
        let mut pcms = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.len() < 5 || !name.starts_with("pcm") || !name.ends_with('p') {
                    continue;
                }
                let device = name[3..name.len() - 1].parse().unwrap_or(0);
                let pcm_name = std::fs::read_to_string(entry.path().join("info"))
                    .ok()
                    .and_then(|info| info.lines().find_map(|l| l.strip_prefix("name:").map(|n| n.trim().to_owned())))
                    .unwrap_or_default();
                pcms.push(AlsaPcm { card_id: card_id.clone(), card_name: card_name.clone(), device, pcm_name });
            }
        }
        pcms.sort_by_key(|p| p.device);
        result.extend(pcms);
    }
    result
}

// --- ALSA ------------------------------------------------------------------------

#[cfg(target_os = "linux")]
mod alsa {
    use super::*;
    use ::alsa::pcm::{Access, Format, HwParams, PCM};
    use ::alsa::{Direction, ValueOr};

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum Sample {
        S32,
        S24In32, // S24_LE: 24 bits in the low three bytes of four
        S24Packed,
        S16,
        Dsd, // DSD_U32_BE / _LE: the words' bytes as they are
    }

    pub struct AlsaOutput {
        key: String,
        format: DeviceFormat,
        stop: Arc<AtomicBool>,
        failed: Arc<AtomicBool>,
        thread: Option<JoinHandle<()>>,
    }

    impl AlsaOutput {
        pub fn open(key: &str, name: &str, direct: bool, request: Request, render: Arc<dyn Render>) -> Result<Self> {
            let err = |what: &str, e: ::alsa::Error| OutputError(format!("{name}: {what}: {e}"));
            // A card just let go by the sound server stays busy for a moment.
            let mut attempts = 0;
            let pcm = loop {
                match PCM::new(name, Direction::Playback, false) {
                    Ok(p) => break p,
                    Err(e) if e.errno() == libc::EBUSY && attempts < 40 => {
                        attempts += 1;
                        std::thread::sleep(Duration::from_millis(25));
                    }
                    Err(e) => return Err(err("open", e)),
                }
            };
            let (format, sample) = {
                let hw = HwParams::any(&pcm).map_err(|e| err("parameters", e))?;
                hw.set_access(Access::RWInterleaved).map_err(|e| err("access", e))?;
                if direct {
                    // The card's own rates only: no ALSA resampling.
                    hw.set_rate_resample(false).map_err(|e| err("resample", e))?;
                }
                let mut channels = request.channels;
                let mut mono_to_stereo = false;
                if hw.set_channels(channels as u32).is_err() {
                    if channels == 1 && hw.set_channels(2).is_ok() {
                        channels = 2;
                        mono_to_stereo = true;
                    } else {
                        return Err(OutputError(format!("{name}: {} channels not supported", request.channels)));
                    }
                }
                let native = request.dsd.filter(|p| p.is_native());
                let mut dsd = request.dsd.filter(|p| !p.is_native());
                let sample = if native.is_some() {
                    // Big endian first.
                    let (format, packing) =
                        [(Format::DSDU32BE, DsdPacking::NativeU32Be), (Format::DSDU32LE, DsdPacking::NativeU32Le)]
                            .into_iter()
                            .find(|(f, _)| hw.test_format(*f).is_ok())
                            .ok_or_else(|| OutputError(format!("{name}: no native DSD")))?;
                    dsd = Some(packing);
                    (format, Sample::Dsd)
                } else {
                    [
                        (Format::s32(), Sample::S32),
                        (Format::s24(), Sample::S24In32),
                        (Format::s24_3(), Sample::S24Packed),
                        (Format::s16(), Sample::S16),
                    ]
                    .into_iter()
                    .filter(|(_, s)| dsd.is_none() || *s != Sample::S16) // DoP needs 24 bits
                    .find(|(f, _)| hw.test_format(*f).is_ok())
                    .ok_or_else(|| OutputError(format!("{name}: no integer sample format")))?
                };
                hw.set_format(sample.0).map_err(|e| err("format", e))?;
                let rate = if request.dsd.is_some() {
                    // DSD only at its own rate: anything else is another stream.
                    hw.test_rate(request.rate)
                        .map_err(|_| OutputError(format!("{name}: DSD at {} Hz not supported", request.rate)))?;
                    request.rate
                } else if hw.test_rate(request.rate).is_ok() {
                    request.rate
                } else {
                    let rates: Vec<u32> = STANDARD_RATES.iter().copied().filter(|&r| hw.test_rate(r).is_ok()).collect();
                    pick_rate(rates, request.rate).ok_or_else(|| OutputError(format!("{name}: no usable rate")))?
                };
                hw.set_rate(rate, ValueOr::Nearest).map_err(|e| err("rate", e))?;
                // Three periods of 40 ms (the engine's device buffer).
                hw.set_period_time_near(40_000, ValueOr::Nearest).map_err(|e| err("period", e))?;
                hw.set_periods_near(3, ValueOr::Nearest).map_err(|e| err("periods", e))?;
                pcm.hw_params(&hw).map_err(|e| err("hardware parameters", e))?;
                let current = pcm.hw_params_current().map_err(|e| err("parameters", e))?;
                let period = current.get_period_size().map_err(|e| err("period", e))? as usize;
                let buffer = current.get_buffer_size().map_err(|e| err("buffer", e))? as usize;
                let rate = current.get_rate().map_err(|e| err("rate", e))?;
                let bits = match sample.1 {
                    Sample::S32 | Sample::Dsd => 32,
                    Sample::S24In32 | Sample::S24Packed => 24,
                    Sample::S16 => 16,
                };
                (
                    DeviceFormat {
                        rate,
                        channels,
                        bits,
                        float: false,
                        mono_to_stereo,
                        exclusive: direct,
                        period_frames: period,
                        buffer_frames: buffer,
                        dsd,
                    },
                    sample.1,
                )
            };
            {
                let sw = pcm.sw_params_current().map_err(|e| err("software parameters", e))?;
                sw.set_start_threshold((format.buffer_frames - format.period_frames).max(format.period_frames) as i64)
                    .map_err(|e| err("start threshold", e))?;
                pcm.sw_params(&sw).map_err(|e| err("software parameters", e))?;
            }
            pcm.prepare().map_err(|e| err("prepare", e))?;

            let stop = Arc::new(AtomicBool::new(false));
            let failed = Arc::new(AtomicBool::new(false));
            let (thread_stop, thread_failed) = (stop.clone(), failed.clone());
            let thread = std::thread::Builder::new()
                .name("sonkore-alsa".into())
                .spawn(move || device_loop(pcm, format, sample, render, thread_stop, thread_failed))
                .map_err(|e| OutputError(e.to_string()))?;
            Ok(AlsaOutput { key: key.to_owned(), format, stop, failed, thread: Some(thread) })
        }
    }

    /// How long a device may keep failing to recover before it counts as gone.
    const RECOVERY_LIMIT: Duration = Duration::from_secs(1);

    /// The device thread: a period from the engine, in the card's format, to
    /// the card; underruns recovered from. A card that is gone (unplugged:
    /// ENODEV) or that cannot be recovered for a second ends the thread and
    /// marks the output failed.
    fn device_loop(
        pcm: PCM,
        format: DeviceFormat,
        sample: Sample,
        render: Arc<dyn Render>,
        stop: Arc<AtomicBool>,
        failed: Arc<AtomicBool>,
    ) {
        super::realtime::raise_priority();
        let ch = format.channels;
        let frames = format.period_frames.max(64);
        let mut block = vec![0i32; frames * ch];
        let mut bytes = vec![0u8; frames * ch * 3];
        let mut bytes4 = vec![0u8; if sample == Sample::Dsd { frames * ch * 4 } else { 0 }];
        let mut short = vec![0i16; frames * ch];
        let mut wide = vec![0i32; frames * ch];
        let io32 = pcm.io_i32();
        let mut failing_since: Option<std::time::Instant> = None;
        'periods: while !stop.load(Ordering::Acquire) {
            render.render(&mut block);
            let mut written = 0;
            while written < frames && !stop.load(Ordering::Acquire) {
                let result = match sample {
                    Sample::Dsd => {
                        // The words hold the DSD bytes in the order they go out.
                        let src = &block[written * ch..];
                        let out = &mut bytes4[..src.len() * 4];
                        for (o, s) in out.chunks_exact_mut(4).zip(src) {
                            o.copy_from_slice(&s.to_ne_bytes());
                        }
                        pcm.io_bytes().writei(out)
                    }
                    Sample::S32 => io32.as_ref().map_err(|e| *e).and_then(|io| io.writei(&block[written * ch..])),
                    Sample::S24In32 => {
                        // Left-justified s32 -> 24 bits in the low bytes.
                        let src = &block[written * ch..];
                        for (d, s) in wide.iter_mut().zip(src) {
                            *d = s >> 8;
                        }
                        io32.as_ref().map_err(|e| *e).and_then(|io| io.writei(&wide[..src.len()]))
                    }
                    Sample::S24Packed => {
                        let src = &block[written * ch..];
                        let out = &mut bytes[..src.len() * 3];
                        for (o, s) in out.chunks_exact_mut(3).zip(src) {
                            let b = s.to_le_bytes();
                            o.copy_from_slice(&b[1..4]);
                        }
                        pcm.io_bytes().writei(out)
                    }
                    Sample::S16 => {
                        let src = &block[written * ch..];
                        for (d, s) in short.iter_mut().zip(src) {
                            *d = (s >> 16) as i16;
                        }
                        match pcm.io_i16() {
                            Ok(io) => io.writei(&short[..src.len()]),
                            Err(e) => Err(e),
                        }
                    }
                };
                match result {
                    Ok(n) => {
                        written += n;
                        failing_since = None;
                    }
                    Err(e) => {
                        if let Err(e) = pcm.try_recover(e, true) {
                            let since = *failing_since.get_or_insert_with(std::time::Instant::now);
                            let gone = matches!(e.errno(), libc::ENODEV | libc::ENXIO | libc::EBADFD | libc::ESHUTDOWN);
                            if gone || since.elapsed() >= RECOVERY_LIMIT {
                                failed.store(true, Ordering::Release);
                                break 'periods;
                            }
                            std::thread::sleep(Duration::from_millis(10));
                        }
                    }
                }
            }
        }
        let _ = pcm.drop();
    }

    impl Output for AlsaOutput {
        fn format(&self) -> DeviceFormat {
            self.format
        }
        fn latency_frames(&self) -> u64 {
            self.format.buffer_frames as u64
        }
        fn key(&self) -> &str {
            &self.key
        }
        fn failed(&self) -> bool {
            self.failed.load(Ordering::Acquire)
        }
    }

    impl Drop for AlsaOutput {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }
}

// --- Memory (tests, measurements) ------------------------------------------------

/// A device that keeps what it is sent: for tests (bit-perfect checks) and
/// for running without sound.
pub struct MemoryOutput {
    key: String,
    format: DeviceFormat,
    speed: f64,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    captured: Arc<Mutex<Vec<i32>>>,
}

impl MemoryOutput {
    /// Played at `speed` times real time (1 = as a device would).
    pub fn open(format: DeviceFormat, render: Arc<dyn Render>, speed: f64, keep_up_to: usize) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let captured = Arc::new(Mutex::new(Vec::new()));
        let (s, c) = (stop.clone(), captured.clone());
        let thread = std::thread::spawn(move || {
            let mut block = vec![0i32; format.period_frames * format.channels];
            let period = Duration::from_secs_f64(format.period_frames as f64 / format.rate as f64 / speed.max(0.01));
            while !s.load(Ordering::Acquire) {
                render.render(&mut block);
                {
                    let mut kept = c.lock().unwrap();
                    if kept.len() < keep_up_to {
                        let room = (keep_up_to - kept.len()).min(block.len());
                        kept.extend_from_slice(&block[..room]);
                    }
                }
                std::thread::sleep(period);
            }
        });
        MemoryOutput { key: "memory".into(), format, speed: speed.max(0.01), stop, thread: Some(thread), captured }
    }

    pub fn captured(&self) -> Arc<Mutex<Vec<i32>>> {
        self.captured.clone()
    }
}

impl Output for MemoryOutput {
    fn format(&self) -> DeviceFormat {
        self.format
    }
    fn latency_frames(&self) -> u64 {
        self.format.period_frames as u64
    }
    fn key(&self) -> &str {
        &self.key
    }
    fn speed(&self) -> f64 {
        self.speed
    }
}

impl Drop for MemoryOutput {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

// --- Thread priority -------------------------------------------------------------

pub(crate) mod realtime {
    /// SCHED_FIFO when the system allows it (rtkit comes with the platform
    /// layer); otherwise the thread keeps its priority.
    #[cfg(unix)]
    pub fn raise_priority() {
        // SAFETY: plain libc calls on the current thread.
        unsafe {
            let min = libc::sched_get_priority_min(libc::SCHED_FIFO);
            let param = libc::sched_param { sched_priority: min + 10 };
            let _ = libc::pthread_setschedparam(libc::pthread_self(), libc::SCHED_FIFO, &param);
        }
    }

    /// MMCSS "Pro Audio" (as WASAPI audio threads usually ask for); the
    /// registration lasts as long as the thread.
    #[cfg(windows)]
    pub fn raise_priority() {
        use windows::Win32::System::Threading::AvSetMmThreadCharacteristicsW;
        let mut index = 0u32;
        // SAFETY: a constant wide string and a local out value.
        let _ = unsafe { AvSetMmThreadCharacteristicsW(windows::core::w!("Pro Audio"), &mut index) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_rates_in_order() {
        let r = vec![44100, 48000, 88200, 96000, 176400, 192000];
        assert_eq!(pick_rate(r.clone(), 44100), Some(88200)); // a whole multiple first
        assert_eq!(pick_rate(vec![48000, 96000, 192000], 44100), Some(48000));
        assert_eq!(pick_rate(r.clone(), 22050), Some(44100));
        assert_eq!(pick_rate(vec![48000, 88200, 96000], 44100), Some(88200));
        assert_eq!(pick_rate(r.clone(), 384000), Some(192000));
        assert_eq!(pick_rate(vec![44100, 48000], 352800), Some(44100));
        assert_eq!(pick_rate(vec![48000], 1000000), Some(48000));
    }
}
