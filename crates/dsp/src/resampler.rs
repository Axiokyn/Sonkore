//! Sample-rate conversion (in place of libsoxr's "very high quality"),
//! built on rubato (linear phase) and on a polyphase FIR of its own
//! (minimum and intermediate phase), double in and out, interleaved.
//!
//! The engine builds one only when it is needed (an output that refuses the
//! track's rate, upsampling, PCM→DSD, the convolver's impulse response at
//! another rate), so a bit-perfect session carries none.
//!
//! **How it converts.** [`Resampler::new`] uses rubato's synchronous FFT
//! resampler: blocks of `k·in/g` input frames (`g` = gcd of the rates) are
//! transformed, multiplied by a windowed-sinc anti-alias filter of the same
//! length (Blackman-Harris², the window rubato is built around) and
//! transformed back at the output size. The filter's length sets its
//! steepness, so `k` is chosen for the filter's length *at the lower of the
//! two rates*: at least [`LINEAR_TAPS`] there for [`Filter::Linear`] and
//! [`STEEP_TAPS`] for [`Filter::LinearSteep`]. The stop band starts at the
//! lower Nyquist.
//!
//! Measured with sines (tests/resampler_quality.rs and, for soxr, reference
//! values recorded earlier), as fractions of the lower Nyquist:
//!
//! | | ripple to 0.9 | flat (±0.1 dB) to | stop band from 1.005 |
//! |---|---|---|---|
//! | soxr VHQ linear | 2–5·10⁻³ dB | 0.925 | 182 dB |
//! | this, `Linear` | < 3·10⁻⁹ dB | 0.945–0.965 | 184–215 dB |
//! | soxr VHQ steep | 1·10⁻³ dB | 0.985 | 189 dB |
//! | this, `LinearSteep` | < 3·10⁻¹¹ dB | 0.985 | 192–195 dB |
//!
//! On music-like material inside both pass bands the output differs from
//! soxr's by 79–93 dB SNR at rational ratios (soxr's own ripple) and by
//! 185–200 dB at integer ones.
//!
//! [`Resampler::new_lowpass`] (DSD→PCM's second stage) uses rubato's sinc
//! interpolator instead, whose cut-off can be placed anywhere: a filter flat
//! to `passband_end` and fully attenuated from `stopband_begin` (fractions of
//! the lower rate's Nyquist), computed exactly at each output position (the
//! "nearest" mode with one sub-filter per output phase, so no interpolation
//! error).
//!
//! **Timing.** As with soxr the output starts at the input's first frame (the
//! filter's delay is taken off), and after [`Resampler::drain`] the output
//! holds exactly `round(input frames · out / in)` frames, the count soxr gives.
//! [`Resampler::delay_frames`] is soxr_delay(): the output frames still owed
//! for the input taken so far (filter delay plus what is buffered).
//!
//! **Phase.** rubato's filters are linear phase only, so [`Filter::Minimum`]
//! and [`Filter::Intermediate`] use the polyphase FIR of
//! `polyphase.rs`: a Kaiser-windowed sinc with the linear filter's
//! specification (flat to 0.96 of the lower Nyquist, ~190 dB down from it)
//! turned minimum phase by the cepstral method, or halfway between that and
//! linear phase (soxr's intermediate phase). Measured as above: ripple
//! < 2·10⁻⁷ dB to 0.9, flat to 0.965, stop band 191–195 dB; up to 0.9 they
//! differ from `Linear` by < 2·10⁻⁷ dB. A click's energy more than 8
//! lower-rate samples before its peak: `Linear` 4–6·10⁻³ (−23 dB),
//! intermediate 7–10·10⁻⁶ (−51 dB), minimum none. The peak is the alignment
//! point (a click peaks on its own time), so the timing rules hold the same;
//! the look-ahead is 9 input frames for minimum phase and ~440 for
//! intermediate (×in/out when downsampling). The FIR costs its length per
//! output frame, so past 16× (PCM→DSD's rates) it only doubles the rate and
//! `Linear` does the rest; that filter's transition is at the input rate,
//! where nothing is left to ring (a click's energy ahead of its peak:
//! 3·10⁻¹⁹). Speed on one core, stereo (`tests/bench.rs`): 44.1 → 48 kHz
//! minimum 154× real time; 44.1 → 192 kHz minimum 37×, intermediate 23×
//! (`Linear` 567×); 44.1 → 705.6 kHz 11×; DSD64's rate 29× (`Linear` 36×),
//! DSD512's 3×.
//!
//! Real-time use: everything is allocated by the constructor; `process` and
//! `drain` only write into the caller's `Vec`, which does not reallocate once
//! it has the capacity [`Resampler::max_output_frames`] gives for the block.

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{
    Async, Fft, FixedAsync, FixedSync, Resampler as _, SincInterpolationParameters, SincInterpolationType,
    WindowFunction,
};

