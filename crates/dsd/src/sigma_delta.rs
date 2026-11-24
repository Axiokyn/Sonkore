//! PCM to DSD: an 8th-order single-bit sigma-delta
//! modulator, run at the DSD rate on PCM already resampled to it, packing the
//! bits for the DAC the same way [`DsdDecoder`](crate::DsdDecoder) does
//! (native DSD_U32 words or DoP). Bit-exact with the recorded reference for
//! the same input.
//!
//! Noise transfer function after Schreier's synthesizeNTF: optimised zeros
//! (Legendre roots across the 0-22.05 kHz band) and poles set for an
//! out-of-band gain of 1.5 (Lee's rule for a stable 1-bit loop), designed for
//! each oversampling ratio (DSD64 / 128 / 256 / 512). Error-feedback form,
//! double precision:
//!   w = u + ((N - D) / D) e,  y = sign(w),  e = y - w
//! so the output is u + NTF * e.
//!
//! A 1-bit loop of this order overloads above about -5.5 dBFS of modulation:
//! the input is scaled so PCM full scale lands at 45 % (-6.9 dB), limited to
//! 50 %, and a loop that diverges all the same is reset (counted in
//! [`SigmaDelta::resets`]).
//!
//! Each channel's loop is independent, so with more than one channel the
//! channels after the first run on their own threads (started by
//! [`SigmaDelta::setup`], parked on a condition variable between blocks,
//! never spawned per block): DSD512 stereo at 22.6 MHz would otherwise need
//! more than one core's worth of time. A block costs one lock/notify round per
//! extra channel and no allocation once the word buffers have grown.

use std::cell::UnsafeCell;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use crate::DsdPacking;

pub const ORDER: usize = 8;
pub const INPUT_SCALE: f64 = 0.45;
pub const INPUT_LIMIT: f64 = 0.5;
const DIVERGENCE: f64 = 8.0; // a stable loop stays within about 1.9

// NTF numerator N(z) and denominator D(z), z^0 .. z^-8, for OSR 64 / 128 /
// 256 / 512.
const K_N: [[f64; 9]; 4] = [
    [
        1.0,
        -7.99550277716134,
        27.973022913961728,
        -55.93256665865651,
        69.91009304371242,
        -55.93256665865652,
        27.973022913961728,
        -7.99550277716134,
        0.9999999999999998,
    ],
    [
        1.0,
        -7.998875573589071,
        27.99325383229928,
        -55.983135166852676,
        69.97751381628494,
        -55.983135166852676,
        27.99325383229928,
        -7.99887557358907,
        0.9999999999999998,
    ],
    [
        1.0,
        -7.999718885852985,
        27.99831333954193,
        -55.995783385490185,
        69.99437786360247,
        -55.995783385490185,
        27.998313339541934,
        -7.999718885852987,
        1.0,
    ],
    [
        1.0,
        -7.999929720991721,
        27.999578327476847,
        -55.99894582098188,
        69.99859442899351,
        -55.99894582098189,
        27.999578327476854,
        -7.999929720991721,
        1.0,
    ],
];
const K_D: [[f64; 9]; 4] = [
    [
        1.0,
        -7.188018653886824,
        22.64220562070373,
        -40.82087305319691,
        46.067024930450735,
        -33.32089641893887,
        15.084749485485242,
        -3.9076376140463376,
        0.4434459698181196,
    ],
    [
        1.0,
        -7.189698064780384,
        22.652621453416984,
        -40.848614520270345,
        46.10815184956516,
        -33.357546121936366,
        15.104379832951356,
        -3.9134887952392416,
        0.44419462851719965,
    ],
    [
        1.0,
        -7.1901178941445245,
        22.655225692998858,
        -40.8555517339486,
        46.11843789208569,
        -33.36671375074137,
        15.109290913235506,
        -3.9149528363122164,
        0.4443819780187692,
    ],
    [
        1.0,
        -7.190222850024472,
        22.655876770472716,
        -40.85728615278703,
        46.12100967228441,
        -33.36900598320961,
        15.110518901725289,
        -3.915318924479541,
        0.4444288269528381,
    ],
];

