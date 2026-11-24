//! The anti-alias low-pass in front of (or behind) the rate transposer:
//! a Hamming-windowed sinc (AAFilter) run as a plain FIR (FIRFilter).

use std::f64::consts::PI;

use crate::fifo::Fifo;

/// Longest filter accepted; the coefficients live in a fixed array.
pub const MAX_AA_LENGTH: usize = 128;

/// Output samples computed side by side. Each one is still summed tap by tap
/// in SoundTouch's order, so the result is bit-identical to its scalar loop;
/// the lanes only let the compiler vectorise across outputs.
const LANES: usize = 32;

pub(crate) struct AaFilter {
    cutoff: f64,
    length: usize,
    coeffs: [f32; MAX_AA_LENGTH], // already divided by 2^14
}

impl AaFilter {
    pub fn new(length: usize) -> Self {
        let mut f = AaFilter { cutoff: 0.5, length: 0, coeffs: [0.0; MAX_AA_LENGTH] };
        f.set_length(length);
        f
    }

    pub fn length(&self) -> usize {
        self.length
    }

    /// Cut-off as a fraction of the sample rate (Nyquist = 0.5).
    pub fn set_cutoff(&mut self, cutoff: f64) {
        self.cutoff = cutoff;
        self.calculate_coeffs();
    }

    /// `length` must be a multiple of 8 in 8..=128 (checked by the caller).
    pub fn set_length(&mut self, length: usize) {
        debug_assert!(length.is_multiple_of(8) && (8..=MAX_AA_LENGTH).contains(&length));
        self.length = length;
        self.calculate_coeffs();
    }

    /// AAFilter::calculateCoeffs, operation for operation. In the float
    /// build the "round to integer" step only adds ±0.5 before the cast to
    /// float, which keeps a tiny bias in the taps; it is reproduced as is.
    fn calculate_coeffs(&mut self) {
        let len = self.length;
        let mut work = [0.0f64; MAX_AA_LENGTH];
        let wc = 2.0 * PI * self.cutoff;
        let temp_coeff = (2.0 * PI) / len as f64;
        let mut sum = 0.0;
        for (i, w_out) in work[..len].iter_mut().enumerate() {
            let cnt = i as f64 - (len / 2) as f64;
            let t = cnt * wc;
            let h = if t != 0.0 { t.sin() / t } else { 1.0 };
            let w = 0.54 + 0.46 * (temp_coeff * cnt).cos();
            *w_out = w * h;
            sum += w * h;
        }
        let scale = 16384.0 / sum;
        for (c, &w) in self.coeffs[..len].iter_mut().zip(&work[..len]) {
            let mut t = w * scale;
            t += if t >= 0.0 { 0.5 } else { -0.5 };
            // FIRFilter::setCoefficients: divide by 2^14 up front.
            *c = ((t as f32) as f64 * (1.0 / 16384.0)) as f32;
        }
    }

    /// Filters what `src` holds into `dest`. The last `length` frames stay
    /// in `src` as history for the next call.
    pub fn evaluate(&self, dest: &mut Fifo, src: &mut Fifo, channels: usize) -> usize {
        let frames = src.len();
        let n = self.run(dest.tail(frames), src.data(), frames, channels);
        src.skip(n);
        dest.commit(n);
        n
    }

    /// FIRFilter::evaluate: out[d] = sum over taps i of src[d + ch*i] * c[i],
    /// the taps added in order. This one formula is SoundTouch's mono,
    /// stereo and multichannel loops alike.
    fn run(&self, dest: &mut [f32], src: &[f32], frames: usize, ch: usize) -> usize {
        let len = self.length;
        if frames < len {
            return 0;
        }
        let out_frames = frames - len;
        let total = out_frames * ch;
        if total < LANES {
            for (d, out) in dest[..total].iter_mut().enumerate() {
                *out = self.tap_sum::<1>(src, d, ch)[0];
            }
            return out_frames;
        }
        // Whole batches; the last one is moved back to end at `total`
        // (recomputing a few outputs, to the same values).
        let mut d = 0;
        loop {
            let at = d.min(total - LANES);
            dest[at..at + LANES].copy_from_slice(&self.tap_sum::<LANES>(src, at, ch));
            if at + LANES == total {
                break;
            }
            d += LANES;
        }
        out_frames
    }

    /// Outputs `d..d + N`.
    #[inline(always)]
    fn tap_sum<const N: usize>(&self, src: &[f32], d: usize, ch: usize) -> [f32; N] {
        let mut acc = [0.0f32; N];
        for (i, &c) in self.coeffs[..self.length].iter().enumerate() {
            let at = d + ch * i;
            let s = &src[at..at + N];
            for l in 0..N {
                acc[l] += s[l] * c;
            }
        }
        acc
    }
}
