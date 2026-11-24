//! The processed path: headroom, the linear-phase EQ, the convolver, the biquad
//! EQ, crossfeed, volume × ReplayGain, the resampler, the true-peak limiter.
//! The worker owns a `Chain`; the control thread changes the settings in
//! `Dsp`, picked up at block boundaries.

use std::sync::{Arc, Mutex};

use sonkore_dsp::convolver::{Convolver, ImpulseResponse};
use sonkore_dsp::crossfeed::{Crossfeed, Preset};
use sonkore_dsp::limiter::Limiter;
use sonkore_dsp::resampler::{Filter, Resampler};
use sonkore_eq::linear_phase::LinearPhaseEq;
use sonkore_eq::super_eq::{EqProcessor, SuperEq};
use sonkore_soundtouch::{StretchControl, TimeStretch};

/// What the control thread sets.
#[derive(Clone)]
pub struct DspSettings {
    /// Headroom before the processing, dB (≤ 0).
    pub headroom_db: f64,
    /// playback/crossfeed: 0 off, 1 Default, 2 Chu Moy, 3 Jan Meier.
    pub crossfeed: i64,
    pub limiter: bool,
    pub limiter_ceiling_db: f64,
    /// The convolver's impulse response (None: off), and its generation.
    pub impulse: Option<Arc<ImpulseResponse>>,
    pub impulse_generation: u64,
    /// playback/resampleFilter: linear, intermediate, minimum, linear steep.
    pub resample_filter: i64,
}

impl Default for DspSettings {
    fn default() -> Self {
        DspSettings {
            headroom_db: 0.0,
            crossfeed: 0,
            limiter: false,
            limiter_ceiling_db: -1.0,
            impulse: None,
            impulse_generation: 0,
            resample_filter: 0,
        }
    }
}

/// Shared between the control thread and the worker.
pub struct Dsp {
    pub settings: Mutex<DspSettings>,
    pub eq: SuperEq,
    /// Pitch, tempo and rate (SoundTouch).
    pub stretch: Arc<StretchControl>,
}

impl Default for Dsp {
    fn default() -> Self {
        Dsp {
            settings: Mutex::new(DspSettings::default()),
            eq: SuperEq::new(),
            stretch: Arc::new(StretchControl::new()),
        }
    }
}

