//! The equalizer (the biquads, the multiband cascade, the linear-phase FIR
//! and the controller's logic):
//!
//! * [`biquad`]: the second-order sections (RBJ and Vicanek's matched
//!   designs), coefficient for coefficient the recorded reference's.
//! * [`SuperEq`] / [`EqProcessor`]: the shared curve and the DSP worker's
//!   real-time biquad cascade (up to 32 bands × 8 channels, f64, ~10 ms
//!   on/off crossfade), plus the graphic-mode Newton solve.
//! * [`LinearPhaseEq`]: the same curve as a linear-phase FIR.
//! * [`EqController`]: graphic / parametric editing, profiles, Equalizer APO
//!   import / export, AutoEQ, and the settings earlier versions wrote.

// The original constants are kept digit for digit (Q 1.41421356237, 0.7071),
// not replaced by the exact √2 values.
#![allow(clippy::approx_constant)]

pub mod apo;
pub mod autoeq;
pub mod biquad;
pub mod controller;
mod convolver;
pub mod linear_phase;
pub mod numfmt;
pub mod super_eq;

pub use biquad::FilterType;
pub use controller::{AutoEqNext, Band, EqController, ImportError, Imported, Mode, Profile, ResponsePoint};
pub use linear_phase::LinearPhaseEq;
pub use super_eq::{Curve, EqProcessor, MAX_BANDS, MAX_CHANNELS, SuperEq};
