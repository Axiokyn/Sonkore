//! DSD (DSF, DSDIFF and DST) to PCM, in double precision, for outputs that
//! take no DSD:
//!
//! 1. the 1-bit stream to 352.8 kHz (384 kHz on the 48 kHz family): a
//!    linear-phase FIR (Kaiser window, 160 dB stop band) flat up to the
//!    output's Nyquist frequency and closed from where content would fold onto
//!    it, evaluated a byte of bits at a time from tables (the filter's sum for
//!    each of the 256 patterns of 8 bits). Bit-exact with the recorded reference.
//! 2. 352.8 kHz to the output rate: rubato's windowed-sinc decimator (squared
//!    Blackman-Harris, linear phase, exact integer positions), sized to hold
//!    SoX VHQ's bands (linear phase, 175 dB): DSD64 to 88.2 kHz, flat to 30 kHz; DSD128 to
//!    176.4 kHz, flat to 50 and closed from 70 kHz; DSD256 and DSD512 to
//!    176.4 kHz, flat to 70 kHz. See [`Stage2Design`].
//!
//! The level is kept: a stream of all ones is PCM full scale (the SACD 0 dB
//! reference, 50 % modulation, is -6 dBFS), so nothing clips. The output is
//! time-aligned with the DSD (both stages are linear phase and their delay is
//! taken out), its length is the file's, and a seek lands on the same samples
//! as playing from the start.

use std::f64::consts::PI;
use std::path::Path;

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{
    Async, FixedAsync, Indexing, Resampler, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};

use crate::decoder::DsdDecoder;
use crate::{Decoder, DsdError, DsdMode, DsdPacking, Result, SILENCE_BYTE, Source, StreamInfo, double_to_s32};

pub const STOPBAND_DB: f64 = 160.0;
const BLOCK_WORDS: usize = 4096; // DSD read per step: 16 KB per channel
const SEEK_PREROLL: u64 = 8192; // intermediate samples run before a seek's target (23 ms)

/// DSD bytes per intermediate sample: the intermediate rate is the DSD rate /
/// 8k, 352.8 or 384 kHz (DSD on the 44.1 or the 48 kHz family), k a power
/// of two. Rounding against 352.8 kHz alone gave DSD512 on the 48 kHz family
/// (24.576 MHz) k = 9: 341.3 kHz, and a 170.7 kHz output.
fn bytes_per_sample(dsd_rate: u32) -> u32 {
    let base = if dsd_rate % 44_100 == 0 { 352_800 } else { 384_000 };
    let k = ((dsd_rate / 8 + base / 2) / base).max(1);
    // The nearest power of two at or below (exact on both families).
    1 << (31 - k.leading_zeros())
}

fn bessel_i0(x: f64) -> f64 {
    let (mut sum, mut term) = (1.0f64, 1.0f64);
    for k in 1..200 {
        term *= (x / (2.0 * k as f64)) * (x / (2.0 * k as f64));
        sum += term;
        if term < sum * 1e-17 {
            break;
        }
    }
    sum
}

/// The second stage's band edges in Hz (flat to `pass_hz`, closed from `stop_hz`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Band {
    pub pass_hz: f64,
    pub stop_hz: f64,
}

/// How stage 2 is built with rubato for a DSD rate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stage2Design {
    /// 352800 (or 384000).
    pub input_rate: u32,
    pub output_rate: u32,
    pub band: Band,
    /// Taps, at the input rate (rubato's `sinc_len`).
    pub sinc_len: usize,
    /// The -6 dB point relative to the output's Nyquist (rubato's `f_cutoff`).
    pub f_cutoff: f32,
}

/// Transition width (cycles per sample) × length of a squared Blackman-Harris
/// windowed sinc giving ≥ 175 dB from the stop edge (soxr VHQ's figure) and
/// < 0.0001 dB of pass-band deviation (measured in `stage2_bands`).
const BH2_WIDTH_TAPS: f64 = 21.0;

