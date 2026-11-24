//! Minimum- and intermediate-phase sample-rate conversion: a polyphase FIR
//! whose kernel comes from a linear-phase prototype by the cepstral
//! (homomorphic) method.
//!
//! **Prototype.** A Kaiser-windowed sinc of [`TAPS`] taps at the lower rate
//! (β = [`BETA`], cut-off [`CUTOFF`] of its Nyquist): flat (< 10⁻⁶ dB) to
//! 0.96 and ~190 dB down from Nyquist on, the linear filter's specification.
//! It is sampled [`OVERSAMPLE`] times finer than the lower rate, so the
//! design covers its stop band out to 8× that rate.
//!
//! **Minimum phase** ([`design`]). On an FFT of [`PADDING`]× the
//! prototype's length (2¹⁸ points): the log of its magnitude (floored at
//! [`FLOOR`], since the stop band's nulls are zeros), transformed back, is
//! the real cepstrum; folding it onto positive quefrencies (c₀, 2c₁ …
//! 2c_{M/2−1}, c_{M/2}, 0 …) and transforming again gives `ln|H| + jφ_min`,
//! φ_min being the minimum phase of that magnitude.
//!
//! - Minimum phase: `H = |H|·e^{jφ_min}`.
//! - Intermediate phase, soxr's (its phase response 25 of 0–100): the phase
//!   halfway between the minimum phase and the prototype's linear phase,
//!   `φ = ½·φ_min − ½·ω·D` (D the prototype's centre), same magnitude.
//!
//! The nulls make the cepstrum decay slowly, so it aliases: the response
//! gets a faint copy (~10⁻⁵) half the buffer away. The kernel is the span
//! from half the prototype's length before time 0 to 1.5 lengths after it
//! (the copy is 25 lengths away), trimmed where it stays below [`TRIM`] of
//! its peak: 622 taps for minimum phase, 1074 for intermediate (whose
//! pre-ringing starts ~440 samples before its peak). Measured against the
//! prototype (tests below): the same magnitude to < 10⁻⁵ dB up to 0.97 of
//! Nyquist, and 187–188 dB down from Nyquist on (the prototype: 188.6).
//!
//! **Conversion** ([`Polyphase`]). Output `m` sits at input time
//! `t = m·a/b` (a/b = in/out reduced). With the kernel's peak at time 0,
//! `y[m] = Σₖ x[k]·g(t − k)`, where `g` reads the design points with a
//! 16-point Lagrange interpolator (error < 10⁻¹² at 8× oversampling) and is
//! stretched by in/out when downsampling (the cut-off follows the output's
//! Nyquist). The `b` fractional parts of `t` get a row of coefficients
//! each, computed once; when that table would pass [`TABLE_MAX`]
//! coefficients (rates of no common family, such as 44100 → 47999) there
//! are [`GRID`] rows and each output blends four of them (cubic Lagrange,
//! ~200 dB down). Every row sums to 1 (DC gain 1). Coefficients and sums
//! are f64.
//!
//! **Timing.** The kernel's peak is the alignment point, as in soxr: a click
//! comes out with its peak on the output frame of its time, and the
//! converter looks `lead` input frames ahead for it (9 for minimum phase,
//! ~440 for intermediate, ×in/out when downsampling). The minimum-phase response
//! rises over the ~6 samples before its peak and rings only after it.

use std::sync::OnceLock;

use realfft::RealFftPlanner;
use realfft::num_complex::Complex;

