//! The multiband graphic / parametric equaliser:
//! a cascade of biquads processed in f64 on interleaved buffers.
//!
//! It is split in two halves sharing one curve:
//! * [`SuperEq`], a cheap cloneable handle any thread may use: the bands,
//!   the preamp, on/off and the linear-phase flag, the neutral test and the
//!   curve's peak gain. Setters only touch a "pending" copy behind a mutex.
//! * [`EqProcessor`], owned by the DSP worker: it picks the pending curve up
//!   at block boundaries (with `try_lock`, so a setter holding the lock only
//!   delays the change by one block) and filters. `process()` never
//!   allocates, locks or waits.

use std::f64::consts::PI;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, TryLockError};

use crate::biquad::{self, Coeffs, Complex, FilterType, State};

pub const MAX_BANDS: usize = 32;
pub const MAX_CHANNELS: usize = 8;

/// Centres of the ten graphic bands (an ISO-like 31 Hz .. 16 kHz layout).
pub const GRAPHIC_CENTERS: [f64; 10] = [31.25, 62.5, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0];
/// About one octave of bandwidth.
pub const GRAPHIC_Q: f64 = 1.41421356237;
/// The rate the graphic solve is done at (the matched designs barely
/// depend on it).
const GRAPHIC_RATE: f64 = 48000.0;

/// One band as the engine runs it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Band {
    pub kind: FilterType,
    pub freq_hz: f64,
    pub gain_db: f64,
    pub q: f64,
    pub enabled: bool,
}

impl Default for Band {
    fn default() -> Self {
        Band { kind: FilterType::Peaking, freq_hz: 1000.0, gain_db: 0.0, q: GRAPHIC_Q, enabled: true }
    }
}

/// The largest gain or preamp the engine takes, in dB: beyond, 10^(dB/20)
/// heads for infinity. The controller keeps far within (±24 dB, the graphic
/// solve ±36).
pub const MAX_ENGINE_DB: f64 = 60.0;

/// A preamp as the engine takes it: finite, within ±[`MAX_ENGINE_DB`].
fn sanitised_db(db: f64) -> f64 {
    if db.is_finite() { biquad::clamp(db, -MAX_ENGINE_DB, MAX_ENGINE_DB) } else { 0.0 }
}

impl Band {
    /// The last line of defence before the coefficients: a frequency or Q that
    /// is not a finite positive number takes the default's, a gain that is
    /// not finite is 0 dB, and the gain stays within ±[`MAX_ENGINE_DB`] (the
    /// designs clamp the frequency and Q themselves).
    pub fn sanitised(self) -> Band {
        let d = Band::default();
        let positive = |v: f64, default: f64| if v.is_finite() && v > 0.0 { v } else { default };
        Band {
            freq_hz: positive(self.freq_hz, d.freq_hz),
            gain_db: sanitised_db(self.gain_db),
            q: positive(self.q, d.q),
            ..self
        }
    }

    /// Whether the band changes the signal: peaking and shelving bands only
    /// with a gain, the other types always (when enabled).
    pub fn has_effect(&self) -> bool {
        self.enabled && (!self.kind.has_gain() || self.gain_db != 0.0)
    }
}

/// Bands and preamp: the whole curve.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Curve {
    pub bands: Vec<Band>,
    pub preamp_db: f64,
}

/// The ten graphic bands, all at 0 dB.
pub fn graphic_10_band() -> Vec<Band> {
    GRAPHIC_CENTERS.iter().map(|&f| Band { freq_hz: f, q: GRAPHIC_Q, ..Band::default() }).collect()
}