use crate::polyphase::{self, Phase, Polyphase};

/// The filter's phase response and steepness (`playback/resampleFilter` 0–3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Filter {
    /// Symmetric: pre- and post-ringing. Flat to ~0.95 of Nyquist.
    #[default]
    Linear,
    /// soxr's intermediate phase: the phase halfway between linear and
    /// minimum, a little pre-ringing (−51 dB of a click's energy).
    Intermediate,
    /// Minimum phase: no pre-ringing, all the ringing after the peak.
    Minimum,
    /// Linear with a steeper transition: flat to ~0.985 of Nyquist
    /// (21.7 kHz at 44.1 kHz instead of ~21 kHz).
    LinearSteep,
}

impl Filter {
    /// From the settings' index (0 linear, 1 intermediate, 2 minimum, 3 steep).
    pub fn from_index(i: i64) -> Self {
        match i {
            1 => Filter::Intermediate,
            2 => Filter::Minimum,
            3 => Filter::LinearSteep,
            _ => Filter::Linear,
        }
    }
}

/// Filter length at the lower rate for [`Filter::Linear`]: flat to 0.945 of
/// Nyquist, 184 dB down from 1.005 (SoX VHQ: flat to 0.913, 175 dB).
pub const LINEAR_TAPS: usize = 400;
/// Filter length at the lower rate for [`Filter::LinearSteep`]: flat to
/// 0.985 of Nyquist (SoX steep: 0.982).
pub const STEEP_TAPS: usize = 2048;
/// The most channels a resampler takes.
pub const MAX_CHANNELS: usize = 8;
/// Input frames per call of the sinc (low-pass) engine.
const SINC_CHUNK: usize = 1024;
/// The largest sub-filter table the exact sinc mode builds (phases × taps).
const SINC_TABLE_MAX: usize = 1 << 22;

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// `x⁻¹ mod m` for coprime `x`, `m` (m ≥ 1).
fn mod_inverse(x: u64, m: u64) -> u64 {
    if m == 1 {
        return 0;
    }
    let (mut old_r, mut r) = (x as i128 % m as i128, m as i128);
    let (mut old_s, mut s) = (1i128, 0i128);
    while r != 0 {
        let q = old_r / r;
        (old_r, r) = (r, old_r - q * r);
        (old_s, s) = (s, old_s - q * s);
    }
    old_s.rem_euclid(m as i128) as u64
}

pub struct Resampler {
    engine: Engine,
    channels: usize,
    in_rate: u32,
    out_rate: u32,
    /// in / gcd and out / gcd: the exact ratio is b / a.
    a: u64,
    b: u64,
    in_total: u64,
    out_total: u64,
    drained: bool,
}

/// What converts: rubato (linear phase) or the polyphase FIR (minimum and
/// intermediate phase).
enum Engine {
    Rubato(Rubato),
    Fir(Polyphase),
    /// Upsampling by more than [`FIR_RATIO_MAX`] (PCM→DSD): the FIR to twice
    /// the input rate, then rubato's linear filter the rest of the way. The
    /// FIR costs its length per output frame, which at the 1-bit rates would
    /// not keep up; the second filter's transition is up at the input rate,
    /// where the first one left nothing (190 dB down) to ring.
    TwoStage {
        fir: Box<Resampler>,
        rest: Box<Resampler>,
        mid: Vec<f64>,
    },
}

/// The largest upsampling the polyphase FIR does in one stage (44.1 → 705.6
/// kHz).
const FIR_RATIO_MAX: u64 = 16;
/// Input frames the two-stage converter passes through at a time.
const STAGE_CHUNK: usize = 1024;

struct Rubato {
    engine: Box<dyn rubato::Resampler<f64>>,
    /// Input frames the engine takes per call (always the same).
    block_in: usize,
    /// The most output frames one engine call gives.
    block_out_max: usize,
    /// Input gathered for the next engine call (block_in frames).
    fifo: Vec<f64>,
    fifo_frames: usize,
    /// One engine call's output.
    scratch: Vec<f64>,
    /// Silent frames put before the input so the output grid lands on it.
    pad: usize,
    /// Leading output frames to drop (the filter's delay).
    skip: usize,
    skip_left: usize,
}