/// The prototype's length at the lower rate.
pub const TAPS: usize = 640;
/// Its Kaiser window's β: ~190 dB side lobes.
pub const BETA: f64 = 19.98;
/// Its cut-off (−6 dB), as a fraction of the lower rate's Nyquist: the
/// transition runs from 0.96 to 1.
pub const CUTOFF: f64 = 0.98;
/// Design points per sample of the lower rate.
pub const OVERSAMPLE: usize = 8;
/// Half the Lagrange interpolator that reads the kernel between its points.
const HALF: usize = 8;
/// The magnitude's floor before the logarithm (relative to DC): −240 dB,
/// far under the stop band.
const FLOOR: f64 = 1e-12;
/// The FFT is at least this many times the prototype's length.
const PADDING: usize = 32;
/// Relative level under which the kernel's ends are dropped.
const TRIM: f64 = 1e-11;
/// The largest exact phase table (coefficients), 32 MB.
const TABLE_MAX: usize = 1 << 22;
/// Rows of the interpolated table when the exact one is too big.
const GRID: usize = 512;
/// Input frames copied in at a time.
const CHUNK: usize = 1024;
/// Rows are padded to a multiple of this many coefficients.
const LANES: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Minimum,
    Intermediate,
}

/// A designed kernel: its impulse response at [`OVERSAMPLE`] points per
/// sample of the lower rate, and where its peak is (in those points,
/// fractional).
pub struct Kernel {
    pub h: Vec<f64>,
    pub peak: f64,
}

/// The kernel of `phase` (designed once, ~5 ms).
pub fn kernel(phase: Phase) -> &'static Kernel {
    static MINIMUM: OnceLock<Kernel> = OnceLock::new();
    static INTERMEDIATE: OnceLock<Kernel> = OnceLock::new();
    match phase {
        Phase::Minimum => MINIMUM.get_or_init(|| design(Phase::Minimum)),
        Phase::Intermediate => INTERMEDIATE.get_or_init(|| design(Phase::Intermediate)),
    }
}

fn sinc(x: f64) -> f64 {
    if x == 0.0 {
        1.0
    } else {
        let p = std::f64::consts::PI * x;
        p.sin() / p
    }
}

/// Modified Bessel function of the first kind, order 0.
fn bessel_i0(x: f64) -> f64 {
    let q = x * x / 4.0;
    let mut sum = 1.0;
    let mut term = 1.0;
    for k in 1..500 {
        term *= q / (k * k) as f64;
        sum += term;
        if term < sum * 1e-17 {
            break;
        }
    }
    sum
}

/// The linear-phase prototype at [`OVERSAMPLE`] points per lower-rate
/// sample, DC gain 1 per lower-rate sample; its centre is at index
/// `TAPS·OVERSAMPLE / 2`.
pub fn prototype() -> Vec<f64> {
    let p = OVERSAMPLE as f64;
    let len = TAPS * OVERSAMPLE;
    let norm = bessel_i0(BETA);
    let mut h: Vec<f64> = (0..len)
        .map(|i| {
            let x = (i as f64 - (len / 2) as f64) / p;
            let r = 2.0 * x / TAPS as f64;
            bessel_i0(BETA * (1.0 - r * r).max(0.0).sqrt()) / norm * sinc(x * CUTOFF)
        })
        .collect();
    let sum: f64 = h.iter().sum::<f64>() / p;
    h.iter_mut().for_each(|v| *v /= sum);
    h
}