/// Response (dB) of the ten graphic bands at `gains`, at their centres.
pub fn graphic_response(gains: &[f64; 10]) -> [f64; 10] {
    let mut sections = [Coeffs::default(); 10];
    let mut n = 0;
    for (j, &g) in gains.iter().enumerate() {
        if g != 0.0 {
            sections[n] = biquad::design(FilterType::Peaking, GRAPHIC_RATE, GRAPHIC_CENTERS[j], GRAPHIC_Q, g);
            n += 1;
        }
    }
    let mut r = [0.0; 10];
    for (i, out) in r.iter_mut().enumerate() {
        let z1 = Complex::polar1(-2.0 * PI * GRAPHIC_CENTERS[i] / GRAPHIC_RATE);
        let z2 = z1.mul(z1);
        for c in &sections[..n] {
            *out += 20.0 * c.response_at(z1, z2).abs().log10();
        }
    }
    r
}

/// The overlap matrix, inverted once: column j of the matrix is band j's
/// response per dB of its gain at the ten centres.
#[allow(clippy::needless_range_loop)] // the original indices, kept
static GRAPHIC_INVERSE: LazyLock<[[f64; 10]; 10]> = LazyLock::new(|| {
    let mut m = [[0.0f64; 20]; 10];
    for j in 0..10 {
        let mut g = [0.0; 10];
        g[j] = 1.0;
        let r = graphic_response(&g);
        for i in 0..10 {
            m[i][j] = r[i];
        }
        m[j][10 + j] = 1.0;
    }
    // Gauss-Jordan; diagonally dominant, no pivoting needed.
    for c in 0..10 {
        let p = m[c][c];
        for v in m[c].iter_mut() {
            *v /= p;
        }
        for r in 0..10 {
            if r != c {
                let f = m[r][c];
                for k in 0..20 {
                    m[r][k] -= f * m[c][k];
                }
            }
        }
    }
    let mut inv = [[0.0; 10]; 10];
    for i in 0..10 {
        inv[i].copy_from_slice(&m[i][10..]);
    }
    inv
});

/// The gains to give the ten graphic bands so the cascade passes through
/// `sliders` (dB) at the band centres: neighbouring one-octave bands overlap
/// (one at +6 dB adds +1.1 dB at the next centre, ten at +6 dB give +8.8 dB),
/// so each is solved against the others. Five Newton steps on the inverse
/// overlap matrix, each gain clamped to ±36 dB.
pub fn graphic_gains(sliders: &[f64; 10]) -> [f64; 10] {
    if sliders.iter().all(|&g| g == 0.0) {
        return *sliders;
    }
    let inverse = &*GRAPHIC_INVERSE;
    let mut gains = *sliders;
    for _ in 0..5 {
        let got = graphic_response(&gains);
        let mut next = gains;
        for i in 0..10 {
            let mut d = 0.0;
            for j in 0..10 {
                d += inverse[i][j] * (sliders[j] - got[j]);
            }
            next[i] = biquad::clamp(gains[i] + d, -36.0, 36.0);
        }
        gains = next;
    }
    gains
}

struct Shared {
    pending: Mutex<Curve>,
    dirty: AtomicBool,
    enabled: AtomicBool,
    linear_phase: AtomicBool,
    generation: AtomicU64,
    /// The worker's output still carries filtered signal.
    wet: AtomicBool,
}

/// The equaliser's curve and switches, shared by the controller and the
/// DSP worker. Clones are handles to the same equaliser.
#[derive(Clone)]
pub struct SuperEq {
    shared: Arc<Shared>,
}

impl Default for SuperEq {
    fn default() -> Self {
        Self::new()
    }
}