impl Stage2Design {
    pub fn for_rate(dsd_rate: u32) -> Self {
        let k = bytes_per_sample(dsd_rate);
        let input_rate = dsd_rate / (8 * k);
        let output_rate = DsdToPcm::output_rate(dsd_rate);
        let band = DsdToPcm::second_stage(dsd_rate);
        let nyquist = output_rate as f64 / 2.0;
        let width = (band.stop_hz - band.pass_hz) / input_rate as f64; // cycles per input sample
        let sinc_len = ((BH2_WIDTH_TAPS / width).ceil() as usize).next_multiple_of(8);
        let f_cutoff = (((band.pass_hz + band.stop_hz) / 2.0) / nyquist) as f32;
        Stage2Design { input_rate, output_rate, band, sinc_len, f_cutoff }
    }
}

/// rubato's decimator with the delay taken out and a drain, like the
/// engine's resampler (sonkore-dsp): interleaved f64 in, out.
struct Stage2 {
    rs: Async<f64>,
    ch: usize,
    ratio: u64, // input frames per output frame
    chunk: usize,
    input: Vec<f64>,
    scratch: Vec<f64>,
    trim: usize,
    fed: u64,
    produced: u64,
}

impl Stage2 {
    fn new(design: &Stage2Design, channels: usize) -> Result<Self> {
        let params = SincInterpolationParameters {
            sinc_len: design.sinc_len,
            f_cutoff: Some(design.f_cutoff),
            oversampling_factor: 1, // integer ratio: every output sits on an input sample
            interpolation: SincInterpolationType::Nearest,
            window: WindowFunction::BlackmanHarris2,
        };
        let ratio = design.input_rate / design.output_rate;
        let chunk = 4096;
        let rs = Async::<f64>::new_sinc(1.0 / ratio as f64, 1.0, &params, chunk, channels, FixedAsync::Input)
            .map_err(|e| DsdError(format!("Resampler: {e}")))?;
        let scratch = vec![0.0; rs.output_frames_max() * channels];
        let trim = rs.output_delay();
        let mut s = Stage2 {
            rs,
            ch: channels,
            ratio: ratio as u64,
            chunk,
            input: Vec::new(),
            scratch,
            trim,
            fed: 0,
            produced: 0,
        };
        s.clear();
        Ok(s)
    }

    /// rubato's first output after its `output_delay` stands for input
    /// sample `ratio + 1` (its index starts at `1 - sinc_len` and steps
    /// before the first dot product); that many zeros ahead of the input put
    /// output frame m on input sample `m * ratio`, as soxr does.
    fn lead(&self) -> usize {
        self.ratio as usize + 1
    }

    fn clear(&mut self) {
        self.rs.reset();
        self.input.clear();
        self.input.resize(self.lead() * self.ch, 0.0);
        self.trim = self.rs.output_delay();
        self.fed = 0;
        self.produced = 0;
    }

    fn run(&mut self, offset: usize, partial: Option<usize>, out: &mut Vec<f64>) -> Result<()> {
        let ch = self.ch;
        let frames = partial.unwrap_or(self.chunk);
        let input = InterleavedSlice::new(&self.input[offset * ch..offset * ch + frames * ch], ch, frames)
            .map_err(|e| DsdError(format!("Resampler: {e}")))?;
        let max = self.scratch.len() / ch;
        let mut output =
            InterleavedSlice::new_mut(&mut self.scratch, ch, max).map_err(|e| DsdError(format!("Resampler: {e}")))?;
        let indexing = partial.map(|n| Indexing::new().partial_len(n));
        let (_, produced) = self
            .rs
            .process_into_buffer(&input, &mut output, indexing.as_ref())
            .map_err(|e| DsdError(format!("Resampler: {e}")))?;
        let skip = self.trim.min(produced);
        self.trim -= skip;
        out.extend_from_slice(&self.scratch[skip * ch..produced * ch]);
        self.produced += (produced - skip) as u64;
        Ok(())
    }

