//! The rate transposer's interpolators (InterpolateLinear / Cubic / Shannon,
//! float build). Each keeps SoundTouch's per-channel-count arithmetic: the
//! linear one, for instance, works in double for mono and stereo but in
//! float for more channels, and the results differ in the last bit.

use crate::fifo::Fifo;

/// Interpolation used to change the sample rate. SoundTouch's default, and
/// the one Sonkore uses, is [`Interpolation::Cubic`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Interpolation {
    Linear,
    #[default]
    Cubic,
    /// Windowed sinc over 8 points. SoundTouch never implemented it beyond
    /// stereo (it outputs nothing there); this port extends the stereo
    /// arithmetic to any channel count.
    Shannon,
}

const CUBIC: [f32; 16] = [
    -0.5, 1.0, -0.5, 0.0, //
    1.5, -2.5, 0.0, 1.0, //
    -1.5, 2.0, 0.5, 0.0, //
    0.5, -0.5, 0.0, 0.0,
];

/// Kaiser window, beta 2, scaled down by 5 %.
const KAISER8: [f64; 8] = [
    0.41778693317814,
    0.64888025049173,
    0.83508562409944,
    0.93887857733412,
    0.93887857733412,
    0.83508562409944,
    0.64888025049173,
    0.41778693317814,
];

#[inline]
fn sinc(x: f64) -> f64 {
    #[allow(clippy::approx_constant)]
    const PI: f64 = 3.1415926536; // SoundTouch's rounded value
    (PI * x).sin() / (PI * x)
}

pub(crate) struct Transposer {
    pub algorithm: Interpolation,
    pub rate: f64,
    fract: f64,
    channels: usize,
}

impl Transposer {
    pub fn new(algorithm: Interpolation, channels: usize) -> Self {
        Transposer { algorithm, rate: 1.0, fract: 0.0, channels }
    }

    pub fn reset(&mut self) {
        self.fract = 0.0;
    }

    /// Frames of look-ahead the interpolator needs.
    pub fn latency(&self) -> usize {
        match self.algorithm {
            Interpolation::Linear => 0,
            Interpolation::Cubic => 1,
            Interpolation::Shannon => 3,
        }
    }

    /// TransposerBase::transpose: resamples all of `src` it can into `dest`.
    pub fn transpose(&mut self, dest: &mut Fifo, src: &mut Fifo) -> usize {
        let n_src = src.len();
        let demand = ((n_src as f64 / self.rate) as i32).saturating_add(8).max(0) as usize;
        let out = dest.tail(demand);
        let input = src.data();
        let (produced, consumed) = match self.algorithm {
            Interpolation::Linear if self.channels <= 2 => self.linear_double(out, input, n_src),
            Interpolation::Linear => self.linear_float(out, input, n_src),
            Interpolation::Cubic => self.cubic(out, input, n_src),
            Interpolation::Shannon if self.channels == 1 => self.shannon_mono(out, input, n_src),
            Interpolation::Shannon => self.shannon_multi(out, input, n_src),
        };
        dest.commit(produced);
        src.skip(consumed);
        produced
    }

    /// Advances the read position by the rate; returns the whole frames.
    #[inline]
    fn step(&mut self) -> usize {
        self.fract += self.rate;
        let whole = self.fract as i32;
        self.fract -= whole as f64;
        whole.max(0) as usize
    }

    /// Mono and stereo: double arithmetic.
    fn linear_double(&mut self, dest: &mut [f32], src: &[f32], n_src: usize) -> (usize, usize) {
        let ch = self.channels;
        let end = n_src as isize - 1;
        let max_out = dest.len() / ch;
        let (mut i, mut count) = (0, 0usize);
        while (count as isize) < end && i < max_out {
            let s = &src[count * ch..];
            for c in 0..ch {
                let out = (1.0 - self.fract) * s[c] as f64 + self.fract * s[c + ch] as f64;
                dest[i * ch + c] = out as f32;
            }
            i += 1;
            count += self.step();
        }
        (i, count)
    }

    /// More than two channels: float arithmetic.
    fn linear_float(&mut self, dest: &mut [f32], src: &[f32], n_src: usize) -> (usize, usize) {
        let ch = self.channels;
        let end = n_src as isize - 1;
        let max_out = dest.len() / ch;
        let (mut i, mut count) = (0, 0usize);
        while (count as isize) < end && i < max_out {
            let vol1 = (1.0 - self.fract) as f32;
            let f = self.fract as f32;
            let s = &src[count * ch..];
            for c in 0..ch {
                dest[i * ch + c] = vol1 * s[c] + f * s[c + ch];
            }
            i += 1;
            count += self.step();
        }
        (i, count)
    }

