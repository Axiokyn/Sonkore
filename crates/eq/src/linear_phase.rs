//! The equaliser's curve as a linear-phase FIR:
//! the same magnitude response as the biquads, but every frequency delayed
//! alike, so the EQ leaves the waveform's timing (transients, the stereo
//! image) untouched.
//!
//! Design: the curve sampled on a grid of fs/N, made zero-phase, centred and
//! shaped by a Tukey window (flat over 3/4 of its length, so the slow decay
//! of a narrow bass band is kept). N is about 0.5 s of taps, more when a
//! narrow low band rings longer (a Q 10 notch at 40 Hz needs ~2 s to be
//! realised to its full depth), up to 4 s; the filter delays the music by
//! half of it. It is sized when it engages (a curve that later needs more is
//! truncated until the next track or seek), and runs on the partitioned
//! overlap-save convolver.
//!
//! The delay is hidden rather than heard:
//! * engaging (the curve leaves flat, the mode is chosen, a seek): the
//!   filter is primed with the input that went just before (kept here all
//!   along) and its first outputs are dropped, so the music carries on from
//!   the same sample, filtered, with no gap and no repeat;
//! * a new curve while it runs: the filter is swapped with a one-block
//!   crossfade;
//! * disengaging (back to flat): the filter is drained with the input that
//!   follows, which is then handed back to go on unfiltered.
//!
//! Worker thread only (except [`LinearPhaseEq::busy_flag`]). Engaging and
//! curve changes design a filter (FFTs; engaging also allocates); the FFT
//! plans and the design's buffers are kept from one design to the next, and
//! a curve that keeps changing (a slider being dragged) is redesigned at most
//! once per [`LinearPhaseEq::redesign_interval`] frames of audio, so the
//! designs cannot pile up in the worker's blocks. Steady-state blocks reuse
//! their buffers.

use std::f64::consts::PI;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use realfft::RealFftPlanner;
use realfft::num_complex::Complex as FftComplex;

use crate::biquad::{self, Complex};
use crate::convolver::{BLOCK, Convolver};
use crate::super_eq::{Band, SuperEq};

/// Shortest filter, seconds.
pub const MIN_SECONDS: f64 = 0.5;
/// Longest filter, seconds.
pub const MAX_SECONDS: f64 = 4.0;
/// Tukey window: tapered fraction of each half.
pub const TAPER: f64 = 0.25;
/// Taps come in whole convolver partitions, an even count.
const UNIT: usize = 2 * BLOCK;
/// ln(10^4), correctly rounded (GCC folds `std::log(1e4)`).
const LN_1E4: f64 = 9.210340371976184;
/// Full scale of left-justified 32-bit PCM.
const S32_INV_FULL_SCALE: f64 = 1.0 / 2147483648.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Off,
    Engaged,
    Draining,
}

/// Taps for a sample rate: whole partitions, about [`MIN_SECONDS`].
pub fn taps_for_rate(sample_rate: f64) -> usize {
    let units = (sample_rate * MIN_SECONDS / UNIT as f64).ceil() as usize;
    units.max(1) * UNIT
}

/// ...or as long as the narrowest band rings (80 dB down within the flat
/// part of the window), up to [`MAX_SECONDS`]. A resonance at f0 with
/// quality Q decays as exp(-π f0 t / Q): 80 dB down after ln(10^4)·Q/(π f0).
pub fn taps_for(sample_rate: f64, bands: &[Band]) -> usize {
    let mut seconds = MIN_SECONDS;
    for b in bands {
        if b.has_effect() && b.freq_hz > 0.0 {
            seconds = biquad::max(seconds, 2.0 * LN_1E4 * b.q / (PI * b.freq_hz) / (1.0 - TAPER));
        }
    }
    let seconds = biquad::min(seconds, MAX_SECONDS);
    let units = (sample_rate * seconds / UNIT as f64).ceil() as usize;
    units.max(1) * UNIT
}

/// The FIR for `bands` and `preamp_db` at `sample_rate`, centred at
/// taps / 2: the cascade's magnitude on the FFT grid, inverse transformed,
/// rotated to the middle and windowed (flat over most of the length, a
/// raised-cosine taper only at the ends).
pub fn design(bands: &[Band], preamp_db: f64, sample_rate: f64, taps: usize) -> Vec<f64> {
    Designer::default().design(bands, preamp_db, sample_rate, taps).to_vec()
}

