//! Sonkore's pitch / tempo / rate stage on top of SoundTouch (TimeStretch).
//!
//! The settings live in a [`StretchControl`] that any thread may change:
//! each setter stores the value in an atomic and raises a dirty bit. The DSP
//! worker owns the [`TimeStretch`] and calls
//! [`TimeStretch::apply_pending_changes`] at every block boundary, which
//! forwards the values to SoundTouch. Pitch, tempo and rate stay `f64` end
//! to end.
//!
//! Memory: SoundTouch is only built while it has something to do, and
//! [`TimeStretch::release`] frees it again (the worker calls it once the
//! values are back to neutral), so a bit-perfect session holds no buffers.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering};

use crate::MAX_SAMPLE_RATE;
use crate::soundtouch::SoundTouch;

/// Frames received per call when draining.
pub const CHUNK_FRAMES: usize = 4096;

pub const MIN_SEMITONES: f64 = -48.0;
pub const MAX_SEMITONES: f64 = 48.0;
pub const MIN_RATIO: f64 = 0.05;
pub const MAX_RATIO: f64 = 20.0;

const DIRTY_RATIOS: u32 = 1 << 0;
const DIRTY_SOUNDTOUCH: u32 = 1 << 1;
const DIRTY_ALL: u32 = DIRTY_RATIOS | DIRTY_SOUNDTOUCH;

/// SoundTouch's tuning, as the settings store it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SoundTouchSettings {
    pub use_aa_filter: bool,
    /// 8..=128, a multiple of 8.
    pub aa_filter_length: i32,
    pub use_quick_seek: bool,
    /// 0 = automatic.
    pub sequence_ms: i32,
    /// 0 = automatic.
    pub seek_window_ms: i32,
    pub overlap_ms: i32,
}

impl Default for SoundTouchSettings {
    fn default() -> Self {
        SoundTouchSettings {
            use_aa_filter: true,
            aa_filter_length: 64,
            use_quick_seek: false,
            sequence_ms: 0,
            seek_window_ms: 0,
            overlap_ms: 8,
        }
    }
}

/// An `f64` in an `AtomicU64`.
struct AtomicF64(AtomicU64);

impl AtomicF64 {
    fn new(v: f64) -> Self {
        AtomicF64(AtomicU64::new(v.to_bits()))
    }
    fn load(&self) -> f64 {
        f64::from_bits(self.0.load(Ordering::Relaxed))
    }
    fn store(&self, v: f64) {
        self.0.store(v.to_bits(), Ordering::Relaxed);
    }
}

/// The values shared between the interface and the DSP worker.
pub struct StretchControl {
    pitch_semitones: AtomicF64,
    tempo: AtomicF64,
    rate: AtomicF64,
    enabled: AtomicBool,
    use_aa_filter: AtomicBool,
    aa_filter_length: AtomicI32,
    use_quick_seek: AtomicBool,
    sequence_ms: AtomicI32,
    seek_window_ms: AtomicI32,
    overlap_ms: AtomicI32,
    dirty: AtomicU32,
    /// Worker-written: source frames in minus frames out times the speed.
    held_source: AtomicF64,
}

impl Default for StretchControl {
    fn default() -> Self {
        let s = SoundTouchSettings::default();
        StretchControl {
            pitch_semitones: AtomicF64::new(0.0),
            tempo: AtomicF64::new(1.0),
            rate: AtomicF64::new(1.0),
            enabled: AtomicBool::new(false), // a fresh install starts bit-perfect
            use_aa_filter: AtomicBool::new(s.use_aa_filter),
            aa_filter_length: AtomicI32::new(s.aa_filter_length),
            use_quick_seek: AtomicBool::new(s.use_quick_seek),
            sequence_ms: AtomicI32::new(s.sequence_ms),
            seek_window_ms: AtomicI32::new(s.seek_window_ms),
            overlap_ms: AtomicI32::new(s.overlap_ms),
            dirty: AtomicU32::new(DIRTY_ALL),
            held_source: AtomicF64::new(0.0),
        }
    }
}