impl Dsp {
    pub fn update(&self, f: impl FnOnce(&mut DspSettings)) {
        if let Ok(mut s) = self.settings.lock() {
            f(&mut s);
        }
    }
    pub fn snapshot(&self) -> DspSettings {
        self.settings.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// The settings change nothing in a stream of `channels` (the
    /// bit-perfect condition, without the gains): no pitch or tempo, a
    /// neutral EQ (biquads or linear phase), no convolver, no crossfeed on
    /// stereo. The worker's Chain::neutral() adds what is still fading out.
    pub fn neutral(&self, channels: usize) -> bool {
        let (convolver, crossfeed) =
            self.settings.lock().map(|s| (s.impulse.is_some(), s.crossfeed)).unwrap_or((false, 0));
        self.stretch.is_neutral()
            && self.eq.is_neutral()
            && !convolver
            && !(Preset::from_index(crossfeed) != Preset::Off && channels == 2)
    }
}

/// The worker's processing chain for one stream (rate in → rate out).
pub struct Chain {
    shared: Arc<Dsp>,
    pub in_rate: u32,
    pub out_rate: u32,
    channels: usize,
    eq: EqProcessor,
    linear: LinearPhaseEq,
    convolver: Option<Convolver>,
    impulse_generation: u64,
    crossfeed: Crossfeed,
    crossfeed_preset: i64,
    resampler: Option<Resampler>,
    resampled: Vec<f64>,
    limiter: Limiter,
    limiter_on: bool,
    /// Turned off: what it holds goes out with the next block, then it is off.
    limiter_off: bool,
    limited: Vec<f64>,
    pub applied_headroom: f64,
    pub applied_gain: f64,
    headroom: f64,
    work: Vec<f64>,
    tail_buf: Vec<f64>,
    /// The end of the stream: the tails' frames still to come out (finish_step).
    tail_left: Option<usize>,
    stretch: TimeStretch,
    stretch_active: bool,
    stretch_in: Vec<f32>,
    stretch_out: Vec<f32>,
    stretched: Vec<f64>,
}

impl Chain {
    pub fn new(shared: Arc<Dsp>, in_rate: u32, out_rate: u32, channels: usize) -> Chain {
        let filter = Filter::from_index(shared.snapshot().resample_filter);
        let resampler =
            (in_rate != out_rate).then(|| Resampler::new(in_rate, out_rate, channels, filter).ok()).flatten();
        let mut eq = EqProcessor::new(&shared.eq);
        eq.prepare(in_rate as f64, channels);
        let mut linear = LinearPhaseEq::new();
        linear.prepare(in_rate, channels);
        let mut chain = Chain {
            eq,
            linear,
            convolver: None,
            impulse_generation: 0,
            crossfeed: Crossfeed::new(Preset::Off, in_rate as f64),
            crossfeed_preset: 0,
            resampler,
            resampled: Vec::new(),
            limiter: Limiter::new(out_rate, channels, -1.0),
            limiter_on: false,
            limiter_off: false,
            limited: Vec::new(),
            applied_headroom: 1.0,
            applied_gain: 1.0,
            headroom: 1.0,
            work: Vec::new(),
            tail_buf: Vec::new(),
            tail_left: None,
            stretch: {
                let mut t = TimeStretch::with_control(shared.stretch.clone(), in_rate, channels);
                t.prepare(in_rate, channels, 1024);
                t
            },
            stretch_active: false,
            stretch_in: Vec::new(),
            stretch_out: Vec::new(),
            stretched: Vec::new(),
            in_rate,
            out_rate,
            channels,
            shared,
        };
        chain.sync();
        chain
    }

    /// The settings as they are now (block boundary).
    pub fn sync(&mut self) {
        let s = self.shared.snapshot();
        self.headroom = 10f64.powf(s.headroom_db.min(0.0) / 20.0);
        if s.crossfeed != self.crossfeed_preset {
            self.crossfeed_preset = s.crossfeed;
            self.crossfeed.setup(Preset::from_index(s.crossfeed), self.in_rate as f64);
        }
        if s.limiter {
            self.limiter_off = false;
            if !self.limiter_on {
                self.limiter_on = true;
                self.limiter.setup(self.out_rate, self.channels, s.limiter_ceiling_db);
            } else if self.limiter.ceiling_db() != s.limiter_ceiling_db {
                self.limiter.set_ceiling_db(s.limiter_ceiling_db);
            }
        } else if self.limiter_on {
            // Its look-ahead holds music: dropping it would be a jump.
            self.limiter_off = true;
        }
        if s.impulse_generation != self.impulse_generation {
            self.impulse_generation = s.impulse_generation;
            self.convolver = s.impulse.as_ref().and_then(|ir| Convolver::new(ir, self.in_rate, self.channels).ok());
        }
    }

    pub fn resampling(&self) -> bool {
        self.resampler.is_some()
    }

    /// Nothing would change the samples (the bit-perfect condition, without
    /// the gains).
    pub fn neutral(&self) -> bool {
        self.shared.stretch.is_neutral()
            && !self.stretch_active
            && self.shared.eq.is_neutral()
            && self.convolver.is_none()
            && !self.linear.active()
            && !(self.crossfeed.active() && self.channels == 2)
    }

    pub fn headroom(&self) -> f64 {
        self.headroom
    }

    /// Frames of output still inside the chain (the track change and the
    /// position wait for them).
    pub fn pending_out_frames(&self) -> u64 {
        let held = self.convolver.as_ref().map_or(0, |c| c.latency()) + self.linear.latency();
        let held = held as u64 * self.out_rate as u64 / self.in_rate.max(1) as u64;
        let converting = self.resampler.as_ref().map_or(0.0, |r| r.delay_frames()).round() as u64;
        held + converting + if self.limiter_on { self.limiter.latency() as u64 } else { 0 }
    }