fn lock(m: &Mutex<Curve>) -> MutexGuard<'_, Curve> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl SuperEq {
    /// The ten graphic bands at 0 dB, enabled, no preamp.
    pub fn new() -> Self {
        SuperEq {
            shared: Arc::new(Shared {
                pending: Mutex::new(Curve { bands: graphic_10_band(), preamp_db: 0.0 }),
                dirty: AtomicBool::new(true),
                enabled: AtomicBool::new(true),
                linear_phase: AtomicBool::new(false),
                generation: AtomicU64::new(0),
                wet: AtomicBool::new(false),
            }),
        }
    }

    /// A processor for the DSP worker, filtering with this curve.
    pub fn processor(&self) -> EqProcessor {
        EqProcessor::new(self)
    }

    fn changed(&self) {
        self.shared.dirty.store(true, Ordering::Release);
        self.shared.generation.fetch_add(1, Ordering::Release);
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.shared.enabled.store(enabled, Ordering::Relaxed);
        self.shared.generation.fetch_add(1, Ordering::Release);
    }

    pub fn is_enabled(&self) -> bool {
        self.shared.enabled.load(Ordering::Relaxed)
    }

    /// Linear phase: the engine applies the curve as an FIR
    /// ([`crate::LinearPhaseEq`]) instead of these biquads.
    pub fn set_linear_phase(&self, on: bool) {
        self.shared.linear_phase.store(on, Ordering::Relaxed);
    }

    pub fn linear_phase(&self) -> bool {
        self.shared.linear_phase.load(Ordering::Relaxed)
    }

    /// Bumped by every change of the curve (bands, preamp, on/off).
    pub fn generation(&self) -> u64 {
        self.shared.generation.load(Ordering::Acquire)
    }

    pub fn curve(&self) -> Curve {
        lock(&self.shared.pending).clone()
    }

    /// At most [`MAX_BANDS`] are kept, each [`Band::sanitised`].
    pub fn set_bands(&self, mut bands: Vec<Band>) {
        bands.truncate(MAX_BANDS);
        for b in &mut bands {
            *b = b.sanitised();
        }
        lock(&self.shared.pending).bands = bands;
        self.changed();
    }

    /// Bands and preamp together, as one change: the worker never runs a
    /// block with the new bands and the old preamp (or the other way round).
    pub fn set_curve(&self, mut curve: Curve) {
        curve.bands.truncate(MAX_BANDS);
        for b in &mut curve.bands {
            *b = b.sanitised();
        }
        curve.preamp_db = sanitised_db(curve.preamp_db);
        *lock(&self.shared.pending) = curve;
        self.changed();
    }

    /// False when there is no band `index`.
    pub fn set_band(&self, index: usize, band: Band) -> bool {
        {
            let mut p = lock(&self.shared.pending);
            let Some(slot) = p.bands.get_mut(index) else {
                return false;
            };
            *slot = band.sanitised();
        }
        self.changed();
        true
    }

    pub fn bands(&self) -> Vec<Band> {
        lock(&self.shared.pending).bands.clone()
    }

    /// Within ±[`MAX_ENGINE_DB`]; 0 dB when not a finite number.
    pub fn set_preamp_db(&self, db: f64) {
        lock(&self.shared.pending).preamp_db = sanitised_db(db);
        self.changed();
    }

    pub fn preamp_db(&self) -> f64 {
        lock(&self.shared.pending).preamp_db
    }

    /// True when processing would be an identity (disabled, or no preamp and
    /// no band with an effect) and nothing filtered is still fading out: the
    /// bit-perfect bypass may then skip the EQ entirely.
    pub fn is_neutral(&self) -> bool {
        if self.shared.wet.load(Ordering::Relaxed) {
            return false;
        }
        if !self.is_enabled() {
            return true;
        }
        let p = lock(&self.shared.pending);
        p.preamp_db == 0.0 && !p.bands.iter().any(Band::has_effect)
    }

    /// Highest gain of the whole curve (preamp included) across 20 Hz -
    /// 20 kHz (400 log points, designed at 96 kHz), in dB; 0 when neutral.
    /// What the EQ can add to a full-scale signal.
    pub fn peak_gain_db(&self) -> f64 {
        if self.is_neutral() {
            return 0.0;
        }
        const RATE: f64 = 96000.0;
        let (preamp, sections) = {
            let p = lock(&self.shared.pending);
            let sections: Vec<Coeffs> = p
                .bands
                .iter()
                .filter(|b| b.has_effect())
                .map(|b| biquad::design(b.kind, RATE, b.freq_hz, b.q, b.gain_db))
                .collect();
            (p.preamp_db, sections)
        };
        let mut peak = -1e9;
        let lo = LOG10_20;
        let hi = LOG10_20000;
        for i in 0..400 {
            let w = 2.0 * PI * biquad::pow10(lo + (hi - lo) * f64::from(i) / 399.0) / RATE;
            let z1 = Complex::polar1(-w);
            let z2 = z1.mul(z1);
            let mut db = preamp;
            for c in &sections {
                db += 20.0 * c.response_at(z1, z2).abs().log10();
            }
            peak = biquad::max(peak, db);
        }
        peak
    }
}

