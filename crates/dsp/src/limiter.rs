//! True-peak limiter, double precision, at the end of the
//! processed path (after the EQ, the volume and the resampler, before the
//! quantiser): what a boost or an inter-sample peak pushes above the ceiling
//! is brought down to it, instead of being clipped at full scale by the
//! quantiser.
//!
//! * The peak is the true peak (ITU-R BS.1770: 4× oversampled with a 48-tap
//!   windowed-sinc interpolator), so the waveform the DAC reconstructs
//!   between the samples stays under the ceiling too (-1 dBTP by default,
//!   EBU R128's ceiling). At 352.8 kHz and above the samples themselves are
//!   taken as the true peak.
//! * Look-ahead: the gain ramps down over `lookahead` frames (1.5 ms) and
//!   reaches its minimum exactly at the peak (a sliding minimum over the
//!   look-ahead window, then a box average of the same length), so the
//!   attack is a linear fade, not a step. The release back to unity is
//!   exponential (50 ms). All channels share one gain: the stereo image is
//!   not shifted.
//! * The interpolator is centred: what it finds lies TAPS_PER_PHASE / 2
//!   frames back. The samples wait that much longer than the look-ahead (and
//!   their own peaks are taken as many frames late), so the gain is down
//!   when the inter-sample peak goes by. The recorded reference applied the
//!   gain 6 frames early, and an isolated inter-sample over came out up to
//!   0.16 dB above the ceiling at 44.1 kHz.
//! * Below the ceiling the gain is exactly 1.0 and the samples pass through
//!   untouched (a multiplication by 1.0 is exact), delayed by
//!   [`Limiter::delay`] frames (`lookahead`, + 6 below 352.8 kHz).
//!
//! The arithmetic otherwise follows the recorded reference operation for
//! operation. Nothing allocates after [`Limiter::setup`]:
//! [`Limiter::process`] and [`Limiter::flush`] write into the caller's slice.

/// The most channels a limiter takes (the engine's limit).
pub const MAX_CHANNELS: usize = 8;

pub const LOOKAHEAD_SECONDS: f64 = 0.0015;
pub const RELEASE_SECONDS: f64 = 0.05;
pub const OVERSAMPLE: usize = 4;
/// 12 taps per phase: 48 taps in all.
pub const TAPS_PER_PHASE: usize = 12;

/// The ceiling's range and default in dBTP (the controller's clamp).
pub const CEILING_MIN_DB: f64 = -6.0;
pub const CEILING_MAX_DB: f64 = 0.0;
pub const CEILING_DEFAULT_DB: f64 = -1.0;

/// The 4× windowed-sinc interpolator (Blackman window), 12 taps per phase:
/// the BS.1770 true-peak measurement filter, each phase scaled to unity gain.
fn interpolator() -> [f64; OVERSAMPLE * TAPS_PER_PHASE] {
    const TOTAL: usize = OVERSAMPLE * TAPS_PER_PHASE;
    let pi = std::f64::consts::PI;
    let mut taps = [0.0; TOTAL];
    let centre = (TOTAL - 1) as f64 / 2.0;
    let mut sum = 0.0;
    for (i, tap) in taps.iter_mut().enumerate() {
        let x = (i as f64 - centre) / OVERSAMPLE as f64;
        let sinc = if x == 0.0 { 1.0 } else { (pi * x).sin() / (pi * x) };
        let w = 0.42 - 0.5 * (2.0 * pi * i as f64 / (TOTAL - 1) as f64).cos()
            + 0.08 * (4.0 * pi * i as f64 / (TOTAL - 1) as f64).cos();
        *tap = sinc * w;
        sum += *tap;
    }
    for t in taps.iter_mut() {
        // Unity gain per phase: DC through each phase sums to 1.
        *t *= OVERSAMPLE as f64 / sum;
    }
    taps
}

/// `std::max(a, b)`: `b` only when `a < b` (the reference's order, kept for
/// exact agreement).
#[inline(always)]
fn max(a: f64, b: f64) -> f64 {
    if a < b { b } else { a }
}

