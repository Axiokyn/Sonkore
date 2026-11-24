//! The SoundTouch processor: rate transposer and time stretcher chained in
//! the order that keeps the stretcher's work smallest (SoundTouch class).

use std::fmt;

use crate::interpolate::Interpolation;
use crate::rate_transposer::RateTransposer;
use crate::tdstretch::TdStretch;
use crate::{MAX_CHANNELS, MAX_SAMPLE_RATE};

/// Silence pushed through by [`SoundTouch::flush`].
static FLUSH_SILENCE: [f32; 128 * MAX_CHANNELS] = [0.0; 128 * MAX_CHANNELS];

/// Why a processor could not be built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Above 192 kHz SoundTouch refuses the rate.
    SampleRateTooHigh(u32),
    /// 1 to 16 channels are supported.
    ChannelCount(usize),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::SampleRateTooHigh(r) => write!(f, "sample rate {r} Hz is above {MAX_SAMPLE_RATE} Hz"),
            Error::ChannelCount(n) => write!(f, "{n} channels: 1 to {MAX_CHANNELS} are supported"),
        }
    }
}

impl std::error::Error for Error {}

/// SoundTouch's rounded ln 2, which sets the pitch ratio.
#[allow(clippy::approx_constant)]
pub(crate) const SOUNDTOUCH_LN2: f64 = 0.69314718056;

/// `fabs(a - b) < 1e-10`: SoundTouch's test for "unchanged".
fn same(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-10
}

/// Pitch, tempo and playback rate on interleaved f32 audio, as SoundTouch
/// 2.3.3's float build computes them.
///
/// Rates up to 1 transpose first and stretch the (shorter or equal) result;
/// faster rates stretch first and transpose after. Switching between the two
/// moves the buffered audio across, as SoundTouch does.
pub struct SoundTouch {
    rt: RateTransposer,
    td: TdStretch,
    td_last: bool, // the stretcher is the last stage (rate <= 1)
    virtual_rate: f64,
    virtual_tempo: f64,
    virtual_pitch: f64,
    rate: f64,
    tempo: f64,
    samples_expected_out: f64,
    samples_output: i64,
    channels: usize,
}

impl SoundTouch {
    /// A neutral processor with cubic interpolation. A `sample_rate` of 0
    /// keeps SoundTouch's 44100.
    pub fn new(sample_rate: u32, channels: usize) -> Result<Self, Error> {
        Self::with_interpolation(sample_rate, channels, Interpolation::Cubic)
    }