    fn process(&mut self, x: &[f64], out: &mut Vec<f64>) -> Result<()> {
        self.input.extend_from_slice(x);
        self.fed += (x.len() / self.ch) as u64;
        let mut offset = 0;
        while self.input.len() / self.ch - offset >= self.chunk {
            self.run(offset, None, out)?;
            offset += self.chunk;
        }
        self.input.drain(..offset * self.ch);
        Ok(())
    }

    /// End of input: everything still held, up to the input's length.
    fn drain(&mut self, out: &mut Vec<f64>) -> Result<()> {
        let target = self.fed.div_ceil(self.ratio);
        let start = out.len();
        let pending = self.input.len() / self.ch;
        if pending > 0 {
            self.run(0, Some(pending), out)?;
            self.input.clear();
        }
        while self.produced < target {
            self.run(0, Some(0), out)?;
        }
        let excess = (self.produced - target) as usize;
        out.truncate(out.len() - excess.min((out.len() - start) / self.ch) * self.ch);
        self.produced = target;
        Ok(())
    }
}

/// DSD to PCM: f64 at 88.2 kHz (DSD64) or 176.4 kHz.
pub struct DsdToPcm {
    dsd: DsdDecoder,
    info: StreamInfo,
    channels: usize,
    bytes_per_sample: u32, // DSD bytes per intermediate sample (1 at DSD64, 8 at DSD512)
    ratio: u32,            // intermediate samples per output frame
    table_bytes: u32,      // filter length in bytes (taps / 8)
    intermediate_total: u64,
    table: Vec<f64>, // [table_bytes][256]

    // DSD bytes of each channel, absolute byte `buffer_start` first.
    bytes: Vec<Vec<u8>>,
    buffer_start: i64,
    next_intermediate: u64, // the next intermediate sample to compute
    input_ended: bool,
    drained: bool,
    words: Vec<i32>,
    mid: Vec<f64>,

    stage2: Option<Stage2>, // None: the intermediate samples themselves (tests)
    pending: Vec<f64>,      // converted output not handed out yet (1.0 = full scale)
    pending_pos: usize,
    discard: u64,  // output frames to drop (the pre-roll of a seek)
    position: u64, // output frames handed out
}

trait OutSample: Copy + Default {
    fn from_f64(v: f64) -> Self;
}

impl OutSample for f64 {
    #[inline]
    fn from_f64(v: f64) -> Self {
        v
    }
}

impl OutSample for i32 {
    #[inline]
    fn from_f64(v: f64) -> Self {
        double_to_s32(v)
    }
}

impl DsdToPcm {
    pub const STOPBAND_DB: f64 = STOPBAND_DB;

    pub fn open(path: &Path) -> Result<Self> {
        Self::with_decoder(DsdDecoder::open(path, DsdPacking::NativeU32Be)?, true)
    }

    pub fn from_source(source: Box<dyn Source>) -> Result<Self> {
        Self::with_decoder(DsdDecoder::from_source(source, DsdPacking::NativeU32Be)?, true)
    }

    /// Stage 1 alone: the 352.8 kHz intermediate samples (for comparisons).
    #[doc(hidden)]
    pub fn open_intermediate(path: &Path) -> Result<Self> {
        Self::with_decoder(DsdDecoder::open(path, DsdPacking::NativeU32Be)?, false)
    }

