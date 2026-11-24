//! Sonkore's audio engine: decoding, the DSP chain and the outputs (ALSA
//! direct / bit-perfect, PipeWire).
//!
//! See docs/engine.md for what the engine does and must keep.

pub mod decode;
pub mod dsp;
pub mod engine;
pub mod output;
pub mod player;
pub mod quantize;
pub mod queue;
pub mod ring;

pub use engine::{Engine, EngineError, ErrorKind, OutputStatus, State};
pub use player::{Changes, Player};
pub use queue::{Queue, Repeat};
