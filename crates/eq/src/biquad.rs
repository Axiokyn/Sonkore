//! Second-order sections: RBJ cookbook designs,
//! Vicanek's matched peaking / shelving design and the choice between them,
//! and the transposed direct form II state, all in f64.
//!
//! The arithmetic follows the recorded reference operation by operation,
//! including how GCC evaluates `std::complex<double>` (libgcc's `__divdc3`,
//! glibc's `cexp` / `csqrt` / `cabs`), so the coefficients come out bit for
//! bit the same as the reference values.

use std::f64::consts::PI;

/// The filter types (the controller's "PK", "LSC", "HSC", "LPQ", "HPQ",
/// "BP", "NO").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FilterType {
    #[default]
    Peaking,
    LowShelf,
    HighShelf,
    LowPass,
    HighPass,
    /// Constant 0 dB peak gain.
    BandPass,
    Notch,
}

impl FilterType {
    /// Peaking and shelving bands: shaped by their gain (0 dB is no filter).
    pub fn has_gain(self) -> bool {
        matches!(self, FilterType::Peaking | FilterType::LowShelf | FilterType::HighShelf)
    }
}

/// Normalised (a0 == 1) second-order IIR coefficients.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Coeffs {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
}

impl Default for Coeffs {
    fn default() -> Self {
        Coeffs { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0 }
    }
}

impl Coeffs {
    /// H(e^{-jw}) evaluated as:
    /// `(b0 + b1 z1 + b2 z2) / (1 + a1 z1 + a2 z2)`, z1 = polar(1, -w).
    #[inline]
    pub(crate) fn response_at(&self, z1: Complex, z2: Complex) -> Complex {
        let num = Complex { re: (self.b1 * z1.re + self.b0) + self.b2 * z2.re, im: self.b1 * z1.im + self.b2 * z2.im };
        let den = Complex { re: (self.a1 * z1.re + 1.0) + self.a2 * z2.re, im: self.a1 * z1.im + self.a2 * z2.im };
        num.div(den)
    }

    /// |H| at the normalised angular frequency `w` (radians per sample).
    pub fn magnitude(&self, w: f64) -> f64 {
        let z1 = Complex::polar1(-w);
        self.response_at(z1, z1.mul(z1)).abs()
    }
}

/// Transposed direct form II state. Double precision keeps low-frequency,
/// high-Q sections stable and quiet even at 192/384 kHz.
#[derive(Clone, Copy, Debug, Default)]
pub struct State {
    pub z1: f64,
    pub z2: f64,
}

impl State {
    #[inline(always)]
    pub fn process(&mut self, c: &Coeffs, x: f64) -> f64 {
        let y = c.b0 * x + self.z1;
        self.z1 = c.b1 * x - c.a1 * y + self.z2;
        self.z2 = c.b2 * x - c.a2 * y;
        y
    }

    pub fn reset(&mut self) {
        *self = State::default();
    }
}

// --- std::complex<double> as GCC + glibc evaluate it ---------------------------

#[derive(Clone, Copy, Debug)]
pub(crate) struct Complex {
    pub re: f64,
    pub im: f64,
}

impl Complex {
    /// std::polar(1.0, theta).
    #[inline]
    pub fn polar1(theta: f64) -> Complex {
        Complex { re: theta.cos(), im: theta.sin() }
    }

    #[inline]
    pub fn mul(self, o: Complex) -> Complex {
        Complex { re: self.re * o.re - self.im * o.im, im: self.re * o.im + self.im * o.re }
    }

    /// libgcc's `__divdc3` (GCC 12 and later). Its power-of-two rescaling
    /// of tiny or huge operands is exact, so only the two evaluation orders
    /// are kept.
    #[inline]
    pub fn div(self, o: Complex) -> Complex {
        let (a, b, c, d) = (self.re, self.im, o.re, o.im);
        let (x, y);
        if c.abs() < d.abs() {
            let ratio = c / d;
            let denom = c * ratio + d;
            if ratio.abs() > f64::MIN_POSITIVE {
                x = (a * ratio + b) / denom;
                y = (b * ratio - a) / denom;
            } else {
                x = (c * (a / d) + b) / denom;
                y = (c * (b / d) - a) / denom;
            }
        } else {
            let ratio = d / c;
            let denom = d * ratio + c;
            if ratio.abs() > f64::MIN_POSITIVE {
                x = (b * ratio + a) / denom;
                y = (b - a * ratio) / denom;
            } else {
                x = (a + d * (b / c)) / denom;
                y = (b - d * (a / c)) / denom;
            }
        }
        Complex { re: x, im: y }
    }

    /// std::abs: glibc's cabs, which is hypot.
    #[inline]
    pub fn abs(self) -> f64 {
        self.re.hypot(self.im)
    }