    fn with_decoder(dsd: DsdDecoder, stage2: bool) -> Result<Self> {
        let format = dsd.format();
        let dsd_rate = format.dsd_rate;
        let channels = format.channels as usize;
        let bytes_per_sample = bytes_per_sample(dsd_rate);
        // The DSD reader pads its last word with silence: the file's own length.
        let intermediate_total = format.bytes_per_channel / bytes_per_sample as u64;

        let h = Self::first_stage(dsd_rate);
        let table_bytes = (h.len() / 8) as u32;
        let mut table = vec![0.0f64; table_bytes as usize * 256];
        for b in 0..table_bytes as usize {
            for v in 0..256usize {
                let mut s = 0.0f64;
                for j in 0..8 {
                    // MSB first: the oldest bit meets the first tap
                    s += (if (v >> (7 - j)) & 1 != 0 { 1.0 } else { -1.0 }) * h[b * 8 + j];
                }
                table[b * 256 + v] = s;
            }
        }

        let mid = dsd_rate / (8 * bytes_per_sample);
        let (stage2, out_rate) = if stage2 {
            let design = Stage2Design::for_rate(dsd_rate);
            (Some(Stage2::new(&design, channels)?), design.output_rate)
        } else {
            (None, mid)
        };
        let ratio = mid / out_rate;
        let dst = dsd.is_dst();
        let info = StreamInfo {
            rate: out_rate,
            channels,
            bits: 0,         // a conversion: no source depth to keep
            float: true,     // f64 samples
            lossless: false, // ... and nothing bit-perfect about it
            total_frames: intermediate_total / ratio as u64,
            codec: crate::rate_label(dsd_rate) + if dst { " DST" } else { "" },
            dsd: DsdMode::None,
            dsd_rate, // PCM made from DSD (the signal path says so)
            decoder: if dst { "DST decoder" } else { "DSD reader" },
        };
        let mut d = DsdToPcm {
            dsd,
            info,
            channels,
            bytes_per_sample,
            ratio,
            table_bytes,
            intermediate_total,
            table,
            bytes: vec![Vec::new(); channels],
            buffer_start: 0,
            next_intermediate: 0,
            input_ended: false,
            drained: false,
            words: Vec::new(),
            mid: Vec::new(),
            stage2,
            pending: Vec::new(),
            pending_pos: 0,
            discard: 0,
            position: 0,
        };
        d.restart(0)?;
        Ok(d)
    }

    /// The first stage's length for a DSD rate (a multiple of 16).
    pub fn first_stage_taps(dsd_rate: u32) -> usize {
        let fs = dsd_rate as f64;
        let mid = fs / (8.0 * bytes_per_sample(dsd_rate) as f64); // 352.8 kHz
        let pass = Self::output_rate(dsd_rate) as f64 / 2.0; // the output's Nyquist frequency
        let stop = mid - pass; // what folds onto it after decimation
        // Kaiser's estimate of the length.
        let taps = ((STOPBAND_DB - 7.95) / (14.36 * (stop - pass) / fs)).ceil() as usize + 1;
        taps.div_ceil(16) * 16 // whole bytes, and an even number of them
    }

    /// The rate it converts a DSD rate to: 88.2 kHz for DSD64, 176.4 kHz from
    /// DSD128 (96 / 192 kHz on the 48 kHz family).
    pub fn output_rate(dsd_rate: u32) -> u32 {
        let k = bytes_per_sample(dsd_rate);
        dsd_rate / (8 * k) / if k <= 1 { 4 } else { 2 }
    }

    /// The first stage's taps for a DSD rate (DC gain 1).
    pub fn first_stage(dsd_rate: u32) -> Vec<f64> {
        let fs = dsd_rate as f64;
        let mid = fs / (8.0 * bytes_per_sample(dsd_rate) as f64);
        let taps = Self::first_stage_taps(dsd_rate);
        let beta = 0.1102 * (STOPBAND_DB - 8.7);
        let cutoff = mid / 2.0 / fs; // cycles per bit, in the middle of the transition
        let centre = (taps as f64 - 1.0) / 2.0;
        let norm = bessel_i0(beta);
        let mut sum = 0.0;
        let mut h: Vec<f64> = (0..taps)
            .map(|n| {
                let t = n as f64 - centre;
                let x = 2.0 * PI * cutoff * t;
                let sinc = 2.0 * cutoff * if x == 0.0 { 1.0 } else { x.sin() / x };
                let r = t / centre;
                let v = sinc * bessel_i0(beta * (1.0 - r * r).max(0.0).sqrt()) / norm;
                sum += v;
                v
            })
            .collect();
        for v in &mut h {
            *v /= sum; // DC gain 1: all ones is full scale
        }
        h
    }