/// log10(20) and log10(20000), correctly rounded (GCC folds `std::log10`
/// of a constant with MPFR).
pub(crate) const LOG10_20: f64 = 1.3010299956639813;
pub(crate) const LOG10_20000: f64 = 4.301029995663981;

/// The DSP worker's half of the equaliser: coefficients, filter state and
/// the on/off crossfade. Real-time safe: `process()` never allocates or
/// blocks.
pub struct EqProcessor {
    eq: SuperEq,
    sample_rate: f64,
    channels: usize,
    active_bands: usize,
    preamp_linear: f64,
    /// Share of the filtered signal (crossfades on/off).
    mix: f64,
    /// The curve went flat while the output was filtered: the old sections
    /// stay until the fade to dry is over (fading with a flat curve would
    /// crossfade the dry signal with itself, a step).
    flat_pending: bool,
    coeffs: [Coeffs; MAX_BANDS],
    state: [[State; MAX_BANDS]; MAX_CHANNELS],
}

impl EqProcessor {
    pub fn new(eq: &SuperEq) -> Self {
        EqProcessor {
            eq: eq.clone(),
            sample_rate: 44100.0,
            channels: 2,
            active_bands: 0,
            preamp_linear: 1.0,
            mix: 0.0,
            flat_pending: false,
            coeffs: [Coeffs::default(); MAX_BANDS],
            state: [[State::default(); MAX_BANDS]; MAX_CHANNELS],
        }
    }

    /// The handle this processor follows.
    pub fn equalizer(&self) -> &SuperEq {
        &self.eq
    }

    /// A new stream: its rate and channel count (clamped to 1..8). The
    /// coefficients are redesigned on the next block; the state is cleared.
    pub fn prepare(&mut self, sample_rate: f64, channels: usize) {
        self.sample_rate = sample_rate;
        self.channels = channels.clamp(1, MAX_CHANNELS);
        self.eq.shared.dirty.store(true, Ordering::Release);
        self.reset();
    }

    /// A block went around the biquads (linear phase filters it instead): no
    /// crossfade left half-way, no stale filter memory, and the EQ no longer
    /// counts as "still fading out". Only `process` cleared that flag, so after
    /// switching to linear phase the EQ never became neutral again: turned off,
    /// its curve stayed on and bit-perfect never came back.
    pub fn bypass(&mut self) {
        if self.mix != 0.0 {
            self.mix = 0.0;
            self.faded_out();
        }
        self.eq.shared.wet.store(false, Ordering::Relaxed);
    }

    /// The output is dry again: a flat curve waiting for the fade takes
    /// over, and the filters forget what they held (switched on later, they
    /// start from silence, not from the music of before).
    fn faded_out(&mut self) {
        if self.flat_pending {
            self.flat_pending = false;
            self.active_bands = 0;
            self.preamp_linear = 1.0;
        }
        self.reset();
    }

