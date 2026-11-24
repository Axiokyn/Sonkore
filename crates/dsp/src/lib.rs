//! DSP stages of the processed path: the true-peak limiter, bs2b crossfeed,
//! partitioned convolution, sample-rate conversion and the test signals.
//!
//! All stages work on interleaved f64 frames, up to 8 channels. Their
//! processing calls neither allocate nor lock once built, so they can run on
//! the engine's worker thread.

pub mod convolver;
pub mod crossfeed;
pub mod limiter;
pub mod resampler;
pub mod test_signals;

pub use convolver::{Convolver, ImpulseResponse};
pub use crossfeed::{Crossfeed, Preset as CrossfeedPreset};
pub use limiter::Limiter;
pub use resampler::{Filter as ResampleFilter, Resampler};
