//! FIR convolution in f64 (the single-response case of the engine's convolver):
//! uniformly partitioned overlap-save with FFTs of 2 × 1024, the same
//! response for every channel, 1024 frames of latency, and a response
//! swap that crossfades across one block.

use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

/// Frames per partition (and the latency).
pub const BLOCK: usize = 1024;
const N: usize = 2 * BLOCK; // FFT size
const BINS: usize = N / 2 + 1;

pub struct Convolver {
    channels: usize,
    taps: usize,
    partitions: usize,
    forward: Arc<dyn RealToComplex<f64>>,
    inverse: Arc<dyn ComplexToReal<f64>>,
    /// Partition spectra of the response (partitions × BINS).
    filter: Vec<Complex<f64>>,
    /// replace_taps(): the next response's spectra, faded in on the next step.
    pending: Option<Vec<Complex<f64>>>,
    /// The spectra replaced last time, reused by the next replace_taps().
    spare: Vec<Complex<f64>>,
    /// Per channel: the frequency-domain delay line of input spectra.
    history: Vec<Vec<Complex<f64>>>,
    /// Per channel: the last 2 × BLOCK input samples.
    window: Vec<Vec<f64>>,
    head: usize,
    in_block: Vec<Vec<f64>>,
    out_block: Vec<Vec<f64>>,
    fill: usize,
    acc: Vec<Complex<f64>>,
    acc_new: Vec<Complex<f64>>,
    time: Vec<f64>,
    time_new: Vec<f64>,
    scratch_fwd: Vec<Complex<f64>>,
    scratch_inv: Vec<Complex<f64>>,
}

impl Convolver {
    /// `taps` applied to each of `channels` interleaved channels.
    pub fn new(taps: &[f64], channels: usize) -> Self {
        let mut planner = RealFftPlanner::<f64>::new();
        let forward = planner.plan_fft_forward(N);
        let inverse = planner.plan_fft_inverse(N);
        let partitions = taps.len().div_ceil(BLOCK).max(1);
        let scratch_fwd = forward.make_scratch_vec();
        let scratch_inv = inverse.make_scratch_vec();
        let mut c = Convolver {
            channels,
            taps: taps.len(),
            partitions,
            forward,
            inverse,
            filter: Vec::new(),
            pending: None,
            spare: Vec::new(),
            history: vec![vec![Complex::default(); partitions * BINS]; channels],
            window: vec![vec![0.0; N]; channels],
            head: 0,
            in_block: vec![vec![0.0; BLOCK]; channels],
            out_block: vec![vec![0.0; BLOCK]; channels],
            fill: 0,
            acc: vec![Complex::default(); BINS],
            acc_new: vec![Complex::default(); BINS],
            time: vec![0.0; N],
            time_new: vec![0.0; N],
            scratch_fwd,
            scratch_inv,
        };
        let mut filter = Vec::new();
        c.spectra(taps, &mut filter);
        c.filter = filter;
        c
    }

    /// Partition spectra of a response (zero-padded to whole partitions),
    /// into `s` (allocating only when it is too small).
    fn spectra(&mut self, taps: &[f64], s: &mut Vec<Complex<f64>>) {
        s.clear();
        s.resize(self.partitions * BINS, Complex::default());
        for (p, part) in s.as_chunks_mut::<BINS>().0.iter_mut().enumerate() {
            self.time.fill(0.0);
            let from = (p * BLOCK).min(taps.len());
            let to = ((p + 1) * BLOCK).min(taps.len());
            self.time[..to - from].copy_from_slice(&taps[from..to]);
            self.forward.process_with_scratch(&mut self.time, part, &mut self.scratch_fwd).expect("fft sizes");
        }
    }

    /// Frames still owed after the input ends (the delay plus the tail).
    pub fn tail(&self) -> usize {
        BLOCK + self.taps
    }

    /// A new response of the same partition count: the input already seen
    /// is kept, and the next block crossfades from the old filter's output
    /// to the new one's. False when the length differs.
    pub fn replace_taps(&mut self, taps: &[f64]) -> bool {
        if taps.is_empty() || taps.len().div_ceil(BLOCK) != self.partitions {
            return false;
        }
        // Into the spectra not faded in yet, or those the last swap freed.
        let mut s = self.pending.take().unwrap_or_else(|| std::mem::take(&mut self.spare));
        self.spectra(taps, &mut s);
        self.pending = Some(s);
        self.taps = taps.len();
        true
    }

    fn accumulate(
        acc: &mut [Complex<f64>],
        history: &[Complex<f64>],
        filter: &[Complex<f64>],
        head: usize,
        parts: usize,
    ) {
        acc.fill(Complex::default());
        for p in 0..parts {
            let x = &history[((head + p) % parts) * BINS..][..BINS];
            let h = &filter[p * BINS..][..BINS];
            for ((a, x), h) in acc.iter_mut().zip(x).zip(h) {
                a.re += x.re * h.re - x.im * h.im;
                a.im += x.re * h.im + x.im * h.re;
            }
        }
        // A real signal's DC and Nyquist bins are real.
        acc[0].im = 0.0;
        acc[BINS - 1].im = 0.0;
    }