impl Resampler {
    /// A converter from `in_rate` to `out_rate` (soxr VHQ with the phase
    /// response of `filter`).
    pub fn new(in_rate: u32, out_rate: u32, channels: usize, filter: Filter) -> Result<Self, String> {
        Self::check(in_rate, out_rate, channels)?;
        let g = gcd(in_rate as u64, out_rate as u64);
        let (a, b) = (in_rate as u64 / g, out_rate as u64 / g);
        let taps = match filter {
            Filter::LinearSteep => STEEP_TAPS,
            _ => LINEAR_TAPS,
        };
        // k sub-blocks of the minimal (a in, b out) block, so the filter has
        // at least `taps` taps at the lower rate; k even, so the filter's
        // centre (half the block) is a whole number of output frames.
        let low = a.min(b) as usize;
        let k = taps.div_ceil(low).next_multiple_of(2);
        if let Some(phase) = match filter {
            Filter::Minimum => Some(Phase::Minimum),
            Filter::Intermediate => Some(Phase::Intermediate),
            _ => None,
        } {
            if b > FIR_RATIO_MAX * a {
                let fir = Box::new(Self::new(in_rate, in_rate * 2, channels, filter)?);
                let rest = Box::new(Self::new(in_rate * 2, out_rate, channels, Filter::Linear)?);
                let mid = Vec::with_capacity(fir.max_output_frames(STAGE_CHUNK) * channels);
                return Ok(Self::build(Engine::TwoStage { fir, rest, mid }, in_rate, out_rate, channels, a, b));
            }
            let fir = Polyphase::new(polyphase::kernel(phase), a, b, channels);
            return Ok(Self::build(Engine::Fir(fir), in_rate, out_rate, channels, a, b));
        }
        let fft_in = k * a as usize;
        let fft_out = k * b as usize;
        let engine = Fft::<f64>::new_custom(
            in_rate as usize,
            out_rate as usize,
            fft_in,
            1,
            channels,
            WindowFunction::BlackmanHarris2,
            FixedSync::Both,
        )
        .map_err(|e| format!("Resampler: {e}"))?;
        debug_assert_eq!(engine.input_frames_next(), fft_in);
        debug_assert_eq!(engine.output_frames_next(), fft_out);
        let skip = engine.output_delay();
        Ok(Self::build(Rubato::new(Box::new(engine), channels, 0, skip), in_rate, out_rate, channels, a, b))
    }

    /// A linear-phase low-pass of its own (DSD→PCM's second stage): flat to
    /// `passband_end` and fully attenuated from `stopband_begin`, both as
    /// fractions of the lower rate's Nyquist frequency.
    pub fn new_lowpass(
        in_rate: u32,
        out_rate: u32,
        channels: usize,
        passband_end: f64,
        stopband_begin: f64,
    ) -> Result<Self, String> {
        Self::check(in_rate, out_rate, channels)?;
        if !(passband_end > 0.0 && passband_end < stopband_begin && stopband_begin <= 1.0) {
            return Err("Resampler: the pass band must end before the stop band begins".into());
        }
        let g = gcd(in_rate as u64, out_rate as u64);
        let (a, b) = (in_rate as u64 / g, out_rate as u64 / g);
        let ratio = out_rate as f64 / in_rate as f64;
        // A Blackman-Harris² windowed sinc of N taps (counted at the lower
        // rate) stays within 0.1 dB up to ~8.5 / N of Nyquist below its
        // cut-off and is 175 dB down from ~18 / N above it (measured): N =
        // 32 / transition with the cut-off a third of the way into the
        // transition leaves a margin on both sides. The filter runs at the
        // input rate, so its length there is N / ratio when downsampling.
        let width = stopband_begin - passband_end;
        let low_taps = (32.0 / width).ceil();
        let sinc_len = ((low_taps / ratio.min(1.0)).ceil() as usize).next_multiple_of(8).max(8);
        let cutoff = (passband_end + width / 3.0) as f32;
        // One sub-filter per output phase (b of them) computes every output
        // exactly; a very fine ratio falls back to cubic interpolation.
        let exact = b as usize * sinc_len <= SINC_TABLE_MAX;
        let params = SincInterpolationParameters {
            sinc_len,
            f_cutoff: Some(cutoff),
            oversampling_factor: if exact { b as usize } else { 512 },
            interpolation: if exact { SincInterpolationType::Nearest } else { SincInterpolationType::Cubic },
            window: WindowFunction::BlackmanHarris2,
        };
        // rubato's sinc engine puts output j (from 0) at input time
        // (j + 1) / ratio - len / 2 + 1 / factor. With `pad` silent frames
        // first and `skip` outputs dropped, output 0 lands on input 0 when
        // (skip + 1)·a = (len/2 + pad)·b - 1 (exact mode, factor = b).
        let half = (sinc_len / 2) as u64;
        let (pad, skip) = if exact {
            // (half + pad)·b ≡ 1 (mod a)
            let want = mod_inverse(b % a, a);
            let mut pad = (want + a - half % a) % a;
            while ((half + pad) * b - 1) / a < 1 {
                pad += a;
            }
            (pad as usize, (((half + pad) * b - 1) / a - 1) as usize)
        } else {
            (0, ((half as f64 * ratio) - 1.0).round().max(0.0) as usize)
        };
        // The lead-in goes through the FIFO in one block.
        let chunk = SINC_CHUNK.max(pad);
        let engine = Async::<f64>::new_sinc(ratio, 1.0, &params, chunk, channels, FixedAsync::Input)
            .map_err(|e| format!("Resampler: {e}"))?;
        Ok(Self::build(Rubato::new(Box::new(engine), channels, pad, skip), in_rate, out_rate, channels, a, b))
    }

