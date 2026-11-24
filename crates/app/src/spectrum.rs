//! The visualizer's bars: a 2048-point
//! Hann-windowed FFT of what is heard, 24 log bands from 40 Hz to 16 kHz
//! (or Nyquist), each its peak bin, -66..0 dBFS mapped to 0..1; and the
//! view of them: levels that ease towards the bars
//! (paint.rs::SpectrumMeter), painted every frame only while something plays
//! or the bars are still falling.

use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};

pub const BANDS: usize = 24;
const FFT_SIZE: usize = 2048;

pub struct Spectrum {
    fft: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
    samples: Vec<f32>,
    bins: Vec<Complex<f32>>,
    pub levels: [f32; BANDS],
}

impl Default for Spectrum {
    fn default() -> Self {
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(FFT_SIZE);
        let window = (0..FFT_SIZE)
            .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (FFT_SIZE - 1) as f32).cos())
            .collect();
        Spectrum { bins: fft.make_output_vec(), fft, window, samples: vec![0.0; FFT_SIZE], levels: [0.0; BANDS] }
    }
}

impl Spectrum {
    /// New bars from the engine's tap; false when there is nothing yet.
    pub fn update(&mut self, engine: &sonkore_engine::Engine) -> bool {
        if !engine.recent_output(&mut self.samples) {
            return false;
        }
        for (s, w) in self.samples.iter_mut().zip(&self.window) {
            *s *= w;
        }
        if self.fft.process(&mut self.samples, &mut self.bins).is_err() {
            return false;
        }
        let rate = engine.device_format().map_or(48000, |f| f.rate).max(8000) as f64;
        let (lo, hi) = (40.0, 16000f64.min(rate / 2.0 - 1.0));
        let bin_hz = rate / FFT_SIZE as f64;
        for b in 0..BANDS {
            let f0 = lo * (hi / lo).powf(b as f64 / BANDS as f64);
            let f1 = lo * (hi / lo).powf((b + 1) as f64 / BANDS as f64);
            let i0 = ((f0 / bin_hz) as usize).clamp(1, FFT_SIZE / 2 - 1);
            let i1 = ((f1 / bin_hz) as usize + 1).clamp(i0 + 1, FFT_SIZE / 2);
            let peak = self.bins[i0..i1].iter().map(|c| c.norm()).fold(0.0f32, f32::max);
            let db = 20.0 * (peak as f64 / (FFT_SIZE as f64 / 4.0)).max(1e-9).log10();
            self.levels[b] = ((db + 66.0) / 66.0).clamp(0.0, 1.0) as f32;
        }
        true
    }
}

/// The visualizer as shown: the meter, when it last moved, its timer.
pub struct View {
    pub meter: crate::paint::SpectrumMeter,
    pub last: Option<std::time::Instant>,
    pub timer: slint::Timer,
    /// The playing bars' clock.
    pub clock: std::time::Instant,
}

impl Default for View {
    fn default() -> Self {
        View { meter: Default::default(), last: None, timer: Default::default(), clock: std::time::Instant::now() }
    }
}