/// The prototype's magnitude with minimum or intermediate phase.
pub fn design(phase: Phase) -> Kernel {
    let proto = prototype();
    let len = proto.len();
    let m = (len * PADDING).next_power_of_two();
    let mut planner = RealFftPlanner::<f64>::new();
    let fwd = planner.plan_fft_forward(m);
    let inv = planner.plan_fft_inverse(m);
    let mut time = vec![0.0; m];
    let mut spec = fwd.make_output_vec();
    time[..len].copy_from_slice(&proto);
    fwd.process(&mut time, &mut spec).expect("fft");
    // Real cepstrum of the floored magnitude.
    let floor2 = (spec[0].norm() * FLOOR).powi(2);
    for s in spec.iter_mut() {
        *s = Complex::new(0.5 * (s.norm_sqr() + floor2).ln(), 0.0);
    }
    inv.process(&mut spec, &mut time).expect("fft");
    let scale = 1.0 / m as f64;
    // Folded onto positive quefrencies: the minimum-phase cepstrum.
    time[0] *= scale;
    for v in &mut time[1..m / 2] {
        *v *= 2.0 * scale;
    }
    time[m / 2] *= scale;
    time[m / 2 + 1..].fill(0.0);
    fwd.process(&mut time, &mut spec).expect("fft");
    // spec = ln|H| + jφ_min.
    let centre = (len / 2) as f64;
    for (k, s) in spec.iter_mut().enumerate() {
        let phase = match phase {
            Phase::Minimum => s.im,
            Phase::Intermediate => {
                let w = 2.0 * std::f64::consts::PI * k as f64 / m as f64;
                0.5 * s.im - 0.5 * w * centre
            }
        };
        *s = Complex::from_polar(s.re.exp(), phase);
    }
    // Bins 0 and M/2 of a real signal are real (the latter is at the floor).
    spec[0].im = 0.0;
    spec[m / 2].im = 0.0;
    inv.process(&mut spec, &mut time).expect("fft");
    // The response is circular (negative times at the end of the buffer):
    // the span from −len/2 to 1.5·len, trimmed.
    let span: Vec<f64> = (0..2 * len).map(|i| time[(m - len / 2 + i) % m] * scale).collect();
    let top = span.iter().fold(0.0f64, |a, v| a.max(v.abs()));
    let first = span.iter().position(|v| v.abs() > top * TRIM).unwrap_or(0);
    let last = span.iter().rposition(|v| v.abs() > top * TRIM).unwrap_or(0);
    let h = span[first..=last].to_vec();
    // The peak, between design points (parabola through the top three).
    let i = h.iter().enumerate().fold(0, |best, (i, v)| if v.abs() > h[best].abs() { i } else { best });
    let mut peak = i as f64;
    if i > 0 && i + 1 < h.len() {
        let (l, c, r) = (h[i - 1].abs(), h[i].abs(), h[i + 1].abs());
        let curve = l - 2.0 * c + r;
        if curve < 0.0 {
            peak += 0.5 * (l - r) / curve;
        }
    }
    Kernel { h, peak }
}

/// Barycentric weights of equispaced Lagrange nodes -HALF+1..=HALF.
fn lagrange_nodes() -> [f64; 2 * HALF] {
    // (-1)^j · C(2·HALF − 1, j)
    let mut w = [0.0; 2 * HALF];
    let mut c = 1.0;
    for (j, v) in w.iter_mut().enumerate() {
        *v = if j % 2 == 0 { c } else { -c };
        c = c * (2 * HALF - 1 - j) as f64 / (j + 1) as f64;
    }
    w
}

/// Reads the kernel between its points (16-point Lagrange); remembers the
/// weights of the last fractional position (every tap of a row has the same
/// one when upsampling).
struct Reader<'a> {
    h: &'a [f64],
    nodes: [f64; 2 * HALF],
    u: f64,
    weights: [f64; 2 * HALF],
}

impl<'a> Reader<'a> {
    fn new(h: &'a [f64]) -> Self {
        Reader { h, nodes: lagrange_nodes(), u: f64::NAN, weights: [0.0; 2 * HALF] }
    }

    /// The kernel at `x` (in design points, fractional), 0 outside it.
    fn at(&mut self, x: f64) -> f64 {
        let base = x.floor();
        let u = x - base;
        let base = base as i64;
        let lo = base - HALF as i64 + 1;
        if lo + 2 * HALF as i64 <= 0 || lo >= self.h.len() as i64 {
            return 0.0;
        }
        if u == 0.0 {
            return usize::try_from(base).ok().and_then(|i| self.h.get(i)).copied().unwrap_or(0.0);
        }
        if u != self.u {
            self.u = u;
            let mut den = 0.0;
            for (j, (w, c)) in self.weights.iter_mut().zip(&self.nodes).enumerate() {
                *w = c / (u - (j as f64 - HALF as f64 + 1.0));
                den += *w;
            }
            self.weights.iter_mut().for_each(|w| *w /= den);
        }
        if lo >= 0 && lo as usize + 2 * HALF <= self.h.len() {
            let h = &self.h[lo as usize..lo as usize + 2 * HALF];
            return h.iter().zip(&self.weights).map(|(h, w)| h * w).sum();
        }
        let mut v = 0.0;
        for (j, w) in self.weights.iter().enumerate() {
            let i = lo + j as i64;
            if i >= 0 && (i as usize) < self.h.len() {
                v += w * self.h[i as usize];
            }
        }
        v
    }
}

