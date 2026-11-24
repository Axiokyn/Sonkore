//! HDCD decoding (a bit-exact port of libhdcd) and CUE sheets (for the
//! decoder and the library).

pub mod cue;
pub mod hdcd;

pub use cue::{CueSheet, Track, TrackRange};
pub use hdcd::{Detected, Detection, Hdcd, PeakExtend};
