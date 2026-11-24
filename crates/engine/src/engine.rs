//! The engine: a worker thread decodes and processes into
//! a lock-free ring; the device thread only copies from it. One control
//! thread calls the methods here.
//!
//! Bit-perfect: a lossless source at unity gain, on a device as deep as it
//! and at its rate, goes from the decoder to the device untouched. Anything
//! else (volume, ReplayGain, a float or lossy source) goes through f64, a
//! sample-by-sample gain glide and the quantizer onto the device's grid.
//!
//! Gapless: the next track is opened ahead; when the current one ends, the
//! worker carries straight on into it (same rate and channels), and the
//! track change is reported when the device plays its first frame.
//!
//! De-click: pause, seek and stop fade out over 8 ms (raised cosine) on the
//! device thread, and resume fades in; faded samples are re-dithered onto
//! the device's grid as everything else.

use std::cell::UnsafeCell;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::decode::{self, Decoder, StreamInfo};
use crate::dsp::{Chain, Dsp};
use crate::output::{self, DeviceFormat, DsdPacking, Output, Render, Request};
use crate::quantize::{DitherMode, Quantizer};
use crate::ring::Ring;

const DECODE_BLOCK: usize = 1024; // frames per worker step
const DEFAULT_BUFFER_MS: u32 = 150;

const OPEN: u8 = 0;
const FADING_OUT: u8 = 1;
const MUTED: u8 = 2;
const NO_SPLICE: u64 = u64::MAX;
const ANALYSIS_SIZE: usize = 65536; // the visualizer's tap (a power of two)

/// The formats the engine takes: anything else is a corrupt header (its rate
/// would size the ring). The top rate is DSD1024 sent as DoP frames.
const MIN_RATE: u32 = 1000;
const MAX_RATE: u32 = 3_072_000;
const MAX_CHANNELS: usize = crate::quantize::MAX_CHANNELS;

/// What failed: the file, or the device it was to play on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// Missing, unreadable, or a format the engine does not play.
    Decode,
    /// Busy, gone, or refusing every format: every file would fail alike.
    Output,
}

#[derive(Debug)]
pub struct EngineError(pub String, pub ErrorKind);

impl EngineError {
    pub fn decode(message: impl Into<String>) -> Self {
        EngineError(message.into(), ErrorKind::Decode)
    }
    pub fn output(message: impl Into<String>) -> Self {
        EngineError(message.into(), ErrorKind::Output)
    }
    pub fn kind(&self) -> ErrorKind {
        self.1
    }
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for EngineError {}

impl From<decode::DecodeError> for EngineError {
    fn from(e: decode::DecodeError) -> Self {
        EngineError::decode(e.0)
    }
}

impl From<output::OutputError> for EngineError {
    fn from(e: output::OutputError) -> Self {
        EngineError::output(e.0)
    }
}

/// Frames of room a decoded block needs in the ring at the output rate.
fn block_need(in_rate: u32, out_rate: u32) -> usize {
    (DECODE_BLOCK as u64 * out_rate.clamp(1, MAX_RATE) as u64 / in_rate.clamp(MIN_RATE, MAX_RATE) as u64) as usize + 64
}

/// The ring's length in frames (docs/engine.md): the buffer at the device's
/// rate or two device periods, whichever is longer, plus a block; and room
/// for two blocks at least, however much the resampler multiplies them.
fn ring_frames(out_rate: u32, buffer_ms: u32, period_frames: usize, need: usize) -> usize {
    let rate = out_rate.clamp(1, MAX_RATE) as usize;
    let buffer = rate * buffer_ms.min(2000) as usize / 1000;
    // (A device period over a second long is not one.)
    let periods = 2 * period_frames.min(rate);
    (buffer.max(periods) + 1024).max(2 * need)
}

/// The engine's state for a device that is not made yet: the device thread
/// starts as it opens, and the ring is sized for the rate it opened at.
#[derive(Default)]
struct Pending(std::sync::OnceLock<Arc<Shared>>);

impl Render for Pending {
    fn render(&self, out: &mut [i32]) {
        match self.0.get() {
            Some(shared) => shared.render(out),
            None => out.fill(0),
        }
    }
}

pub type Result<T> = std::result::Result<T, EngineError>;

/// Opens a device: `output::open` normally, something else in tests.
pub type OutputFactory = Box<dyn FnMut(&str, Request, Arc<dyn Render>) -> output::Result<Box<dyn Output>> + Send>;

// --- What the device thread sees ----------------------------------------------------

/// State of one open device: the ring and what the device thread needs.
struct Shared {
    ring: Ring,
    channels: usize,            // the source's (frames in the ring)
    mono_to_stereo: AtomicBool, // the device has two channels for a mono track
    device_depth: AtomicU64,    // the faded samples' grid
    ramp_frames: usize,         // the de-click fade, 8 ms
    mute: AtomicU8,
    mute_ack: AtomicBool,
    ramp_in: AtomicBool,
    frames_pushed: AtomicU64,  // into the ring, whole frames
    frames_played: AtomicU64,  // out of it
    decoded: AtomicU64,        // source frames of the current track pushed
    splice_at: AtomicU64,      // frames_played at which the next track starts
    splice_old_end: AtomicU64, // where the previous track ended
    eof: AtomicBool,           // the decoder ended, nothing follows
    tail_frames: AtomicU64,    // silence played since then
    lead_silence: AtomicU64,   // frames of silence before the music (resync delay)
    latency_frames: AtomicU64,
    drained: AtomicBool, // the last sample has left the device
    underruns: AtomicU64,
    chain_pending: AtomicU64, // output frames inside the chain and its backlog
    stretch_held: AtomicU64,  // f64 bits: source frames SoundTouch holds
    limiter_db: AtomicU64,    // f64 bits: the limiter's last reduction
    analysis: Box<[std::sync::atomic::AtomicU32]>, // mono f32 bits of what the device got
    analysis_written: AtomicU64,
    analysis_on: AtomicBool,
    in_rate: AtomicU64,             // the stream's rate (the chain's input)
    dsd: Option<DsdPacking>,        // DSD words to the device (silence is 0x69, no fades)
    device: UnsafeCell<DeviceSide>, // the device thread's own
    /// What the ring holds ahead of the device (playback/bufferMs).
    buffer_ms: u32,
    /// The device's rate, which the ring was sized for.
    out_rate: u32,
    /// The worker sends the last block through the chain (the samples change).
    processing: AtomicBool,
}

// `device` is touched by the device thread only.
unsafe impl Sync for Shared {}

struct DeviceSide {
    last_mute: u8,
    ramp_pos: usize,
    ramping_in: bool,
    ramp_in_pos: usize,
    rng: u32,
    dop_phase: bool,
}

impl Shared {
    /// For a device at `rate` (the rate it opened at) taking blocks that need
    /// `need` frames each (block_need).
    fn new(
        rate: u32,
        channels: usize,
        buffer_ms: u32,
        period_frames: usize,
        need: usize,
        dsd: Option<DsdPacking>,
    ) -> Self {
        let rate = rate.clamp(1, MAX_RATE);
        let channels = channels.clamp(1, MAX_CHANNELS);
        let frames = ring_frames(rate, buffer_ms, period_frames, need);
        Shared {
            buffer_ms,
            out_rate: rate,
            processing: AtomicBool::new(false),
            ring: Ring::new(frames * channels),
            channels,
            mono_to_stereo: AtomicBool::new(false),
            device_depth: AtomicU64::new(24),
            ramp_frames: 64usize.max(rate as usize * 8 / 1000),
            mute: AtomicU8::new(MUTED),
            mute_ack: AtomicBool::new(true),
            ramp_in: AtomicBool::new(false),
            frames_pushed: AtomicU64::new(0),
            frames_played: AtomicU64::new(0),
            decoded: AtomicU64::new(0),
            splice_at: AtomicU64::new(NO_SPLICE),
            splice_old_end: AtomicU64::new(0),
            eof: AtomicBool::new(false),
            tail_frames: AtomicU64::new(0),
            lead_silence: AtomicU64::new(0),
            latency_frames: AtomicU64::new(0),
            drained: AtomicBool::new(false),
            underruns: AtomicU64::new(0),
            chain_pending: AtomicU64::new(0),
            stretch_held: AtomicU64::new(0),
            limiter_db: AtomicU64::new(0f64.to_bits()),
            analysis: (0..ANALYSIS_SIZE).map(|_| std::sync::atomic::AtomicU32::new(0)).collect(),
            analysis_written: AtomicU64::new(0),
            analysis_on: AtomicBool::new(false),
            in_rate: AtomicU64::new(rate as u64),
            dsd,
            device: UnsafeCell::new(DeviceSide {
                last_mute: MUTED,
                ramp_pos: 0,
                ramping_in: false,
                ramp_in_pos: 0,
                rng: 0x9E3779B9,
                dop_phase: false,
            }),
        }
    }
}

impl Shared {
    /// Whether this period is part of the lead silence (playback/resyncDelayMs
    /// after a rate change); counts it off. A period is silent whole.
    fn take_lead_silence(&self, frames: usize) -> bool {
        let left = self.lead_silence.load(Ordering::Relaxed);
        if left == 0 {
            return false;
        }
        self.lead_silence.store(left.saturating_sub(frames as u64), Ordering::Relaxed);
        true
    }