/// `std::min(a, b)`: `b` only when `b < a`.
#[inline(always)]
fn min(a: f64, b: f64) -> f64 {
    if b < a { b } else { a }
}

/// One interleaved block's true peak (linear), as the limiter measures it:
/// always interpolated 4×, whatever the rate.
pub fn true_peak(input: &[f64], channels: usize, _sample_rate: u32) -> f64 {
    let taps = interpolator();
    let mut hist = [[0.0f64; TAPS_PER_PHASE]; MAX_CHANNELS];
    let channels = channels.clamp(1, MAX_CHANNELS);
    let mut peak = 0.0;
    let mut pos = 0;
    for frame in input.chunks_exact(channels) {
        for (c, &s) in frame.iter().enumerate() {
            let h = &mut hist[c];
            h[pos] = s;
            peak = max(peak, h[pos].abs());
            for phase in 1..OVERSAMPLE {
                let mut acc = 0.0;
                for k in 0..TAPS_PER_PHASE {
                    acc += taps[phase + k * OVERSAMPLE] * h[(pos + TAPS_PER_PHASE - k) % TAPS_PER_PHASE];
                }
                peak = max(peak, acc.abs());
            }
        }
        pos = (pos + 1) % TAPS_PER_PHASE;
    }
    peak
}

/// The look-ahead limiter. Worker thread only. [`process`](Limiter::process)
/// hands back fewer frames than it took while the delay line fills;
/// [`flush`](Limiter::flush) returns what is still held (end of track, or
/// the path going back to untouched).
pub struct Limiter {
    rate: u32,
    channels: usize,
    ceiling_db: f64,
    ceiling: f64,
    lookahead: usize,
    /// 1 at rates >= 352.8 kHz: the samples are the true peak there.
    phases: usize,
    /// How far behind the input the interpolated peaks are (TAPS_PER_PHASE /
    /// 2, 0 with one phase): the samples are delayed by `lookahead + late`.
    late: usize,
    release_coef: f64,
    /// The interpolator by phase: `phase_taps[p][k]` = tap `p + k * OVERSAMPLE`.
    phase_taps: [[f64; TAPS_PER_PHASE]; OVERSAMPLE],
    /// Per channel, the last TAPS_PER_PHASE inputs: a ring written twice (at
    /// p and p + TAPS_PER_PHASE), so the newest-last window is always the
    /// contiguous run (p, p + TAPS_PER_PHASE].
    history: [[f64; 2 * TAPS_PER_PHASE]; MAX_CHANNELS],
    history_pos: usize,
    /// The delay line: `lookahead + late` frames (a ring).
    delay: Vec<f64>,
    delay_pos: usize,
    held: usize,
    /// Monotonic queue of (frame index, target gain) for the sliding
    /// minimum: a ring of `lookahead + 1` entries.
    min_index: Vec<usize>,
    min_gain: Vec<f64>,
    min_head: usize,
    min_len: usize,
    index: usize,
    /// The last `lookahead` envelope values (a ring) and their sum.
    boxes: Vec<f64>,
    box_pos: usize,
    box_sum: f64,
    envelope: f64,
    last_reduction_db: f64,
}

impl Default for Limiter {
    fn default() -> Self {
        Limiter {
            rate: 0,
            channels: 0,
            ceiling_db: CEILING_DEFAULT_DB,
            ceiling: 1.0,
            lookahead: 0,
            phases: OVERSAMPLE,
            late: 0,
            release_coef: 0.0,
            phase_taps: [[0.0; TAPS_PER_PHASE]; OVERSAMPLE],
            history: [[0.0; 2 * TAPS_PER_PHASE]; MAX_CHANNELS],
            history_pos: 0,
            delay: Vec::new(),
            delay_pos: 0,
            held: 0,
            min_index: Vec::new(),
            min_gain: Vec::new(),
            min_head: 0,
            min_len: 0,
            index: 0,
            boxes: Vec::new(),
            box_pos: 0,
            box_sum: 0.0,
            envelope: 1.0,
            last_reduction_db: 0.0,
        }
    }
}

