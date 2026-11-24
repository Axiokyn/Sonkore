//! FIR convolution in double precision: uniformly
//! partitioned overlap-save (FFT size 2 × 1024), so filters of tens of
//! thousands of taps (room or headphone correction from REW, Dirac,
//! Acourate...) run cheaply with 1024 frames of latency. Built for one sample
//! rate and channel count; the impulse response is resampled (the VHQ
//! linear-phase converter, gain kept) when its rate differs.
//!
//! Impulse responses have 1 channel (applied to every channel), 2 (one per
//! channel) or 4 (true stereo: LL LR RL RR, so left out = L∗LL + R∗RL and
//! right out = L∗LR + R∗RR).
//!
//! Everything is allocated by [`Convolver::new`]; `process`, `reset` and
//! `replace_taps` neither allocate nor lock (worker thread).

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use symphonia::core::audio::{AudioBufferRef, Signal};
use symphonia::core::codecs::{CODEC_TYPE_NULL, DecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use crate::resampler::{Filter, Resampler};

/// Frames per partition, and the latency.
pub const BLOCK: usize = 1024;
/// The longest impulse response: 2^20 taps.
pub const MAX_TAPS: usize = 1 << 20;
/// The most channels a convolver takes.
pub const MAX_CHANNELS: usize = 8;
/// FFT size.
const N: usize = 2 * BLOCK;
/// Bins of a real spectrum of N points.
const BINS: usize = N / 2 + 1;

const S32_INV_FULL_SCALE: f64 = 1.0 / 2147483648.0;

/// "44.1", "96" (a rate in kHz, for messages).
fn khz(rate: u32) -> String {
    if rate.is_multiple_of(1000) { (rate / 1000).to_string() } else { format!("{:.1}", rate as f64 / 1000.0) }
}

/// An impulse response as read from its file, before it is fitted to a track.
#[derive(Clone, Debug, Default)]
pub struct ImpulseResponse {
    pub path: PathBuf,
    pub sample_rate: u32,
    /// 1 (all channels), 2 (one per channel) or 4 (true stereo: LL LR RL RR).
    pub channels: usize,
    /// Per IR channel.
    pub taps: Vec<Vec<f64>>,
}

impl ImpulseResponse {
    /// Reads an impulse response from WAV / FLAC / any format symphonia
    /// decodes. Up to 2^20 taps. The samples are taken as the engine's
    /// decoders give them: integer sources left-justified to 32 bits, float
    /// sources rounded to the same 32-bit grid (clamped), then / 2^31.
    pub fn load(path: &Path) -> Result<ImpulseResponse, String> {
        let file = File::open(path).map_err(|e| e.to_string())?;
        let stream = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }
        let probed = symphonia::default::get_probe()
            .format(&hint, stream, &FormatOptions::default(), &MetadataOptions::default())
            .map_err(|e| e.to_string())?;
        let mut format = probed.format;
        let track = format
            .tracks()
            .iter()
            .find(|t| t.codec_params.codec != CODEC_TYPE_NULL && t.codec_params.sample_rate.is_some())
            .ok_or_else(|| "no audio track".to_string())?;
        let track_id = track.id;
        let params = track.codec_params.clone();
        let mut decoder =
            symphonia::default::get_codecs().make(&params, &DecoderOptions::default()).map_err(|e| e.to_string())?;
        let channels = params.channels.map(|c| c.count()).unwrap_or(0);
        if !matches!(channels, 1 | 2 | 4) {
            return Err("An impulse response has 1, 2 or 4 (true stereo) channels".into());
        }
        let mut out = ImpulseResponse {
            path: path.to_path_buf(),
            sample_rate: params.sample_rate.unwrap_or(0),
            channels,
            taps: vec![Vec::new(); channels],
        };
        loop {
            let packet = match format.next_packet() {
                Ok(p) => p,
                Err(SymphoniaError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(SymphoniaError::ResetRequired) => {
                    decoder.reset();
                    continue;
                }
                Err(e) => return Err(e.to_string()),
            };
            if packet.track_id() != track_id {
                continue;
            }
            let decoded = match decoder.decode(&packet) {
                Ok(d) => d,
                Err(SymphoniaError::DecodeError(_)) => continue, // a damaged packet: skip it
                Err(e) => return Err(e.to_string()),
            };
            append_taps(&decoded, &mut out.taps);
            if out.taps[0].len() > MAX_TAPS {
                break;
            }
        }
        if out.taps[0].is_empty() {
            return Err("The impulse response is empty".into());
        }
        if out.taps[0].len() > MAX_TAPS {
            return Err("The impulse response is longer than 2^20 taps".into());
        }
        Ok(out)
    }
}

/// A float sample to left-justified s32 (lrint and clamp).
fn float_to_s32(v: f64) -> i32 {
    (v * 2147483648.0).round_ties_even().clamp(-2147483648.0, 2147483647.0) as i32
}

/// One decoded packet's samples, per channel, as the engine reads them.
fn append_taps(buf: &AudioBufferRef<'_>, taps: &mut [Vec<f64>]) {
    macro_rules! copy {
        ($b:expr, $conv:expr) => {{
            let b = $b;
            for (c, t) in taps.iter_mut().enumerate().take(b.spec().channels.count()) {
                t.extend(b.chan(c).iter().map(|&v| $conv(v) as f64 * S32_INV_FULL_SCALE));
            }
        }};
    }
    match buf {
        AudioBufferRef::U8(b) => copy!(b, |v: u8| (v as i32 - 128) << 24),
        AudioBufferRef::U16(b) => copy!(b, |v: u16| (v as i32 - 32768) << 16),
        AudioBufferRef::U24(b) => copy!(b, |v: symphonia::core::sample::u24| (v.inner() as i32 - 8388608) << 8),
        AudioBufferRef::U32(b) => copy!(b, |v: u32| (v ^ 0x8000_0000) as i32),
        AudioBufferRef::S8(b) => copy!(b, |v: i8| (v as i32) << 24),
        AudioBufferRef::S16(b) => copy!(b, |v: i16| (v as i32) << 16),
        AudioBufferRef::S24(b) => copy!(b, |v: symphonia::core::sample::i24| v.inner() << 8),
        AudioBufferRef::S32(b) => copy!(b, |v: i32| v),
        AudioBufferRef::F32(b) => copy!(b, |v: f32| float_to_s32(v as f64)),
        AudioBufferRef::F64(b) => copy!(b, float_to_s32),
    }
}

/// The convolver, for one rate and channel count.
pub struct Convolver {
    channels: usize,
    taps: usize,
    resampled_from: u32,
    partitions: usize,
    fft: Arc<dyn RealToComplex<f64>>,
    ifft: Arc<dyn ComplexToReal<f64>>,
    fft_scratch: Vec<Complex<f64>>,
    ifft_scratch: Vec<Complex<f64>>,
    /// filters[out][src]: partitions × BINS spectra; `sources[out][src]` is
    /// the input channel each one applies to.
    filters: Vec<Vec<Vec<Complex<f64>>>>,
    sources: Vec<Vec<usize>>,
    /// Per input channel: the frequency-domain delay line of input spectra
    /// (partitions × BINS), newest at `head`.
    history: Vec<Vec<Complex<f64>>>,
    /// Per input channel: the last 2 × BLOCK samples.
    window: Vec<Vec<f64>>,
    head: usize,
    in_block: Vec<Vec<f64>>,
    out_block: Vec<Vec<f64>>,
    /// Frames gathered in `in_block` / handed out of `out_block`.
    fill: usize,
    acc: Vec<Complex<f64>>,
    acc_new: Vec<Complex<f64>>,
    time: Vec<f64>,
    time_new: Vec<f64>,
    /// replace_taps(): the next filter's spectra (one channel), faded in at
    /// the next block.
    pending: Vec<Complex<f64>>,
    pending_active: bool,
}

impl Convolver {
    /// The convolver for a stream; an error (with the reason) when the
    /// response does not fit it.
    pub fn new(ir: &ImpulseResponse, sample_rate: u32, channels: usize) -> Result<Convolver, String> {
        if channels == 0 || channels > MAX_CHANNELS {
            return Err("The convolver takes 1 to 8 channels".into());
        }
        if !matches!(ir.channels, 1 | 2 | 4) || ir.taps.len() != ir.channels || ir.taps[0].is_empty() {
            return Err("An impulse response has 1, 2 or 4 (true stereo) channels".into());
        }
        if ir.taps.iter().any(|t| t.len() != ir.taps[0].len()) {
            return Err("The impulse response's channels differ in length".into());
        }
        if ir.channels == 4 && channels != 2 {
            return Err("A true-stereo impulse response needs a stereo track".into());
        }
        if ir.channels == 2 && channels != 2 {
            return Err("A stereo impulse response needs a stereo track".into());
        }

        // The response at the track's rate. Resampling changes the number of
        // taps per second, so the taps are scaled back to keep the filter's
        // gain.
        let too_long = |rate: u32| format!("The impulse response is longer than 2^20 taps at {} kHz", khz(rate));
        if ir.taps[0].len() > MAX_TAPS {
            return Err("The impulse response is longer than 2^20 taps".into());
        }
        // Resampled to a higher rate the response gets longer: the limit
        // (what runs in real time) holds for the taps actually convolved.
        // Checked ahead too, so an IR that cannot fit is not resampled first.
        if ir.sample_rate != sample_rate
            && ir.sample_rate > 0
            && (ir.taps[0].len() as u64 * sample_rate as u64) / ir.sample_rate as u64 > MAX_TAPS as u64
        {
            return Err(too_long(sample_rate));
        }
        let mut taps = ir.taps.clone();
        let mut resampled_from = 0;
        if ir.sample_rate != sample_rate {
            let scale = ir.sample_rate as f64 / sample_rate as f64;
            for t in taps.iter_mut() {
                let mut r = Resampler::new(ir.sample_rate, sample_rate, 1, Filter::Linear)?;
                let mut out = Vec::new();
                let mut part = Vec::new();
                r.process(t, &mut part);
                out.extend_from_slice(&part);
                while r.drain(&mut part) > 0 {
                    out.extend_from_slice(&part);
                }
                for v in out.iter_mut() {
                    *v *= scale;
                }
                *t = out;
            }
            resampled_from = ir.sample_rate;
        }
        let tap_count = taps[0].len();
        if tap_count == 0 {
            return Err("The impulse response is empty".into());
        }
        if tap_count > MAX_TAPS {
            return Err(too_long(sample_rate));
        }
        let partitions = tap_count.div_ceil(BLOCK);

        let mut planner = RealFftPlanner::<f64>::new();
        let fft = planner.plan_fft_forward(N);
        let ifft = planner.plan_fft_inverse(N);
        let mut c = Convolver {
            channels,
            taps: tap_count,
            resampled_from,
            partitions,
            fft_scratch: fft.make_scratch_vec(),
            ifft_scratch: ifft.make_scratch_vec(),
            fft,
            ifft,
            filters: Vec::with_capacity(channels),
            sources: Vec::with_capacity(channels),
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
            pending: vec![Complex::default(); partitions * BINS],
            pending_active: false,
        };
        let spectra = |c: &mut Convolver, t: &[f64]| {
            let mut s = vec![Complex::default(); partitions * BINS];
            c.partition_spectra(t, &mut s);
            s
        };
        for o in 0..channels {
            if ir.channels == 4 {
                // LL LR RL RR
                let a = spectra(&mut c, &taps[if o == 0 { 0 } else { 1 }]);
                let b = spectra(&mut c, &taps[if o == 0 { 2 } else { 3 }]);
                c.filters.push(vec![a, b]);
                c.sources.push(vec![0, 1]);
            } else {
                let a = spectra(&mut c, &taps[if ir.channels == 2 { o } else { 0 }]);
                c.filters.push(vec![a]);
                c.sources.push(vec![o]);
            }
        }
        Ok(c)
    }

    /// Spectra of each partition of a filter (zero-padded to whole
    /// partitions and to the FFT size), into `out` (partitions × BINS).
    fn partition_spectra(&mut self, t: &[f64], out: &mut [Complex<f64>]) {
        for p in 0..self.partitions {
            let start = p * BLOCK;
            let n = BLOCK.min(t.len().saturating_sub(start));
            self.time.fill(0.0);
            self.time[..n].copy_from_slice(&t[start..start + n]);
            let _ = self.fft.process_with_scratch(
                &mut self.time,
                &mut out[p * BINS..(p + 1) * BINS],
                &mut self.fft_scratch,
            );
        }
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    pub fn taps(&self) -> usize {
        self.taps
    }

    /// Frames of delay: the output is the input convolved, BLOCK frames late.
    pub fn latency(&self) -> usize {
        BLOCK
    }

    /// Frames still owed after the input ends (the delay plus the filter's
    /// tail).
    pub fn tail(&self) -> usize {
        BLOCK + self.taps
    }

    pub fn resampled(&self) -> bool {
        self.resampled_from != 0
    }

    /// The impulse response's own rate when it was resampled, else 0.
    pub fn resampled_from(&self) -> u32 {
        self.resampled_from
    }

    /// Interleaved frames in place; the output is the input convolved,
    /// delayed by BLOCK frames.
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

    /// Forgets the input seen (silence in, silence out).
    pub fn reset(&mut self) {
        for h in self.history.iter_mut() {
            h.fill(Complex::default());
        }
        for w in self.window.iter_mut() {
            w.fill(0.0);
        }
        for b in self.out_block.iter_mut() {
            b.fill(0.0);
        }
        self.head = 0;
        self.fill = 0;
    }

    /// A new single-channel response of the same number of partitions, for
    /// every channel (the linear-phase EQ's new curve): the input already
    /// seen is kept, and the next block crossfades from the old filter's
    /// output to the new one's (no gap, no click). False when the length
    /// does not fit or the convolver is true stereo.
    pub fn replace_taps(&mut self, taps: &[f64]) -> bool {
        if taps.is_empty() || taps.len().div_ceil(BLOCK) != self.partitions {
            return false;
        }
        if self.sources.iter().any(|s| s.len() != 1) {
            return false; // true stereo: not a single-channel response
        }
        let mut pending = std::mem::take(&mut self.pending);
        self.partition_spectra(taps, &mut pending);
        self.pending = pending;
        self.pending_active = true;
        self.taps = taps.len();
        true
    }

    /// sum over partitions of X[n-p] · H[p], into `acc`.
    fn accumulate(acc: &mut [Complex<f64>], x: &[Complex<f64>], h: &[Complex<f64>], head: usize, parts: usize) {
        for p in 0..parts {
            let slot = (head + p) % parts;
            let xs = &x[slot * BINS..(slot + 1) * BINS];
            let hs = &h[p * BINS..(p + 1) * BINS];
            for ((a, x), h) in acc.iter_mut().zip(xs).zip(hs) {
                a.re += x.re * h.re - x.im * h.im;
                a.im += x.re * h.im + x.im * h.re;
            }
        }
    }

    /// Overlap-save: the FFT of the last two input blocks enters the
    /// frequency-domain delay line; the output block is the second half of
    /// the inverse FFT of sum(X[n-p] · H[p]).
    fn step(&mut self) {
        let parts = self.partitions;
        self.head = (self.head + parts - 1) % parts;
        for c in 0..self.channels {
            let w = &mut self.window[c];
            w.copy_within(BLOCK.., 0);
            w[BLOCK..].copy_from_slice(&self.in_block[c]);
            self.time.copy_from_slice(w);
            let slot = &mut self.history[c][self.head * BINS..(self.head + 1) * BINS];
            let _ = self.fft.process_with_scratch(&mut self.time, slot, &mut self.fft_scratch);
        }
        let crossfade = self.pending_active;
        let scale = 1.0 / N as f64;
        for o in 0..self.channels {
            self.acc.fill(Complex::default());
            for (s, h) in self.filters[o].iter().enumerate() {
                let x = &self.history[self.sources[o][s]];
                Self::accumulate(&mut self.acc, x, h, self.head, parts);
            }
            // A real signal's spectrum: no imaginary part at DC and Nyquist.
            self.acc[0].im = 0.0;
            self.acc[BINS - 1].im = 0.0;
            let _ = self.ifft.process_with_scratch(&mut self.acc, &mut self.time, &mut self.ifft_scratch);
            if crossfade {
                // The same input through the new filter, faded in across the
                // block.
                self.acc_new.fill(Complex::default());
                let x = &self.history[self.sources[o][0]];
                Self::accumulate(&mut self.acc_new, x, &self.pending, self.head, parts);
                self.acc_new[0].im = 0.0;
                self.acc_new[BINS - 1].im = 0.0;
                let _ = self.ifft.process_with_scratch(&mut self.acc_new, &mut self.time_new, &mut self.ifft_scratch);
                for i in 0..BLOCK {
                    let w = (i as f64 + 0.5) / BLOCK as f64;
                    let old = self.time[BLOCK + i] * scale;
                    let new = self.time_new[BLOCK + i] * scale;
                    self.out_block[o][i] = old + (new - old) * w;
                }
            } else {
                for (d, &v) in self.out_block[o].iter_mut().zip(&self.time[BLOCK..]) {
                    *d = v * scale;
                }
            }
        }
        if crossfade {
            for f in self.filters.iter_mut() {
                f[0].copy_from_slice(&self.pending);
            }
            self.pending_active = false;
        }
    }
}