    /// The second stage's band edges.
    pub fn second_stage(dsd_rate: u32) -> Band {
        let k = bytes_per_sample(dsd_rate);
        let nyquist = Self::output_rate(dsd_rate) as f64 / 2.0;
        match k {
            0 | 1 => Band { pass_hz: 30_000.0, stop_hz: nyquist }, // DSD64: the noise rises from ~25 kHz
            2 => Band { pass_hz: 50_000.0, stop_hz: 70_000.0 },    // DSD128
            _ => Band { pass_hz: 70_000.0, stop_hz: nyquist },     // DSD256 and DSD512
        }
    }

    /// Positions the reader so the next sample out is intermediate sample `start`.
    fn restart(&mut self, start: u64) -> Result<()> {
        let half = (self.table_bytes / 2) as i64;
        let window = (start * self.bytes_per_sample as u64) as i64 - half; // first byte the sample reads
        let mut read = 0i64;
        for b in &mut self.bytes {
            b.clear();
        }
        if window < 0 {
            self.buffer_start = window;
            for b in &mut self.bytes {
                b.resize((-window) as usize, SILENCE_BYTE);
            }
        } else {
            read = window & !3; // whole 32-bit words
            self.buffer_start = read;
        }
        self.dsd.seek((read / 4) as u64)?;
        self.next_intermediate = start;
        self.input_ended = false;
        self.drained = false;
        if let Some(s) = &mut self.stage2 {
            s.clear();
        }
        self.pending.clear();
        self.pending_pos = 0;
        Ok(())
    }

    /// Converts the next block.
    fn produce(&mut self) -> Result<()> {
        let ch = self.channels;
        if !self.input_ended {
            self.words.resize(BLOCK_WORDS * ch, 0);
            let got = self.dsd.read_i32(&mut self.words)?;
            for (c, b) in self.bytes.iter_mut().enumerate() {
                b.reserve(got * 4);
                for f in 0..got {
                    b.extend_from_slice(&self.words[f * ch + c].to_ne_bytes()); // the word's bytes are in time order
                }
            }
            if got < BLOCK_WORDS {
                // The end: silence after it, enough for the last windows.
                self.input_ended = true;
                for b in &mut self.bytes {
                    b.resize(b.len() + self.table_bytes as usize + 8, SILENCE_BYTE);
                }
            }
        }

        // Every intermediate sample whose window is in.
        let half = (self.table_bytes / 2) as i64;
        let k = self.bytes_per_sample as u64;
        let end = self.buffer_start + self.bytes[0].len() as i64;
        let mut last = self.next_intermediate;
        while last < self.intermediate_total && (last * k) as i64 - half + self.table_bytes as i64 <= end {
            last += 1;
        }
        let count = (last - self.next_intermediate) as usize;
        self.mid.resize(count * ch, 0.0);
        let table_bytes = self.table_bytes as usize;
        for (c, bytes) in self.bytes.iter().enumerate() {
            for i in 0..count {
                let first = (((self.next_intermediate + i as u64) * k) as i64 - half - self.buffer_start) as usize;
                let p = &bytes[first..first + table_bytes];
                let t = &self.table[..];
                // Four running sums: independent additions the CPU overlaps.
                let (mut s0, mut s1, mut s2, mut s3) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
                let mut b = 0;
                while b + 4 <= table_bytes {
                    let tb = &t[b * 256..b * 256 + 1024];
                    s0 += tb[p[b] as usize];
                    s1 += tb[256 + p[b + 1] as usize];
                    s2 += tb[512 + p[b + 2] as usize];
                    s3 += tb[768 + p[b + 3] as usize];
                    b += 4;
                }
                while b < table_bytes {
                    s0 += t[b * 256 + p[b] as usize];
                    b += 1;
                }
                self.mid[i * ch + c] = (s0 + s1) + (s2 + s3);
            }
        }
        self.next_intermediate = last;
        // Bytes no window reads any more.
        let keep = (self.next_intermediate * k) as i64 - half;
        if keep > self.buffer_start {
            let drop = ((keep - self.buffer_start) as usize).min(self.bytes[0].len());
            for b in &mut self.bytes {
                b.drain(..drop);
            }
            self.buffer_start += drop as i64;
        }

        let at_end = self.next_intermediate >= self.intermediate_total;
        match &mut self.stage2 {
            Some(s) => {
                if count > 0 {
                    s.process(&self.mid, &mut self.pending)?;
                }
                if at_end {
                    s.drain(&mut self.pending)?;
                }
            }
            None => self.pending.extend_from_slice(&self.mid),
        }
        if at_end {
            self.drained = true;
        }
        Ok(())
    }