impl Limiter {
    pub fn new(sample_rate: u32, channels: usize, ceiling_db: f64) -> Self {
        let mut l = Limiter::default();
        l.setup(sample_rate, channels, ceiling_db);
        l
    }

    /// Builds the limiter for a rate and channel count (1–8). Allocates.
    pub fn setup(&mut self, sample_rate: u32, channels: usize, ceiling_db: f64) {
        let channels = channels.clamp(1, MAX_CHANNELS);
        self.rate = sample_rate;
        self.channels = channels;
        self.lookahead = 8usize.max((LOOKAHEAD_SECONDS * sample_rate as f64).round() as usize);
        self.release_coef = 1.0 - (-1.0 / (RELEASE_SECONDS * sample_rate as f64)).exp();
        self.phases = if sample_rate >= 352_800 { 1 } else { OVERSAMPLE };
        self.late = if self.phases > 1 { TAPS_PER_PHASE / 2 } else { 0 };
        let taps = interpolator();
        for (p, phase) in self.phase_taps.iter_mut().enumerate() {
            for (k, t) in phase.iter_mut().enumerate() {
                *t = taps[p + k * OVERSAMPLE];
            }
        }
        self.delay = vec![0.0; self.delay() * channels];
        self.boxes = vec![1.0; self.lookahead];
        // One more than the window: the new entry goes in before the oldest
        // one leaves.
        self.min_index = vec![0; self.lookahead + 1];
        self.min_gain = vec![0.0; self.lookahead + 1];
        self.set_ceiling_db(ceiling_db);
        self.reset();
    }

    /// A new ceiling, without a reset. Clamped to [-6, 0] dBTP.
    pub fn set_ceiling_db(&mut self, db: f64) {
        self.ceiling_db = db.clamp(CEILING_MIN_DB, CEILING_MAX_DB);
        self.ceiling = 10f64.powf(self.ceiling_db / 20.0);
    }

    /// Empties the delay line and the envelope.
    pub fn reset(&mut self) {
        self.history = [[0.0; 2 * TAPS_PER_PHASE]; MAX_CHANNELS];
        self.delay.fill(0.0);
        self.boxes.fill(1.0);
        self.history_pos = 0;
        self.delay_pos = 0;
        self.box_pos = 0;
        self.held = 0;
        self.min_head = 0;
        self.min_len = 0;
        self.index = 0;
        self.box_sum = self.lookahead as f64;
        self.envelope = 1.0;
        self.last_reduction_db = 0.0;
    }

    /// Frees everything: not set up any more.
    pub fn release(&mut self) {
        *self = Limiter::default();
    }

    pub fn ready(&self) -> bool {
        self.channels != 0
    }