    /// DSD to the device: 1-bit words cannot be
    /// faded, so pause and stop cut to the idle pattern at once; gaps are the
    /// idle pattern too, and DoP markers alternate without a break.
    fn render_dsd(&self, dev: &mut DeviceSide, out: &mut [i32], packing: DsdPacking) {
        let ch = self.channels;
        let wanted = out.len() / ch * ch;
        let mute = self.mute.load(Ordering::Acquire);
        dev.last_mute = mute;
        if mute == OPEN && self.take_lead_silence(wanted / ch) {
            sonkore_dsd::output::fill_silence(&mut out[..wanted], ch, packing, &mut dev.dop_phase);
            return;
        }
        if mute != OPEN {
            sonkore_dsd::output::fill_silence(&mut out[..wanted], ch, packing, &mut dev.dop_phase);
            if mute == FADING_OUT {
                self.mute.store(MUTED, Ordering::Release);
                dev.last_mute = MUTED;
            }
            self.mute_ack.store(true, Ordering::Release);
            return;
        }
        self.ramp_in.store(false, Ordering::Relaxed);
        let available = self.ring.available_to_read().min(wanted) / ch * ch;
        let got = self.ring.read(&mut out[..available]);
        self.frames_played.fetch_add((got / ch) as u64, Ordering::Release);
        if got < wanted {
            out[got..wanted].fill(sonkore_dsd::silence(packing, false));
            let missing = ((wanted - got) / ch) as u64;
            if self.eof.load(Ordering::Acquire) {
                let silent = self.tail_frames.fetch_add(missing, Ordering::Relaxed) + missing;
                if silent >= self.latency_frames.load(Ordering::Relaxed) {
                    self.drained.store(true, Ordering::Release);
                }
            } else {
                self.underruns.fetch_add(1, Ordering::Relaxed);
            }
        }
        if packing == DsdPacking::Dop {
            sonkore_dsd::output::stamp_dop_markers(&mut out[..wanted], ch, &mut dev.dop_phase);
        }
    }
}

impl Render for Shared {
    fn render(&self, out: &mut [i32]) {
        // SAFETY: only the device thread calls render().
        let dev = unsafe { &mut *self.device.get() };
        if let Some(packing) = self.dsd {
            self.render_dsd(dev, out, packing);
            return;
        }
        let ch = self.channels;
        let expand = self.mono_to_stereo.load(Ordering::Relaxed) && ch == 1;
        let frames = if expand { out.len() / 2 } else { out.len() / ch };
        let wanted = frames * ch;
        let expand_mono = |out: &mut [i32]| {
            if expand {
                for f in (0..frames).rev() {
                    out[2 * f] = out[f];
                    out[2 * f + 1] = out[f];
                }
            }
        };

        let mute = self.mute.load(Ordering::Acquire);
        if mute != dev.last_mute {
            if mute == FADING_OUT {
                dev.ramp_pos = 0;
            }
            dev.last_mute = mute;
        }
        if mute == MUTED {
            out.fill(0);
            self.mute_ack.store(true, Ordering::Release);
            return;
        }
        if mute == OPEN && self.take_lead_silence(frames) {
            // The DAC locks to the new rate on silence; the music (and its
            // fade-in) waits in the ring, the position does not move.
            out.fill(0);
            return;
        }
        let limit =
            if mute == FADING_OUT { (self.ramp_frames - dev.ramp_pos.min(self.ramp_frames)) * ch } else { wanted };
        let available = self.ring.available_to_read().min(limit) / ch * ch;
        let got = self.ring.read(&mut out[..wanted.min(available)]);
        self.frames_played.fetch_add((got / ch) as u64, Ordering::Release);
        let depth = self.device_depth.load(Ordering::Relaxed) as u32;

        if mute == FADING_OUT {
            let mut pos = dev.ramp_pos;
            apply_ramp(&mut out[..got], ch, self.ramp_frames, &mut pos, true, depth, &mut dev.rng);
            dev.ramp_pos = if got < limit { self.ramp_frames } else { pos };
            out[got..wanted].fill(0);
            if dev.ramp_pos >= self.ramp_frames {
                self.mute.store(MUTED, Ordering::Release);
                dev.last_mute = MUTED;
                self.mute_ack.store(true, Ordering::Release);
            }
            expand_mono(out);
            return;
        }
        if self.ramp_in.swap(false, Ordering::AcqRel) {
            dev.ramping_in = true;
            dev.ramp_in_pos = 0;
        }
        if dev.ramping_in {
            let mut pos = dev.ramp_in_pos;
            apply_ramp(&mut out[..got], ch, self.ramp_frames, &mut pos, false, depth, &mut dev.rng);
            dev.ramp_in_pos = pos;
            dev.ramping_in = pos < self.ramp_frames;
        }
        if self.analysis_on.load(Ordering::Relaxed) {
            // A mono copy of what goes to the device, for the visualizer.
            let mut w = self.analysis_written.load(Ordering::Relaxed);
            let scale = 1.0 / 2147483648.0 / ch as f32;
            for f in 0..got / ch {
                let sum: f32 = out[f * ch..(f + 1) * ch].iter().map(|&s| s as f32).sum();
                self.analysis[(w as usize) & (ANALYSIS_SIZE - 1)].store((sum * scale).to_bits(), Ordering::Relaxed);
                w += 1;
            }
            self.analysis_written.store(w, Ordering::Release);
        }
        if got < wanted {
            out[got..wanted].fill(0);
            let missing = ((wanted - got) / ch) as u64;
            if self.eof.load(Ordering::Acquire) {
                // The end is reached once the device has played its buffer of
                // silence after the last samples: a stop then never cuts them.
                let silent = self.tail_frames.fetch_add(missing, Ordering::Relaxed) + missing;
                if silent >= self.latency_frames.load(Ordering::Relaxed) {
                    self.drained.store(true, Ordering::Release);
                }
            } else {
                self.underruns.fetch_add(1, Ordering::Relaxed);
            }
        }
        expand_mono(out);
    }
}

/// A raised-cosine ramp over `len` frames from `pos`: 1 → 0 (and silence
/// after) when `down`, 0 → 1 otherwise; the scaled samples requantised to
/// the device's grid with TPDF dither.
fn apply_ramp(out: &mut [i32], ch: usize, len: usize, pos: &mut usize, down: bool, depth: u32, rng: &mut u32) {
    let lsb = if depth >= 32 { 1.0 } else { 2f64.powi(32 - depth as i32) };
    let mut noise = || {
        *rng ^= *rng << 13;
        *rng ^= *rng >> 17;
        *rng ^= *rng << 5;
        *rng as f64 * (1.0 / 4294967296.0)
    };
    let frames = out.len() / ch;
    for f in 0..frames {
        if *pos >= len {
            if !down {
                return;
            }
            out[f * ch..].fill(0);
            *pos += frames - f;
            return;
        }
        let rise = 0.5 - 0.5 * (std::f64::consts::PI * *pos as f64 / len as f64).cos();
        let g = if down { 1.0 - rise } else { rise };
        for c in 0..ch {
            let v = out[f * ch + c] as f64 * g / lsb;
            let d = noise() - noise();
            let q = (if lsb > 1.0 { v + d } else { v }).round_ties_even() * lsb;
            out[f * ch + c] = q.clamp(-2147483648.0, 2147483647.0 - (lsb - 1.0)) as i32;
        }
        *pos += 1;
    }
}

// --- The worker --------------------------------------------------------------------

/// Gains the control thread sets and the worker applies.
#[derive(Default)]
struct Gains {
    volume: AtomicU64,     // f64 bits
    track_gain: AtomicU64, // f64 bits
}

impl Gains {
    fn new() -> Self {
        let g = Gains::default();
        g.volume.store(1f64.to_bits(), Ordering::Relaxed);
        g.track_gain.store(1f64.to_bits(), Ordering::Relaxed);
        g
    }
    fn target(&self) -> f64 {
        f64::from_bits(self.volume.load(Ordering::Relaxed)) * f64::from_bits(self.track_gain.load(Ordering::Relaxed))
    }
}

struct Worker {
    shared: Option<Arc<Shared>>,
    device: Option<DeviceFormat>,
    /// Output::speed() of the device (its clock against real time).
    device_speed: f64,
    decoder: Option<Box<dyn Decoder>>,
    /// The file each decoder reads: the track heard after a join is the one
    /// actually spliced, whatever set_next() said since.
    path: Option<PathBuf>,
    next: Option<Box<dyn Decoder>>,
    next_path: Option<PathBuf>,
    /// The next track's ReplayGain (set_next_track_gain): it takes over at the
    /// join, on the joined track's first sample.
    next_gain: Option<f64>,
    /// The track spliced away from while its join has not been heard yet:
    /// a seek then goes back to it (and to its gain).
    previous: Option<Box<dyn Decoder>>,
    previous_path: Option<PathBuf>,
    previous_gain: Option<f64>,
    /// The processed path for the stream (built at load).
    chain: Option<Chain>,
    quantizer: Quantizer,
    /// The chain holds samples (going back to bit-perfect, or the end of the
    /// stream, flushes it first).
    processed: bool,
    ints: Vec<i32>,
    floats: Vec<f64>,
    staged: Vec<f64>,
    /// Output waiting for room in the ring (resampled blocks, tails), from
    /// `backlog_pos` on.
    backlog: Vec<i32>,
    backlog_pos: usize,
    clipped: u64,
    /// The stream is DSD words: they go to the ring untouched, always.
    dsd: bool,
    quit: bool,
    /// Times the worker woke up (the tests count them).
    wakeups: u64,
}

/// What the worker does after a step.
enum Step {
    /// Another block now (the control thread may take the lock in between).
    Next,
    /// Wait for the device to make room (or for a wake-up).
    Wait(Duration),
    /// Nothing can move until the control thread changes something: it
    /// wakes the worker then.
    Idle,
}

impl Worker {
    /// Quantizes what the chain produced into the backlog.
    fn stage(&mut self) {
        if self.staged.is_empty() {
            return;
        }
        let start = self.backlog.len();
        self.backlog.resize(start + self.staged.len(), 0);
        self.quantizer.process(&self.staged, &mut self.backlog[start..]);
        self.clipped += self.quantizer.take_clipped();
        self.staged.clear();
    }