    /// std::norm (libstdc++: re² + im²).
    #[inline]
    pub fn norm(self) -> f64 {
        self.re * self.re + self.im * self.im
    }

    /// glibc's cexp for a finite argument whose real part is below ~709.
    fn exp(self) -> Complex {
        let (sin, cos) =
            if self.im.abs() > f64::MIN_POSITIVE { (self.im.sin(), self.im.cos()) } else { (self.im, 1.0) };
        let e = self.re.exp();
        Complex { re: e * cos, im: e * sin }
    }

    /// glibc's csqrt of a real number (imaginary part +0).
    fn sqrt_real(x: f64) -> Complex {
        if x < 0.0 { Complex { re: 0.0, im: (-x).sqrt() } } else { Complex { re: x.sqrt().abs(), im: 0.0 } }
    }
}

/// std::clamp: `v < lo ? lo : hi < v ? hi : v` (no panic when hi < lo).
#[inline]
pub(crate) fn clamp(v: f64, lo: f64, hi: f64) -> f64 {
    if v < lo {
        lo
    } else if hi < v {
        hi
    } else {
        v
    }
}

/// std::max(a, b): `a < b ? b : a`.
#[inline]
pub(crate) fn max(a: f64, b: f64) -> f64 {
    if a < b { b } else { a }
}

/// std::min(a, b): `b < a ? b : a`.
#[inline]
pub(crate) fn min(a: f64, b: f64) -> f64 {
    if b < a { b } else { a }
}

/// 10^x through `pow` (LLVM may turn `powf(10, x)` into `exp10`, which
/// rounds differently from the reference's `std::pow`).
#[inline]
pub(crate) fn pow10(x: f64) -> f64 {
    std::hint::black_box(10.0f64).powf(x)
}

// --- Designs -------------------------------------------------------------------

/// RBJ "Audio EQ Cookbook" designs. `freq_hz` is clamped to
/// [1, 0.4999·fs] and `q` to at least 1e-4.
pub fn design_rbj(kind: FilterType, sample_rate: f64, freq_hz: f64, q: f64, gain_db: f64) -> Coeffs {
    let freq_hz = clamp(freq_hz, 1.0, sample_rate * 0.4999);
    let q = max(q, 1e-4);

    let a = pow10(gain_db / 40.0);
    let w0 = 2.0 * PI * freq_hz / sample_rate;
    let cw = w0.cos();
    let sw = w0.sin();
    let alpha = sw / (2.0 * q);
    let sq_a2_alpha = 2.0 * a.sqrt() * alpha;

    let (b0, b1, b2, a0, a1, a2);
    match kind {
        FilterType::Peaking => {
            b0 = 1.0 + alpha * a;
            b1 = -2.0 * cw;
            b2 = 1.0 - alpha * a;
            a0 = 1.0 + alpha / a;
            a1 = -2.0 * cw;
            a2 = 1.0 - alpha / a;
        }
        FilterType::LowShelf => {
            b0 = a * ((a + 1.0) - (a - 1.0) * cw + sq_a2_alpha);
            b1 = 2.0 * a * ((a - 1.0) - (a + 1.0) * cw);
            b2 = a * ((a + 1.0) - (a - 1.0) * cw - sq_a2_alpha);
            a0 = (a + 1.0) + (a - 1.0) * cw + sq_a2_alpha;
            a1 = -2.0 * ((a - 1.0) + (a + 1.0) * cw);
            a2 = (a + 1.0) + (a - 1.0) * cw - sq_a2_alpha;
        }
        FilterType::HighShelf => {
            b0 = a * ((a + 1.0) + (a - 1.0) * cw + sq_a2_alpha);
            b1 = -2.0 * a * ((a - 1.0) + (a + 1.0) * cw);
            b2 = a * ((a + 1.0) + (a - 1.0) * cw - sq_a2_alpha);
            a0 = (a + 1.0) - (a - 1.0) * cw + sq_a2_alpha;
            a1 = 2.0 * ((a - 1.0) - (a + 1.0) * cw);
            a2 = (a + 1.0) - (a - 1.0) * cw - sq_a2_alpha;
        }
        FilterType::LowPass => {
            b0 = (1.0 - cw) * 0.5;
            b1 = 1.0 - cw;
            b2 = (1.0 - cw) * 0.5;
            a0 = 1.0 + alpha;
            a1 = -2.0 * cw;
            a2 = 1.0 - alpha;
        }
        FilterType::HighPass => {
            b0 = (1.0 + cw) * 0.5;
            b1 = -(1.0 + cw);
            b2 = (1.0 + cw) * 0.5;
            a0 = 1.0 + alpha;
            a1 = -2.0 * cw;
            a2 = 1.0 - alpha;
        }
        FilterType::BandPass => {
            b0 = alpha;
            b1 = 0.0;
            b2 = -alpha;
            a0 = 1.0 + alpha;
            a1 = -2.0 * cw;
            a2 = 1.0 - alpha;
        }
        FilterType::Notch => {
            b0 = 1.0;
            b1 = -2.0 * cw;
            b2 = 1.0;
            a0 = 1.0 + alpha;
            a1 = -2.0 * cw;
            a2 = 1.0 - alpha;
        }
    }

    let inv = 1.0 / a0;
    Coeffs { b0: b0 * inv, b1: b1 * inv, b2: b2 * inv, a1: a1 * inv, a2: a2 * inv }
}

