//! The pure-logic half of DSD output (the native ALSA DSD device and the DSD
//! parts of the engine): which ALSA format a packing is, period and buffer sizes,
//! the DoP marker stamping, DSD silence and pre-roll, and the PCM→DSD rate
//! ladder. The ALSA device thread itself lives in the engine.
//!
//! # Opening a native DSD device
//!
//! * Only `hw:` devices (no plugin can convert DSD).
//! * Probing ([`NATIVE_CANDIDATES`], in order): `hw_params_any`, access
//!   `RW_INTERLEAVED`, format [`AlsaDsdFormat::for_packing`], channels, rate =
//!   [`frame_rate`](crate::frame_rate)`(dsd_rate, NativeU32*)` exactly
//!   (`set_rate(rate, 0)`); the first that is accepted is the packing.
//!   Open non-blocking (`SND_PCM_NONBLOCK`).
//! * Configure: [`PERIOD_US`] with `set_period_time_near`, [`PERIODS`] with
//!   `set_periods_near`, then sw params: start threshold = the whole buffer,
//!   avail_min = one period ([`SwParams::for_sizes`]).
//! * Writer thread: SCHED_FIFO at min + 10 when allowed; render one period
//!   of words, `writei` until it is all written; on `EAGAIN` wait up to 20 ms
//!   (so a stop is noticed); other errors through `snd_pcm_recover(err, 1)`,
//!   giving up when that fails. Stop: clear the flag, join, `drop`.
//! * The words are written as they are: an `i32`'s memory bytes are what the
//!   DAC receives.

use crate::{DOP_MARKERS, DsdPacking, silence};

/// Period length: 3 × 40 ms, like the PCM path.
pub const PERIOD_US: u32 = 40_000;
pub const PERIODS: u32 = 3;
/// `snd_pcm_wait` timeout on `EAGAIN`, so the writer notices a stop.
pub const WAIT_MS: i32 = 20;
/// DSD silence ahead of the music, while the DAC switches mode.
pub const PREROLL_MS: u32 = 60;

/// The packings a native device is probed with, in order.
pub const NATIVE_CANDIDATES: [DsdPacking; 2] = [DsdPacking::NativeU32Be, DsdPacking::NativeU32Le];

/// ALSA's native DSD formats (`SND_PCM_FORMAT_DSD_U32_BE` / `_LE`;
/// `alsa::pcm::Format::DSDU32BE` / `DSDU32LE` in the `alsa` crate).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlsaDsdFormat {
    DsdU32Be,
    DsdU32Le,
}

impl AlsaDsdFormat {
    /// The format for a native packing (DoP is not native: it maps to BE,
    /// but is never opened this way).
    pub fn for_packing(packing: DsdPacking) -> Self {
        if packing == DsdPacking::NativeU32Le { AlsaDsdFormat::DsdU32Le } else { AlsaDsdFormat::DsdU32Be }
    }

    pub fn packing(self) -> DsdPacking {
        match self {
            AlsaDsdFormat::DsdU32Be => DsdPacking::NativeU32Be,
            AlsaDsdFormat::DsdU32Le => DsdPacking::NativeU32Le,
        }
    }

    /// The ALSA name ("DSD_U32_BE").
    pub fn name(self) -> &'static str {
        match self {
            AlsaDsdFormat::DsdU32Be => "DSD_U32_BE",
            AlsaDsdFormat::DsdU32Le => "DSD_U32_LE",
        }
    }

    /// `snd_pcm_format_t` value (alsa-lib: DSD_U32_LE = 50, DSD_U32_BE = 52).
    pub fn alsa_value(self) -> i32 {
        match self {
            AlsaDsdFormat::DsdU32Le => 50,
            AlsaDsdFormat::DsdU32Be => 52,
        }
    }
}

/// The software parameters for the sizes ALSA chose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SwParams {
    /// Start once the buffer is full.
    pub start_threshold: u64,
    pub avail_min: u64,
}

impl SwParams {
    pub fn for_sizes(period_frames: u64, buffer_frames: u64) -> Self {
        SwParams { start_threshold: buffer_frames, avail_min: period_frames }
    }
}

/// What `set_period_time_near(40 ms)` asks for, in frames at a frame rate
/// (ALSA may pick another size: use what it returns).
pub fn nominal_period_frames(frame_rate: u32) -> u64 {
    frame_rate as u64 * PERIOD_US as u64 / 1_000_000
}

pub fn nominal_buffer_frames(frame_rate: u32) -> u64 {
    nominal_period_frames(frame_rate) * PERIODS as u64
}

/// The pre-roll's length at the output frame rate: 60 ms, an even number of
/// frames, so a DoP pre-roll ends on 0xFA and the music starts on 0x05.
pub fn preroll_frames(output_rate: u32) -> usize {
    (output_rate as usize * PREROLL_MS as usize / 1000) & !1
}