/// Sum of products, eight lanes at a time (`a` and `b` the same length, a
/// multiple of [`LANES`]).
fn dot(a: &[f64], b: &[f64]) -> f64 {
    let mut s = [0.0f64; 8];
    for (x, y) in a.chunks_exact(8).zip(b.chunks_exact(8)) {
        let (x, y): (&[f64; 8], &[f64; 8]) = (x.try_into().unwrap(), y.try_into().unwrap());
        for k in 0..8 {
            s[k] += x[k] * y[k];
        }
    }
    ((s[0] + s[4]) + (s[1] + s[5])) + ((s[2] + s[6]) + (s[3] + s[7]))
}

/// The same with fused multiply-adds on 32 lanes (eight AVX registers, to
/// hide the FMA's latency): 13.5 instead of 7.5 GMAC/s here.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,fma")]
fn dot_fma(a: &[f64], b: &[f64]) -> f64 {
    use std::arch::x86_64::*;
    let n = a.len().min(b.len()) / LANES * LANES;
    let (pa, pb) = (a.as_ptr(), b.as_ptr());
    let mut s = [_mm256_setzero_pd(); LANES / 4];
    let mut i = 0;
    while i < n {
        for (k, acc) in s.iter_mut().enumerate() {
            // SAFETY: i + 4k + 4 <= n, within both slices.
            let (x, y) = unsafe { (_mm256_loadu_pd(pa.add(i + 4 * k)), _mm256_loadu_pd(pb.add(i + 4 * k))) };
            *acc = _mm256_fmadd_pd(x, y, *acc);
        }
        i += LANES;
    }
    let v = s.iter().fold(_mm256_setzero_pd(), |v, &a| _mm256_add_pd(v, a));
    let mut t = [0.0f64; 4];
    // SAFETY: t holds four doubles.
    unsafe { _mm256_storeu_pd(t.as_mut_ptr(), v) };
    (t[0] + t[2]) + (t[1] + t[3])
}

/// The fastest sum of products this processor has.
fn best_dot() -> fn(&[f64], &[f64]) -> f64 {
    #[cfg(target_arch = "x86_64")]
    if is_x86_feature_detected!("avx2") && is_x86_feature_detected!("fma") {
        // SAFETY: the processor has both features.
        return |a, b| unsafe { dot_fma(a, b) };
    }
    dot
}

/// The streaming converter (planar history, interleaved in and out).
pub struct Polyphase {
    channels: usize,
    a: u64,
    b: u64,
    /// Coefficients per row (a multiple of LANES).
    taps: usize,
    dot: fn(&[f64], &[f64]) -> f64,
    /// Input frames looked ahead: output m needs input up to ⌊m·a/b⌋ + lead.
    lead: usize,
    /// Rows of `taps` coefficients, each reversed (coefficient t multiplies
    /// input frame ⌊m·a/b⌋ + lead − taps + 1 + t).
    table: Vec<f64>,
    /// Interpolated table: rows at phases (q − 1) / GRID, q = 0..GRID+3.
    grid: bool,
    /// Per channel `cap` frames, the first `len` held.
    hist: Vec<f64>,
    cap: usize,
    len: usize,
    /// Input index of `hist[0]`.
    base: i64,
    /// ⌊m·a/b⌋ and (m·a) mod b for the next output m.
    whole: i64,
    frac: u64,
}