    pub fn sample_rate(&self) -> u32 {
        self.rate
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    pub fn ceiling_db(&self) -> f64 {
        self.ceiling_db
    }

    /// Frames held in the delay line (what the limiter adds to the latency).
    pub fn latency(&self) -> usize {
        self.held
    }

    /// The look-ahead in frames: how long the gain takes to come down.
    pub fn lookahead(&self) -> usize {
        self.lookahead
    }

    /// The samples' delay in frames (the most it holds): the look-ahead, and
    /// half the interpolator below 352.8 kHz.
    pub fn delay(&self) -> usize {
        self.lookahead + self.late
    }

    /// The most reduction applied in the last block, in dB (<= 0; 0 = none).
    pub fn last_reduction_db(&self) -> f64 {
        self.last_reduction_db
    }

    #[inline]
    fn gain_for(&self, peak: f64) -> f64 {
        if peak > self.ceiling { self.ceiling / peak } else { 1.0 }
    }

    /// The true peak `late` frames back: the samples then and the three
    /// interpolated values the centred interpolator gives around them, then
    /// the sliding minimum of the target gain over the next `lookahead` frames.
    #[inline]
    fn push_frame(&mut self, frame: &[f64]) {
        let mut peak = 0.0;
        let pos = self.history_pos;
        for (c, &s) in frame.iter().enumerate() {
            let hist = &mut self.history[c];
            hist[pos] = s;
            hist[pos + TAPS_PER_PHASE] = s;
            // The window, oldest first: w[TAPS_PER_PHASE - 1 - k] is the
            // input k frames back.
            let w: &[f64; TAPS_PER_PHASE] = hist[pos + 1..pos + 1 + TAPS_PER_PHASE].try_into().unwrap();
            peak = max(peak, w[TAPS_PER_PHASE - 1 - self.late].abs());
            for taps in &self.phase_taps[1..self.phases] {
                let mut acc = 0.0;
                for k in 0..TAPS_PER_PHASE {
                    acc += taps[k] * w[TAPS_PER_PHASE - 1 - k];
                }
                peak = max(peak, acc.abs());
            }
        }
        self.history_pos = if pos + 1 == TAPS_PER_PHASE { 0 } else { pos + 1 };

        let target = self.gain_for(peak);
        // The queue's ring holds lookahead + 1 entries: head + len < 2 × that.
        let cap = self.lookahead + 1;
        let wrap = |i: usize| if i >= cap { i - cap } else { i };
        while self.min_len > 0 && self.min_gain[wrap(self.min_head + self.min_len - 1)] >= target {
            self.min_len -= 1;
        }
        let slot = wrap(self.min_head + self.min_len);
        self.min_index[slot] = self.index;
        self.min_gain[slot] = target;
        self.min_len += 1;
        while self.min_index[self.min_head] + self.lookahead <= self.index {
            self.min_head = wrap(self.min_head + 1);
            self.min_len -= 1;
        }
        self.index += 1;
    }

    /// One frame in, at most one frame out (into `out`, which holds a frame).
    /// Returns whether a frame came out.
    #[inline]
    fn step(&mut self, frame: &[f64], out: &mut [f64], worst: &mut f64) -> bool {
        let ch = self.channels;
        self.push_frame(frame);
        // The envelope for the frame leaving the delay line: the minimum over
        // the window that ends at the frame just pushed, released
        // exponentially, then averaged over the window length (a linear
        // attack).
        let minimum = self.min_gain[self.min_head];
        self.envelope = if minimum < self.envelope {
            minimum
        } else {
            self.envelope + (minimum - self.envelope) * self.release_coef
        };
        self.box_sum += self.envelope - self.boxes[self.box_pos];
        self.boxes[self.box_pos] = self.envelope;
        self.box_pos += 1;
        if self.box_pos == self.lookahead {
            self.box_pos = 0;
        }
        let mut gain = min(1.0, self.box_sum / self.lookahead as f64);
        if gain > 1.0 - 1e-12 {
            gain = 1.0; // exact pass-through, not 0.999999999
        }
        *worst = min(*worst, gain);

        let slot = &mut self.delay[self.delay_pos * ch..(self.delay_pos + 1) * ch];
        let produced = if self.held == self.lookahead + self.late {
            for (o, &s) in out.iter_mut().zip(slot.iter()) {
                *o = s * gain;
            }
            true
        } else {
            self.held += 1;
            false
        };
        slot.copy_from_slice(frame);
        self.delay_pos += 1;
        if self.delay_pos == self.lookahead + self.late {
            self.delay_pos = 0;
        }
        produced
    }

    /// Limits interleaved frames. `output` needs room for as many samples as
    /// `input` has; returns the frames written (fewer while the delay line
    /// fills).
    pub fn process(&mut self, input: &[f64], output: &mut [f64]) -> usize {
        let ch = self.channels;
        assert!(output.len() >= input.len() / ch * ch, "limiter: output too small");
        let mut produced = 0;
        let mut worst = 1.0;
        for frame in input.chunks_exact(ch) {
            let out = &mut output[produced * ch..(produced + 1) * ch];
            if self.step(frame, out, &mut worst) {
                produced += 1;
            }
        }
        self.last_reduction_db = if worst < 1.0 { 20.0 * worst.log10() } else { 0.0 };
        produced
    }

    /// Hands back the frames still held, with their gains, and empties the
    /// limiter (the ceiling stays). `output` needs room for
    /// [`latency`](Limiter::latency) frames; returns the frames written.
    pub fn flush(&mut self, output: &mut [f64]) -> usize {
        // Push silence through: the held frames come out with their gains;
        // of what comes out, only `held` frames were real input (the rest is
        // the silence that went through the fill-up).
        let ch = self.channels;
        let held = self.held;
        let zeros = [0.0f64; MAX_CHANNELS];
        let mut scratch = [0.0f64; MAX_CHANNELS];
        let mut produced = 0;
        let mut worst = 1.0;
        for _ in 0..self.delay() {
            if self.step(&zeros[..ch], &mut scratch[..ch], &mut worst) {
                if produced < held {
                    output[produced * ch..(produced + 1) * ch].copy_from_slice(&scratch[..ch]);
                }
                produced += 1;
            }
        }
        self.reset();
        produced.min(held)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Everything through the limiter, flushed: the output, as long as the input.
    fn limit(l: &mut Limiter, x: &[f64]) -> Vec<f64> {
        let ch = l.channels();
        let mut out = vec![0.0; x.len()];
        let n = l.process(x, &mut out);
        out.truncate(n * ch);
        let mut tail = vec![0.0; l.latency() * ch];
        let t = l.flush(&mut tail);
        out.extend_from_slice(&tail[..t * ch]);
        out
    }

    /// Quiet noise with isolated inter-sample overs every 200 ms (four
    /// shapes: two equal samples, a quarter-rate burst at 45°, alternating
    /// pairs, a lone sample next to a smaller one) at several offsets.
    fn isolated_overs(rate: u32, channels: usize) -> Vec<f64> {
        let frames = rate as usize * 4;
        let mut seed = 0x1234_5678_9abc_def0u64;
        let mut x: Vec<f64> = (0..frames * channels)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                ((seed >> 11) as f64 / (1u64 << 53) as f64 - 0.5) * 0.02
            })
            .collect();
        let gap = rate as usize / 5;
        for (n, at) in (gap..frames - gap).step_by(gap).enumerate() {
            let shape: &[f64] = match n % 4 {
                0 => &[0.8, 0.8],
                1 => &[0.6, 0.6, -0.6, -0.6, 0.6, 0.6, -0.6, -0.6],
                2 => &[0.7, -0.7, 0.7, -0.7],
                _ => &[0.85, 0.5],
            };
            for (k, &v) in shape.iter().enumerate() {
                for c in 0..channels {
                    x[(at + k + n % 3) * channels + c] = 1.4 * v * if c == 1 { -1.0 } else { 1.0 };
                }
            }
        }
        x
    }

