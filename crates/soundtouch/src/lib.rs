//! SoundTouch 2.3.3 (pitch, tempo, rate) ported to Rust, with Sonkore's
//! TimeStretch stage on top.
//!
//! The port follows SoundTouch's float build with its plain kernels
//! (no SSE), operation for operation, so it produces the same samples bit
//! for bit: WSOLA time stretch with the full or the quick seek, the rate
//! transposer with linear, cubic or Shannon interpolation, and the
//! Hamming-windowed anti-alias FIR. BPM detection is not included.
//!
//! The hot loops score many candidates (overlap positions, FIR outputs)
//! side by side, each still summed in SoundTouch's order, so they vectorise
//! without changing a bit. Steady-state processing does not allocate.
//!
//! SoundTouch built with its SSE kernels and `-Ofast` adds those sums in
//! another order. Against that the samples differ by at most ~6e-7 (about one float
//! ulp) while both pick the same splice points; where two overlap positions
//! score almost the same it may pick the other one, and from there on the
//! two outputs are equally valid but no longer comparable sample by sample.
//!
//! - [`SoundTouch`]: the processor itself.
//! - [`TimeStretch`] / [`StretchControl`]: the engine stage, with settings
//!   any thread can change and the worker applies at block boundaries.

mod aa_filter;
mod fifo;
mod interpolate;
mod rate_transposer;
mod soundtouch;
mod tdstretch;
mod time_stretch;

pub use interpolate::Interpolation;
pub use soundtouch::{Error, SoundTouch};
pub use time_stretch::{
    CHUNK_FRAMES, MAX_RATIO, MAX_SEMITONES, MIN_RATIO, MIN_SEMITONES, SoundTouchSettings, StretchControl, TimeStretch,
};

/// Most channels SoundTouch takes.
pub const MAX_CHANNELS: usize = 16;

/// Highest sample rate SoundTouch takes; [`TimeStretch`] caps to it.
pub const MAX_SAMPLE_RATE: u32 = 192_000;