    /// Samples in the backlog not written yet.
    fn backlog_len(&self) -> usize {
        self.backlog.len() - self.backlog_pos
    }

    /// Writes as much of the backlog as fits; true when it is all out.
    fn drain_backlog(&mut self, shared: &Shared) -> bool {
        let ch = shared.channels;
        let room = shared.ring.available_to_write() / ch * ch;
        let n = room.min(self.backlog_len()) / ch * ch;
        if n > 0 {
            shared.ring.write(&self.backlog[self.backlog_pos..self.backlog_pos + n]);
            shared.frames_pushed.fetch_add((n / ch) as u64, Ordering::Release);
            self.backlog_pos += n;
        }
        if self.backlog_len() == 0 {
            // (A read offset: draining the front of a long tail chunk by
            // chunk would move all the rest each time.)
            self.backlog.clear();
            self.backlog_pos = 0;
        }
        self.backlog.is_empty()
    }

    /// Nothing waits in the chain or the backlog (a new stream or a seek).
    fn clear_output(&mut self) {
        self.processed = false;
        self.staged.clear();
        self.backlog.clear();
        self.backlog_pos = 0;
    }

    /// One block decoded into the ring; what to do next.
    fn step(&mut self, gains: &Gains) -> Step {
        let (Some(shared), Some(device)) = (self.shared.clone(), self.device) else { return Step::Idle };
        let ch = shared.channels;
        let rate = device.rate.max(1) as f64 * self.device_speed;
        // Room comes as the device takes a period; a muted device takes none.
        let wait = |missing: usize| {
            if shared.mute.load(Ordering::Acquire) == MUTED {
                return Step::Idle;
            }
            let quarter = (shared.ring.capacity() / ch / 4).max(1);
            let frames = missing.max(device.period_frames).min(quarter);
            Step::Wait(Duration::from_secs_f64(frames as f64 / rate).max(Duration::from_millis(2)))
        };
        if !self.drain_backlog(&shared) {
            return wait(self.backlog_len() / ch);
        }
        if shared.eof.load(Ordering::Relaxed) {
            return Step::Idle;
        }
        let Some(decoder) = self.decoder.as_ref() else { return Step::Idle };
        let (in_rate, in_channels, float, source_bits) = {
            let i = decoder.info();
            (i.rate, i.channels, i.float, i.source_bits())
        };
        let chain = self.chain.as_mut().expect("a chain with the decoder");
        // Room for a block at the output rate (what does not fit waits in the backlog).
        let need = block_need(chain.in_rate, chain.out_rate).min(shared.ring.capacity() / ch / 2);
        let free = shared.ring.available_to_write() / ch;
        if free < need {
            return wait(need - free);
        }
        chain.sync();
        let target = gains.target();
        // Bit-perfect: nothing in the chain would change the samples, unity
        // gain, a source no deeper than the device at its rate; the gains
        // glide back to 1 first.
        let wanted = target == 1.0
            && chain.neutral()
            && !chain.resampling()
            && !float
            && source_bits <= device.depth()
            && device.rate == in_rate;
        let at_unity = chain.applied_gain == 1.0 && chain.applied_headroom == 1.0;
        let bypass = self.dsd || (wanted && at_unity);
        shared.processing.store(!bypass, Ordering::Relaxed);
        if bypass && self.processed {
            // What the limiter still holds goes out first.
            let mut staged = std::mem::take(&mut self.staged);
            chain.flush_limiter(&mut |b| staged.extend_from_slice(b));
            self.staged = staged;
            self.processed = false;
            self.stage();
            return Step::Next;
        }
        let n = if bypass {
            self.ints.resize(DECODE_BLOCK * ch, 0);
            let decoder = self.decoder.as_mut().unwrap();
            match decoder.read_i32(&mut self.ints) {
                Ok(n) => {
                    chain.remember_s32(&self.ints[..n * ch]);
                    shared.ring.write(&self.ints[..n * ch]);
                    shared.frames_pushed.fetch_add(n as u64, Ordering::Release);
                    n
                }
                Err(_) => 0,
            }
        } else {
            self.floats.resize(DECODE_BLOCK * ch, 0.0);
            let decoder = self.decoder.as_mut().unwrap();
            match decoder.read_f64(&mut self.floats) {
                Ok(n) if n > 0 => {
                    let mut staged = std::mem::take(&mut self.staged);
                    chain.process(&mut self.floats[..n * ch], target, wanted, &mut |b| staged.extend_from_slice(b));
                    self.staged = staged;
                    // (Only real samples: at the end, an empty read marked the
                    // chain as holding some again, and its tail came out over
                    // and over whenever it did not fit the ring at once.)
                    self.processed = true;
                    n
                }
                _ => 0,
            }
        };
        if !bypass {
            self.stage();
            self.drain_backlog(&shared);
        }
        shared.chain_pending.store(
            self.chain.as_ref().map_or(0, |c| c.pending_out_frames()) + (self.backlog_len() / ch) as u64,
            Ordering::Relaxed,
        );
        shared.stretch_held.store(self.chain.as_ref().map_or(0.0, |c| c.stretch_held()).to_bits(), Ordering::Relaxed);
        shared
            .limiter_db
            .store(self.chain.as_ref().map_or(0.0, |c| c.limiter_reduction_db()).to_bits(), Ordering::Relaxed);
        if n > 0 {
            shared.decoded.fetch_add(n as u64, Ordering::Relaxed);
            return Step::Next;
        }
        // The end of the track: carry on into the next one when it fits
        // (not once the tails have started: they belong after the last sample).
        let finishing = self.chain.as_ref().is_some_and(Chain::finishing);
        let fits = self.next.as_ref().is_some_and(|next| {
            let ni = next.info();
            ni.rate == in_rate && ni.channels == in_channels
        });
        if fits && !finishing && shared.splice_at.load(Ordering::Acquire) == NO_SPLICE {
            let next = self.next.take().unwrap();
            self.previous = self.decoder.replace(next);
            self.previous_path = std::mem::replace(&mut self.path, self.next_path.take());
            // Its gain from its first sample (the app only hears of the join
            // a ring later: that much played at the previous track's gain).
            self.previous_gain = self.next_gain.take().map(|g| {
                let old = gains.track_gain.swap(g.to_bits(), Ordering::Relaxed);
                if let Some(chain) = self.chain.as_mut() {
                    chain.applied_gain = gains.target();
                }
                f64::from_bits(old)
            });
            shared.splice_old_end.store(shared.decoded.load(Ordering::Relaxed), Ordering::Relaxed);
            shared.decoded.store(0, Ordering::Relaxed);
            let pending = shared.chain_pending.load(Ordering::Relaxed);
            shared.splice_at.store(shared.frames_pushed.load(Ordering::Acquire) + pending, Ordering::Release);
            return Step::Next;
        }
        // The last track: the filters' tails, a block per step (the lock is
        // let go in between), then the end.
        if self.processed {
            let chain = self.chain.as_mut().unwrap();
            let mut staged = std::mem::take(&mut self.staged);
            let done = chain.finish_step(target, &mut |b| staged.extend_from_slice(b));
            self.staged = staged;
            self.stage();
            if done {
                self.processed = false;
            }
            if !self.drain_backlog(&shared) {
                return wait(self.backlog_len() / ch);
            }
            if !done {
                return Step::Next;
            }
        }
        shared.eof.store(true, Ordering::Release);
        Step::Idle
    }
}

// --- The engine ------------------------------------------------------------------

/// What the output is doing, for the signal
/// path and the player bar's badge.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OutputStatus {
    /// "ALSA", "PipeWire"; empty while nothing is open.
    pub backend: String,
    pub device_name: String,
    pub exclusive: bool,
    pub device_rate: u32,
    pub device_channels: usize,
    pub device_bits: u32,
    pub device_float: bool,
    /// Converted to the device's rate by the resampler.
    pub resampled: bool,
    pub source_rate: u32,
    /// The device matches the source (rate, channels, depth), nothing mixes in.
    pub path_can_be_bit_perfect: bool,
    /// …and the processing is neutral now.
    pub bit_perfect_now: bool,
    /// "DSD64 native", "DSD128 DoP"; empty for PCM.
    pub dsd: String,
    /// HDCD codes found: decoded to 20 bits.
    pub hdcd: bool,
    /// Resampled on purpose (upsampling), not because the device refused the rate.
    pub upsampled: bool,
    pub pcm_to_dsd_rate: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Stopped,
    Paused,
    Playing,
}