    /// Same arithmetic for every channel count; mono and stereo get their
    /// own copies so the channel loop unrolls.
    fn cubic(&mut self, dest: &mut [f32], src: &[f32], n_src: usize) -> (usize, usize) {
        match self.channels {
            1 => self.cubic_n::<1>(dest, src, n_src, 1),
            2 => self.cubic_n::<2>(dest, src, n_src, 2),
            ch => self.cubic_n::<0>(dest, src, n_src, ch),
        }
    }

    /// `CH` is the channel count, or 0 for "read `ch`".
    #[inline(always)]
    fn cubic_n<const CH: usize>(&mut self, dest: &mut [f32], src: &[f32], n_src: usize, ch: usize) -> (usize, usize) {
        let ch = if CH == 0 { ch } else { CH };
        let end = n_src as isize - 4;
        let k = &CUBIC;
        let (mut i, mut count) = (0, 0usize);
        for out in dest.chunks_exact_mut(ch) {
            if (count as isize) >= end {
                break;
            }
            let x3 = 1.0f32;
            let x2 = self.fract as f32;
            let x1 = x2 * x2;
            let x0 = x1 * x2;
            let y0 = k[0] * x0 + k[1] * x1 + k[2] * x2 + k[3] * x3;
            let y1 = k[4] * x0 + k[5] * x1 + k[6] * x2 + k[7] * x3;
            let y2 = k[8] * x0 + k[9] * x1 + k[10] * x2 + k[11] * x3;
            let y3 = k[12] * x0 + k[13] * x1 + k[14] * x2 + k[15] * x3;
            let s = &src[count * ch..(count + 4) * ch];
            for c in 0..ch {
                out[c] = y0 * s[c] + y1 * s[c + ch] + y2 * s[c + 2 * ch] + y3 * s[c + 3 * ch];
            }
            i += 1;
            count += self.step();
        }
        (i, count)
    }

    fn shannon_mono(&mut self, dest: &mut [f32], src: &[f32], n_src: usize) -> (usize, usize) {
        let end = n_src as isize - 8;
        let max_out = dest.len();
        let (mut i, mut count) = (0, 0usize);
        while (count as isize) < end && i < max_out {
            let f = self.fract;
            let s = &src[count..count + 8];
            let mut out = s[0] as f64 * sinc(-3.0 - f) * KAISER8[0];
            out += s[1] as f64 * sinc(-2.0 - f) * KAISER8[1];
            out += s[2] as f64 * sinc(-1.0 - f) * KAISER8[2];
            if f < 1e-6 {
                out += s[3] as f64 * KAISER8[3];
            } else {
                out += s[3] as f64 * sinc(-f) * KAISER8[3];
            }
            out += s[4] as f64 * sinc(1.0 - f) * KAISER8[4];
            out += s[5] as f64 * sinc(2.0 - f) * KAISER8[5];
            out += s[6] as f64 * sinc(3.0 - f) * KAISER8[6];
            out += s[7] as f64 * sinc(4.0 - f) * KAISER8[7];
            dest[i] = out as f32;
            i += 1;
            count += self.step();
        }
        (i, count)
    }

    /// Stereo arithmetic (weight first, then per channel), for 2+ channels.
    fn shannon_multi(&mut self, dest: &mut [f32], src: &[f32], n_src: usize) -> (usize, usize) {
        const MAX_CH: usize = crate::MAX_CHANNELS;
        let ch = self.channels;
        let end = n_src as isize - 8;
        let max_out = dest.len() / ch;
        let (mut i, mut count) = (0, 0usize);
        while (count as isize) < end && i < max_out {
            let f = self.fract;
            let s = &src[count * ch..];
            let mut acc = [0.0f64; MAX_CH];
            for (t, &k) in KAISER8.iter().enumerate() {
                let w = if t == 3 { k * if f < 1e-5 { 1.0 } else { sinc(-f) } } else { sinc(t as f64 - 3.0 - f) * k };
                for c in 0..ch {
                    let v = s[t * ch + c] as f64 * w;
                    acc[c] = if t == 0 { v } else { acc[c] + v };
                }
            }
            for c in 0..ch {
                dest[i * ch + c] = acc[c] as f32;
            }
            i += 1;
            count += self.step();
        }
        (i, count)
    }
}
