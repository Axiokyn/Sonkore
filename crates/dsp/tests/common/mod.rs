//! Helpers shared by the integration tests: spectra with a very deep window
//! (to see 200 dB down) and whole-signal resampling.

#![allow(dead_code)]

use realfft::RealFftPlanner;
use sonkore_dsp::resampler::Resampler;

/// Modified Bessel function of the first kind, order 0.
fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let q = x * x / 4.0;
    for k in 1..500 {
        term *= q / (k as f64 * k as f64);
        sum += term;
        if term < sum * 1e-20 {
            break;
        }
    }
    sum
}

/// A Kaiser window; beta 38 puts its side lobes ~330 dB down (12 bins wide
/// main lobe), so a resampler's 180 dB stop band is measurable.
pub fn kaiser(n: usize, beta: f64) -> Vec<f64> {
    let norm = bessel_i0(beta);
    (0..n)
        .map(|i| {
            let r = 2.0 * i as f64 / (n - 1) as f64 - 1.0;
            bessel_i0(beta * (1.0 - r * r).max(0.0).sqrt()) / norm
        })
        .collect()
}

pub struct Spectrum {
    pub power: Vec<f64>,
    pub rate: f64,
    pub reference: f64,
}

pub const LOBE: usize = 14;

impl Spectrum {
    /// Power spectrum of `x` (one channel), windowed; `reference` is the lobe
    /// power a full-scale (amplitude 1) sine would show.
    pub fn of(x: &[f64], rate: f64) -> Spectrum {
        let n = x.len();
        let w = kaiser(n, 38.0);
        let mut planner = RealFftPlanner::<f64>::new();
        let fft = planner.plan_fft_forward(n);
        let mut buf: Vec<f64> = x.iter().zip(&w).map(|(a, b)| a * b).collect();
        let mut out = fft.make_output_vec();
        fft.process(&mut buf, &mut out).unwrap();
        let power: Vec<f64> = out.iter().map(|c| c.norm_sqr()).collect();
        // A unit sine's windowed power: (sum w)^2 / 4 on its bin, spread over
        // the lobe; Parseval: sum over the lobe = n * sum(w^2) / 4.
        let reference = n as f64 * w.iter().map(|v| v * v).sum::<f64>() / 4.0;
        Spectrum { power, rate, reference }
    }

    pub fn bin(&self, hz: f64) -> f64 {
        let n = (self.power.len() - 1) * 2;
        hz * n as f64 / self.rate
    }

    /// The level (dB re full-scale sine) of what lies within the main lobe
    /// around `hz`.
    pub fn level_db(&self, hz: f64) -> f64 {
        let c = self.bin(hz).round() as isize;
        let lo = (c - LOBE as isize).max(0) as usize;
        let hi = ((c + LOBE as isize) as usize).min(self.power.len() - 1);
        let p: f64 = self.power[lo..=hi].iter().sum();
        10.0 * (p / self.reference).max(1e-40).log10()
    }
}

/// A sine of amplitude `amp`, `frames` long, on `channels` channels.
pub fn sine(hz: f64, rate: f64, frames: usize, channels: usize, amp: f64) -> Vec<f64> {
    let mut v = Vec::with_capacity(frames * channels);
    for i in 0..frames {
        let s = amp * (2.0 * std::f64::consts::PI * hz * i as f64 / rate).sin();
        for _ in 0..channels {
            v.push(s);
        }
    }
    v
}

/// Resamples a whole signal in blocks of `block` frames, then drains.
pub fn resample_all(r: &mut Resampler, input: &[f64], block: usize) -> Vec<f64> {
    let ch = r.channels();
    let mut out = Vec::new();
    let mut part = Vec::new();
    for chunk in input.chunks(block * ch) {
        r.process(chunk, &mut part);
        out.extend_from_slice(&part);
    }
    while r.drain(&mut part) > 0 {
        out.extend_from_slice(&part);
    }
    out
}

pub fn channel(x: &[f64], channels: usize, c: usize) -> Vec<f64> {
    x.iter().skip(c).step_by(channels).copied().collect()
}

/// A path for scratch files, under the target dir.
pub fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("data");
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

/// Deterministic noise in [-1, 1) (xorshift64*).
pub fn noise(n: usize, seed: u64) -> Vec<f64> {
    let mut s = seed | 1;
    (0..n)
        .map(|_| {
            s ^= s >> 12;
            s ^= s << 25;
            s ^= s >> 27;
            (s.wrapping_mul(0x2545F4914F6CDD1D) >> 11) as f64 / (1u64 << 52) as f64 - 1.0
        })
        .collect()
}