impl StretchControl {
    pub fn new() -> Self {
        Self::default()
    }

    fn mark_dirty(&self, bits: u32) {
        self.dirty.fetch_or(bits, Ordering::Release);
    }

    /// Clamped to ±48. A NaN is ignored.
    pub fn set_pitch_semitones(&self, semitones: f64) {
        if !semitones.is_nan() {
            self.pitch_semitones.store(semitones.clamp(MIN_SEMITONES, MAX_SEMITONES));
            self.mark_dirty(DIRTY_RATIOS);
        }
    }

    /// Clamped to 0.05..=20. A NaN is ignored.
    pub fn set_tempo(&self, ratio: f64) {
        if !ratio.is_nan() {
            self.tempo.store(ratio.clamp(MIN_RATIO, MAX_RATIO));
            self.mark_dirty(DIRTY_RATIOS);
        }
    }

    /// Clamped to 0.05..=20. A NaN is ignored.
    pub fn set_rate(&self, ratio: f64) {
        if !ratio.is_nan() {
            self.rate.store(ratio.clamp(MIN_RATIO, MAX_RATIO));
            self.mark_dirty(DIRTY_RATIOS);
        }
    }

    pub fn pitch_semitones(&self) -> f64 {
        self.pitch_semitones.load()
    }

    pub fn tempo(&self) -> f64 {
        self.tempo.load()
    }

    pub fn rate(&self) -> f64 {
        self.rate.load()
    }

    pub fn set_use_aa_filter(&self, on: bool) {
        self.use_aa_filter.store(on, Ordering::Relaxed);
        self.mark_dirty(DIRTY_SOUNDTOUCH);
    }

    /// Snapped to the nearest multiple of 8 (halves up), within 8..=128.
    pub fn set_aa_filter_length(&self, taps: i32) {
        let taps = (taps.saturating_add(4) / 8 * 8).clamp(8, 128);
        self.aa_filter_length.store(taps, Ordering::Relaxed);
        self.mark_dirty(DIRTY_SOUNDTOUCH);
    }

    pub fn set_use_quick_seek(&self, on: bool) {
        self.use_quick_seek.store(on, Ordering::Relaxed);
        self.mark_dirty(DIRTY_SOUNDTOUCH);
    }

    /// 0 (automatic) to 500.
    pub fn set_sequence_ms(&self, ms: i32) {
        self.sequence_ms.store(ms.clamp(0, 500), Ordering::Relaxed);
        self.mark_dirty(DIRTY_SOUNDTOUCH);
    }

    /// 0 (automatic) to 250.
    pub fn set_seek_window_ms(&self, ms: i32) {
        self.seek_window_ms.store(ms.clamp(0, 250), Ordering::Relaxed);
        self.mark_dirty(DIRTY_SOUNDTOUCH);
    }

    /// 1 to 100.
    pub fn set_overlap_ms(&self, ms: i32) {
        self.overlap_ms.store(ms.clamp(1, 100), Ordering::Relaxed);
        self.mark_dirty(DIRTY_SOUNDTOUCH);
    }

    pub fn soundtouch_settings(&self) -> SoundTouchSettings {
        SoundTouchSettings {
            use_aa_filter: self.use_aa_filter.load(Ordering::Relaxed),
            aa_filter_length: self.aa_filter_length.load(Ordering::Relaxed),
            use_quick_seek: self.use_quick_seek.load(Ordering::Relaxed),
            sequence_ms: self.sequence_ms.load(Ordering::Relaxed),
            seek_window_ms: self.seek_window_ms.load(Ordering::Relaxed),
            overlap_ms: self.overlap_ms.load(Ordering::Relaxed),
        }
    }