    /// One block through every channel: the FFT of the last two input
    /// blocks enters the delay line; the output block is the second half
    /// of the inverse FFT of Σ X[n-p]·H[p].
    fn step(&mut self) {
        let parts = self.partitions;
        self.head = (self.head + parts - 1) % parts;
        let scale = 1.0 / N as f64;
        for c in 0..self.channels {
            let w = &mut self.window[c];
            w.copy_within(BLOCK.., 0);
            w[BLOCK..].copy_from_slice(&self.in_block[c]);
            self.time.copy_from_slice(w);
            let slot = &mut self.history[c][self.head * BINS..][..BINS];
            self.forward.process_with_scratch(&mut self.time, slot, &mut self.scratch_fwd).expect("fft sizes");

            Self::accumulate(&mut self.acc, &self.history[c], &self.filter, self.head, parts);
            self.inverse.process_with_scratch(&mut self.acc, &mut self.time, &mut self.scratch_inv).expect("fft sizes");
            let out = &mut self.out_block[c];
            if let Some(pending) = &self.pending {
                // The same input through the new filter, faded in across the block.
                Self::accumulate(&mut self.acc_new, &self.history[c], pending, self.head, parts);
                self.inverse
                    .process_with_scratch(&mut self.acc_new, &mut self.time_new, &mut self.scratch_inv)
                    .expect("fft sizes");
                for (i, ((o, old), new)) in
                    out.iter_mut().zip(&self.time[BLOCK..]).zip(&self.time_new[BLOCK..]).enumerate()
                {
                    let w = (i as f64 + 0.5) / BLOCK as f64;
                    let (old, new) = (old * scale, new * scale);
                    *o = old + (new - old) * w;
                }
            } else {
                for (o, t) in out.iter_mut().zip(&self.time[BLOCK..]) {
                    *o = t * scale;
                }
            }
        }
        if let Some(pending) = self.pending.take() {
            self.spare = std::mem::replace(&mut self.filter, pending);
        }
    }

    /// Interleaved frames in place: the input convolved, delayed by BLOCK.
    pub fn process(&mut self, samples: &mut [f64]) {
        let ch = self.channels;
        for frame in samples.chunks_exact_mut(ch) {
            for (c, s) in frame.iter_mut().enumerate() {
                self.in_block[c][self.fill] = *s;
                *s = self.out_block[c][self.fill]; // the block computed one step ago
            }
            self.fill += 1;
            if self.fill == BLOCK {
                self.step();
                self.fill = 0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn convolves_with_a_block_of_delay() {
        let taps: Vec<f64> = (0..3000).map(|i| ((i * 7919) % 101) as f64 / 101.0 - 0.5).collect();
        let input: Vec<f64> = (0..6000).map(|i| ((i * 104729) % 997) as f64 / 997.0 - 0.5).collect();
        let mut conv = Convolver::new(&taps, 1);
        let mut out = input.clone();
        for chunk in out.chunks_mut(333) {
            conv.process(chunk);
        }
        for n in (BLOCK..6000).step_by(37) {
            let k = n - BLOCK;
            let want: f64 = (0..taps.len().min(k + 1)).map(|j| taps[j] * input[k - j]).sum();
            assert!((out[n] - want).abs() < 1e-11, "{n}: {} vs {want}", out[n]);
        }
        assert!(out[..BLOCK].iter().all(|&v| v == 0.0));
    }

    #[test]
    fn replacing_the_response_reuses_its_buffers() {
        let taps = vec![0.5; 3000];
        let mut conv = Convolver::new(&taps, 2);
        let mut block = vec![0.1; 2 * BLOCK];
        assert!(conv.replace_taps(&[0.25; 3000]));
        conv.process(&mut block);
        let buffers = [conv.filter.as_ptr(), conv.spare.as_ptr()];
        for k in 0..6 {
            block.fill(0.1);
            assert!(conv.replace_taps(&[0.1 * k as f64; 2900]));
            assert!(conv.replace_taps(&[0.2; 3000])); // twice before a step
            conv.process(&mut block);
            assert!(buffers.contains(&conv.filter.as_ptr()) && buffers.contains(&conv.spare.as_ptr()));
        }
        // And the response is the last one given: 0.2 × (sum of 0.1 over
        // 3000 taps) once the input has filled them all.
        for _ in 0..4 {
            block.fill(0.1);
            conv.process(&mut block);
        }
        assert!((block[2 * BLOCK - 1] - 0.2 * 0.1 * 3000.0).abs() < 1e-9, "{}", block[2 * BLOCK - 1]);
    }
}