// --- Frequency response by sines ---------------------------------------------------

const M: usize = 1 << 16; // analysed output frames

#[derive(Debug, Clone, Copy)]
pub struct Quality {
    /// Largest deviation from 0 dB up to `pass_to` of the lower Nyquist.
    pub ripple_db: f64,
    /// The highest tested frequency (fraction of the lower Nyquist) still
    /// within 0.1 dB.
    pub flat_to: f64,
    /// The weakest stop-band rejection found (dB, positive), from 1.005 of
    /// the lower Nyquist (up to `stop_from` when given).
    pub stopband_db: f64,
}

/// Measures a converter (mono, whole signal in, whole signal out) with
/// sines: ripple up to `pass_to` of the lower Nyquist, stop band from
/// `stop_from` (fraction of the lower Nyquist, 1.005 for a resampler).
pub fn measure(
    in_rate: u32,
    out_rate: u32,
    pass_to: f64,
    stop_from: f64,
    convert: &mut dyn FnMut(&[f64]) -> Vec<f64>,
) -> Quality {
    let low_nyq = in_rate.min(out_rate) as f64 / 2.0;
    let ratio = out_rate as f64 / in_rate as f64;
    let lead = 30000usize; // output frames skipped at each end
    let in_frames = ((M + 2 * lead) as f64 / ratio) as usize;
    let mut through = |hz: f64| {
        let y = convert(&sine(hz, in_rate as f64, in_frames, 1, 1.0));
        Spectrum::of(&y[lead..lead + M], out_rate as f64)
    };
    let on_bin = |hz: f64| (hz * M as f64 / out_rate as f64).round() * out_rate as f64 / M as f64;
    let mut ripple: f64 = 0.0;
    let mut flat_to = 0.0;
    let mut f = 0.005;
    while f < 0.995 {
        let hz = on_bin(f * low_nyq);
        let level = through(hz).level_db(hz);
        if f <= pass_to {
            ripple = ripple.max(level.abs());
        }
        if level.abs() <= 0.1 && f > flat_to {
            flat_to = f;
        }
        f += if f < 0.85 { 0.05 } else { 0.01 };
    }
    let mut stop: f64 = f64::INFINITY;
    let guard = 30.0 * out_rate as f64 / M as f64;
    if out_rate > in_rate {
        // Images of tones below the input's Nyquist.
        for i in 1..=40 {
            let hz = on_bin(low_nyq * (0.025 * i as f64).min(2.0 - stop_from));
            let s = through(hz);
            let mut k = 1.0;
            loop {
                let mut any = false;
                for img in [k * in_rate as f64 - hz, k * in_rate as f64 + hz] {
                    if img < out_rate as f64 / 2.0 - guard {
                        if img > stop_from * low_nyq - guard / 2.0 {
                            stop = stop.min(-s.level_db(img));
                        }
                        any = true;
                    }
                }
                if !any {
                    break;
                }
                k += 1.0;
            }
        }
    } else {
        // Tones above the stop-band edge and their aliases (or, at one rate,
        // the tone itself).
        let in_nyq = in_rate as f64 / 2.0;
        for i in 0..40 {
            let hz = low_nyq * stop_from + (in_nyq * 0.995 - low_nyq * stop_from) * i as f64 / 39.0;
            let s = through(hz);
            let fold = {
                let m = hz % out_rate as f64;
                if m > out_rate as f64 / 2.0 { out_rate as f64 - m } else { m }
            };
            stop = stop.min(-s.level_db(fold));
        }
    }
    Quality { ripple_db: ripple, flat_to, stopband_db: stop }
}

/// The gain (dB) a converter gives a sine at `hz` (rounded to an analysis
/// bin).
pub fn gain_db(in_rate: u32, out_rate: u32, hz: f64, convert: &mut dyn FnMut(&[f64]) -> Vec<f64>) -> f64 {
    let ratio = out_rate as f64 / in_rate as f64;
    let lead = 30000usize;
    let hz = (hz * M as f64 / out_rate as f64).round() * out_rate as f64 / M as f64;
    let in_frames = ((M + 2 * lead) as f64 / ratio) as usize;
    let y = convert(&sine(hz, in_rate as f64, in_frames, 1, 1.0));
    Spectrum::of(&y[lead..lead + M], out_rate as f64).level_db(hz)
}
