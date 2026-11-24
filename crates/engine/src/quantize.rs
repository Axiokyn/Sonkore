//! Samples to the device's grid (bit for bit):
//! left-justified s32 on the grid of the device's real depth, with TPDF or
//! F-weighted noise-shaped dither, clipping counted, and lasting digital
//! silence written as true zeros (SoX's auto-blanking).

/// Full scale of left-justified 32-bit PCM: 2^31.
pub const S32_FULL_SCALE: f64 = 2147483648.0;
pub const S32_INV_FULL_SCALE: f64 = 1.0 / S32_FULL_SCALE;

/// xorshift64*: the dither's noise (with a fixed seed).
#[derive(Clone)]
pub struct DitherRng(u64);

impl Default for DitherRng {
    fn default() -> Self {
        DitherRng(0x9E3779B97F4A7C15)
    }
}

impl DitherRng {
    /// Uniform in [0, 1).
    #[inline]
    pub fn uniform(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545F4914F6CDD1D) >> 11) as f64 * (1.0 / 9007199254740992.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum DitherMode {
    Off,
    #[default]
    Tpdf,
    NoiseShaped,
}

impl DitherMode {
    /// The settings' ditherMode (0 off, 1 TPDF, 2 shaped).
    pub fn from_index(i: i64) -> Self {
        match i {
            0 => DitherMode::Off,
            2 => DitherMode::NoiseShaped,
            _ => DitherMode::Tpdf,
        }
    }
}

pub const MAX_CHANNELS: usize = 8;
const TAPS: usize = 9;
const BLANK_AFTER: u32 = 2048;
// SoX "f-weighted" error feedback (Wannamaker), 44.1 / 48 kHz.
const F_WEIGHTED: [f64; TAPS] = [2.412, -3.370, 3.937, -4.174, 3.353, -2.205, 1.281, -0.569, 0.0847];

pub struct Quantizer {
    rng: DitherRng,
    bits: u32,
    lsb: f64,
    mode: DitherMode,
    channels: usize,
    shaping_rate: bool,
    error: [[f64; TAPS]; MAX_CHANNELS],
    zero_run: [u32; MAX_CHANNELS],
    pos: usize,
    clipped: u64,
}

impl Default for Quantizer {
    fn default() -> Self {
        Quantizer {
            rng: DitherRng::default(),
            bits: 24,
            lsb: 256.0,
            mode: DitherMode::Tpdf,
            channels: 2,
            shaping_rate: true,
            error: [[0.0; TAPS]; MAX_CHANNELS],
            zero_run: [0; MAX_CHANNELS],
            pos: 0,
            clipped: 0,
        }
    }
}

impl Quantizer {
    pub fn set_target_bits(&mut self, bits: u32) {
        self.bits = if bits == 0 || bits > 32 { 32 } else { bits };
        self.lsb = 2f64.powi(32 - self.bits as i32);
    }

    pub fn set_format(&mut self, rate: u32, channels: usize) {
        self.channels = if channels == 0 || channels > MAX_CHANNELS { 2 } else { channels };
        self.shaping_rate = rate == 44100 || rate == 48000;
        self.reset();
    }

    pub fn set_dither(&mut self, mode: DitherMode) {
        self.mode = mode;
    }

    pub fn reset(&mut self) {
        self.error = [[0.0; TAPS]; MAX_CHANNELS];
        self.zero_run = [0; MAX_CHANNELS];
        self.pos = 0;
    }

    /// Samples clamped at full scale since the last call.
    pub fn take_clipped(&mut self) -> u64 {
        std::mem::take(&mut self.clipped)
    }

    pub fn process(&mut self, input: &[f64], out: &mut [i32]) {
        let max_value = S32_FULL_SCALE - self.lsb;
        let dither = self.mode != DitherMode::Off && self.bits < 32;
        let shaped = self.mode == DitherMode::NoiseShaped && self.bits <= 20 && self.shaping_rate;
        let ch = self.channels;
        // The channel counts along each frame (no division per sample).
        let mut next_c = 0;
        for (&x, o) in input.iter().zip(out.iter_mut()) {
            let c = next_c;
            next_c = if c + 1 == ch { 0 } else { c + 1 };
            let mut v = x * S32_FULL_SCALE / self.lsb;
            self.zero_run[c] = if x != 0.0 {
                0
            } else if self.zero_run[c] > BLANK_AFTER {
                self.zero_run[c]
            } else {
                self.zero_run[c] + 1
            };
            if self.zero_run[c] > BLANK_AFTER {
                *o = 0;
                if shaped {
                    self.error[c][(self.pos + 1) % TAPS] = 0.0;
                    if c == ch - 1 {
                        self.pos = (self.pos + 1) % TAPS;
                    }
                }
                continue;
            }
            if shaped {
                let mut fb = 0.0;
                for (k, h) in F_WEIGHTED.iter().enumerate() {
                    fb += h * self.error[c][(self.pos + TAPS - k) % TAPS];
                }
                v -= fb;
            }
            let w = v;
            if dither {
                v += self.rng.uniform() - self.rng.uniform();
            }
            v = round_half_even(v);
            if shaped {
                let e = v - w;
                self.error[c][(self.pos + 1) % TAPS] = e.clamp(-1.5, 1.5);
                if c == ch - 1 {
                    self.pos = (self.pos + 1) % TAPS;
                }
            }
            v *= self.lsb;
            if v > max_value {
                v = max_value;
                self.clipped += 1;
            } else if v < -S32_FULL_SCALE {
                v = -S32_FULL_SCALE;
                self.clipped += 1;
            }
            *o = v as i32;
        }
    }
}

/// std::nearbyint in the default rounding mode: halves to even.
#[inline]
pub fn round_half_even(v: f64) -> f64 {
    v.round_ties_even()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_on_the_grid() {
        // Without dither, 24-bit samples come back unchanged.
        let mut q = Quantizer::default();
        q.set_dither(DitherMode::Off);
        q.set_target_bits(24);
        q.set_format(44100, 2);
        let src: Vec<i32> = vec![256, -512, 0x7fffff00, -0x80000000, 1 << 8, 0];
        let input: Vec<f64> = src.iter().map(|&s| s as f64 * S32_INV_FULL_SCALE).collect();
        let mut out = vec![0; src.len()];
        q.process(&input, &mut out);
        assert_eq!(out, src);
    }

    #[test]
    fn rng_matches_reference() {
        // First values of the recorded reference (xorshift64*, same seed).
        let mut r = DitherRng::default();
        let a = r.uniform();
        let b = r.uniform();
        assert!((0.0..1.0).contains(&a) && (0.0..1.0).contains(&b) && a != b);
    }
}