impl Polyphase {
    /// `kernel` at the lower rate; `in_rate`/`out_rate` = a/b reduced.
    pub fn new(kernel: &Kernel, a: u64, b: u64, channels: usize) -> Self {
        let p = OVERSAMPLE as f64;
        // Input frames per lower-rate sample.
        let stretch = if b >= a { 1.0 } else { a as f64 / b as f64 };
        let before = (kernel.peak + HALF as f64) / p * stretch;
        let after = ((kernel.h.len() - 1) as f64 - kernel.peak + HALF as f64) / p * stretch;
        let lead = before.ceil() as usize + 2;
        let taps = (lead + after.ceil() as usize + 3).next_multiple_of(LANES);
        let grid = b as usize * taps > TABLE_MAX;
        let rows = if grid { GRID + 3 } else { b as usize };
        let mut reader = Reader::new(&kernel.h);
        let mut table = vec![0.0; rows * taps];
        for (r, row) in table.chunks_exact_mut(taps).enumerate() {
            let phase = if grid { (r as f64 - 1.0) / GRID as f64 } else { r as f64 / b as f64 };
            for (t, c) in row.iter_mut().enumerate() {
                // Time from the peak, in input frames, then design points.
                let tau = phase + (taps - 1 - lead) as f64 - t as f64;
                *c = reader.at(tau / stretch * p + kernel.peak);
            }
            let sum: f64 = row.iter().sum();
            row.iter_mut().for_each(|c| *c /= sum);
        }
        let cap = taps + 4 * CHUNK;
        let mut r = Polyphase {
            channels,
            a,
            b,
            taps,
            dot: best_dot(),
            lead,
            table,
            grid,
            hist: vec![0.0; cap * channels],
            cap,
            len: 0,
            base: 0,
            whole: 0,
            frac: 0,
        };
        r.clear();
        r
    }

    /// Input frames it looks ahead.
    pub fn lead(&self) -> usize {
        self.lead
    }

    pub fn clear(&mut self) {
        self.hist.fill(0.0);
        // Silence before input 0, as far back as output 0 reads.
        let before = self.taps - 1 - self.lead;
        self.len = before;
        self.base = -(before as i64);
        self.whole = 0;
        self.frac = 0;
    }

    /// Takes interleaved frames (`None`: that many silent frames) and appends
    /// to `out` every output they complete, up to `limit` outputs.
    pub fn push(&mut self, input: Option<&[f64]>, frames: usize, out: &mut Vec<f64>, mut limit: u64) -> u64 {
        let ch = self.channels;
        let mut done = 0;
        let mut given = 0u64;
        while done < frames {
            if self.cap - self.len < CHUNK {
                self.compact();
            }
            let n = (frames - done).min(self.cap - self.len);
            for c in 0..ch {
                let dst = &mut self.hist[c * self.cap + self.len..c * self.cap + self.len + n];
                match input {
                    Some(x) => {
                        for (d, s) in dst.iter_mut().zip(x[done * ch + c..].iter().step_by(ch)) {
                            *d = *s;
                        }
                    }
                    None => dst.fill(0.0),
                }
            }
            self.len += n;
            done += n;
            let made = self.produce(out, limit);
            limit -= made;
            given += made;
        }
        given
    }

    fn compact(&mut self) {
        let keep_from = self.whole + self.lead as i64 - (self.taps as i64 - 1) - self.base;
        let drop = keep_from.clamp(0, self.len as i64) as usize;
        if drop == 0 {
            return;
        }
        for c in 0..self.channels {
            let at = c * self.cap;
            self.hist.copy_within(at + drop..at + self.len, at);
        }
        self.len -= drop;
        self.base += drop as i64;
    }