/// DSD silence for the pre-roll: frame f gets the
/// marker of phase `f & 1`. `out.len()` is frames × channels.
pub fn fill_preroll(out: &mut [i32], channels: usize, packing: DsdPacking) {
    for (f, frame) in out.chunks_exact_mut(channels).enumerate() {
        frame.fill(silence(packing, f & 1 == 1));
    }
}

/// Silence for the device in DSD mode: the idle pattern,
/// with DoP markers that keep alternating from `phase` (zeros there would be
/// a full-scale click).
pub fn fill_silence(out: &mut [i32], channels: usize, packing: DsdPacking, phase: &mut bool) {
    for frame in out.chunks_exact_mut(channels) {
        frame.fill(silence(packing, *phase));
        *phase = !*phase;
    }
}

/// Stamps the DoP markers on what leaves for the DAC:
/// they alternate without fail across underrun fills, joins and seeks,
/// whatever the count of frames in between. The 16 DSD bits of each sample
/// are not touched.
pub fn stamp_dop_markers(out: &mut [i32], channels: usize, phase: &mut bool) {
    for frame in out.chunks_exact_mut(channels) {
        let marker = (DOP_MARKERS[*phase as usize] as u32) << 24;
        for s in frame {
            *s = ((*s as u32 & 0x00FF_FFFF) | marker) as i32;
        }
        *phase = !*phase;
    }
}

/// The PCM→DSD setting (the stored order and values).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PcmToDsd {
    Off = 0,
    Dsd64 = 1,
    Dsd128 = 2,
    Dsd256 = 3,
    /// The highest the DAC takes, up to DSD256.
    Max = 4,
    Dsd512 = 5,
}

/// The DSD multiples tried, best first (`openPcmToDsd`): Max goes up to
/// DSD256; DSD512 only when chosen (a 22.6 MHz loop per channel).
pub fn pcm_to_dsd_multiples(mode: PcmToDsd) -> &'static [u32] {
    match mode {
        PcmToDsd::Off => &[],
        PcmToDsd::Dsd64 => &[64],
        PcmToDsd::Dsd128 => &[128, 64],
        PcmToDsd::Dsd256 | PcmToDsd::Max => &[256, 128, 64],
        PcmToDsd::Dsd512 => &[512, 256, 128, 64],
    }
}

/// 44100 × multiple.
pub fn dsd_rate_for_multiple(multiple: u32) -> u32 {
    44_100 * multiple
}

/// DoP rides the PCM path, so only where nothing can alter a sample: the
/// device at exactly the DoP frame rate, in an integer format of 24 bits or
/// more (and an exclusive, bit-perfect path, which the engine checks).
pub fn dop_device_ok(device_rate: u32, dsd_rate: u32, device_float: bool, device_bits: u32) -> bool {
    device_rate == crate::frame_rate(dsd_rate, DsdPacking::Dop) && !device_float && device_bits >= 24
}

/// The words the modulator can write for a block of `frames` PCM frames
/// (`frames / bits per word + 1`), per channel.
pub fn modulator_words(frames: usize, packing: DsdPacking) -> usize {
    frames / packing.bits_per_word() + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dop_stamping_keeps_alternating() {
        let mut phase = false;
        let mut buf = vec![0x0012_3400i32; 6]; // 3 frames, stereo
        stamp_dop_markers(&mut buf, 2, &mut phase);
        assert_eq!(buf[0] as u32, 0x0512_3400);
        assert_eq!(buf[1] as u32, 0x0512_3400);
        assert_eq!(buf[2] as u32, 0xFA12_3400);
        assert_eq!(buf[4] as u32, 0x0512_3400);
        assert!(phase);
        let mut sil = vec![0i32; 4];
        fill_silence(&mut sil, 2, DsdPacking::Dop, &mut phase);
        assert_eq!(sil[0] as u32, 0xFA69_6900);
        assert_eq!(sil[2] as u32, 0x0569_6900);
        assert!(phase);
    }

    #[test]
    fn preroll_and_sizes() {
        assert_eq!(preroll_frames(176_400), 10_584);
        assert_eq!(preroll_frames(88_200) % 2, 0);
        let mut pre = vec![0i32; 4 * 2];
        fill_preroll(&mut pre, 2, DsdPacking::Dop);
        assert_eq!(pre[6] as u32 >> 24, 0xFA); // ends on 0xFA
        assert_eq!(nominal_period_frames(88_200), 3528);
        assert_eq!(nominal_buffer_frames(88_200), 3528 * 3);
        assert_eq!(SwParams::for_sizes(3528, 10584), SwParams { start_threshold: 10584, avail_min: 3528 });
        assert_eq!(AlsaDsdFormat::for_packing(DsdPacking::NativeU32Le).name(), "DSD_U32_LE");
        assert_eq!(pcm_to_dsd_multiples(PcmToDsd::Max), &[256, 128, 64]);
        assert!(dop_device_ok(176_400, 2_822_400, false, 24));
        assert!(!dop_device_ok(176_400, 2_822_400, true, 32));
    }
}