    /// Off: the values are kept but not applied (bypassed as if neutral).
    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
    }

    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// The values differ from neutral, whether or not the stage is on.
    /// Exact comparison on purpose: -0.0000001 semitones is a real change.
    pub fn has_effect(&self) -> bool {
        !(self.pitch_semitones() == 0.0 && self.tempo() == 1.0 && self.rate() == 1.0)
    }

    /// Nothing to apply: switched off, or neutral values.
    pub fn is_neutral(&self) -> bool {
        !self.enabled() || !self.has_effect()
    }

    /// Source frames consumed per output frame (tempo × rate; 1 while
    /// neutral).
    pub fn speed_factor(&self) -> f64 {
        if self.is_neutral() { 1.0 } else { self.tempo() * self.rate() }
    }

    /// Source frames given to SoundTouch that have not come out yet (its
    /// latency plus what waits for a full analysis window), so the playback
    /// position can leave them out: ~50-100 ms.
    pub fn held_source_frames(&self) -> f64 {
        self.held_source.load()
    }
}

/// The worker's side: owns SoundTouch while the stage is active.
pub struct TimeStretch {
    control: Arc<StretchControl>,
    backend: Option<SoundTouch>,
    sample_rate: u32,
    channels: usize,
    max_block_frames: usize,
    applied_speed: f64, // tempo × rate SoundTouch runs at
}

impl TimeStretch {
    /// A stage with its own (disabled, neutral) control.
    pub fn new(sample_rate: u32, channels: usize) -> Self {
        Self::with_control(Arc::new(StretchControl::new()), sample_rate, channels)
    }

    /// A stage driven by a control shared with other threads.
    pub fn with_control(control: Arc<StretchControl>, sample_rate: u32, channels: usize) -> Self {
        TimeStretch { control, backend: None, sample_rate, channels, max_block_frames: 1024, applied_speed: 1.0 }
    }

    pub fn control(&self) -> &Arc<StretchControl> {
        &self.control
    }

    pub fn is_neutral(&self) -> bool {
        self.control.is_neutral()
    }

    pub fn speed_factor(&self) -> f64 {
        self.control.speed_factor()
    }