    /// Clears the filters' memory (a seek, or blocks that bypassed the EQ).
    pub fn reset(&mut self) {
        for ch in self.state.iter_mut() {
            for s in ch.iter_mut() {
                s.reset();
            }
        }
    }

    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Takes the pending curve if it changed and the lock is free right
    /// now. Sections that shift position keep their old state; the filters
    /// settle within a few samples, which is inaudible compared to a full
    /// reset.
    fn apply_pending(&mut self) {
        let shared = &self.eq.shared;
        if !shared.dirty.load(Ordering::Acquire) {
            return;
        }
        let guard = match shared.pending.try_lock() {
            Ok(g) => g,
            Err(TryLockError::Poisoned(e)) => e.into_inner(),
            Err(TryLockError::WouldBlock) => return, // next block
        };
        let mut bands = [Band::default(); MAX_BANDS];
        let mut n = 0;
        for b in guard.bands.iter().filter(|b| b.has_effect()).take(MAX_BANDS) {
            bands[n] = *b;
            n += 1;
        }
        let preamp_db = guard.preamp_db;
        shared.dirty.store(false, Ordering::Relaxed);
        drop(guard);
        let preamp_linear = biquad::pow10(preamp_db / 20.0);
        if n == 0 && preamp_linear == 1.0 && self.mix > 0.0 {
            // Back to flat: fade out through the sections there are.
            self.flat_pending = true;
            return;
        }
        self.flat_pending = false;
        for (c, b) in self.coeffs.iter_mut().zip(&bands[..n]) {
            *c = biquad::design(b.kind, self.sample_rate, b.freq_hz, b.q, b.gain_db);
        }
        self.active_bands = n;
        self.preamp_linear = preamp_linear;
    }