pub struct Engine {
    worker: Arc<(Mutex<Worker>, Condvar)>,
    dsp: Arc<Dsp>,
    upsampling: i64,
    fixed_rate: u32,
    gains: Arc<Gains>,
    thread: Option<JoinHandle<()>>,
    output: Option<Box<dyn Output>>,
    shared: Option<Arc<Shared>>,
    factory: OutputFactory,
    device_key: String,
    buffer_ms: u32,
    resync_ms: u32,
    last_output_rate: u32,
    follow_rate: bool,
    exclusive: bool,
    hdcd: bool,
    /// playback/dsdOutput: 0 PCM, 1 native, 2 DoP.
    dsd_output: u8,
    /// The open device takes DSD words in this packing.
    dsd_packing: Option<DsdPacking>,
    /// The track's 1-bit rate when it is DSD (also played as PCM), else 0.
    dsd_rate: u32,
    /// (backend, name) of the open device.
    device_label: (String, String),
    analysis: bool,
    state: State,
    info: Option<StreamInfo>,
    /// The track heard (the worker may already be on the next one).
    path: Option<PathBuf>,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    pub fn new() -> Self {
        Self::with_outputs(Box::new(|key: &str, request: Request, render: Arc<dyn Render>| {
            output::open(key, request, render)
        }))
    }

    /// An engine whose devices come from `factory` (tests: a memory output).
    pub fn with_outputs(factory: OutputFactory) -> Self {
        let worker = Arc::new((
            Mutex::new(Worker {
                shared: None,
                device: None,
                device_speed: 1.0,
                decoder: None,
                path: None,
                next: None,
                next_path: None,
                next_gain: None,
                previous: None,
                previous_path: None,
                previous_gain: None,
                chain: None,
                quantizer: Quantizer::default(),
                processed: false,
                ints: Vec::new(),
                floats: Vec::new(),
                staged: Vec::new(),
                backlog: Vec::new(),
                backlog_pos: 0,
                clipped: 0,
                dsd: false,
                quit: false,
                wakeups: 0,
            }),
            Condvar::new(),
        ));
        let gains = Arc::new(Gains::new());
        let (w, g) = (worker.clone(), gains.clone());
        let thread = std::thread::Builder::new()
            .name("sonkore-worker".into())
            .spawn(move || {
                flush_denormals_to_zero();
                let (lock, wake) = &*w;
                let mut guard = lock.lock().unwrap();
                loop {
                    if guard.quit {
                        break;
                    }
                    guard.wakeups += 1;
                    match guard.step(&g) {
                        Step::Next => {
                            // Let the control thread in between blocks.
                            drop(guard);
                            guard = lock.lock().unwrap();
                        }
                        Step::Wait(wait) => guard = wake.wait_timeout(guard, wait).unwrap().0,
                        Step::Idle => guard = wake.wait(guard).unwrap(),
                    }
                }
            })
            .expect("worker thread");
        Engine {
            worker,
            dsp: Arc::new(Dsp::default()),
            upsampling: 0,
            fixed_rate: 0,
            gains,
            thread: Some(thread),
            output: None,
            shared: None,
            factory,
            device_key: String::new(),
            buffer_ms: DEFAULT_BUFFER_MS,
            resync_ms: 0,
            last_output_rate: 0,
            follow_rate: false,
            exclusive: true,
            hdcd: false,
            dsd_output: 1,
            dsd_packing: None,
            dsd_rate: 0,
            device_label: (String::new(), String::new()),
            analysis: false,
            state: State::Stopped,
            info: None,
            path: None,
        }
    }