    pub fn held_source_frames(&self) -> f64 {
        self.control.held_source_frames()
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// SoundTouch, while it is built.
    pub fn backend(&self) -> Option<&SoundTouch> {
        self.backend.as_ref()
    }

    /// New track format: a different one frees SoundTouch (it is rebuilt
    /// when next needed); the same one only clears it.
    pub fn prepare(&mut self, sample_rate: u32, channels: usize, max_block_frames: usize) {
        if sample_rate != self.sample_rate || channels != self.channels || max_block_frames != self.max_block_frames {
            self.release();
            self.sample_rate = sample_rate;
            self.channels = channels;
            self.max_block_frames = max_block_frames;
        } else {
            self.clear();
        }
    }

    /// Builds SoundTouch if needed and forwards the pending values to it.
    /// Nothing happens while neutral: the values wait.
    ///
    /// A channel count SoundTouch cannot take (0 or above 16) leaves the
    /// stage unbuilt: it then takes and returns nothing.
    pub fn apply_pending_changes(&mut self) {
        let c = &*self.control;
        if c.is_neutral() {
            return;
        }
        let mut dirty = c.dirty.swap(0, Ordering::Acquire);
        if self.backend.is_none() {
            // SoundTouch refuses rates above 192 kHz (352.8 / 384 kHz PCM,
            // DSD read as PCM). The rate only turns its millisecond settings
            // into frames, so it is capped: the windows get shorter in time,
            // the ratios stay exact.
            let Ok(mut st) = SoundTouch::new(self.sample_rate.min(MAX_SAMPLE_RATE), self.channels) else {
                c.mark_dirty(dirty);
                return;
            };
            st.reserve(self.max_block_frames.max(CHUNK_FRAMES));
            self.backend = Some(st);
            dirty = DIRTY_ALL;
        }
        let Some(st) = self.backend.as_mut() else { return };
        if dirty & DIRTY_RATIOS != 0 {
            let (pitch, tempo, rate) = (c.pitch_semitones(), c.tempo(), c.rate());
            st.set_pitch_semitones(pitch);
            st.set_tempo(tempo);
            st.set_rate(rate);
            self.applied_speed = tempo * rate;
        }
        if dirty & DIRTY_SOUNDTOUCH != 0 {
            let s = c.soundtouch_settings();
            st.set_aa_filter(s.use_aa_filter);
            st.set_aa_filter_length(s.aa_filter_length as usize);
            st.set_quick_seek(s.use_quick_seek);
            st.set_sequence_ms(s.sequence_ms);
            st.set_seek_window_ms(s.seek_window_ms);
            st.set_overlap_ms(s.overlap_ms);
        }
    }

    /// Feeds interleaved frames. Ignored while SoundTouch is not built.
    pub fn put_samples(&mut self, interleaved: &[f32]) {
        let Some(st) = self.backend.as_mut() else { return };
        st.put_samples(interleaved);
        let frames = interleaved.len() / self.channels;
        let c = &self.control;
        c.held_source.store(c.held_source.load() + frames as f64);
    }

    /// Copies up to `out.len() / channels` processed frames; returns how many.
    pub fn receive_samples(&mut self, out: &mut [f32]) -> usize {
        let Some(st) = self.backend.as_mut() else { return 0 };
        let got = st.receive_samples(out);
        if got > 0 {
            let c = &self.control;
            let held = c.held_source.load() - got as f64 * self.applied_speed;
            c.held_source.store(held.max(0.0));
        }
        got
    }

    /// Frames ready to be received.
    pub fn available_frames(&self) -> usize {
        self.backend.as_ref().map_or(0, SoundTouch::num_samples)
    }

    /// Puts `input`, then appends everything ready to `output`, received in
    /// [`CHUNK_FRAMES`] chunks as the engine does. `output` only allocates
    /// if it has to grow.
    pub fn process(&mut self, input: &[f32], output: &mut Vec<f32>) {
        self.put_samples(input);
        self.drain_into(output);
    }

    /// Appends everything ready to `output`, in [`CHUNK_FRAMES`] chunks.
    pub fn drain_into(&mut self, output: &mut Vec<f32>) {
        let ch = self.channels;
        loop {
            let n = self.available_frames().min(CHUNK_FRAMES);
            if n == 0 {
                return;
            }
            let start = output.len();
            output.resize(start + n * ch, 0.0);
            let got = self.receive_samples(&mut output[start..]);
            output.truncate(start + got * ch);
            if got == 0 {
                return;
            }
        }
    }

    /// End of stream: pushes the tail out (receive it afterwards).
    pub fn flush(&mut self) {
        if let Some(st) = self.backend.as_mut() {
            st.flush();
        }
    }

    /// Drops everything buffered (a seek, or entering the active state).
    pub fn clear(&mut self) {
        if let Some(st) = self.backend.as_mut() {
            st.clear();
        }
        self.control.held_source.store(0.0);
    }

    /// Frees SoundTouch; it is rebuilt, with every value, when next needed.
    pub fn release(&mut self) {
        self.backend = None;
        self.control.held_source.store(0.0);
        self.control.mark_dirty(DIRTY_ALL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setters_clamp_and_snap() {
        let c = StretchControl::new();
        c.set_pitch_semitones(60.0);
        assert_eq!(c.pitch_semitones(), 48.0);
        c.set_pitch_semitones(-100.0);
        assert_eq!(c.pitch_semitones(), -48.0);
        c.set_pitch_semitones(f64::NAN);
        assert_eq!(c.pitch_semitones(), -48.0);
        c.set_tempo(0.0);
        assert_eq!(c.tempo(), 0.05);
        c.set_rate(100.0);
        assert_eq!(c.rate(), 20.0);
        for (taps, want) in [(0, 8), (3, 8), (11, 8), (12, 16), (64, 64), (67, 64), (68, 72), (500, 128), (-20, 8)] {
            c.set_aa_filter_length(taps);
            assert_eq!(c.soundtouch_settings().aa_filter_length, want, "{taps} taps");
        }
        c.set_sequence_ms(900);
        c.set_seek_window_ms(-3);
        c.set_overlap_ms(0);
        let s = c.soundtouch_settings();
        assert_eq!((s.sequence_ms, s.seek_window_ms, s.overlap_ms), (500, 0, 1));
    }

    #[test]
    fn neutral_is_exact() {
        let c = StretchControl::new();
        assert!(c.is_neutral(), "disabled by default");
        c.set_enabled(true);
        assert!(c.is_neutral());
        assert_eq!(c.speed_factor(), 1.0);
        c.set_pitch_semitones(-0.0000001);
        assert!(!c.is_neutral());
        c.set_pitch_semitones(0.0);
        c.set_tempo(1.25);
        c.set_rate(0.8);
        assert!(!c.is_neutral());
        assert_eq!(c.speed_factor(), 1.25 * 0.8);
        c.set_enabled(false);
        assert!(c.has_effect() && c.is_neutral());
        assert_eq!(c.speed_factor(), 1.0);
    }

    #[test]
    fn built_lazily_and_released() {
        let mut ts = TimeStretch::new(48000, 2);
        ts.apply_pending_changes();
        assert!(ts.backend().is_none(), "neutral: nothing built");
        ts.control().set_enabled(true);
        ts.control().set_pitch_semitones(2.0);
        ts.control().set_use_quick_seek(true);
        ts.apply_pending_changes();
        let st = ts.backend().expect("built");
        assert!(st.quick_seek());
        assert!((st.effective_rate() - (crate::soundtouch::SOUNDTOUCH_LN2 * 2.0 / 12.0).exp()).abs() < 1e-15);

        ts.put_samples(&vec![0.1; 16384 * 2]);
        assert_eq!(ts.held_source_frames(), 16384.0);
        let mut out = vec![0.0; CHUNK_FRAMES * 2];
        while ts.receive_samples(&mut out) > 0 {}
        let held = ts.held_source_frames();
        assert!(held > 0.0 && held < 16384.0, "{held}");

        ts.release();
        assert!(ts.backend().is_none());
        assert_eq!(ts.held_source_frames(), 0.0);
        // Rebuilt with every value, not only the dirty ones.
        ts.apply_pending_changes();
        assert!(ts.backend().expect("rebuilt").quick_seek());
    }

    #[test]
    fn high_rates_are_capped_and_bad_layouts_stay_unbuilt() {
        let mut ts = TimeStretch::new(384000, 2);
        ts.control().set_enabled(true);
        ts.control().set_tempo(1.1);
        ts.apply_pending_changes();
        assert_eq!(ts.backend().expect("built").sample_rate(), 192000);

        let mut ts = TimeStretch::new(48000, 0);
        ts.control().set_enabled(true);
        ts.control().set_tempo(1.1);
        ts.apply_pending_changes();
        assert!(ts.backend().is_none());
        ts.put_samples(&[0.0; 64]);
        assert_eq!(ts.receive_samples(&mut [0.0; 64]), 0);
        // Once the layout is valid the pending values are applied.
        ts.prepare(48000, 2, 1024);
        ts.apply_pending_changes();
        assert!((ts.backend().expect("built").effective_tempo() - 1.1).abs() < 1e-15);
    }

    #[test]
    fn prepare_same_format_only_clears() {
        let mut ts = TimeStretch::new(44100, 2);
        ts.control().set_enabled(true);
        ts.control().set_rate(1.5);
        ts.prepare(44100, 2, 1024);
        ts.apply_pending_changes();
        ts.put_samples(&vec![0.2; 4096 * 2]);
        ts.prepare(44100, 2, 1024);
        assert!(ts.backend().is_some());
        assert_eq!(ts.held_source_frames(), 0.0);
        assert_eq!(ts.available_frames(), 0);
        ts.prepare(48000, 2, 1024);
        assert!(ts.backend().is_none());
    }
}