/// [`design`] with its FFT planner (which caches the plans by length) and
/// its buffers kept between designs: the same taps, without planning an
/// inverse FFT and allocating some 30 MB (4 s at 384 kHz) at every change.
#[derive(Default)]
struct Designer {
    planner: RealFftPlanner<f64>,
    sections: Vec<biquad::Coeffs>,
    spectrum: Vec<FftComplex<f64>>,
    scratch: Vec<FftComplex<f64>>,
    impulse: Vec<f64>,
    taps: Vec<f64>,
}

impl Designer {
    fn design(&mut self, bands: &[Band], preamp_db: f64, sample_rate: f64, taps: usize) -> &[f64] {
        let n = taps;
        let half = taps / 2;
        self.sections.clear();
        self.sections.extend(
            bands
                .iter()
                .filter(|b| b.has_effect())
                .map(|b| biquad::design(b.kind, sample_rate, b.freq_hz, b.q, b.gain_db)),
        );
        let preamp = biquad::pow10(preamp_db / 20.0);

        // Zero phase: real bins, whose inverse is the impulse response centred
        // on sample 0, symmetric.
        let sections = &self.sections;
        self.spectrum.clear();
        self.spectrum.extend((0..=half).map(|k| {
            let w = 2.0 * PI * k as f64 / n as f64;
            let z1 = Complex::polar1(-w);
            let z2 = z1.mul(z1);
            let mut m = preamp;
            for c in sections {
                m *= c.response_at(z1, z2).abs();
            }
            FftComplex::new(m, 0.0)
        }));
        self.impulse.clear();
        self.impulse.resize(n, 0.0);
        let inverse = self.planner.plan_fft_inverse(n);
        self.scratch.resize(inverse.get_scratch_len(), FftComplex::default());
        inverse.process_with_scratch(&mut self.spectrum, &mut self.impulse, &mut self.scratch).expect("fft sizes");
        let scale = 1.0 / n as f64;

        let h = &self.impulse;
        self.taps.clear();
        self.taps.extend((0..n).map(|i| {
            let offset = i as isize - half as isize;
            let dist = offset.unsigned_abs() as f64 / half as f64;
            let mut w = 1.0;
            if dist > 1.0 - TAPER {
                w = 0.5 * (1.0 + (PI * (dist - (1.0 - TAPER)) / TAPER).cos());
            }
            let src = (offset + n as isize) as usize % n;
            h[src] * scale * w
        }));
        &self.taps
    }
}

pub struct LinearPhaseEq {
    rate: u32,
    channels: usize,
    taps: usize,
    latency: usize,
    mode: Mode,
    conv: Option<Box<Convolver>>,
    /// The SuperEq generation the filter is for.
    generation: u64,
    /// Frames through the filter since it was last designed.
    since_design: usize,
    designer: Designer,
    /// Outputs still to drop (engaging).
    discard: usize,
    /// Input still to feed (draining).
    drain_left: usize,
    /// Ring of the last input frames (history_size).
    history: Vec<f64>,
    history_size: usize,
    history_pos: usize,
    history_fill: usize,
    /// Draining: the input fed, to hand back.
    held: Vec<f64>,
    work: Vec<f64>,
    out: Vec<f64>,
    pass: Vec<f64>,
    busy: Arc<AtomicBool>,
}

impl Default for LinearPhaseEq {
    fn default() -> Self {
        Self::new()
    }
}