    pub fn with_interpolation(sample_rate: u32, channels: usize, algorithm: Interpolation) -> Result<Self, Error> {
        if sample_rate > MAX_SAMPLE_RATE {
            return Err(Error::SampleRateTooHigh(sample_rate));
        }
        if !(1..=MAX_CHANNELS).contains(&channels) {
            return Err(Error::ChannelCount(channels));
        }
        let sample_rate = if sample_rate == 0 { 44100 } else { sample_rate as i32 };
        let mut st = SoundTouch {
            rt: RateTransposer::new(algorithm, channels),
            td: TdStretch::new(sample_rate, channels),
            td_last: true,
            virtual_rate: 1.0,
            virtual_tempo: 1.0,
            virtual_pitch: 1.0,
            rate: 0.0,
            tempo: 0.0,
            samples_expected_out: 0.0,
            samples_output: 0,
            channels,
        };
        st.calc_effective_rate_and_tempo();
        Ok(st)
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    pub fn sample_rate(&self) -> u32 {
        self.td.parameters().0 as u32
    }

    /// Sizes the buffers for puts of `frames` frames at the current ratios,
    /// so the first blocks do not grow them.
    pub fn reserve(&mut self, frames: usize) {
        let speed = (self.rate * self.tempo).max(1e-3);
        let out = (frames as f64 / speed) as usize + 2 * self.td.output_batch_size().max(0) as usize + 64;
        let transposed = (frames as f64 / self.rate.max(1e-3)) as usize + 64;
        self.rt.reserve(frames.max(transposed) + 256);
        self.td.reserve(frames.max(transposed).max(out));
    }

    /// Playback rate: pitch and tempo together (1 = original).
    pub fn set_rate(&mut self, rate: f64) {
        self.virtual_rate = rate;
        self.calc_effective_rate_and_tempo();
    }

    /// Rate as a change in percent.
    pub fn set_rate_change(&mut self, percent: f64) {
        self.virtual_rate = 1.0 + 0.01 * percent;
        self.calc_effective_rate_and_tempo();
    }

    /// Tempo without changing the pitch (1 = original).
    pub fn set_tempo(&mut self, tempo: f64) {
        self.virtual_tempo = tempo;
        self.calc_effective_rate_and_tempo();
    }

    pub fn set_tempo_change(&mut self, percent: f64) {
        self.virtual_tempo = 1.0 + 0.01 * percent;
        self.calc_effective_rate_and_tempo();
    }

    /// Pitch as a frequency ratio, without changing the tempo.
    pub fn set_pitch(&mut self, pitch: f64) {
        self.virtual_pitch = pitch;
        self.calc_effective_rate_and_tempo();
    }

    pub fn set_pitch_octaves(&mut self, octaves: f64) {
        self.virtual_pitch = (SOUNDTOUCH_LN2 * octaves).exp();
        self.calc_effective_rate_and_tempo();
    }

    pub fn set_pitch_semitones(&mut self, semitones: f64) {
        self.set_pitch_octaves(semitones / 12.0);
    }

    /// The rate the transposer runs at (pitch × rate).
    pub fn effective_rate(&self) -> f64 {
        self.rate
    }

    /// The tempo the stretcher runs at (tempo / pitch).
    pub fn effective_tempo(&self) -> f64 {
        self.tempo
    }

    /// Output frames per input frame.
    pub fn input_output_sample_ratio(&self) -> f64 {
        1.0 / (self.tempo * self.rate)
    }

    fn calc_effective_rate_and_tempo(&mut self) {
        let old_tempo = self.tempo;
        let old_rate = self.rate;
        self.tempo = self.virtual_tempo / self.virtual_pitch;
        self.rate = self.virtual_pitch * self.virtual_rate;

        if !same(self.rate, old_rate) {
            self.rt.set_rate(self.rate);
        }
        if !same(self.tempo, old_tempo) {
            self.td.set_tempo(self.tempo);
        }

        if self.rate <= 1.0 {
            if !self.td_last {
                // Transposer output → stretcher output.
                let n = self.rt.output.len();
                self.td.output.put(self.rt.output.data());
                self.rt.output.skip(n);
                self.td_last = true;
            }
        } else if self.td_last {
            // Stretcher output → transposer output, and the stretcher's
            // pending input through the transposer.
            let n = self.td.output.len();
            self.rt.output.put(self.td.output.data());
            self.td.output.skip(n);
            let n = self.td.input.len();
            self.rt.put_samples(self.td.input.data());
            self.td.input.skip(n);
            self.td_last = false;
        }
    }

    /// Feeds interleaved frames (a trailing partial frame is ignored).
    pub fn put_samples(&mut self, samples: &[f32]) {
        let frames = samples.len() / self.channels;
        let samples = &samples[..frames * self.channels];
        self.samples_expected_out += frames as f64 / (self.rate * self.tempo);
        if self.rate <= 1.0 {
            self.rt.put_samples(samples);
            let n = self.rt.output.len();
            self.td.put_samples(self.rt.output.data());
            self.rt.output.skip(n);
        } else {
            self.td.put_samples(samples);
            let n = self.td.output.len();
            self.rt.put_samples(self.td.output.data());
            self.td.output.skip(n);
        }
    }

    /// Copies up to `out.len() / channels` processed frames into `out`;
    /// returns how many.
    pub fn receive_samples(&mut self, out: &mut [f32]) -> usize {
        let n = if self.td_last { self.td.output.receive_into(out) } else { self.rt.output.receive_into(out) };
        self.samples_output += n as i64;
        n
    }

    /// Drops up to `frames` processed frames; returns how many.
    pub fn skip_samples(&mut self, frames: usize) -> usize {
        let n = if self.td_last { self.td.output.skip(frames) } else { self.rt.output.skip(frames) };
        self.samples_output += n as i64;
        n
    }

    /// Processed frames ready to be received.
    pub fn num_samples(&self) -> usize {
        if self.td_last { self.td.output.len() } else { self.rt.output.len() }
    }

    pub fn is_empty(&self) -> bool {
        self.num_samples() == 0
    }

    /// Frames waiting in the stretcher's input.
    pub fn num_unprocessed_samples(&self) -> usize {
        self.td.input.len()
    }

    /// Pushes the tail out with silence, trims the output to the length the
    /// input implies, and empties the stretcher's input. The transposer keeps
    /// its state, as in SoundTouch.
    pub fn flush(&mut self) {
        let mut still = ((self.samples_expected_out + 0.5) as i64).wrapping_sub(self.samples_output) as i32;
        if still < 0 {
            still = 0;
        }
        let silence = &FLUSH_SILENCE[..128 * self.channels];
        let mut i = 0;
        while still > self.num_samples() as i32 && i < 200 {
            self.put_samples(silence);
            i += 1;
        }
        let still = still as usize;
        if self.td_last {
            self.td.output.truncate(still);
        } else {
            self.rt.output.truncate(still);
        }
        self.td.clear_input();
    }

    /// Drops everything buffered.
    pub fn clear(&mut self) {
        self.samples_expected_out = 0.0;
        self.samples_output = 0;
        self.rt.clear();
        self.td.clear();
    }

    // --- settings (SoundTouch::setSetting) ------------------------------

    /// Anti-alias filter on or off. Clears the transposer.
    pub fn set_aa_filter(&mut self, on: bool) {
        self.rt.set_aa_enabled(on);
    }

    pub fn aa_filter(&self) -> bool {
        self.rt.aa_enabled()
    }

    /// Filter taps: a multiple of 8 in 8..=128, otherwise refused (false).
    pub fn set_aa_filter_length(&mut self, taps: usize) -> bool {
        if !taps.is_multiple_of(8) || !(8..=crate::aa_filter::MAX_AA_LENGTH).contains(&taps) {
            return false;
        }
        self.rt.set_aa_length(taps);
        true
    }

    pub fn aa_filter_length(&self) -> usize {
        self.rt.aa_length()
    }

    pub fn set_quick_seek(&mut self, on: bool) {
        self.td.set_quick_seek(on);
    }

    pub fn quick_seek(&self) -> bool {
        self.td.quick_seek()
    }

    /// Sequence length in ms; 0 = automatic, negative = unchanged.
    pub fn set_sequence_ms(&mut self, ms: i32) {
        let (sr, _, seek, ovl) = self.td.parameters();
        self.td.set_parameters(sr, ms, seek, ovl);
    }

    /// Seek window in ms; 0 = automatic, negative = unchanged.
    pub fn set_seek_window_ms(&mut self, ms: i32) {
        let (sr, seq, _, ovl) = self.td.parameters();
        self.td.set_parameters(sr, seq, ms, ovl);
    }

    /// Overlap in ms; values below 1 leave it unchanged.
    pub fn set_overlap_ms(&mut self, ms: i32) {
        let (sr, seq, seek, _) = self.td.parameters();
        self.td.set_parameters(sr, seq, seek, ms);
    }

    /// Sequence ms, 0 while automatic.
    pub fn sequence_ms(&self) -> i32 {
        self.td.parameters().1
    }

    /// Seek window ms, 0 while automatic.
    pub fn seek_window_ms(&self) -> i32 {
        self.td.parameters().2
    }

    pub fn overlap_ms(&self) -> i32 {
        self.td.parameters().3
    }

    /// SETTING_NOMINAL_INPUT_SEQUENCE: input frames per processing batch.
    pub fn nominal_input_sequence(&self) -> i32 {
        let size = self.td.input_sample_req();
        if self.rate <= 1.0 { (size as f64 * self.rate + 0.5) as i32 } else { size }
    }

    /// SETTING_NOMINAL_OUTPUT_SEQUENCE: output frames per processing batch.
    pub fn nominal_output_sequence(&self) -> i32 {
        let size = self.td.output_batch_size();
        if self.rate > 1.0 { (size as f64 / self.rate + 0.5) as i32 } else { size }
    }

    /// SETTING_INITIAL_LATENCY: input frames before the first output.
    pub fn initial_latency(&self) -> i32 {
        let mut latency = self.td.latency() as f64;
        let tr = self.rt.latency() as f64;
        if self.rate <= 1.0 {
            latency = (latency + tr) * self.rate;
        } else {
            latency += tr / self.rate;
        }
        (latency + 0.5) as i32
    }
}