    /// Filters interleaved frames in place (`interleaved.len()` is a whole
    /// number of frames of the prepared channel count). Switched on or off
    /// (or back to flat), the output crossfades between the dry and the
    /// filtered signal over ~10 ms instead of a step (a +6 dB band toggled
    /// on a loud passage is an instant 2:1 jump).
    pub fn process(&mut self, interleaved: &mut [f64]) {
        self.apply_pending();
        let target =
            if self.eq.is_enabled() && !self.flat_pending && (self.active_bands > 0 || self.preamp_linear != 1.0) {
                1.0
            } else {
                0.0
            };
        if target == 0.0 && self.mix == 0.0 {
            self.eq.shared.wet.store(false, Ordering::Relaxed);
            return;
        }
        let channels = self.channels;
        let frames = interleaved.len() / channels;
        let interleaved = &mut interleaved[..frames * channels];
        let bands = self.active_bands;
        let pre = self.preamp_linear;
        let coeffs = &self.coeffs[..bands];

        if self.mix == target {
            // Two channels at a time: their recursions are independent, so
            // the CPU overlaps them (each channel's arithmetic is unchanged).
            let mut ch = 0;
            while ch < channels {
                if ch + 1 < channels {
                    let (left, right) = self.state.split_at_mut(ch + 1);
                    let (sl, sr) = (&mut left[ch][..bands], &mut right[0][..bands]);
                    for frame in interleaved.chunks_exact_mut(channels) {
                        let mut l = frame[ch] * pre;
                        let mut r = frame[ch + 1] * pre;
                        for ((a, b), c) in sl.iter_mut().zip(sr.iter_mut()).zip(coeffs) {
                            l = a.process(c, l);
                            r = b.process(c, r);
                        }
                        frame[ch] = l;
                        frame[ch + 1] = r;
                    }
                    ch += 2;
                } else {
                    let state = &mut self.state[ch][..bands];
                    for x in interleaved[ch..].iter_mut().step_by(channels) {
                        let mut v = *x * pre;
                        for (s, c) in state.iter_mut().zip(coeffs) {
                            v = s.process(c, v);
                        }
                        *x = v;
                    }
                    ch += 1;
                }
            }
        } else {
            // Frame by frame: each sample is mixed with its own dry value,
            // so no copy of the block is needed.
            let step = 1.0 / biquad::max(1.0, self.sample_rate * 0.01);
            for frame in interleaved.chunks_exact_mut(channels) {
                self.mix = if target > self.mix {
                    biquad::min(target, self.mix + step)
                } else {
                    biquad::max(target, self.mix - step)
                };
                for (x, state) in frame.iter_mut().zip(self.state.iter_mut()) {
                    let dry = *x;
                    let mut v = dry * pre;
                    for (s, c) in state[..bands].iter_mut().zip(coeffs) {
                        v = s.process(c, v);
                    }
                    *x = dry + (v - dry) * self.mix;
                }
            }
            if self.mix == 0.0 {
                self.faded_out();
            }
        }
        self.eq.shared.wet.store(self.mix > 0.0, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutral_until_a_band_has_gain() {
        let eq = SuperEq::new();
        assert!(eq.is_neutral());
        assert_eq!(eq.peak_gain_db(), 0.0);
        let mut b = eq.bands()[3];
        b.gain_db = 3.0;
        assert!(eq.set_band(3, b));
        assert!(!eq.is_neutral());
        assert!((eq.peak_gain_db() - 3.0).abs() < 0.05);
        eq.set_enabled(false);
        assert!(eq.is_neutral());
    }

    #[test]
    fn neutral_passes_samples_untouched() {
        let eq = SuperEq::new();
        let mut p = eq.processor();
        p.prepare(48000.0, 2);
        let input: Vec<f64> = (0..512).map(|i| (i as f64 * 0.01).sin()).collect();
        let mut buf = input.clone();
        p.process(&mut buf);
        assert_eq!(buf, input);
    }

    #[test]
    fn crossfade_reaches_the_filtered_signal() {
        let eq = SuperEq::new();
        eq.set_preamp_db(-6.0);
        let mut p = eq.processor();
        p.prepare(48000.0, 1);
        let mut buf = vec![1.0; 1024];
        p.process(&mut buf);
        let g = 10f64.powf(-6.0 / 20.0);
        assert!(buf[0] < 1.0 && buf[0] > g);
        assert!((buf[1000] - g).abs() < 1e-15);
        assert!(!eq.is_neutral());
        eq.set_preamp_db(0.0);
        let mut buf = vec![1.0; 1024];
        p.process(&mut buf);
        assert_eq!(buf[1000], 1.0);
        let mut buf = vec![1.0; 16];
        p.process(&mut buf);
        assert!(eq.is_neutral());
    }

    #[test]
    fn values_that_are_not_numbers_never_reach_the_filters() {
        let eq = SuperEq::new();
        let mut p = eq.processor();
        p.prepare(48000.0, 2);
        let bad = [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 1e300, -1e300];
        for (i, &v) in bad.iter().enumerate() {
            let b = Band { gain_db: v, freq_hz: bad[(i + 1) % 5], q: bad[(i + 2) % 5], ..Band::default() };
            eq.set_curve(Curve { bands: vec![b, Band { gain_db: 3.0, ..Band::default() }], preamp_db: v });
            let mut x: Vec<f64> = (0..2000).map(|n| (n as f64 * 0.03).sin() * 0.5).collect();
            p.process(&mut x);
            assert!(x.iter().all(|s| s.is_finite()), "set_curve with {v}");
            assert!(eq.peak_gain_db().is_finite());
            assert!(eq.set_band(0, b));
            eq.set_preamp_db(v);
            eq.set_bands(vec![b; 3]);
            p.process(&mut x);
            assert!(x.iter().all(|s| s.is_finite()), "set_band / set_preamp_db / set_bands with {v}");
            for b in eq.bands() {
                assert!(b.freq_hz.is_finite() && b.gain_db.abs() <= MAX_ENGINE_DB && b.q.is_finite() && b.q > 0.0);
            }
        }
    }

    #[test]
    fn back_to_flat_fades_out_the_filtered_signal() {
        let eq = SuperEq::new();
        eq.set_preamp_db(-6.0);
        let mut p = eq.processor();
        p.prepare(48000.0, 1);
        let mut buf = vec![1.0; 2048];
        p.process(&mut buf);
        let g = 10f64.powf(-6.0 / 20.0);
        assert!((buf[2047] - g).abs() < 1e-15);
        // Reset to flat: the output fades from the filtered level to the
        // dry one over ~10 ms, it does not jump.
        eq.set_curve(Curve { bands: graphic_10_band(), preamp_db: 0.0 });
        let mut buf = vec![1.0; 1024];
        p.process(&mut buf);
        assert!((buf[0] - g).abs() < 0.01, "first sample after the reset: {} (filtered {g})", buf[0]);
        for w in buf.windows(2) {
            assert!(w[1] - w[0] < 0.01, "a step of {}", w[1] - w[0]);
        }
        assert_eq!(buf[1000], 1.0);
        let mut buf = vec![1.0; 16];
        p.process(&mut buf);
        assert_eq!(buf, vec![1.0; 16]);
        assert!(eq.is_neutral());
        // And on again from there: a fade in, from the new curve.
        eq.set_preamp_db(-12.0);
        let mut buf = vec![1.0; 1024];
        p.process(&mut buf);
        assert!(buf[0] > 0.99 && (buf[1000] - 10f64.powf(-12.0 / 20.0)).abs() < 1e-15);
    }

    #[test]
    fn switched_on_again_the_filters_start_clean() {
        let eq = SuperEq::new();
        eq.set_curve(Curve {
            bands: vec![Band { freq_hz: 40.0, gain_db: 12.0, q: 8.0, ..Band::default() }],
            preamp_db: 0.0,
        });
        let mut p = eq.processor();
        p.prepare(48000.0, 2);
        let mut loud: Vec<f64> = (0..20_000).map(|n| (n as f64 * 2.0 * PI * 40.0 / 48000.0).sin() * 0.5).collect();
        p.process(&mut loud);
        eq.set_enabled(false);
        let mut quiet = vec![0.0; 4000];
        p.process(&mut quiet); // the fade out, then nothing
        assert!(!quiet[2000..].iter().any(|&v| v != 0.0));
        p.process(&mut quiet);
        // Silence in, switched on: silence out, not the old ringing.
        eq.set_enabled(true);
        let mut silence = vec![0.0; 4000];
        p.process(&mut silence);
        let worst = silence.iter().fold(0.0f64, |m, v| m.max(v.abs()));
        assert_eq!(worst, 0.0, "the filters kept what they held before the fade out");
    }

    #[test]
    fn bypass_completes_a_fade_to_flat() {
        let eq = SuperEq::new();
        eq.set_preamp_db(-6.0);
        let mut p = eq.processor();
        p.prepare(48000.0, 1);
        p.process(&mut [1.0; 2048]);
        eq.set_preamp_db(0.0);
        p.process(&mut [1.0; 64]); // fading
        assert!(!eq.is_neutral());
        p.bypass();
        assert!(eq.is_neutral());
        let mut buf = vec![1.0; 256];
        p.process(&mut buf);
        assert_eq!(buf, vec![1.0; 256]);
        assert!(eq.is_neutral());
    }

    #[test]
    fn constants_are_gccs() {
        // GCC's folded std::log10(20.0) / (20000.0), printed with %a.
        assert_eq!(LOG10_20.to_bits(), 0x3FF4D104D427DE80);
        assert_eq!(LOG10_20000.to_bits(), 0x401134413509F7A0);
    }

    #[test]
    fn graphic_solve_lands_on_the_sliders() {
        let sliders = [6.0; 10];
        let gains = graphic_gains(&sliders);
        let got = graphic_response(&gains);
        for (g, s) in got.iter().zip(&sliders) {
            assert!((g - s).abs() < 0.01, "{got:?}");
        }
        assert!(gains[5] < 6.0);
    }
}