    fn check(in_rate: u32, out_rate: u32, channels: usize) -> Result<(), String> {
        if in_rate == 0 || out_rate == 0 {
            return Err("Resampler: invalid sample rate".into());
        }
        if channels == 0 || channels > MAX_CHANNELS {
            return Err("Resampler: 1 to 8 channels".into());
        }
        Ok(())
    }

    fn build(engine: Engine, in_rate: u32, out_rate: u32, channels: usize, a: u64, b: u64) -> Self {
        let mut r = Resampler { engine, channels, in_rate, out_rate, a, b, in_total: 0, out_total: 0, drained: false };
        r.clear();
        r
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    pub fn in_rate(&self) -> u32 {
        self.in_rate
    }

    pub fn out_rate(&self) -> u32 {
        self.out_rate
    }

    /// out / in.
    pub fn ratio(&self) -> f64 {
        self.b as f64 / self.a as f64
    }

    /// An upper bound of the frames one `process` call with `input_frames`
    /// frames (or a `drain`) writes: reserve this much to keep the path free
    /// of allocations.
    pub fn max_output_frames(&self, input_frames: usize) -> usize {
        match &self.engine {
            Engine::Rubato(r) => {
                let calls = (r.fifo_frames.max(r.block_in) + input_frames).div_ceil(r.block_in) + 1;
                let drain_calls = (r.skip + r.block_out_max).div_ceil(r.block_out_max.max(1)) + 2;
                calls.max(drain_calls) * r.block_out_max
            }
            // Every output the input completes, and the look-ahead's when
            // draining.
            Engine::Fir(f) => ((input_frames + f.lead() + 2) as u64 * self.b).div_ceil(self.a) as usize + 2,
            Engine::TwoStage { fir, rest, .. } => {
                rest.max_output_frames(fir.max_output_frames(input_frames)) + rest.max_output_frames(0)
            }
        }
    }

    /// Converts interleaved frames; `output` is cleared and gets what is
    /// ready. Returns the frames written.
    pub fn process(&mut self, input: &[f64], output: &mut Vec<f64>) -> usize {
        output.clear();
        self.feed(input, output);
        output.len() / self.channels
    }

    /// `process`, appending to `output`.
    fn feed(&mut self, input: &[f64], output: &mut Vec<f64>) {
        let ch = self.channels;
        let rest = &input[..input.len() / ch * ch];
        self.in_total += (rest.len() / ch) as u64;
        self.drained = false;
        match &mut self.engine {
            Engine::Rubato(r) => {
                let mut rest = rest;
                while !rest.is_empty() {
                    let room = (r.block_in - r.fifo_frames) * ch;
                    let n = room.min(rest.len());
                    let at = r.fifo_frames * ch;
                    r.fifo[at..at + n].copy_from_slice(&rest[..n]);
                    r.fifo_frames += n / ch;
                    rest = &rest[n..];
                    if r.fifo_frames == r.block_in {
                        self.out_total += r.run(ch, output, u64::MAX, self.out_total);
                    }
                }
            }
            Engine::Fir(f) => self.out_total += f.push(Some(rest), rest.len() / ch, output, u64::MAX),
            Engine::TwoStage { fir, rest: next, mid } => {
                let before = output.len();
                for chunk in rest.chunks(STAGE_CHUNK * ch) {
                    mid.clear();
                    fir.feed(chunk, mid);
                    next.feed(mid, output);
                }
                self.out_total += ((output.len() - before) / ch) as u64;
            }
        }
    }

    /// End of input: everything still owed, up to exactly
    /// `round(input · out / in)` frames in all. Returns the frames written;
    /// 0 once drained (so `while drain() > 0` loops end as with soxr).
    pub fn drain(&mut self, output: &mut Vec<f64>) -> usize {
        output.clear();
        self.finish(output);
        output.len() / self.channels
    }

    /// `drain`, appending to `output`.
    fn finish(&mut self, output: &mut Vec<f64>) {
        if self.drained {
            return;
        }
        let ch = self.channels;
        // round(in · b / a), exactly.
        let target = ((self.in_total as u128 * self.b as u128 * 2 + self.a as u128) / (2 * self.a as u128)) as u64;
        match &mut self.engine {
            Engine::Rubato(r) => {
                // (Bounded: each call gives output once the filter is primed.)
                let mut calls = (r.skip + r.block_in * 2) / r.block_out_max.max(1) + 8;
                while self.out_total < target && calls > 0 {
                    r.fifo[r.fifo_frames * ch..].fill(0.0);
                    r.fifo_frames = r.block_in;
                    self.out_total += r.run(ch, output, target, self.out_total);
                    calls -= 1;
                }
            }
            Engine::Fir(f) => {
                // Silence past the end, as far as the last output looks.
                let owed = target.saturating_sub(self.out_total);
                self.out_total += f.push(None, f.lead() + 2, output, owed);
            }
            Engine::TwoStage { fir, rest, mid } => {
                // Both stages keep the length rule, so the second ends on
                // round(in · out / in) too.
                let before = output.len();
                mid.clear();
                fir.finish(mid);
                rest.feed(mid, output);
                rest.finish(output);
                self.out_total += ((output.len() - before) / ch) as u64;
            }
        }
        self.drained = true;
    }

    /// Back to the start: no input held, the filter's delay to drop again.
    pub fn clear(&mut self) {
        match &mut self.engine {
            Engine::Rubato(r) => r.clear(),
            Engine::Fir(f) => f.clear(),
            Engine::TwoStage { fir, rest, .. } => {
                fir.clear();
                rest.clear();
            }
        }
        self.in_total = 0;
        self.out_total = 0;
        self.drained = false;
    }

    /// Output frames the converter owes for the input taken so far (its
    /// filter's delay plus what is buffered): heard later than the input
    /// that went in. soxr_delay().
    pub fn delay_frames(&self) -> f64 {
        if self.drained {
            return 0.0;
        }
        self.in_total as f64 * self.ratio() - self.out_total as f64
    }

    /// The same delay in input frames (for position arithmetic).
    pub fn delay_input_frames(&self) -> f64 {
        self.delay_frames() / self.ratio()
    }
}

impl Rubato {
    fn new(engine: Box<dyn rubato::Resampler<f64>>, channels: usize, pad: usize, skip: usize) -> Engine {
        let block_in = engine.input_frames_max();
        let block_out_max = engine.output_frames_max();
        Engine::Rubato(Rubato {
            engine,
            block_in,
            block_out_max,
            fifo: vec![0.0; block_in * channels],
            fifo_frames: 0,
            scratch: vec![0.0; block_out_max * channels],
            pad,
            skip,
            skip_left: skip,
        })
    }