#[derive(Clone, Default)]
struct Loop {
    e: [f64; ORDER], // past quantisation errors, newest first
    r: [f64; ORDER], // past feedback values, newest first
    bits: u32,       // the word being filled
    resets: u64,
    words: Vec<i32>, // this channel's words of the current block
}

/// What every loop shares (read-only during a block).
#[derive(Clone, Copy)]
struct Params {
    fb: [f64; ORDER], // (N - D) coefficients, z^-1 .. z^-8
    d: [f64; ORDER],  // D coefficients, z^-1 .. z^-8
    channels: usize,
    packing: DsdPacking,
    bits_per_word: usize,
}

/// Bits (oldest first, MSB) to the device word, as DsdDecoder packs a file.
#[inline]
fn pack(packing: DsdPacking, bits: u32) -> i32 {
    match packing {
        DsdPacking::Dop => (0x0500_0000u32 | (bits & 0xFFFF) << 8) as i32, // the output stamps the real marker
        DsdPacking::NativeU32Be => i32::from_ne_bytes(bits.to_be_bytes()),
        DsdPacking::NativeU32Le => i32::from_ne_bytes(bits.to_le_bytes()),
    }
}

/// Runs channel `c` over `frames` interleaved frames: its words of the block
/// into `l.words`.
fn modulate(p: &Params, l: &mut Loop, c: usize, input: &[f64], frames: usize, filled_start: usize) {
    l.words.clear();
    let ch = p.channels;
    let bits_per_word = p.bits_per_word;
    let mut filled = filled_start;
    let mut bits = l.bits;
    // The histories in rings written twice (at p and p + ORDER), so the
    // newest-first window is always the contiguous run [p, p + ORDER): no
    // shifting per sample, and the feedback is a plain dot product.
    let mut he = [0.0f64; 2 * ORDER];
    let mut hr = [0.0f64; 2 * ORDER];
    he[..ORDER].copy_from_slice(&l.e);
    he[ORDER..].copy_from_slice(&l.e);
    hr[..ORDER].copy_from_slice(&l.r);
    hr[ORDER..].copy_from_slice(&l.r);
    let fbc = &p.fb;
    let dc = &p.d;
    let mut pos = 0usize;
    let mut resets = 0u64;
    for f in 0..frames {
        let u = (input[f * ch + c] * INPUT_SCALE).clamp(-INPUT_LIMIT, INPUT_LIMIT);
        let e: &[f64; ORDER] = he[pos..pos + ORDER].try_into().unwrap();
        let r: &[f64; ORDER] = hr[pos..pos + ORDER].try_into().unwrap();
        // Summed as a tree: the sixteen products are independent, so the
        // chain the next sample has to wait for is four additions deep.
        let fb = (((fbc[0] * e[0] - dc[0] * r[0]) + (fbc[1] * e[1] - dc[1] * r[1]))
            + ((fbc[2] * e[2] - dc[2] * r[2]) + (fbc[3] * e[3] - dc[3] * r[3])))
            + (((fbc[4] * e[4] - dc[4] * r[4]) + (fbc[5] * e[5] - dc[5] * r[5]))
                + ((fbc[6] * e[6] - dc[6] * r[6]) + (fbc[7] * e[7] - dc[7] * r[7])));
        let w = u + fb;
        let y = if w >= 0.0 { 1.0 } else { -1.0 };
        if w.abs() > DIVERGENCE || !w.is_finite() {
            he.fill(0.0); // overloaded: start the loop again
            hr.fill(0.0);
            resets += 1;
        } else {
            pos = (pos + ORDER - 1) % ORDER;
            he[pos] = y - w;
            he[pos + ORDER] = y - w;
            hr[pos] = fb;
            hr[pos + ORDER] = fb;
        }
        bits = bits << 1 | (y > 0.0) as u32;
        filled += 1;
        if filled == bits_per_word {
            l.words.push(pack(p.packing, bits));
            filled = 0;
        }
    }
    l.e.copy_from_slice(&he[pos..pos + ORDER]);
    l.r.copy_from_slice(&hr[pos..pos + ORDER]);
    l.bits = bits;
    l.resets += resets;
}