    fn read<T: OutSample>(&mut self, out: &mut [T]) -> Result<usize> {
        let ch = self.channels;
        let total = self.info.total_frames;
        let frames = ((out.len() / ch) as u64).min(total - total.min(self.position)) as usize;
        let mut done = 0usize;
        while done < frames {
            if self.pending_pos >= self.pending.len() {
                self.pending.clear();
                self.pending_pos = 0;
                if self.drained {
                    // The filters gave a frame or two fewer than the length: silence.
                    out[done * ch..frames * ch].fill(T::default());
                    done = frames;
                    break;
                }
                self.produce()?;
                continue;
            }
            let available = (self.pending.len() - self.pending_pos) / ch;
            if self.discard > 0 {
                let skip = self.discard.min(available as u64) as usize;
                self.pending_pos += skip * ch;
                self.discard -= skip as u64;
                continue;
            }
            let n = available.min(frames - done);
            for (o, &v) in out[done * ch..(done + n) * ch].iter_mut().zip(&self.pending[self.pending_pos..]) {
                *o = T::from_f64(v);
            }
            self.pending_pos += n * ch;
            done += n;
        }
        self.position += done as u64;
        Ok(done)
    }
}

impl Decoder for DsdToPcm {
    fn info(&self) -> &StreamInfo {
        &self.info
    }

    fn read_i32(&mut self, out: &mut [i32]) -> Result<usize> {
        self.read(out)
    }

    fn read_f64(&mut self, out: &mut [f64]) -> Result<usize> {
        self.read(out)
    }