    /// Source frames SoundTouch holds (taken in, not given back yet).
    /// The limiter's reduction in the last block (dB, <= 0).
    pub fn limiter_reduction_db(&self) -> f64 {
        if self.limiter_on { self.limiter.last_reduction_db() } else { 0.0 }
    }

    pub fn stretch_held(&self) -> f64 {
        if self.stretch_active { self.stretch.held_source_frames() } else { 0.0 }
    }

    /// The linear-phase EQ keeps the bypassed input in mind.
    pub fn remember_s32(&mut self, samples: &[i32]) {
        if self.shared.eq.linear_phase() {
            self.linear.remember_s32(samples);
        }
    }

    /// A seek or a new track: no history.
    pub fn reset(&mut self) {
        self.eq.reset();
        self.linear.reset();
        if let Some(c) = &mut self.convolver {
            c.reset();
        }
        self.crossfeed.reset();
        if let Some(r) = &mut self.resampler {
            r.clear();
        }
        self.stretch.clear();
        if self.limiter_off {
            self.limiter_on = false;
            self.limiter_off = false;
        }
        if self.limiter_on {
            self.limiter.reset();
        }
        self.tail_left = None;
    }

    /// Runs `samples` (interleaved, at the input rate) through the chain;
    /// `out` gets every block that comes out (at the output rate).
    pub fn process(&mut self, samples: &mut [f64], gain: f64, to_unity: bool, out: &mut dyn FnMut(&[f64])) {
        // Pitch / tempo / rate first, at the source's rate.
        self.stretch.apply_pending_changes();
        let wants = !self.shared.stretch.is_neutral();
        if wants || self.stretch_active {
            self.stretch_active = wants;
            self.stretch_in.clear();
            self.stretch_in.extend(samples.iter().map(|&s| s as f32));
            self.stretch_out.clear();
            self.stretch.process(&self.stretch_in, &mut self.stretch_out);
            if !wants {
                // Back to neutral: this block went in as the others, then
                // what it holds plays out and it is freed. (Flushing before
                // the block put it into an emptied stretcher, freed with it.)
                self.stretch.flush();
                self.stretch.drain_into(&mut self.stretch_out);
                self.stretch.release();
            }
            let mut stretched = std::mem::take(&mut self.stretched);
            stretched.clear();
            stretched.extend(self.stretch_out.iter().map(|&s| s as f64));
            if !stretched.is_empty() {
                self.after_stretch(&mut stretched, gain, to_unity, out);
            }
            self.stretched = stretched;
            return;
        }
        self.after_stretch(samples, gain, to_unity, out);
    }

    fn after_stretch(&mut self, samples: &mut [f64], gain: f64, to_unity: bool, out: &mut dyn FnMut(&[f64])) {
        let ch = self.channels;
        apply_gain(samples, ch, &mut self.applied_headroom, if to_unity { 1.0 } else { self.headroom });
        let linear = self.shared.eq.linear_phase();
        if linear || self.linear.active() {
            self.linear.update(linear && !self.shared.eq.is_neutral(), &self.shared.eq);
            self.linear.remember(samples);
        }
        if !self.linear.active() {
            self.rest(samples, !linear, gain, to_unity, out);
            return;
        }
        self.linear.process(samples);
        let mut filtered = std::mem::take(&mut self.work);
        filtered.clear();
        filtered.extend_from_slice(self.linear.filtered());
        let mut passed = self.linear.take_passed();
        if !filtered.is_empty() {
            self.rest(&mut filtered, false, gain, to_unity, out);
        }
        self.work = filtered;
        if !passed.is_empty() {
            self.rest(&mut passed, !linear, gain, to_unity, out);
        }
        self.linear.give_back(passed);
    }