struct Job {
    generation: u64,
    pending: usize,
    /// A worker's modulation panicked during this block.
    failed: bool,
    quit: bool,
    input: *const f64,
    len: usize,
    frames: usize,
    filled: usize,
}

// The raw input pointer is only read by workers while `process` waits for them.
unsafe impl Send for Job {}

struct Shared {
    job: Mutex<Job>,
    wake: Condvar,
    done: Condvar,
    params: UnsafeCell<Params>,
    loops: Vec<UnsafeCell<Loop>>,
    #[cfg(test)]
    fault: tests::Fault,
}

/// Test hook: a channel's modulation can be made to panic, or to be slow.
#[cfg(test)]
fn inject(shared: &Shared, c: usize) {
    shared.fault.hit(c);
}

#[cfg(not(test))]
#[inline(always)]
fn inject(_: &Shared, _: usize) {}

// Each loop is touched by one thread at a time: worker `c` during a block
// (between the generation bump and its `pending` decrement, both under the
// mutex), the owner otherwise. `params` is written only with no workers running.
unsafe impl Sync for Shared {}
unsafe impl Send for Shared {}

/// The modulator.
pub struct SigmaDelta {
    shared: Arc<Shared>,
    workers: Vec<JoinHandle<()>>,
    filled: usize, // bits in the current word (the same in every channel)
}

impl Default for SigmaDelta {
    fn default() -> Self {
        Self::new()
    }
}

impl SigmaDelta {
    pub const ORDER: usize = ORDER;
    pub const INPUT_SCALE: f64 = INPUT_SCALE;
    pub const INPUT_LIMIT: f64 = INPUT_LIMIT;

    /// An idle modulator: call [`setup`](Self::setup) before use.
    pub fn new() -> Self {
        SigmaDelta { shared: Arc::new(Self::shared(0, DsdPacking::NativeU32Be, 64)), workers: Vec::new(), filled: 0 }
    }

    fn shared(channels: usize, packing: DsdPacking, dsd_multiple: u32) -> Shared {
        let set = if dsd_multiple >= 512 {
            3
        } else if dsd_multiple >= 256 {
            2
        } else if dsd_multiple >= 128 {
            1
        } else {
            0
        };
        let mut fb = [0.0; ORDER];
        let mut d = [0.0; ORDER];
        for k in 0..ORDER {
            fb[k] = K_N[set][k + 1] - K_D[set][k + 1];
            d[k] = K_D[set][k + 1];
        }
        Shared {
            job: Mutex::new(Job {
                generation: 0,
                pending: 0,
                failed: false,
                quit: false,
                input: std::ptr::null(),
                len: 0,
                frames: 0,
                filled: 0,
            }),
            wake: Condvar::new(),
            done: Condvar::new(),
            params: UnsafeCell::new(Params { fb, d, channels, packing, bits_per_word: packing.bits_per_word() }),
            loops: (0..channels).map(|_| UnsafeCell::new(Loop::default())).collect(),
            #[cfg(test)]
            fault: tests::Fault::default(),
        }
    }

    /// `dsd_multiple`: 64, 128, 256 or 512 (times 44.1 kHz). Starts one
    /// worker thread per channel after the first.
    pub fn setup(&mut self, dsd_multiple: u32, channels: usize, packing: DsdPacking) {
        self.stop_workers();
        self.shared = Arc::new(Self::shared(channels, packing, dsd_multiple));
        self.filled = 0;
        for c in 1..channels {
            let shared = Arc::clone(&self.shared);
            let handle = std::thread::Builder::new()
                .name(format!("sonkore-sd{c}"))
                .spawn(move || worker(&shared, c))
                .expect("spawn the modulator's worker");
            self.workers.push(handle);
        }
    }

    fn stop_workers(&mut self) {
        {
            let mut job = self.shared.job.lock().unwrap_or_else(|e| e.into_inner());
            job.quit = true;
        }
        self.shared.wake.notify_all();
        for w in self.workers.drain(..) {
            let _ = w.join();
        }
    }