    /// From a little before the target, so the filters hold the music that
    /// leads to it, then the pre-roll's output is dropped.
    fn seek(&mut self, frame: u64) -> Result<u64> {
        let frame = frame.min(self.info.total_frames);
        let target = frame * self.ratio as u64;
        let start = target.saturating_sub(SEEK_PREROLL); // a whole number of output frames before
        self.restart(start)?;
        self.discard = (target - start) / self.ratio as u64;
        self.position = frame;
        Ok(frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stage 2's whole impulse response at the input rate: an impulse at
    /// each of the `ratio` phases, the outputs interleaved back.
    fn stage2_impulse(dsd_rate: u32) -> (Stage2Design, Vec<f64>) {
        let design = Stage2Design::for_rate(dsd_rate);
        let ratio = (design.input_rate / design.output_rate) as usize;
        let n = 4 * design.sinc_len + 64;
        let centre = n / 2;
        let mut h = vec![0.0; n];
        for phase in 0..ratio {
            let mut s = Stage2::new(&design, 1).unwrap();
            let mut x = vec![0.0; n];
            x[centre + phase] = 1.0;
            let mut y = Vec::new();
            s.process(&x, &mut y).unwrap();
            s.drain(&mut y).unwrap();
            // y[m] = sum h[k] x[m*ratio - k]: tap (m*ratio - centre - phase).
            for (m, v) in y.iter().enumerate() {
                let k = (m * ratio) as isize - (centre + phase) as isize + centre as isize;
                if (0..n as isize).contains(&k) {
                    h[k as usize] = *v;
                }
            }
        }
        (design, h)
    }

    fn response_db(h: &[f64], f: f64, fs: f64) -> f64 {
        let (mut re, mut im) = (0.0, 0.0);
        for (k, v) in h.iter().enumerate() {
            let a = -2.0 * PI * f / fs * k as f64;
            re += v * a.cos();
            im += v * a.sin();
        }
        20.0 * (re * re + im * im).sqrt().max(1e-30).log10()
    }

    /// The bands rubato's filter keeps (soxr VHQ's: flat within
    /// ~0.01 dB to the pass edge, 175 dB from the stop edge).
    #[test]
    fn stage2_bands() {
        for rate in [2_822_400u32, 5_644_800, 11_289_600, 22_579_200] {
            let (d, h) = stage2_impulse(rate);
            let fs = d.input_rate as f64;
            let dc: f64 = h.iter().sum::<f64>();
            // Linear phase: symmetric about the centre sample.
            let c = h.len() / 2;
            let asym = (1..d.sinc_len / 2 - 1).map(|k| (h[c + k] - h[c - k]).abs()).fold(0.0, f64::max);
            let mut pass_dev = 0.0f64;
            let mut f = 0.0;
            while f <= d.band.pass_hz {
                pass_dev = pass_dev.max(response_db(&h, f, fs).abs());
                f += 250.0;
            }
            let mut stop = f64::MIN;
            let mut f = d.band.stop_hz;
            while f <= fs / 2.0 {
                stop = stop.max(response_db(&h, f, fs));
                f += 125.0;
            }
            eprintln!(
                "DSD{}: {} taps, cutoff {:.4}, DC {:.12}, pass ±{:.2e} dB to {} Hz, stop {:.1} dB from {} Hz, asym {:.1e}",
                rate / 44100,
                d.sinc_len,
                d.f_cutoff,
                dc,
                pass_dev,
                d.band.pass_hz,
                stop,
                d.band.stop_hz,
                asym
            );
            assert!((dc - 1.0).abs() < 1e-9);
            assert!(pass_dev < 1e-4, "pass band");
            assert!(stop < -175.0, "stop band");
            assert!(asym < 1e-12, "linear phase");
        }
    }

    #[test]
    fn every_dsd_rate_lands_on_its_family() {
        // DSD64 … DSD1024 on both families: the intermediate rate is 352.8
        // or 384 kHz exactly, and the output an integer division of it.
        for (multiple, out44, out48) in
            [(64u32, 88_200u32, 96_000u32), (128, 176_400, 192_000), (256, 176_400, 192_000), (512, 176_400, 192_000)]
                .into_iter()
                .chain([(1024, 176_400, 192_000)])
        {
            for (dsd_rate, mid, out) in [(44_100 * multiple, 352_800, out44), (48_000 * multiple, 384_000, out48)] {
                let k = bytes_per_sample(dsd_rate);
                assert!(k.is_power_of_two(), "DSD{multiple} at {dsd_rate}: {k} bytes per sample");
                assert_eq!(dsd_rate / (8 * k), mid, "DSD{multiple} at {dsd_rate}");
                assert_eq!(dsd_rate % (8 * k), 0, "DSD{multiple} at {dsd_rate}");
                assert_eq!(DsdToPcm::output_rate(dsd_rate), out, "DSD{multiple} at {dsd_rate}");
                let design = Stage2Design::for_rate(dsd_rate);
                assert_eq!((design.input_rate, design.output_rate), (mid, out));
                assert_eq!(mid % out, 0);
            }
        }
    }

    #[test]
    fn rates_and_lengths() {
        assert_eq!(DsdToPcm::output_rate(2_822_400), 88_200);
        assert_eq!(DsdToPcm::output_rate(5_644_800), 176_400);
        assert_eq!(DsdToPcm::output_rate(11_289_600), 176_400);
        assert_eq!(DsdToPcm::output_rate(22_579_200), 176_400);
        assert_eq!(DsdToPcm::output_rate(3_072_000), 96_000);
        assert_eq!(DsdToPcm::first_stage_taps(2_822_400), 128);
        for rate in [2_822_400, 5_644_800, 11_289_600, 22_579_200] {
            let h = DsdToPcm::first_stage(rate);
            assert_eq!(h.len() % 16, 0);
            let dc: f64 = h.iter().sum();
            assert!((dc - 1.0).abs() < 1e-12);
            // Symmetric (linear phase).
            for i in 0..h.len() / 2 {
                assert!((h[i] - h[h.len() - 1 - i]).abs() < 1e-18);
            }
        }
    }
}