    /// The limiter written plainly, on the whole signal at once (no rings,
    /// no queue): what [`Limiter`] has to give bit for bit. `late` is how
    /// many frames the output waits beyond the look-ahead (and the sample
    /// peak with it).
    fn plain(x: &[f64], channels: usize, rate: u32, ceiling_db: f64, late: usize) -> Vec<f64> {
        let taps = interpolator();
        let la = 8usize.max((LOOKAHEAD_SECONDS * rate as f64).round() as usize);
        let release = 1.0 - (-1.0 / (RELEASE_SECONDS * rate as f64)).exp();
        let phases = if rate >= 352_800 { 1 } else { OVERSAMPLE };
        let ceiling = 10f64.powf(ceiling_db.clamp(CEILING_MIN_DB, CEILING_MAX_DB) / 20.0);
        let frames = x.len() / channels;
        let delay = la + late;
        let at = |i: usize, k: usize, c: usize| if i >= k && i - k < frames { x[(i - k) * channels + c] } else { 0.0 };
        let mut targets = Vec::new();
        let (mut envelope, mut boxes, mut box_sum) = (1.0, vec![1.0; la], la as f64);
        let mut y = vec![0.0; frames * channels];
        for i in 0..frames + delay {
            let mut peak = 0.0;
            for c in 0..channels {
                peak = max(peak, at(i, late, c).abs());
                for phase in 1..phases {
                    let mut acc = 0.0;
                    for k in 0..TAPS_PER_PHASE {
                        acc += taps[phase + k * OVERSAMPLE] * at(i, k, c);
                    }
                    peak = max(peak, acc.abs());
                }
            }
            targets.push(if peak > ceiling { ceiling / peak } else { 1.0 });
            let minimum = targets[(i + 1).saturating_sub(la)..].iter().fold(f64::INFINITY, |m, &t| min(m, t));
            envelope = if minimum < envelope { minimum } else { envelope + (minimum - envelope) * release };
            box_sum += envelope - boxes[i % la];
            boxes[i % la] = envelope;
            let mut gain = min(1.0, box_sum / la as f64);
            if gain > 1.0 - 1e-12 {
                gain = 1.0;
            }
            if i >= delay {
                for c in 0..channels {
                    y[(i - delay) * channels + c] = x[(i - delay) * channels + c] * gain;
                }
            }
        }
        y
    }