    fn params(&self) -> Params {
        // SAFETY: written only in `shared()`, before any worker exists.
        unsafe { *self.shared.params.get() }
    }

    pub fn reset(&mut self) {
        for l in &self.shared.loops {
            // SAFETY: no block is running (`process` takes `&mut self` and
            // returns only once every worker is done).
            let l = unsafe { &mut *l.get() };
            l.e = [0.0; ORDER];
            l.r = [0.0; ORDER];
            l.bits = 0;
        }
        self.filled = 0;
    }

    /// DSD bits per output word: 32 native, 16 DoP.
    pub fn bits_per_word(&self) -> usize {
        self.params().bits_per_word
    }

    pub fn channels(&self) -> usize {
        self.params().channels
    }

    /// Loop restarts after an overload, all channels.
    pub fn resets(&self) -> u64 {
        // SAFETY: as in `reset`.
        self.shared.loops.iter().map(|l| unsafe { (*l.get()).resets }).sum()
    }

    /// Interleaved PCM frames at the DSD rate in (1.0 = full scale); packed
    /// words out (one per channel and output frame: 32 DSD bits native, 16
    /// for DoP). Returns the output frames written to `out`, which needs room
    /// for `(frames / bits_per_word + 1) * channels` words.
    pub fn process(&mut self, input: &[f64], out: &mut [i32]) -> usize {
        let p = self.params();
        let ch = p.channels;
        if ch == 0 {
            return 0;
        }
        let frames = input.len() / ch;
        if frames == 0 {
            return 0;
        }
        if ch > 1 {
            {
                let mut job = self.shared.job.lock().unwrap_or_else(|e| e.into_inner());
                job.input = input.as_ptr();
                job.len = input.len();
                job.frames = frames;
                job.filled = self.filled;
                job.pending = ch - 1;
                job.failed = false;
                job.generation += 1;
            }
            // From here on the workers read `input`: the guard waits for them
            // however this scope is left (a panic in channel 0 included), so
            // the input outlives every read.
            let block = Block(&self.shared);
            self.shared.wake.notify_all();
            inject(&self.shared, 0);
            // SAFETY: loop 0 is never touched by a worker.
            modulate(&p, unsafe { &mut *self.shared.loops[0].get() }, 0, input, frames, self.filled);
            if block.finish() {
                // A worker's channel panicked (its loop is left half-way):
                // the caller sees it as it would on one thread.
                panic!("sigma-delta modulator: a channel's worker panicked");
            }
        } else {
            // SAFETY: no workers with one channel.
            modulate(&p, unsafe { &mut *self.shared.loops[0].get() }, 0, input, frames, self.filled);
        }
        // SAFETY: every worker is done with its loop.
        let written = unsafe { (*self.shared.loops[0].get()).words.len() };
        for c in 0..ch {
            let words = unsafe { &(*self.shared.loops[c].get()).words };
            for (w, &word) in words.iter().enumerate().take(written) {
                out[w * ch + c] = word;
            }
        }
        self.filled = (self.filled + frames) % p.bits_per_word;
        written
    }

    /// Completes the last partial word with DSD silence; returns 0 or 1 frame.
    pub fn flush(&mut self, out: &mut [i32]) -> usize {
        if self.filled == 0 {
            return 0;
        }
        let p = self.params();
        for c in 0..p.channels {
            // SAFETY: as in `reset`.
            let mut bits = unsafe { (*self.shared.loops[c].get()).bits };
            for i in self.filled..p.bits_per_word {
                bits = bits << 1 | ((0x69u32 >> (7 - (i % 8))) & 1); // the rest is DSD idle pattern
            }
            out[c] = pack(p.packing, bits);
        }
        self.filled = 0;
        1
    }
}

/// A block handed to the workers; dropping it waits until all are done.
struct Block<'a>(&'a Shared);

impl Block<'_> {
    /// Waits for the workers; true when one of them panicked.
    fn finish(self) -> bool {
        let failed = self.wait();
        std::mem::forget(self);
        failed
    }

    fn wait(&self) -> bool {
        let mut job = self.0.job.lock().unwrap_or_else(|e| e.into_inner());
        while job.pending > 0 {
            job = self.0.done.wait(job).unwrap_or_else(|e| e.into_inner());
        }
        job.failed
    }
}