impl LinearPhaseEq {
    pub fn new() -> Self {
        LinearPhaseEq {
            rate: 0,
            channels: 0,
            taps: 0,
            latency: 0,
            mode: Mode::Off,
            conv: None,
            generation: u64::MAX,
            since_design: 0,
            designer: Designer::default(),
            discard: 0,
            drain_left: 0,
            history: Vec::new(),
            history_size: 0,
            history_pos: 0,
            history_fill: 0,
            held: Vec::new(),
            work: Vec::new(),
            out: Vec::new(),
            pass: Vec::new(),
            busy: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Engaged or draining, for other threads (the bit-perfect bypass must
    /// wait while it still holds music).
    pub fn busy_flag(&self) -> Arc<AtomicBool> {
        self.busy.clone()
    }

    pub fn prepare(&mut self, sample_rate: u32, channels: usize) {
        if sample_rate != self.rate || channels != self.channels {
            self.release();
        }
        self.rate = sample_rate;
        self.channels = channels;
        self.taps = taps_for_rate(f64::from(sample_rate));
        self.latency = self.taps / 2 + BLOCK;
        if self.history_size < self.taps / 2 {
            self.history = Vec::new();
            self.history_size = self.taps / 2;
        }
        self.reset();
    }

    /// Frees everything (the stream ended).
    pub fn release(&mut self) {
        self.reset();
        self.designer = Designer::default();
        self.history = Vec::new();
        self.history_size = 0;
        self.held = Vec::new();
        self.work = Vec::new();
        self.out = Vec::new();
        self.pass = Vec::new();
    }

    /// Seek or a new track: no history, not engaged.
    pub fn reset(&mut self) {
        self.mode = Mode::Off;
        self.busy.store(false, Ordering::Relaxed);
        self.conv = None;
        self.generation = u64::MAX;
        self.discard = 0;
        self.drain_left = 0;
        self.history_pos = 0;
        self.history_fill = 0;
        self.held.clear();
    }

    /// A longer filter wants more of the past: the ring grows, what it holds
    /// kept (oldest first, at the start of the new ring).
    fn grow_history(&mut self, frames: usize) {
        let ch = self.channels;
        let mut grown = vec![0.0; frames * ch];
        if !self.history.is_empty() {
            for i in 0..self.history_fill {
                let from = (self.history_pos + self.history_size - self.history_fill + i) % self.history_size;
                grown[i * ch..(i + 1) * ch].copy_from_slice(&self.history[from * ch..(from + 1) * ch]);
            }
        }
        self.history = grown;
        self.history_size = frames;
        self.history_pos = self.history_fill % frames;
    }

    /// The chain's input at the filter's place, kept while the mode is on so
    /// the filter can engage seamlessly (blocks that bypass it included).
    pub fn remember(&mut self, samples: &[f64]) {
        let ch = self.channels;
        let size = self.history_size;
        if ch == 0 || size == 0 {
            return;
        }
        if self.history.is_empty() {
            self.history = vec![0.0; size * ch];
        }
        let mut frames = samples.len() / ch;
        let mut samples = samples;
        // Only the last `size` frames matter.
        if frames > size {
            samples = &samples[(frames - size) * ch..];
            frames = size;
        }
        while frames > 0 {
            let n = frames.min(size - self.history_pos);
            self.history[self.history_pos * ch..(self.history_pos + n) * ch].copy_from_slice(&samples[..n * ch]);
            self.history_pos = (self.history_pos + n) % size;
            self.history_fill = size.min(self.history_fill + n);
            samples = &samples[n * ch..];
            frames -= n;
        }
    }

    /// remember() for left-justified s32 input (the bypassed path).
    pub fn remember_s32(&mut self, samples: &[i32]) {
        let mut work = std::mem::take(&mut self.work);
        work.clear();
        work.extend(samples.iter().map(|&s| f64::from(s) * S32_INV_FULL_SCALE));
        self.remember(&work);
        self.work = work;
    }

    /// Once per block, before process(): `want` whether the curve should be
    /// applied now; `eq` its bands. Engages, swaps or starts draining.
    pub fn update(&mut self, want: bool, eq: &SuperEq) {
        match self.mode {
            Mode::Off => {
                if want {
                    self.engage(eq);
                }
            }
            Mode::Engaged => {
                if !want {
                    self.mode = Mode::Draining;
                    self.drain_left = self.latency;
                    self.held.clear();
                } else {
                    let generation = eq.generation();
                    if generation != self.generation && self.since_design >= self.redesign_interval() {
                        let curve = eq.curve();
                        let fir = self.designer.design(&curve.bands, curve.preamp_db, f64::from(self.rate), self.taps);
                        if let Some(conv) = &mut self.conv {
                            conv.replace_taps(fir);
                        }
                        self.generation = generation;
                        self.since_design = 0;
                    }
                }
            }
            Mode::Draining => {} // it engages again (from its history) once drained
        }
        self.busy.store(self.mode != Mode::Off, Ordering::Relaxed);
    }

    /// Frames of audio between two designs while the curve keeps changing:
    /// 50 ms, or an eighth of the filter when that is longer (a design costs
    /// in proportion to the taps). A single change is designed at once.
    pub fn redesign_interval(&self) -> usize {
        (self.rate as usize / 20).max(self.taps / 8)
    }

    /// The filter starts from the input that went just before, so its first
    /// output worth keeping is the next input sample, filtered with its full
    /// context.
    fn engage(&mut self, eq: &SuperEq) {
        let curve = eq.curve();
        self.generation = eq.generation();
        let rate = f64::from(self.rate);
        self.taps = taps_for(rate, &curve.bands);
        self.latency = self.taps / 2 + BLOCK;
        if self.history_size < self.taps / 2 {
            self.grow_history(self.taps / 2);
        }
        let fir = self.designer.design(&curve.bands, curve.preamp_db, rate, self.taps);
        let mut conv = Box::new(Convolver::new(fir, self.channels));
        self.since_design = 0;
        let ch = self.channels;
        let size = self.taps / 2;
        let ring = self.history_size;
        // The last taps/2 input frames, oldest first (silence before a seek).
        let mut prime = vec![0.0; size * ch];
        let have = self.history_fill.min(size);
        if !self.history.is_empty() {
            for i in 0..have {
                let from = (self.history_pos + ring - have + i) % ring;
                prime[(size - have + i) * ch..(size - have + i + 1) * ch]
                    .copy_from_slice(&self.history[from * ch..(from + 1) * ch]);
            }
        }
        conv.process(&mut prime); // outputs of what was already heard: dropped
        self.conv = Some(conv);
        self.discard = self.latency;
        self.mode = Mode::Engaged;
    }

    /// Through the filter, dropping what must be.
    fn feed(&mut self, samples: &[f64]) {
        let ch = self.channels;
        let frames = samples.len() / ch;
        self.work.clear();
        self.work.extend_from_slice(samples);
        if let Some(conv) = &mut self.conv {
            conv.process(&mut self.work);
        }
        let drop = self.discard.min(frames);
        self.discard -= drop;
        self.since_design = self.since_design.saturating_add(frames);
        self.out.extend_from_slice(&self.work[drop * ch..]);
    }

    /// Interleaved input; afterwards [`filtered`](Self::filtered) holds the
    /// frames that come out of the filter (fewer while it engages) and
    /// [`passed`](Self::passed) those handed back unfiltered (once a drain
    /// ends: the input it held, then the rest).
    pub fn process(&mut self, samples: &[f64]) {
        let ch = self.channels;
        let frames = samples.len() / ch.max(1);
        let samples = &samples[..frames * ch];
        self.out.clear();
        self.pass.clear();
        match self.mode {
            Mode::Off => self.pass.extend_from_slice(samples),
            Mode::Engaged => {
                // Room for a drain, made while engaged: switching the curve
                // off then allocates nothing (the worker's blocks stay free
                // of allocations). The drain holds up to `latency` frames
                // and hands them back with the rest of its last block.
                let held = self.latency * ch;
                if self.held.capacity() < held {
                    self.held.reserve(held);
                }
                let handed = (self.latency + 2 * frames) * ch;
                if self.pass.capacity() < handed {
                    self.pass.reserve(handed);
                }
                self.feed(samples)
            }
            Mode::Draining => {
                let n = frames.min(self.drain_left);
                self.held.extend_from_slice(&samples[..n * ch]);
                self.feed(&samples[..n * ch]);
                self.drain_left -= n;
                if self.drain_left == 0 {
                    // Everything it held is out: the input fed meanwhile goes
                    // on unfiltered, then the rest of this block. The
                    // convolver stays (dropping it would free on this
                    // thread) until it is engaged again or reset.
                    self.mode = Mode::Off;
                    self.busy.store(false, Ordering::Relaxed);
                    self.pass.extend_from_slice(&self.held);
                    self.held.clear();
                    self.pass.extend_from_slice(&samples[n * ch..]);
                }
            }
        }
    }

    pub fn filtered(&self) -> &[f64] {
        &self.out
    }

    pub fn filtered_mut(&mut self) -> &mut [f64] {
        &mut self.out
    }

    pub fn passed(&self) -> &[f64] {
        &self.pass
    }

    pub fn passed_mut(&mut self) -> &mut [f64] {
        &mut self.pass
    }

    /// The passed frames as a Vec of its own, so the caller can work on them
    /// while it uses the filter; [`give_back`](Self::give_back) returns the
    /// buffer (its capacity kept: no allocation per block).
    pub fn take_passed(&mut self) -> Vec<f64> {
        std::mem::take(&mut self.pass)
    }

    pub fn give_back(&mut self, mut passed: Vec<f64>) {
        passed.clear();
        self.pass = passed;
    }

    /// Engaged or draining: the frames have to come through process().
    pub fn active(&self) -> bool {
        self.mode != Mode::Off
    }

    /// Frames of music held inside (what was fed and not handed out yet).
    pub fn latency(&self) -> usize {
        if self.active() { self.latency } else { 0 }
    }

    /// Frames still owed after the input ends.
    pub fn tail(&self) -> usize {
        if !self.active() {
            return 0;
        }
        self.conv.as_ref().map_or(0, |c| c.tail())
    }

    pub fn taps(&self) -> usize {
        self.taps
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::biquad::FilterType;

    #[test]
    fn ln_1e4_is_gccs() {
        assert_eq!(LN_1E4.to_bits(), 0x40226BB1BBB55516); // std::log(1e4), %a
    }

    #[test]
    fn tap_lengths() {
        assert_eq!(taps_for_rate(44100.0), 22528);
        assert_eq!(taps_for_rate(48000.0), 24576);
        let narrow = Band { kind: FilterType::Notch, freq_hz: 40.0, q: 10.0, ..Band::default() };
        let t = taps_for(48000.0, &[narrow]);
        assert_eq!(t, (48000.0 * 2.0 * LN_1E4 * 10.0 / (PI * 40.0) / 0.75 / UNIT as f64).ceil() as usize * UNIT);
        assert!(t > 90000 && t.is_multiple_of(UNIT), "{t}");
        let narrower = Band { freq_hz: 10.0, q: 30.0, gain_db: 3.0, ..narrow };
        assert_eq!(taps_for(48000.0, &[narrower]), (48000.0f64 * 4.0 / UNIT as f64).ceil() as usize * UNIT);
    }

    #[test]
    fn dragging_a_slider_redesigns_at_a_bounded_pace() {
        let eq = SuperEq::new();
        eq.set_linear_phase(true);
        eq.set_preamp_db(-1.0);
        let mut lp = LinearPhaseEq::new();
        lp.prepare(48000, 2);
        let block = vec![0.1; 2 * 256];
        lp.update(true, &eq);
        lp.process(&block);
        // A change on every 256-frame block (a slider dragged) for 2 s.
        let mut designs = 0;
        let mut last = lp.generation;
        for k in 0..375 {
            eq.set_preamp_db(-1.0 - k as f64 * 0.01);
            lp.update(true, &eq);
            lp.process(&block);
            if lp.generation != last {
                designs += 1;
                last = lp.generation;
            }
        }
        // At most one per 64 ms of audio (taps / 8 frames at 48 kHz).
        let most = (375 * 256) / (lp.taps() / 8) + 1;
        assert!(designs <= most, "{designs} designs in 2 s (at most {most})");
        assert!(designs >= most - 2, "{designs} designs in 2 s: too few to follow the slider");
        // Let go: the last curve is in within that time.
        for _ in 0..lp.taps() / 8 / 256 + 1 {
            lp.update(true, &eq);
            lp.process(&block);
        }
        assert_eq!(lp.generation, eq.generation());
    }

    #[test]
    fn engage_and_drain_keep_every_sample() {
        let eq = SuperEq::new();
        eq.set_linear_phase(true);
        let mut lp = LinearPhaseEq::new();
        lp.prepare(48000, 2);
        let input: Vec<f64> = (0..400_000).map(|i| ((i as f64) * 0.0123).sin() * 0.5).collect();
        let mut out = Vec::new();
        for (k, block) in input.chunks(1024).enumerate() {
            let want = (20..60).contains(&k);
            if k == 20 {
                eq.set_preamp_db(-6.0);
            }
            lp.update(want, &eq);
            lp.remember(block);
            if lp.active() {
                lp.process(block);
                out.extend_from_slice(lp.filtered());
                out.extend_from_slice(lp.passed());
            } else {
                out.extend_from_slice(block);
            }
        }
        assert!(!lp.active());
        // Nothing lost or repeated: the stream is the input's length less
        // what the filter still held when the drain ended (none).
        assert_eq!(out.len(), input.len());
        // Within the engaged part, the -6 dB preamp is applied in time.
        let g = 10f64.powf(-6.0 / 20.0);
        let at = 30 * 1024;
        for i in (at..at + 2000).step_by(97) {
            assert!((out[i] - input[i] * g).abs() < 1e-6, "{i}");
        }
        // After the drain the input comes back unfiltered.
        let late = 300_000;
        assert_eq!(out[late], input[late]);
    }
}
