//! Sample-rate transposer with its anti-alias filter (RateTransposer).

use crate::aa_filter::AaFilter;
use crate::fifo::Fifo;
use crate::interpolate::{Interpolation, Transposer};

pub(crate) struct RateTransposer {
    aa: AaFilter,
    transposer: Transposer,
    input: Fifo,
    mid: Fifo,
    pub output: Fifo,
    use_aa: bool,
    channels: usize,
}

impl RateTransposer {
    /// As SoundTouch builds it: rate 1, a 64-tap filter, and the input
    /// prefilled with silence for the latency. That prefill is counted for
    /// stereo (FIFOSampleBuffer's default) and reinterpreted when the channel
    /// count is set, so it holds `2 * latency / channels` frames until the
    /// next clear.
    pub fn new(algorithm: Interpolation, channels: usize) -> Self {
        let mut rt = RateTransposer {
            aa: AaFilter::new(64),
            transposer: Transposer::new(algorithm, channels),
            input: Fifo::new(channels),
            mid: Fifo::new(channels),
            output: Fifo::new(channels),
            use_aa: true,
            channels,
        };
        rt.set_rate(1.0);
        rt.input.add_silence(2 * rt.latency() / channels);
        rt
    }

    pub fn reserve(&mut self, frames: usize) {
        self.input.reserve(frames);
        self.mid.reserve(frames);
        self.output.reserve(frames);
    }

    pub fn set_aa_enabled(&mut self, on: bool) {
        self.use_aa = on;
        self.clear();
    }

    pub fn aa_enabled(&self) -> bool {
        self.use_aa
    }

    pub fn aa_length(&self) -> usize {
        self.aa.length()
    }

    /// New filter length; the buffered audio is kept.
    pub fn set_aa_length(&mut self, length: usize) {
        self.aa.set_length(length);
    }

    pub fn set_rate(&mut self, rate: f64) {
        self.transposer.rate = rate;
        self.aa.set_cutoff(if rate > 1.0 { 0.5 / rate } else { 0.5 * rate });
    }

    pub fn put_samples(&mut self, samples: &[f32]) {
        if samples.len() < self.channels {
            return;
        }
        self.input.put(samples);
        if !self.use_aa {
            self.transposer.transpose(&mut self.output, &mut self.input);
        } else if self.transposer.rate < 1.0 {
            // Slowing down: interpolate, then remove the images.
            self.transposer.transpose(&mut self.mid, &mut self.input);
            self.aa.evaluate(&mut self.output, &mut self.mid, self.channels);
        } else {
            // Speeding up: band-limit first so nothing folds over.
            self.aa.evaluate(&mut self.mid, &mut self.input, self.channels);
            self.transposer.transpose(&mut self.output, &mut self.mid);
        }
    }

    /// Empties every buffer and prefills the latency with silence again.
    pub fn clear(&mut self) {
        self.output.clear();
        self.mid.clear();
        self.input.clear();
        self.transposer.reset();
        let prefill = self.latency();
        self.input.add_silence(prefill);
    }

    pub fn latency(&self) -> usize {
        self.transposer.latency() + if self.use_aa { self.aa.length() / 2 } else { 0 }
    }
}