impl Drop for Block<'_> {
    fn drop(&mut self) {
        self.wait();
    }
}

fn worker(shared: &Shared, c: usize) {
    let mut seen = 0u64;
    loop {
        let (input, len, frames, filled) = {
            let mut job = shared.job.lock().unwrap_or_else(|e| e.into_inner());
            while !job.quit && job.generation == seen {
                job = shared.wake.wait(job).unwrap_or_else(|e| e.into_inner());
            }
            if job.quit {
                return;
            }
            seen = job.generation;
            (job.input, job.len, job.frames, job.filled)
        };
        // A panic is caught so `pending` still counts down (`process` would
        // wait forever otherwise) and reported to `process`.
        let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // SAFETY: `process` keeps the input alive and loop `c` untouched
            // until this worker reports back; params are fixed while it runs.
            let input = unsafe { std::slice::from_raw_parts(input, len) };
            let p = unsafe { *shared.params.get() };
            inject(shared, c);
            modulate(&p, unsafe { &mut *shared.loops[c].get() }, c, input, frames, filled);
        }));
        let mut job = shared.job.lock().unwrap_or_else(|e| e.into_inner());
        if ran.is_err() {
            job.failed = true;
        }
        job.pending -= 1;
        if job.pending == 0 {
            shared.done.notify_one();
        }
    }
}

impl Drop for SigmaDelta {
    fn drop(&mut self) {
        self.stop_workers();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[derive(Default)]
    pub(super) struct Fault {
        panic_on: AtomicUsize, // channel + 1; 0: none
        slow_on: AtomicUsize,  // channel + 1; 0: none
    }

    impl Fault {
        pub(super) fn hit(&self, c: usize) {
            if self.slow_on.load(Ordering::Relaxed) == c + 1 {
                std::thread::sleep(Duration::from_millis(300));
            }
            if self.panic_on.load(Ordering::Relaxed) == c + 1 {
                panic!("injected fault in channel {c}");
            }
        }
    }

    fn pending(sd: &SigmaDelta) -> usize {
        sd.shared.job.lock().unwrap_or_else(|e| e.into_inner()).pending
    }

    #[test]
    fn a_panic_on_the_callers_channel_waits_for_the_workers() {
        let mut sd = SigmaDelta::new();
        sd.setup(64, 2, DsdPacking::NativeU32Be);
        sd.shared.fault.panic_on.store(1, Ordering::Relaxed);
        sd.shared.fault.slow_on.store(2, Ordering::Relaxed);
        let input = vec![0.1; 2 * 4096];
        let mut out = vec![0; 2 * 200];
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sd.process(&input, &mut out)));
        assert!(r.is_err());
        // The input is about to go: no worker may still be reading it.
        assert_eq!(pending(&sd), 0);
        drop(input);
        // And the modulator still works.
        sd.shared.fault.panic_on.store(0, Ordering::Relaxed);
        sd.shared.fault.slow_on.store(0, Ordering::Relaxed);
        assert_eq!(sd.process(&vec![0.1; 2 * 64], &mut out), 2);
    }

    #[test]
    fn a_panic_in_a_worker_is_reported_not_a_deadlock() {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut sd = SigmaDelta::new();
            sd.setup(64, 3, DsdPacking::NativeU32Be);
            sd.shared.fault.panic_on.store(3, Ordering::Relaxed);
            let input = vec![0.1; 3 * 4096];
            let mut out = vec![0; 3 * 200];
            let first = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sd.process(&input, &mut out)));
            sd.shared.fault.panic_on.store(0, Ordering::Relaxed);
            let second = sd.process(&input, &mut out);
            let _ = tx.send((first.is_err(), second));
        });
        let (failed, second) = rx.recv_timeout(Duration::from_secs(20)).expect("process() never returned");
        assert!(failed, "the worker's panic was not reported");
        assert_eq!(second, 4096 / 32);
    }
}