/// The analog prototype of an RBJ peaking / shelving filter, s normalised
/// to the corner frequency: (n2 s² + n1 s + n0) / (d2 s² + d1 s + d0).
#[derive(Clone, Copy)]
struct Analog {
    n2: f64,
    n1: f64,
    n0: f64,
    d2: f64,
    d1: f64,
    d0: f64,
}

impl Analog {
    fn new(kind: FilterType, a: f64, q: f64) -> Analog {
        let sa = a.sqrt();
        match kind {
            FilterType::LowShelf => Analog { n2: a, n1: sa * a / q, n0: a * a, d2: a, d1: sa / q, d0: 1.0 },
            FilterType::HighShelf => Analog { n2: a * a, n1: sa * a / q, n0: a, d2: 1.0, d1: sa / q, d0: a },
            _ => Analog { n2: 1.0, n1: a / q, n0: 1.0, d2: 1.0, d1: 1.0 / (a * q), d0: 1.0 },
        }
    }

    /// |H(jx)|².
    fn magnitude2(&self, x: f64) -> f64 {
        let nr = self.n0 - self.n2 * x * x;
        let ni = self.n1 * x;
        let dr = self.d0 - self.d2 * x * x;
        let di = self.d1 * x;
        (nr * nr + ni * ni) / (dr * dr + di * di)
    }
}

fn digital_magnitude2(c: &Coeffs, w: f64) -> f64 {
    let z1 = Complex::polar1(-w);
    c.response_at(z1, z1.mul(z1)).norm()
}

/// Vicanek's matched design ("Matched Second Order Digital Filters", 2016):
/// poles from the prototype's by impulse invariance, zeros chosen so the
/// magnitude equals the prototype's at DC, at the corner and at Nyquist.
/// Unlike the bilinear transform it does not squeeze the response toward
/// Nyquist (a 16 kHz +6 dB band at 44.1 kHz stays a 16 kHz +6 dB band).
/// Cuts are boosts inverted (the prototypes are exact reciprocals), which
/// keeps the poles where the matching works. None when it fails or would
/// be unstable (the caller keeps the RBJ design).
fn design_matched(kind: FilterType, sample_rate: f64, freq_hz: f64, q: f64, gain_db: f64) -> Option<Coeffs> {
    let cut = gain_db < 0.0;
    let a = pow10(gain_db.abs() / 40.0);
    let h = Analog::new(kind, a, q);
    let w0 = 2.0 * PI * freq_hz / sample_rate;
    let disc = Complex::sqrt_real(h.d1 * h.d1 - 4.0 * h.d2 * h.d0);
    // exp((-d1 ± disc) / (2 d2) · w0): a real added to, then dividing and
    // scaling, a complex value, part by part.
    let pole = |sign: f64| {
        let s = 2.0 * h.d2;
        let re = (if sign > 0.0 { disc.re + -h.d1 } else { -disc.re + -h.d1 }) / s * w0;
        let im = (if sign > 0.0 { disc.im } else { -disc.im }) / s * w0;
        Complex { re, im }.exp()
    };
    let p1 = pole(1.0);
    let p2 = pole(-1.0);
    let a1 = -(p1.re + p2.re);
    let a2 = p1.mul(p2).re;

    let a0_ = (1.0 + a1 + a2) * (1.0 + a1 + a2);
    let a1_ = (1.0 - a1 + a2) * (1.0 - a1 + a2);
    let a2_ = -4.0 * a2;
    let s2 = (w0 / 2.0).sin() * (w0 / 2.0).sin();
    let phi0 = 1.0 - s2;
    let phi1 = s2;
    let phi2 = 4.0 * phi0 * phi1;
    if phi2 < 1e-12 {
        return None;
    }
    let bb0 = a0_ * h.magnitude2(0.0);
    let bb1 = a1_ * h.magnitude2(PI / w0);
    let bb2 = (h.magnitude2(1.0) * (a0_ * phi0 + a1_ * phi1 + a2_ * phi2) - bb0 * phi0 - bb1 * phi1) / phi2;
    let sb0 = bb0.sqrt();
    let sb1 = bb1.sqrt();
    let w = 0.5 * (sb0 + sb1);
    let b0 = 0.5 * (w + max(w * w + bb2, 0.0).sqrt());
    #[allow(clippy::neg_cmp_op_on_partial_ord)] // NaN fails too
    if !(b0 > 0.0) || !b0.is_finite() {
        return None;
    }
    let b1 = 0.5 * (sb0 - sb1);
    let b2 = -bb2 / (4.0 * b0);
    let out = if !cut {
        Coeffs { b0, b1, b2, a1, a2 }
    } else {
        let inv = 1.0 / b0;
        Coeffs { b0: inv, b1: a1 * inv, b2: a2 * inv, a1: b1 * inv, a2: b2 * inv }
    };
    let ok = out.b1.is_finite()
        && out.b2.is_finite()
        && out.a1.is_finite()
        && out.a2.is_finite()
        && out.a2.abs() < 1.0
        && out.a1.abs() < 1.0 + out.a2;
    ok.then_some(out)
}