    fn rest(&mut self, samples: &mut [f64], biquads: bool, gain: f64, to_unity: bool, out: &mut dyn FnMut(&[f64])) {
        let ch = self.channels;
        if let Some(c) = &mut self.convolver {
            c.process(samples);
        }
        if biquads {
            self.eq.process(samples);
        } else {
            self.eq.bypass();
        }
        if self.crossfeed.active() && ch == 2 {
            self.crossfeed.process(samples);
        }
        apply_gain(samples, ch, &mut self.applied_gain, if to_unity { 1.0 } else { gain });
        if let Some(r) = &mut self.resampler {
            let mut resampled = std::mem::take(&mut self.resampled);
            r.process(samples, &mut resampled);
            self.limit(&resampled, out);
            self.resampled = resampled;
        } else {
            self.limit(samples, out);
        }
    }

    fn limit(&mut self, samples: &[f64], out: &mut dyn FnMut(&[f64])) {
        if samples.is_empty() {
            return;
        }
        if self.limiter_off {
            // Turned off: the frames it holds come before this block.
            self.flush_limiter(out);
        }
        if !self.limiter_on {
            out(samples);
            return;
        }
        self.limited.resize(samples.len(), 0.0);
        let got = self.limiter.process(samples, &mut self.limited);
        if got > 0 {
            out(&self.limited[..got * self.channels]);
        }
    }

    /// The end of the stream: the filters' tails, the resampler's last
    /// samples and what the limiter holds go out.
    pub fn finish(&mut self, gain: f64, out: &mut dyn FnMut(&[f64])) {
        while !self.finish_step(gain, out) {}
    }

    /// The tails are coming out (finish_step() has started and not ended).
    pub fn finishing(&self) -> bool {
        self.tail_left.is_some()
    }

    /// finish() a block at a time (a convolver's tail can be seconds long):
    /// true once everything is out.
    pub fn finish_step(&mut self, gain: f64, out: &mut dyn FnMut(&[f64])) -> bool {
        let ch = self.channels;
        let left = match self.tail_left {
            Some(left) => left,
            None => {
                if self.stretch_active {
                    self.stretch.flush();
                    self.stretch_out.clear();
                    self.stretch.drain_into(&mut self.stretch_out);
                    let mut stretched: Vec<f64> = self.stretch_out.iter().map(|&s| s as f64).collect();
                    if !stretched.is_empty() {
                        self.after_stretch(&mut stretched, gain, false, out);
                    }
                }
                self.convolver.as_ref().map_or(0, |c| c.tail())
                    + if self.linear.active() { self.linear.tail() } else { 0 }
            }
        };
        if left > 0 {
            let n = left.min(1024);
            let mut zeros = std::mem::take(&mut self.tail_buf);
            zeros.clear();
            zeros.resize(n * ch, 0.0);
            self.after_stretch(&mut zeros, gain, false, out);
            self.tail_buf = zeros;
            self.tail_left = Some(left - n);
            return false;
        }
        self.tail_left = None;
        if let Some(mut r) = self.resampler.take() {
            let mut resampled = std::mem::take(&mut self.resampled);
            while r.drain(&mut resampled) > 0 {
                self.limit(&resampled, out);
            }
            self.resampled = resampled;
            self.resampler = Some(r);
        }
        self.flush_limiter(out);
        true
    }

    /// What the limiter holds goes out (going back to bit-perfect, or the
    /// limiter turned off).
    pub fn flush_limiter(&mut self, out: &mut dyn FnMut(&[f64])) {
        if self.limiter_on && self.limiter.latency() > 0 {
            self.limited.resize(self.limiter.latency() * self.channels + 64 * self.channels, 0.0);
            let got = self.limiter.flush(&mut self.limited);
            if got > 0 {
                out(&self.limited[..got * self.channels]);
            }
        }
        if self.limiter_off {
            self.limiter_on = false;
            self.limiter_off = false;
        }
    }
}

/// Multiplies a block by `target`, gliding there sample by sample from the
/// last gain applied: a step would click.
pub fn apply_gain(samples: &mut [f64], ch: usize, applied: &mut f64, target: f64) {
    if *applied == target {
        if target != 1.0 {
            samples.iter_mut().for_each(|s| *s *= target);
        }
        return;
    }
    let frames = samples.len() / ch;
    if frames == 0 {
        return;
    }
    let step = (target - *applied) / frames as f64;
    let mut g = *applied;
    for f in 0..frames {
        g = if f + 1 == frames { target } else { g + step };
        for s in &mut samples[f * ch..(f + 1) * ch] {
            *s *= g;
        }
    }
    *applied = target;
}

#[cfg(test)]
mod tests {
    use super::*;
    use sonkore_eq::biquad::FilterType;
    use sonkore_eq::super_eq::Band;