    fn lock(&self) -> MutexGuard<'_, Worker> {
        self.worker.0.lock().unwrap()
    }

    /// After a change made under the worker's lock.
    fn wake(&self) {
        self.worker.1.notify_all();
    }

    /// After a change the worker sees without its lock (the mute state):
    /// taking the lock first, the worker is either before its check or
    /// already waiting, so the wake-up cannot fall in between and be lost.
    fn nudge(&self) {
        drop(self.lock());
        self.wake();
    }

    /// The device to play to ("" = the default); takes effect at the next load.
    pub fn set_device(&mut self, key: &str) {
        if key != self.device_key {
            self.device_key = key.to_owned();
            self.close_output();
        }
    }

    pub fn device_key(&self) -> &str {
        &self.device_key
    }

    /// HDCD decoding of the discs that carry it (playback/hdcd); at the next load.
    pub fn set_hdcd(&mut self, on: bool) {
        self.hdcd = on;
    }

    /// WASAPI: play past the Windows mixer (playback/exclusive). Reopens
    /// the device at the next load.
    pub fn set_exclusive(&mut self, on: bool) {
        if on != self.exclusive {
            self.exclusive = on;
            self.close_output();
        }
    }

    /// PipeWire: hold the graph at each track's rate (playback/followTrackRate).
    pub fn set_follow_rate(&mut self, on: bool) {
        if on != self.follow_rate {
            self.follow_rate = on;
            self.close_output();
        }
    }

    /// The open stream's buffer (bufferMs when it was opened), if one is open.
    pub fn buffer_in_use_ms(&self) -> Option<u32> {
        self.shared.as_ref().map(|s| s.buffer_ms)
    }

    /// The ring's length (playback/bufferMs, 60–2000); at the next device open.
    pub fn set_buffer_ms(&mut self, ms: u32) {
        self.buffer_ms = ms.clamp(60, 2000);
    }

    /// Silence the device plays after it changes rate, so the DAC can lock
    /// before the music (playback/resyncDelayMs, 0–3000).
    pub fn set_resync_delay_ms(&mut self, ms: u32) {
        self.resync_ms = ms.min(3000);
    }

    /// A device just opened at `rate`: silence first when the rate changed.
    fn note_output_rate(&mut self, shared: &Shared, rate: u32) {
        let frames = if rate != self.last_output_rate { rate as u64 * self.resync_ms as u64 / 1000 } else { 0 };
        shared.lead_silence.store(frames, Ordering::Relaxed);
        self.last_output_rate = rate;
    }

    /// The processing's settings (EQ, crossfeed, limiter, convolver…).
    pub fn dsp(&self) -> &Arc<Dsp> {
        &self.dsp
    }

    /// playback/upsampling (0 off, 1 ×2, 2 ×4 then ×2, 3 the most) and a
    /// fixed output rate (0 = the track's); at the next load.
    /// How DSD files reach the device (playback/dsdOutput): 0 converted to
    /// PCM, 1 native DSD where the card takes it, 2 DoP on a direct device.
    /// Anything the device refuses plays as PCM.
    pub fn set_dsd_output(&mut self, mode: i64) {
        let mode = mode.clamp(0, 2) as u8;
        if mode != self.dsd_output {
            self.dsd_output = mode;
            if self.dsd_packing.is_some() {
                self.close_output();
            }
        }
    }

    /// The packing DSD at `path` would be asked for (None: PCM).
    fn dsd_wanted(&self, path: &Path) -> Option<DsdPacking> {
        let packing = match self.dsd_output {
            1 => DsdPacking::NativeU32Be,
            2 => DsdPacking::Dop,
            _ => return None,
        };
        // A fixed output rate keeps the DAC at one rate: DSD becomes PCM too.
        (self.fixed_rate == 0 && output::is_direct_key(&self.device_key) && sonkore_dsd::is_dsd_extension(path))
            .then_some(packing)
    }

    /// The device opened for DSD at `path` and its raw decoder; None leaves
    /// nothing open (the track then plays as PCM).
    fn open_dsd(&mut self, path: &Path, packing: DsdPacking) -> Option<(Box<dyn Decoder>, DsdPacking)> {
        let format = sonkore_dsd::probe(path)?;
        let rate = sonkore_dsd::frame_rate(format.dsd_rate, packing);
        let channels = format.channels as usize;
        if rate == 0 || rate > MAX_RATE || channels == 0 || channels > MAX_CHANNELS {
            return None;
        }
        let reuse = self.dsd_packing.is_some_and(|p| p.is_native() == packing.is_native())
            && self
                .output
                .as_ref()
                .is_some_and(|o| !o.failed() && o.format().rate == rate && o.format().channels == channels);
        if !reuse {
            self.close_output();
            // (DSD opens at its own rate or not at all: the ring is sized
            // before, and the device's silence is the DSD idle pattern.)
            let shared =
                Arc::new(Shared::new(rate, channels, self.buffer_ms, 0, block_need(rate, rate), Some(packing)));
            let request = Request { rate, channels, follow_rate: false, exclusive: true, dsd: Some(packing) };
            let output = (self.factory)(&self.device_key, request, shared.clone()).ok()?;
            let f = output.format();
            let usable = f.rate == rate
                && f.channels == channels
                && f.exclusive
                && match f.dsd {
                    Some(p) if p.is_native() => packing.is_native(),
                    Some(_) => {
                        !packing.is_native()
                            && sonkore_dsd::output::dop_device_ok(f.rate, format.dsd_rate, f.float, f.bits)
                    }
                    None => false,
                };
            if !usable {
                return None; // dropping the output closes it
            }
            shared.latency_frames.store(output.latency_frames(), Ordering::Relaxed);
            self.note_output_rate(&shared, f.rate);
            {
                let mut w = self.lock();
                w.shared = Some(shared.clone());
                w.device = Some(f);
                w.device_speed = output.speed();
            }
            self.shared = Some(shared);
            self.output = Some(output);
            self.dsd_packing = f.dsd;
            self.device_label = Self::label_for(&self.device_key, &f);
        }
        let packing = self.dsd_packing?;
        match decode::DsdRaw::open(path, packing) {
            Ok(d) => Some((Box::new(d), packing)),
            Err(_) => {
                self.close_output();
                None
            }
        }
    }

    /// DSD silence ahead of the music while the DAC switches to DSD.
    fn push_dsd_preroll(shared: &Shared, packing: DsdPacking, rate: u32) {
        let ch = shared.channels;
        let frames = sonkore_dsd::output::preroll_frames(rate).min(shared.ring.available_to_write() / ch / 2);
        let mut silence = vec![0i32; frames * ch];
        sonkore_dsd::output::fill_preroll(&mut silence, ch, packing);
        shared.ring.write(&silence);
        shared.frames_pushed.fetch_add(frames as u64, Ordering::Release);
    }

    pub fn set_rate_mode(&mut self, upsampling: i64, fixed_rate: u32) {
        if upsampling != self.upsampling || fixed_rate != self.fixed_rate {
            self.upsampling = upsampling;
            self.fixed_rate = fixed_rate;
            self.close_output();
        }
    }

    /// The rates to try for a track, best first.
    fn wanted_rates(&self, rate: u32) -> Vec<u32> {
        if self.fixed_rate > 0 {
            return vec![self.fixed_rate, rate];
        }
        let factors: &[u32] = match self.upsampling {
            1 => &[2],
            2 => &[4, 2],
            3 => &[16, 8, 4, 2],
            _ => &[],
        };
        let mut rates: Vec<u32> =
            factors.iter().filter_map(|f| rate.checked_mul(*f)).filter(|r| *r <= 768_000).collect();
        rates.push(rate);
        rates
    }

    pub fn set_dither(&mut self, mode: DitherMode) {
        self.lock().quantizer.set_dither(mode);
    }

    /// The listening volume (linear, 0–4).
    pub fn set_volume(&self, volume: f64) {
        self.gains.volume.store(volume.clamp(0.0, 4.0).to_bits(), Ordering::Relaxed);
    }

    pub fn volume(&self) -> f64 {
        f64::from_bits(self.gains.volume.load(Ordering::Relaxed))
    }

    /// ReplayGain's gain for the current track (linear).
    pub fn set_track_gain(&self, gain: f64) {
        self.gains.track_gain.store(gain.to_bits(), Ordering::Relaxed);
    }

    pub fn state(&self) -> State {
        self.state
    }

    pub fn info(&self) -> Option<&StreamInfo> {
        self.info.as_ref()
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn device_format(&self) -> Option<DeviceFormat> {
        self.output.as_ref().map(|o| o.format())
    }

    /// The samples reach the device as the file holds them: the path allows
    /// it, nothing in the processing would change them (the settings, and
    /// what the worker does with the block it is on: a glide, a flush), and
    /// they are the file's (not HDCD-decoded).
    pub fn bit_perfect(&self) -> bool {
        let (Some(info), Some(device)) = (&self.info, self.device_format()) else { return false };
        if self.dsd_packing.is_some() {
            // DSD words reach the DAC untouched (no DSP, no volume).
            return device.exclusive;
        }
        info.lossless
            && !info.codec.ends_with(" HDCD")
            && device.exclusive
            && device.rate == info.rate
            && (device.channels == info.channels || device.mono_to_stereo)
            && info.source_bits() <= device.depth()
            && self.gains.target() == 1.0
            && self.dsp.neutral(info.channels)
            && !self.shared.as_ref().is_some_and(|s| s.processing.load(Ordering::Relaxed))
    }

    /// The device stopped for good (unplugged, the sound server gone).
    pub fn output_failed(&self) -> bool {
        self.output.as_ref().is_some_and(|o| o.failed())
    }

    /// A stream the engine can carry (a corrupt header's rate or channel
    /// count would size the ring).
    fn check_format(info: &StreamInfo) -> Result<()> {
        if info.rate < MIN_RATE || info.rate > MAX_RATE || info.channels == 0 || info.channels > MAX_CHANNELS {
            return Err(EngineError::decode(format!(
                "unsupported format: {} Hz, {} channels",
                info.rate, info.channels
            )));
        }
        Ok(())
    }

    /// Opens `path` (and the device for it), at `start` seconds, paused.
    pub fn load(&mut self, path: &Path, start: f64) -> Result<StreamInfo> {
        self.dsd_rate = Self::dsd_rate_of(path);
        if let Some(packing) = self.dsd_wanted(path) {
            self.fade_out();
            if let Some((mut decoder, packing)) = self.open_dsd(path, packing) {
                let info = decoder.info().clone();
                let first = (start.max(0.0) * info.rate as f64) as u64;
                if first > 0 {
                    decoder.seek(first)?;
                }
                let shared = self.shared.clone().unwrap();
                {
                    let mut w = self.lock();
                    w.decoder = Some(decoder);
                    w.path = Some(path.to_owned());
                    w.next = None;
                    w.next_path = None;
                    w.previous = None;
                    w.previous_path = None;
                    w.chain = Some(Chain::new(self.dsp.clone(), info.rate, info.rate, info.channels));
                    w.dsd = true;
                    w.clear_output();
                    shared.in_rate.store(info.rate as u64, Ordering::Relaxed);
                    shared.processing.store(false, Ordering::Relaxed);
                    shared.ring.clear();
                    Self::reset_counters(&shared, first);
                    Self::push_dsd_preroll(&shared, packing, info.rate);
                }
                self.wake();
                self.info = Some(info.clone());
                self.path = Some(path.to_owned());
                self.state = State::Paused;
                return Ok(info);
            }
        }
        if self.dsd_packing.is_some() {
            // From DSD back to PCM: the device is opened again for PCM.
            self.fade_out();
            self.close_output();
        }
        let mut decoder = decode::open(path)?;
        Self::check_format(decoder.info())?;
        if self.hdcd {
            decoder = decode::HdcdDecoder::wrap(decoder);
        }
        let info = decoder.info().clone();
        if start > 0.0 {
            decoder.seek((start * info.rate as f64) as u64)?;
        }
        self.fade_out();
        let wanted = self.wanted_rates(info.rate);
        // The device as it is, if it is still there, takes one of the rates
        // and its ring has room for this track's blocks.
        let reuse = self.output.as_ref().is_some_and(|o| {
            let f = o.format();
            !o.failed()
                && wanted.contains(&f.rate)
                && (f.channels == info.channels || (f.mono_to_stereo && info.channels == 1))
        }) && self.shared.as_ref().is_some_and(|s| {
            s.channels == info.channels && s.ring.capacity() / s.channels >= 2 * block_need(info.rate, s.out_rate)
        });
        if !reuse {
            self.close_output();
            // The best rate the device opens at exactly; else what it takes.
            let mut opened = None;
            for (i, &rate) in wanted.iter().enumerate() {
                let pending = Arc::new(Pending::default());
                let request = Request {
                    rate,
                    channels: info.channels,
                    follow_rate: self.follow_rate,
                    exclusive: self.exclusive,
                    dsd: None,
                };
                match (self.factory)(&self.device_key, request, pending.clone()) {
                    Ok(output) if output.format().rate == rate || i + 1 == wanted.len() => {
                        opened = Some((pending, output));
                        break;
                    }
                    Ok(_) => continue,
                    Err(e) if i + 1 == wanted.len() => return Err(e.into()),
                    Err(_) => continue,
                }
            }
            let (pending, output) = opened.ok_or_else(|| EngineError::output("no output"))?;
            let format = output.format();
            if format.rate == 0 || format.rate > MAX_RATE {
                return Err(EngineError::output(format!("the device opened at {} Hz", format.rate)));
            }
            // The ring for the rate the device opened at (which may be far
            // above the track's: 8 kHz on a 192 kHz DAC).
            let shared = Arc::new(Shared::new(
                format.rate,
                info.channels,
                self.buffer_ms,
                format.period_frames,
                block_need(info.rate, format.rate),
                None,
            ));
            shared.mono_to_stereo.store(format.mono_to_stereo, Ordering::Relaxed);
            shared.device_depth.store(format.depth() as u64, Ordering::Relaxed);
            shared.latency_frames.store(output.latency_frames(), Ordering::Relaxed);
            shared.analysis_on.store(self.analysis, Ordering::Relaxed);
            self.note_output_rate(&shared, format.rate);
            // From here the device thread renders the engine's state (muted).
            let _ = pending.0.set(shared.clone());
            {
                let mut w = self.lock();
                w.shared = Some(shared.clone());
                w.device = Some(format);
                w.device_speed = output.speed();
                w.quantizer.set_target_bits(format.quantizer_bits());
            }
            self.device_label = Self::label_for(&self.device_key, &format);
            self.shared = Some(shared);
            self.output = Some(output);
        }
        let shared = self.shared.clone().unwrap();
        {
            let mut w = self.lock();
            let out_rate = w.device.map_or(info.rate, |d| d.rate);
            w.quantizer.set_format(out_rate, info.channels);
            w.decoder = Some(decoder);
            w.path = Some(path.to_owned());
            w.next = None;
            w.next_path = None;
            w.previous = None;
            w.previous_path = None;
            w.dsd = false;
            // A new chain for the stream, at its gain from the first sample.
            let mut chain = Chain::new(self.dsp.clone(), info.rate, out_rate, info.channels);
            chain.applied_gain = self.gains.target();
            chain.applied_headroom = chain.headroom();
            w.chain = Some(chain);
            w.clear_output();
            shared.in_rate.store(info.rate as u64, Ordering::Relaxed);
            shared.processing.store(false, Ordering::Relaxed);
            shared.ring.clear();
            Self::reset_counters(&shared, (start * info.rate as f64) as u64);
        }
        self.wake();
        self.info = Some(info.clone());
        self.path = Some(path.to_owned());
        self.state = State::Paused;
        Ok(info)
    }

    fn reset_counters(shared: &Shared, decoded: u64) {
        let played = shared.frames_played.load(Ordering::Acquire);
        shared.frames_pushed.store(played, Ordering::Release);
        shared.decoded.store(decoded, Ordering::Relaxed);
        shared.splice_at.store(NO_SPLICE, Ordering::Release);
        shared.eof.store(false, Ordering::Release);
        shared.drained.store(false, Ordering::Release);
        shared.tail_frames.store(0, Ordering::Relaxed);
    }

    /// The track to carry on into without a gap (None: nothing follows).
    pub fn set_next(&mut self, path: Option<&Path>) {
        if self.lock().next_path.as_deref() == path {
            return;
        }
        let hdcd = self.hdcd;
        let decoder = match (self.dsd_packing, path) {
            // DSD carries on only into DSD in the same packing (the rate is checked at the join).
            (Some(packing), Some(p)) => {
                if sonkore_dsd::is_dsd_extension(p) && self.dsd_wanted(p).is_some() {
                    decode::DsdRaw::open(p, packing).ok().map(|d| Box::new(d) as Box<dyn Decoder>)
                } else {
                    None
                }
            }
            // A DSD track that will go out as DSD needs its device: no join.
            (None, Some(p)) if self.dsd_wanted(p).is_some() => None,
            (None, Some(p)) => decode::open(p).ok().map(|d| if hdcd { decode::HdcdDecoder::wrap(d) } else { d }),
            (_, None) => None,
        };
        {
            let mut w = self.lock();
            w.next_path = path.filter(|_| decoder.is_some()).map(Path::to_owned);
            w.next = decoder;
            w.next_gain = None; // given again for this track (set_next_track_gain)
        }
        self.wake();
    }

    /// The ReplayGain of the track set with set_next(): applied at the join,
    /// sample-exactly. None: the gain stays (set_track_gain changes it later).
    pub fn set_next_track_gain(&mut self, gain: Option<f64>) {
        self.lock().next_gain = gain.filter(|g| g.is_finite() && *g > 0.0);
    }

    /// True once when the device starts playing the track carried on into:
    /// the current track is now the one the worker joined (the one set with
    /// set_next() then).
    pub fn take_track_change(&mut self) -> bool {
        let Some(shared) = &self.shared else { return false };
        let at = shared.splice_at.load(Ordering::Acquire);
        if at == NO_SPLICE || shared.frames_played.load(Ordering::Acquire) < at {
            return false;
        }
        shared.splice_at.store(NO_SPLICE, Ordering::Release);
        let (info, path) = {
            let mut w = self.lock();
            w.previous = None;
            w.previous_path = None;
            (w.decoder.as_ref().map(|d| d.info().clone()), w.path.clone())
        };
        if info.is_some() {
            self.info = info;
        }
        self.path = path;
        self.dsd_rate = self.path.as_deref().map_or(0, Self::dsd_rate_of);
        true
    }

    fn dsd_rate_of(path: &Path) -> u32 {
        if sonkore_dsd::is_dsd_extension(path) { sonkore_dsd::probe(path).map_or(0, |f| f.dsd_rate) } else { 0 }
    }

    /// The backend and the name the device shows in Settings.
    fn label_for(key: &str, format: &DeviceFormat) -> (String, String) {
        let key = output::resolve_key(key);
        if key == "null" {
            return ("ALSA".into(), "null".into());
        }
        let devices = output::devices();
        if let Some(d) = devices.iter().find(|d| d.key == key) {
            return (d.backend.clone(), d.name.clone());
        }
        if let Some(name) = key.strip_prefix("pipewire|") {
            return ("PipeWire".into(), name.to_owned());
        }
        if let Some(hw) = key.strip_prefix("alsa|") {
            return ("ALSA".into(), hw.to_owned());
        }
        if let Some(name) = key.strip_prefix("wasapi|") {
            return ("WASAPI".into(), name.to_owned());
        }
        output::default_label(&devices, format)
    }

    /// The track's 1-bit rate when it is DSD (as DSD or converted), else 0.
    pub fn dsd_rate(&self) -> u32 {
        self.dsd_rate
    }

    /// What the output does now.
    pub fn output_status(&self) -> OutputStatus {
        let (Some(device), Some(info)) = (self.device_format(), &self.info) else { return OutputStatus::default() };
        let resampled = self.dsd_packing.is_none() && device.rate != info.rate;
        let channels_ok = device.channels == info.channels || device.mono_to_stereo;
        OutputStatus {
            backend: self.device_label.0.clone(),
            device_name: self.device_label.1.clone(),
            exclusive: device.exclusive,
            device_rate: device.rate,
            device_channels: device.channels,
            device_bits: device.bits,
            device_float: device.float,
            resampled,
            source_rate: info.rate,
            path_can_be_bit_perfect: device.exclusive
                && channels_ok
                && (self.dsd_packing.is_some()
                    || (info.lossless && device.rate == info.rate && info.source_bits() <= device.depth())),
            bit_perfect_now: self.bit_perfect(),
            dsd: if self.dsd_packing.is_some() { info.codec.clone() } else { String::new() },
            hdcd: info.codec.ends_with(" HDCD"),
            upsampled: resampled && self.upsampling > 0 && self.fixed_rate == 0 && device.rate > info.rate,
            pcm_to_dsd_rate: 0,
        }
    }

    pub fn play(&mut self) {
        let Some(shared) = &self.shared else { return };
        if self.path.is_none() {
            return;
        }
        if shared.mute.load(Ordering::Acquire) != OPEN {
            // Sound comes back once the ring holds enough (half of it, at
            // most 400 ms): an empty ring would start with an underrun.
            self.wake();
            let deadline = Instant::now() + Duration::from_millis(400);
            while shared.ring.queued() < shared.ring.capacity() / 2
                && !shared.eof.load(Ordering::Acquire)
                && Instant::now() < deadline
            {
                std::thread::sleep(Duration::from_millis(2));
            }
            shared.ramp_in.store(true, Ordering::Release);
            shared.mute.store(OPEN, Ordering::Release);
            // The worker waits without a timeout while the device is muted.
            self.nudge();
        }
        self.state = State::Playing;
    }

    pub fn pause(&mut self) {
        if self.state == State::Playing {
            self.fade_out();
            self.state = State::Paused;
        }
    }

    pub fn stop(&mut self) {
        self.fade_out();
        if let Some(shared) = self.shared.clone() {
            let mut w = self.lock();
            w.decoder = None;
            w.path = None;
            w.next = None;
            w.next_path = None;
            w.previous = None;
            w.previous_path = None;
            shared.ring.clear();
            Self::reset_counters(&shared, 0);
        }
        self.state = State::Stopped;
        self.info = None;
        self.path = None;
    }

    /// Goes to `seconds`; playing goes on from there once the ring has refilled.
    pub fn seek(&mut self, seconds: f64) -> Result<()> {
        let Some(shared) = self.shared.clone() else { return Ok(()) };
        let playing = self.state == State::Playing;
        self.fade_out();
        {
            let mut w = self.lock();
            // A join not heard yet is undone: back to the track still playing.
            if shared.splice_at.load(Ordering::Acquire) != NO_SPLICE {
                if let Some(previous) = w.previous.take() {
                    let mut spliced = w.decoder.replace(previous).unwrap();
                    let _ = spliced.seek(0);
                    w.next = Some(spliced);
                    let previous_path = w.previous_path.take();
                    w.next_path = std::mem::replace(&mut w.path, previous_path);
                    if let Some(g) = w.previous_gain.take() {
                        w.next_gain = Some(f64::from_bits(self.gains.track_gain.swap(g.to_bits(), Ordering::Relaxed)));
                    }
                }
            }
            let Some(decoder) = w.decoder.as_mut() else { return Ok(()) };
            let rate = decoder.info().rate as f64;
            let frame = decoder.seek((seconds.max(0.0) * rate).round() as u64)?;
            shared.ring.clear();
            Self::reset_counters(&shared, frame);
            if let Some(packing) = self.dsd_packing {
                Self::push_dsd_preroll(&shared, packing, rate as u32);
            }
            w.quantizer.reset();
            w.clear_output();
            let target = self.gains.target();
            if let Some(chain) = w.chain.as_mut() {
                chain.reset();
                chain.applied_gain = target;
                chain.applied_headroom = chain.headroom();
            }
        }
        self.wake();
        if playing {
            // Refill half the ring (at most 400 ms) before sound comes back.
            let deadline = Instant::now() + Duration::from_millis(400);
            while shared.ring.queued() < shared.ring.capacity() / 2
                && !shared.eof.load(Ordering::Acquire)
                && Instant::now() < deadline
            {
                std::thread::sleep(Duration::from_millis(2));
            }
            shared.ramp_in.store(true, Ordering::Release);
            shared.mute.store(OPEN, Ordering::Release);
            self.nudge();
        }
        Ok(())
    }

    /// Seconds into the current track, as heard.
    pub fn position(&self) -> f64 {
        let (Some(shared), Some(info)) = (&self.shared, &self.info) else { return 0.0 };
        let played = shared.frames_played.load(Ordering::Acquire);
        let at = shared.splice_at.load(Ordering::Acquire);
        // Output frames are at the device's rate; the position at the track's.
        let ratio = info.rate as f64 / self.device_format().map_or(info.rate, |f| f.rate).max(1) as f64;
        let frames = if at != NO_SPLICE && played < at {
            (shared.splice_old_end.load(Ordering::Relaxed) as f64 - (at - played) as f64 * ratio).max(0.0)
        } else {
            let queued = shared.frames_pushed.load(Ordering::Acquire).saturating_sub(played)
                + shared.chain_pending.load(Ordering::Relaxed);
            // SoundTouch: each output frame is `speed` source frames.
            let speed = self.dsp.stretch.speed_factor();
            let held = f64::from_bits(shared.stretch_held.load(Ordering::Relaxed));
            (shared.decoded.load(Ordering::Relaxed) as f64 - held - queued as f64 * ratio * speed).max(0.0)
        };
        frames / info.rate.max(1) as f64
    }

    pub fn duration(&self) -> f64 {
        self.info.as_ref().and_then(StreamInfo::duration_seconds).unwrap_or(0.0)
    }

    /// The track ended and its last sample has been played.
    pub fn finished(&self) -> bool {
        self.shared.as_ref().is_some_and(|s| s.eof.load(Ordering::Acquire) && s.drained.load(Ordering::Acquire))
    }

    /// The visualizer's tap on (what the device plays is kept, mono).
    pub fn set_analysis(&mut self, on: bool) {
        self.analysis = on;
        if let Some(s) = &self.shared {
            s.analysis_on.store(on, Ordering::Relaxed);
        }
    }

    /// The `dst.len()` mono samples played `delay` frames ago (the device's
    /// buffer: what is heard now); false before there are that many.
    pub fn recent_output(&self, dst: &mut [f32]) -> bool {
        let Some(s) = &self.shared else { return false };
        let delay = self.output.as_ref().map_or(0, |o| o.latency_frames());
        let written = s.analysis_written.load(Ordering::Acquire);
        let n = dst.len() as u64;
        if written < n + delay || n as usize > ANALYSIS_SIZE {
            return false;
        }
        let start = written - delay - n;
        for (i, d) in dst.iter_mut().enumerate() {
            *d =
                f32::from_bits(s.analysis[((start + i as u64) as usize) & (ANALYSIS_SIZE - 1)].load(Ordering::Relaxed));
        }
        true
    }

    /// The limiter's reduction now (dB, <= 0).
    pub fn limiter_reduction_db(&self) -> f64 {
        self.shared.as_ref().map_or(0.0, |s| f64::from_bits(s.limiter_db.load(Ordering::Relaxed)))
    }

    pub fn underruns(&self) -> u64 {
        self.shared.as_ref().map_or(0, |s| s.underruns.load(Ordering::Relaxed))
    }

    /// Samples clipped by the quantizer since the last call.
    pub fn take_clipped(&self) -> u64 {
        std::mem::take(&mut self.lock().clipped)
    }

    /// Fades the device out and waits for it (500 ms at most).
    fn fade_out(&self) {
        let Some(shared) = &self.shared else { return };
        if shared.mute.load(Ordering::Acquire) == MUTED {
            return;
        }
        if self.output_failed() {
            // Nothing renders any more: nothing to wait for.
            shared.mute.store(MUTED, Ordering::Release);
            return;
        }
        shared.mute_ack.store(false, Ordering::Release);
        shared.mute.store(FADING_OUT, Ordering::Release);
        let deadline = Instant::now() + Duration::from_millis(500);
        while !(shared.mute_ack.load(Ordering::Acquire) && shared.mute.load(Ordering::Acquire) == MUTED) {
            if Instant::now() > deadline || self.output_failed() {
                shared.mute.store(MUTED, Ordering::Release);
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Closes the device (it is opened again at the next load).
    pub fn close_output(&mut self) {
        self.fade_out();
        self.output = None; // its thread stops here
        let mut w = self.lock();
        w.shared = None;
        w.device = None;
        w.dsd = false;
        drop(w);
        self.shared = None;
        self.dsd_packing = None;
        self.device_label = (String::new(), String::new());
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.close_output();
        self.lock().quit = true;
        self.wake();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Denormals as zero on this thread (FTZ and DAZ): an equalizer or a reverb
/// tail over digital silence decays into denormal numbers, which x86 handles
/// tens of times slower. Inaudible: they are below −700 dBFS.
fn flush_denormals_to_zero() {
    #[cfg(target_arch = "x86_64")]
    // SAFETY: only MXCSR's FTZ (bit 15) and DAZ (bit 6) change; this thread
    // only, and SSE is always there on x86_64.
    unsafe {
        let mut csr: u32 = 0;
        std::arch::asm!("stmxcsr [{}]", in(reg) &mut csr, options(nostack));
        csr |= 0x8040;
        std::arch::asm!("ldmxcsr [{}]", in(reg) &csr, options(nostack, readonly));
    }
    #[cfg(target_arch = "aarch64")]
    // SAFETY: only FPCR's FZ bit (24) changes, on this thread.
    unsafe {
        let mut fpcr: u64;
        std::arch::asm!("mrs {}, fpcr", out(reg) fpcr, options(nomem, nostack));
        fpcr |= 1 << 24;
        std::arch::asm!("msr fpcr, {}", in(reg) fpcr, options(nomem, nostack));
    }
}

#[cfg(test)]
mod denormal_tests {
    #[test]
    fn the_worker_flushes_denormals() {
        let tiny = std::hint::black_box(f64::from_bits(1)); // the smallest denormal
        assert!(tiny * std::hint::black_box(1.0) != 0.0, "denormals are kept by default");
        let flushed = std::thread::spawn(move || {
            super::flush_denormals_to_zero();
            tiny * std::hint::black_box(1.0)
        })
        .join()
        .unwrap();
        assert_eq!(flushed, 0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::MemoryOutput;

    fn engine() -> Engine {
        Engine::with_outputs(Box::new(|_key: &str, req: Request, render: Arc<dyn Render>| {
            let format = DeviceFormat {
                rate: req.rate,
                channels: req.channels,
                bits: 32,
                float: false,
                mono_to_stereo: false,
                exclusive: true,
                period_frames: 1024,
                buffer_frames: 3072,
                dsd: None,
            };
            Ok(Box::new(MemoryOutput::open(format, render, 1.0, 0)) as Box<dyn Output>)
        }))
    }

    fn wav(name: &str, frames: usize) -> PathBuf {
        let path = std::env::temp_dir().join(format!("sonkore-engine-{name}-{}.wav", std::process::id()));
        decode::write_sine_wav(&path, 44100, 2, frames);
        path
    }

    fn wakeups_in(e: &Engine, d: Duration) -> u64 {
        let before = e.lock().wakeups;
        std::thread::sleep(d);
        e.lock().wakeups - before
    }

    #[test]
    fn the_worker_sleeps_while_paused() {
        // Paused, the ring stays full: the worker woke every 2 ms for good.
        let path = wav("paused", 44100 * 4);
        let mut e = engine();
        e.load(&path, 0.0).unwrap();
        e.play();
        std::thread::sleep(Duration::from_millis(200));
        e.pause();
        std::thread::sleep(Duration::from_millis(100));
        let woke = wakeups_in(&e, Duration::from_millis(500));
        assert!(woke <= 2, "{woke} wakeups in 0.5 s of pause");
        // It plays on when asked.
        e.play();
        let at = e.position();
        std::thread::sleep(Duration::from_millis(300));
        assert!(e.position() > at + 0.2, "playing on: {} after {at}", e.position());
        assert_eq!(e.underruns(), 0);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_worker_wakes_once_a_period_while_playing() {
        // 1024-frame periods at 44.1 kHz: ~22 in 0.5 s.
        let path = wav("playing", 44100 * 4);
        let mut e = engine();
        e.load(&path, 0.0).unwrap();
        e.play();
        std::thread::sleep(Duration::from_millis(300));
        let woke = wakeups_in(&e, Duration::from_millis(500));
        assert!(woke < 80, "{woke} wakeups in 0.5 s of playing");
        assert_eq!(e.underruns(), 0);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_worker_sleeps_once_stopped() {
        let path = wav("stopped", 4410);
        let mut e = engine();
        e.load(&path, 0.0).unwrap();
        e.play();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !e.finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(e.finished());
        let woke = wakeups_in(&e, Duration::from_millis(500));
        assert!(woke <= 2, "{woke} wakeups in 0.5 s at the end");
        e.stop();
        let woke = wakeups_in(&e, Duration::from_millis(500));
        assert!(woke <= 2, "{woke} wakeups in 0.5 s stopped");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_join_keeps_its_track_when_the_next_one_changes() {
        // The next track was set again between the join and the moment it is
        // heard: the track heard was labelled with the new "next" (or none).
        // (A track shorter than the ring ends before set_next() arrives.)
        let (a, b, c) = (wav("join-a", 44100), wav("join-b", 22050), wav("join-c", 22050));
        for then in [None, Some(c.as_path())] {
            let mut e = engine();
            e.load(&a, 0.0).unwrap();
            e.set_next(Some(&b));
            e.play();
            let shared = e.shared.clone().unwrap();
            let deadline = Instant::now() + Duration::from_secs(2);
            while shared.splice_at.load(Ordering::Acquire) == NO_SPLICE && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(2));
            }
            assert_ne!(shared.splice_at.load(Ordering::Acquire), NO_SPLICE, "joined");
            assert!(!e.take_track_change(), "not heard yet");
            e.set_next(then);
            let deadline = Instant::now() + Duration::from_secs(3);
            while !e.take_track_change() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
            assert_eq!(e.path(), Some(b.as_path()), "the track heard, with {then:?} next");
        }
        for p in [a, b, c] {
            let _ = std::fs::remove_file(p);
        }
    }

    #[test]
    fn upsampling_a_huge_rate_does_not_overflow() {
        let mut e = engine();
        e.set_rate_mode(3, 0);
        assert_eq!(e.wanted_rates(u32::MAX / 2), vec![u32::MAX / 2]);
        assert_eq!(e.wanted_rates(44100), vec![705600, 352800, 176400, 88200, 44100]);
    }
}