/// Largest deviation (dB) from the prototype over the audible band: 48 log
/// points from 20 Hz to min(20 kHz, 0.45·fs).
fn prototype_error(c: &Coeffs, kind: FilterType, sample_rate: f64, freq_hz: f64, q: f64, gain_db: f64) -> f64 {
    let h = Analog::new(kind, pow10(gain_db / 40.0), q);
    let top = min(20000.0, sample_rate * 0.45);
    let mut worst = 0.0;
    const POINTS: i32 = 48;
    for i in 0..POINTS {
        let f = 20.0 * (top / 20.0).powf(f64::from(i) / f64::from(POINTS - 1));
        let want = h.magnitude2(f / freq_hz);
        let got = digital_magnitude2(c, 2.0 * PI * f / sample_rate);
        worst = max(worst, (10.0 * (got / want).log10()).abs());
    }
    worst
}

/// The section for a band. Peaking and shelving bands: the matched design,
/// or the RBJ one where that lands closer to the analog prototype (a high
/// shelf whose corner sits near Nyquist); the other types are RBJ's.
pub fn design(kind: FilterType, sample_rate: f64, freq_hz: f64, q: f64, gain_db: f64) -> Coeffs {
    let rbj = design_rbj(kind, sample_rate, freq_hz, q, gain_db);
    if !kind.has_gain() || gain_db == 0.0 {
        return rbj;
    }
    let freq_hz = clamp(freq_hz, 1.0, sample_rate * 0.4999);
    let q = max(q, 1e-4);
    let Some(matched) = design_matched(kind, sample_rate, freq_hz, q, gain_db) else {
        return rbj;
    };
    if prototype_error(&matched, kind, sample_rate, freq_hz, q, gain_db)
        <= prototype_error(&rbj, kind, sample_rate, freq_hz, q, gain_db)
    {
        matched
    } else {
        rbj
    }
}

/// The gain (dB) of a cascade at `hz` for a `sample_rate`, summed as
/// 20·log10|H| per section.
pub fn cascade_db(sections: &[Coeffs], sample_rate: f64, hz: f64) -> f64 {
    let z1 = Complex::polar1(-(2.0 * PI * hz / sample_rate));
    let z2 = z1.mul(z1);
    sections.iter().map(|c| 20.0 * c.response_at(z1, z2).abs().log10()).fold(0.0, |acc, db| acc + db)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_gain_peak_is_identity_magnitude() {
        let c = design(FilterType::Peaking, 48000.0, 1000.0, 1.0, 0.0);
        assert!((c.magnitude(0.3) - 1.0).abs() < 1e-15);
    }

    #[test]
    fn matched_peak_hits_its_gain() {
        for &fs in &[44100.0, 48000.0, 96000.0] {
            let c = design(FilterType::Peaking, fs, 16000.0, 1.0, 6.0);
            let db = 20.0 * c.magnitude(2.0 * PI * 16000.0 / fs).log10();
            assert!((db - 6.0).abs() < 0.05, "{fs}: {db}");
        }
    }

    #[test]
    fn cut_inverts_boost() {
        let boost = design(FilterType::LowShelf, 48000.0, 200.0, 0.7, 9.0);
        let cut = design(FilterType::LowShelf, 48000.0, 200.0, 0.7, -9.0);
        for &w in &[0.01, 0.1, 1.0, 3.0] {
            assert!((boost.magnitude(w) * cut.magnitude(w) - 1.0).abs() < 1e-9);
        }
    }

    #[test]
    fn division_matches_plain_formula() {
        let a = Complex { re: 0.3, im: -1.7 };
        let b = Complex { re: 2.5, im: 0.25 };
        let q = a.div(b);
        let back = q.mul(b);
        assert!((back.re - a.re).abs() < 1e-15 && (back.im - a.im).abs() < 1e-15);
    }
}