    fn produce(&mut self, out: &mut Vec<f64>, limit: u64) -> u64 {
        let taps = self.taps;
        let mut made = 0;
        while made < limit {
            let newest = self.whole + self.lead as i64;
            if newest >= self.base + self.len as i64 {
                break;
            }
            let start = (newest - (taps as i64 - 1) - self.base) as usize;
            if !self.grid {
                let row = &self.table[self.frac as usize * taps..][..taps];
                for c in 0..self.channels {
                    out.push((self.dot)(row, &self.hist[c * self.cap + start..][..taps]));
                }
            } else {
                let pos = self.frac as f64 * GRID as f64 / self.b as f64;
                let q = (pos.floor() as usize).min(GRID - 1);
                let mu = pos - q as f64;
                // Cubic Lagrange over the rows at phases q−1..q+2.
                let w = [
                    -mu * (mu - 1.0) * (mu - 2.0) / 6.0,
                    (mu + 1.0) * (mu - 1.0) * (mu - 2.0) / 2.0,
                    -(mu + 1.0) * mu * (mu - 2.0) / 2.0,
                    (mu + 1.0) * mu * (mu - 1.0) / 6.0,
                ];
                for c in 0..self.channels {
                    let x = &self.hist[c * self.cap + start..][..taps];
                    let mut y = 0.0;
                    for (k, wk) in w.iter().enumerate() {
                        y += wk * (self.dot)(&self.table[(q + k) * taps..][..taps], x);
                    }
                    out.push(y);
                }
            }
            made += 1;
            self.frac += self.a;
            self.whole += (self.frac / self.b) as i64;
            self.frac %= self.b;
        }
        made
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spectrum_db(h: &[f64], n: usize) -> Vec<f64> {
        let mut planner = RealFftPlanner::<f64>::new();
        let f = planner.plan_fft_forward(n);
        let mut x = vec![0.0; n];
        x[..h.len()].copy_from_slice(h);
        let mut y = f.make_output_vec();
        f.process(&mut x, &mut y).unwrap();
        y.iter().map(|c| 20.0 * (c.norm() / OVERSAMPLE as f64).max(1e-300).log10()).collect()
    }

    /// The designed kernels against the prototype: magnitude, stop band,
    /// DC gain, and the energy ahead of the main lobe.
    #[test]
    fn kernels_keep_the_magnitude() {
        let proto = prototype();
        let n = 1 << 20;
        // Bin of a fraction of the lower Nyquist (n / (2·OVERSAMPLE) bins each).
        let bin = |f: f64| (f * n as f64 / (2 * OVERSAMPLE) as f64) as usize;
        let theirs = spectrum_db(&proto, n);
        let stop = |s: &[f64]| s[bin(1.0)..].iter().fold(-400.0f64, |a, &b| a.max(b));
        let energy = |x: &[f64]| x.iter().map(|v| v * v).sum::<f64>();
        let lobe = 8 * OVERSAMPLE;
        let linear_pre = energy(&proto[..proto.len() / 2 - lobe]) / energy(&proto);
        println!("prototype: stop band {:.1} dB, energy 8 samples before the peak {linear_pre:.2e}", stop(&theirs));
        assert!(stop(&theirs) < -188.0);
        for phase in [Phase::Minimum, Phase::Intermediate] {
            let k = design(phase);
            let ours = spectrum_db(&k.h, n);
            let off = (0..bin(0.97)).map(|i| (ours[i] - theirs[i]).abs()).fold(0.0, f64::max);
            let dc = k.h.iter().sum::<f64>() / OVERSAMPLE as f64;
            let pre = energy(&k.h[..(k.peak as usize).saturating_sub(lobe)]) / energy(&k.h);
            println!(
                "{phase:?}: {} points, peak at {:.3} samples, off the prototype by {off:.1e} dB to 0.97, \
                 stop band {:.1} dB, DC {dc:.12}, energy 8 samples before the peak {pre:.2e}",
                k.h.len(),
                k.peak / OVERSAMPLE as f64,
                stop(&ours)
            );
            assert!(off < 1e-4, "{off}");
            assert!(stop(&ours) < -185.0);
            assert!((dc - 1.0).abs() < 1e-9);
            match phase {
                Phase::Minimum => assert!(pre < 1e-20 && k.peak < (8 * OVERSAMPLE) as f64),
                Phase::Intermediate => assert!(pre < linear_pre / 20.0 && pre > 1e-9),
            }
        }
    }
}