    /// One engine call over the full FIFO; what it gives (past the frames to
    /// skip, and up to `limit` in all, `out_total` given so far) is appended
    /// to `output`. Returns the frames appended.
    fn run(&mut self, ch: usize, output: &mut Vec<f64>, limit: u64, out_total: u64) -> u64 {
        let produced = {
            let input = InterleavedSlice::new(&self.fifo[..], ch, self.block_in).expect("resampler input");
            let mut out =
                InterleavedSlice::new_mut(&mut self.scratch[..], ch, self.block_out_max).expect("resampler output");
            match self.engine.process_into_buffer(&input, &mut out, None) {
                Ok((_, written)) => written,
                Err(_) => 0,
            }
        };
        self.fifo_frames = 0;
        let mut from = 0;
        if self.skip_left > 0 {
            from = self.skip_left.min(produced);
            self.skip_left -= from;
        }
        let keep = ((produced - from) as u64).min(limit.saturating_sub(out_total)) as usize;
        output.extend_from_slice(&self.scratch[from * ch..(from + keep) * ch]);
        keep as u64
    }

    fn clear(&mut self) {
        self.engine.reset();
        self.fifo.fill(0.0);
        // The alignment's silent lead-in (never longer than a block).
        self.fifo_frames = self.pad;
        self.skip_left = self.skip;
    }
}