    fn run(chain: &mut Chain, blocks: usize) {
        let mut block: Vec<f64> = (0..2048).map(|i| (i as f64 * 0.07).sin() * 0.25).collect();
        for _ in 0..blocks {
            chain.process(&mut block, 1.0, false, &mut |_| {});
        }
    }

    fn sine_block(frames: usize, from: usize) -> Vec<f64> {
        (from * 2..(from + frames) * 2).map(|i| ((i / 2) as f64 * 0.07 + (i % 2) as f64).sin() * 0.25).collect()
    }

    #[test]
    fn turning_the_limiter_off_keeps_what_it_held() {
        // Its look-ahead (1.5 ms) was dropped: a jump in the music.
        let shared = Arc::new(Dsp::default());
        shared.update(|s| s.limiter = true);
        let mut chain = Chain::new(shared.clone(), 44100, 44100, 2);
        let (mut input, mut output) = (Vec::new(), Vec::new());
        for b in 0..20 {
            if b == 10 {
                shared.update(|s| s.limiter = false);
                chain.sync();
            }
            let mut block = sine_block(1024, b * 1024);
            input.extend_from_slice(&block);
            chain.process(&mut block, 1.0, false, &mut |o| output.extend_from_slice(o));
        }
        // Below the ceiling the limiter's gain is exactly 1.
        assert_eq!(output.len(), input.len(), "every frame comes out");
        assert!(output == input, "unchanged and in order");
    }

    #[test]
    fn pitch_back_to_neutral_skips_nothing() {
        // The block that ended the stretching went into an emptied stretcher
        // that was then freed: ~23 ms of music skipped.
        let shared = Arc::new(Dsp::default());
        shared.stretch.set_enabled(true);
        shared.stretch.set_tempo(1.25);
        let mut chain = Chain::new(shared.clone(), 44100, 44100, 2);
        let mut frames = 0usize;
        for b in 0..60 {
            if b == 40 {
                shared.stretch.set_tempo(1.0);
            }
            let mut block = sine_block(1024, b * 1024);
            chain.process(&mut block, 1.0, false, &mut |o| frames += o.len() / 2);
        }
        let want = 41.0 * 1024.0 / 1.25 + 19.0 * 1024.0;
        assert!((frames as f64 - want).abs() < 4.0, "{frames} frames, want {want}");
    }

    /// The biquad EQ filtering, then linear phase, then the EQ off: it must
    /// come back to neutral (bit-perfect). The biquads' "still fading out"
    /// flag was only cleared by the biquads, which linear phase skips, so the
    /// curve stayed on for good.
    #[test]
    fn eq_off_in_linear_phase_is_neutral_again() {
        let shared = Arc::new(Dsp::default());
        shared.eq.set_bands(vec![Band {
            kind: FilterType::Peaking,
            freq_hz: 1000.0,
            gain_db: 6.0,
            q: 1.0,
            enabled: true,
        }]);
        shared.eq.set_enabled(true);
        let mut chain = Chain::new(shared.clone(), 44100, 44100, 2);
        run(&mut chain, 20);
        assert!(!shared.eq.is_neutral());
        shared.eq.set_linear_phase(true);
        run(&mut chain, 20);
        shared.eq.set_enabled(false);
        run(&mut chain, 200);
        assert!(shared.eq.is_neutral(), "the EQ is off");
        assert!(chain.neutral(), "nothing left in the chain");
    }
}
