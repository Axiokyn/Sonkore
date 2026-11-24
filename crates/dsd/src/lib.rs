//! DSD: DSF/DFF, DST, DSD to PCM, PCM to DSD, DoP — the file reader, the DST
//! decoder, DSD to PCM, the sigma-delta modulator and the pure-logic half of
//! native DSD output.
//!
//! # How this maps onto `sonkore-engine`
//!
//! This crate does not depend on the engine, so it has its own [`StreamInfo`]
//! and [`Decoder`] trait, shaped like `sonkore_engine::decode`'s:
//!
//! | here | engine | note |
//! |---|---|---|
//! | `StreamInfo::rate` | `rate` | the *frame* rate: DSD rate / 16 (DoP), / 32 (native), the PCM rate for [`DsdToPcm`] |
//! | `channels`, `bits`, `float`, `lossless`, `codec` | same | `bits` 24 for DoP, 32 native, 0 for PCM made from DSD |
//! | `total_frames: u64` | `total_frames: Option<u64>` | always known: wrap in `Some` |
//! | `dsd: DsdMode`, `dsd_rate`, `decoder` | — | the DSD mode, the DSD rate and the decoder's name (signal path) |
//! | `Decoder::{info, read_i32, read_f64, seek}` | `Decoder::{…}` | same signatures and meaning |
//!
//! An adapter in the engine (`impl engine::Decoder for Wrap<D: sonkore_dsd::Decoder>`)
//! forwards the four calls and converts the info once.
//!
//! Packed DSD ([`DsdDecoder`], [`SigmaDelta`]) is not PCM: the words go to the
//! device untouched. A word's *memory* bytes are what the DAC receives
//! (`i32::to_ne_bytes`), so the native words are written to ALSA
//! (`DSD_U32_BE` / `DSD_U32_LE`) as they are; DoP words are left-justified
//! s32 (`marker << 24 | bits << 8`) for an S32 / S24 integer device, and the
//! output stamps the real marker on what it sends ([`output::stamp_dop_markers`]).

pub mod decoder;
pub mod dst;
pub mod output;
pub mod sigma_delta;
pub mod to_pcm;

pub use decoder::{DsdDecoder, DsdFormat, frame_rate, is_dsd_extension, probe, rate_label, silence};
pub use dst::DstDecoder;
pub use sigma_delta::SigmaDelta;
pub use to_pcm::DsdToPcm;

/// The DSD idle pattern: 0 V average, no click.
pub const SILENCE_BYTE: u8 = 0x69;
/// DoP markers, alternating frame by frame (false = 0x05, true = 0xFA).
pub const DOP_MARKERS: [u8; 2] = [0x05, 0xFA];

/// How the 1-bit stream reaches the DAC.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DsdPacking {
    /// DSD over PCM: 16 bits per channel in a 24-bit sample, 0x05/0xFA marker.
    Dop,
    /// ALSA `DSD_U32_BE`: 32 bits per channel, oldest bit first.
    NativeU32Be,
    /// ALSA `DSD_U32_LE`: the same 4 bytes in reverse order.
    NativeU32Le,
}

impl DsdPacking {
    /// DSD bits per channel in one output frame: 16 (DoP) or 32.
    pub fn bits_per_word(self) -> usize {
        if self == DsdPacking::Dop { 16 } else { 32 }
    }
    pub fn is_native(self) -> bool {
        self != DsdPacking::Dop
    }
}

/// What the samples are.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DsdMode {
    /// PCM (also PCM made from DSD).
    None,
    /// DoP words: through an integer PCM device, untouched.
    Dop,
    /// Native DSD words: to a DSD_U32 device.
    Native,
}

/// What a stream is (see the crate docs for the engine's).
#[derive(Clone, Debug, PartialEq)]
pub struct StreamInfo {
    /// Frame rate: DSD rate / 16 (DoP), / 32 (native), or the PCM rate.
    pub rate: u32,
    pub channels: usize,
    /// 24 (DoP), 32 (native), 0 (PCM converted from DSD: no source depth).
    pub bits: u32,
    /// The samples are floating point (PCM converted from DSD).
    pub float: bool,
    /// Bit for bit the file (packed DSD); false for the conversion.
    pub lossless: bool,
    pub total_frames: u64,
    /// "DSD64", "DSD64 DST DoP", "DSD128 native"…
    pub codec: String,
    pub dsd: DsdMode,
    /// The 1-bit rate (2822400 for DSD64), also for PCM made from DSD.
    pub dsd_rate: u32,
    /// "DSD reader" or "DST decoder" (the signal path's stage name).
    pub decoder: &'static str,
}

impl StreamInfo {
    pub fn duration_seconds(&self) -> f64 {
        self.total_frames as f64 / self.rate.max(1) as f64
    }
}

#[derive(Debug)]
pub struct DsdError(pub String);

impl std::fmt::Display for DsdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for DsdError {}

impl From<std::io::Error> for DsdError {
    fn from(e: std::io::Error) -> Self {
        DsdError(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, DsdError>;

/// A decoded stream, the same contract as `sonkore_engine::decode::Decoder`.
pub trait Decoder: Send {
    fn info(&self) -> &StreamInfo;
    /// Up to `out.len() / channels` frames; 0 at the end. Packed DSD words for
    /// [`DsdDecoder`], left-justified s32 PCM for [`DsdToPcm`].
    fn read_i32(&mut self, out: &mut [i32]) -> Result<usize>;
    /// Up to `out.len() / channels` frames as f64 (1.0 = full scale; for
    /// packed DSD, the words / 2^31).
    fn read_f64(&mut self, out: &mut [f64]) -> Result<usize>;
    /// Goes to `frame` (clamped to the stream); returns where it is.
    fn seek(&mut self, frame: u64) -> Result<u64>;
}

/// Anything a DSD file can be read from (a file, or a file in memory).
pub trait Source: std::io::Read + std::io::Seek + Send {}
impl<T: std::io::Read + std::io::Seek + Send> Source for T {}

/// A double at 1.0 = full scale to s32, rounded to nearest (ties to even, as
/// `lrint`), clamped.
#[inline]
pub fn double_to_s32(v: f64) -> i32 {
    let v = v * 2147483648.0;
    if v >= 2147483647.0 {
        i32::MAX
    } else if v <= -2147483648.0 {
        i32::MIN
    } else {
        v.round_ties_even() as i32
    }
}