    #[test]
    fn same_samples_as_the_plain_limiter() {
        for (rate, channels, ceiling) in
            [(44100u32, 2usize, -1.0), (48000, 1, -3.0), (96000, 6, 0.0), (192000, 8, -0.5), (352800, 2, -1.0)]
        {
            let x: Vec<f64> = isolated_overs(rate, channels).iter().map(|v| v * 1.3).collect();
            let x = &x[..x.len().min(rate as usize / 2 * channels)];
            let mut l = Limiter::new(rate, channels, ceiling);
            // In blocks of uneven sizes, flushed at the end.
            let mut y = Vec::new();
            let mut out = vec![0.0; 1000 * channels];
            let mut from = 0;
            for size in [1usize, 7, 999, 64, 333, 1000].iter().cycle() {
                if from >= x.len() {
                    break;
                }
                let to = (from + size * channels).min(x.len());
                let n = l.process(&x[from..to], &mut out);
                y.extend_from_slice(&out[..n * channels]);
                from = to;
            }
            let mut tail = vec![0.0; l.latency() * channels];
            let t = l.flush(&mut tail);
            y.extend_from_slice(&tail[..t * channels]);
            let want = plain(x, channels, rate, ceiling, if rate >= 352_800 { 0 } else { TAPS_PER_PHASE / 2 });
            assert_eq!(y.len(), want.len());
            let differ = y.iter().zip(&want).filter(|(a, b)| a.to_bits() != b.to_bits()).count();
            assert_eq!(differ, 0, "{rate} Hz, {channels} ch: {differ} samples differ");
            assert!(l.last_reduction_db() <= 0.0);
        }
    }

    /// The interpolated peaks lie TAPS_PER_PHASE / 2 frames behind the
    /// input. The recorded reference applied their gain that much too early
    /// (over by up to 0.16 dB at 44.1 kHz, 0.03 dB at 192 kHz); the samples
    /// now wait for them.
    #[test]
    fn isolated_inter_sample_overs_stay_under_the_ceiling() {
        for rate in [44100u32, 48000, 88200, 96000, 192000] {
            for ceiling in [-1.0, -3.0, 0.0] {
                let x = isolated_overs(rate, 2);
                let mut l = Limiter::new(rate, 2, ceiling);
                let y = limit(&mut l, &x);
                assert_eq!(y.len(), x.len());
                let limit_lin = 10f64.powf(ceiling / 20.0);
                let over_db = 20.0 * (true_peak(&y, 2, rate) / limit_lin).log10();
                println!(
                    "{rate} Hz, ceiling {ceiling}: input TP {:.3}, over by {over_db:.4} dB",
                    true_peak(&x, 2, rate)
                );
                assert!(over_db < 0.005, "{rate} Hz, ceiling {ceiling} dBTP: over by {over_db:.3} dB");
            }
        }
    }
}
