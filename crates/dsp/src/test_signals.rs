//! Test signals: files made on the spot to check an output, which the engine
//! then plays like any other file (so they go through the same path as the
//! music):
//!
//! * tone: a 1 kHz sine at -20 dBFS on every channel, 10 s, 24 bits, at a
//!   chosen PCM rate. On a DAC with a display, the rate shown is the rate it
//!   receives.
//! * channels: the same tone on each channel in turn (1 s on, 0.3 s off), in
//!   the file's channel order (front left, front right, centre, LFE, back
//!   left, back right, side left, side right), to check which speaker each
//!   one reaches. The LFE gets 60 Hz.
//! * dsd: the tone as DSD64 / 128 / 256 (a DSF file, 5 s), for a DAC that
//!   shows the DSD rate. The bits come from the engine's sigma-delta
//!   modulator, which lives in the DSD crate: it is passed in as a
//!   [`DsdModulator`].
//!
//! Every signal starts and ends with a 10 ms raised-cosine fade. The files
//! are deterministic, so they are kept once made, in
//! `$XDG_CACHE_HOME/sonkore/test-signals`, named by what they hold (the
//! names earlier versions used: they share the cache).
//!
//! (Earlier versions had a fourth signal, a DTS stream in a WAV; it needs
//! FFmpeg's DTS encoder and is not made here.)

use std::path::{Path, PathBuf};

/// The tone's frequency and level.
pub const TONE_HZ: f64 = 1000.0;
pub const TONE_DBFS: f64 = -20.0;
pub const TONE_SECONDS: f64 = 10.0;
pub const DSD_SECONDS: f64 = 5.0;

/// PCM to DSD for [`write_dsd_tone`]: the engine's sigma-delta modulator, set
/// up for the DSD multiple and channel count with native DSD_U32 packing
/// (each word's bytes, in memory order, are the DSD bytes oldest first, the
/// MSB of each byte the oldest bit).
pub trait DsdModulator {
    /// Interleaved PCM frames at the DSD rate in; one packed word per channel
    /// and output frame out. Returns the output frames written (`out` has room
    /// for `frames / 32 + 1`).
    fn process(&mut self, input: &[f64], out: &mut [i32]) -> usize;
    /// Completes the last partial word with DSD silence; returns 0 or 1 frame.
    fn flush(&mut self, out: &mut [i32]) -> usize;
}

/// Where the test signals are kept.
pub fn cache_dir() -> PathBuf {
    sonkore_core::paths::cache_dir().join("test-signals")
}

/// The signals the player offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TestSignal {
    /// The tone at a PCM rate (clamped to 8 kHz – 768 kHz).
    Tone { rate: u32, channels: u32 },
    /// The channel check at a PCM rate.
    Channels { rate: u32, channels: u32 },
    /// The tone as DSD64, 128 or 256 (other values round down to these).
    Dsd { multiple: u32, channels: u32 },
}

impl TestSignal {
    /// The clamps: 1–8 channels, 8 k – 768 kHz,
    /// DSD 64 / 128 / 256 (DSF stops at 6 channels: more is an error when
    /// the file is made).
    pub fn normalized(self) -> TestSignal {
        let ch = |c: u32| c.clamp(1, 8);
        match self {
            TestSignal::Tone { rate, channels } => {
                TestSignal::Tone { rate: rate.clamp(8000, 768_000), channels: ch(channels) }
            }
            TestSignal::Channels { rate, channels } => {
                TestSignal::Channels { rate: rate.clamp(8000, 768_000), channels: ch(channels) }
            }
            TestSignal::Dsd { multiple, channels } => TestSignal::Dsd {
                multiple: if multiple >= 256 {
                    256
                } else if multiple >= 128 {
                    128
                } else {
                    64
                },
                channels: ch(channels),
            },
        }
    }

    /// The cached file's name ("tone-1khz-48000-2ch.wav"...).
    pub fn file_name(self) -> String {
        match self.normalized() {
            TestSignal::Tone { rate, channels } => format!("tone-1khz-{rate}-{channels}ch.wav"),
            TestSignal::Channels { rate, channels } => format!("channels-{rate}-{channels}ch.wav"),
            TestSignal::Dsd { multiple, channels } => format!("tone-1khz-dsd{multiple}-{channels}ch.dsf"),
        }
    }

    /// The file in `dir`, made unless it is already there (and not empty).
    /// The DSD tone needs `modulator`.
    pub fn make_in(self, dir: &Path, modulator: Option<&mut dyn DsdModulator>) -> Result<PathBuf, String> {
        let path = dir.join(self.file_name());
        if std::fs::metadata(&path).map(|m| m.len() > 0).unwrap_or(false) {
            return Ok(path);
        }
        match self.normalized() {
            TestSignal::Tone { rate, channels } => write_tone(&path, rate, channels, TONE_SECONDS, TONE_HZ, TONE_DBFS)?,
            TestSignal::Channels { rate, channels } => {
                write_channel_sequence(&path, rate, channels, TONE_HZ, TONE_DBFS)?
            }
            TestSignal::Dsd { multiple, channels } => {
                let m = modulator.ok_or_else(|| "DSD test signal: no modulator".to_string())?;
                write_dsd_tone(&path, multiple, channels, DSD_SECONDS, TONE_HZ, TONE_DBFS, m)?
            }
        }
        Ok(path)
    }

    /// [`make_in`](TestSignal::make_in) the cache folder.
    pub fn make(self, modulator: Option<&mut dyn DsdModulator>) -> Result<PathBuf, String> {
        self.make_in(&cache_dir(), modulator)
    }
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(path, bytes).map_err(|_| format!("Could not write {}", path.display()))
}

fn put_le(out: &mut Vec<u8>, v: u64, bytes: usize) {
    for i in 0..bytes {
        out.push((v >> (8 * i)) as u8);
    }
}

/// The WAV channel mask for the usual layouts (front left, front right,
/// centre, LFE, back left, back right, side left, side right).
fn channel_mask(channels: u32) -> u32 {
    match channels {
        1 => 0x4,
        2 => 0x3,
        4 => 0x33,
        6 => 0x3F,
        8 => 0x63F,
        _ => 0,
    }
}

fn wav_header(out: &mut Vec<u8>, rate: u32, channels: u32, bits: u32, data_bytes: u64) {
    let extensible = channels > 2 || bits > 16;
    let block_align = channels * (bits / 8);
    let fmt_size: u64 = if extensible { 40 } else { 16 };
    out.clear();
    out.extend_from_slice(b"RIFF");
    put_le(out, 4 + 8 + fmt_size + 8 + data_bytes, 4);
    out.extend_from_slice(b"WAVEfmt ");
    put_le(out, fmt_size, 4);
    put_le(out, if extensible { 0xFFFE } else { 1 }, 2); // WAVE_FORMAT_EXTENSIBLE / PCM
    put_le(out, channels as u64, 2);
    put_le(out, rate as u64, 4);
    put_le(out, rate as u64 * block_align as u64, 4);
    put_le(out, block_align as u64, 2);
    put_le(out, bits as u64, 2);
    if extensible {
        put_le(out, 22, 2);
        put_le(out, bits as u64, 2);
        put_le(out, channel_mask(channels) as u64, 4);
        // KSDATAFORMAT_SUBTYPE_PCM
        out.extend_from_slice(&[
            0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71,
        ]);
    }
    out.extend_from_slice(b"data");
    put_le(out, data_bytes, 4);
}

/// 10 ms raised-cosine fades at both ends of `frames` frames.
fn fade_gain(i: usize, frames: usize, rate: u32) -> f64 {
    let fade = 1usize.max(rate as usize / 100);
    let pi = std::f64::consts::PI;
    if i < fade {
        return 0.5 - 0.5 * (pi * i as f64 / fade as f64).cos();
    }
    if frames - i <= fade {
        return 0.5 - 0.5 * (pi * (frames - i) as f64 / fade as f64).cos();
    }
    1.0
}

/// A WAV (WAVE_FORMAT_EXTENSIBLE beyond stereo or 16 bits) from interleaved
/// samples in [-1, 1]; `bits` 16 or 24.
pub fn write_wav(path: &Path, rate: u32, channels: u32, bits: u32, samples: &[f64]) -> Result<(), String> {
    if bits != 16 && bits != 24 {
        return Err("WAV: 16 or 24 bits".into());
    }
    let bytes = (bits / 8) as usize;
    let mut out = Vec::with_capacity(80 + samples.len() * bytes);
    wav_header(&mut out, rate, channels, bits, (samples.len() * bytes) as u64);
    let full = if bits == 16 { 32768.0 } else { 8388608.0 };
    for &s in samples {
        let v = (s * full).round().clamp(-full, full - 1.0);
        put_le(&mut out, v as i64 as u64, bytes);
    }
    write_file(path, &out)
}

/// A WAV whose 16-bit stereo payload is `bytes` as they are (a DTS CD).
pub fn write_wav_bytes(path: &Path, rate: u32, bytes: &[u8]) -> Result<(), String> {
    let mut out = Vec::with_capacity(64 + bytes.len());
    wav_header(&mut out, rate, 2, 16, bytes.len() as u64);
    out.extend_from_slice(bytes);
    if out.len() & 1 != 0 {
        out.push(0);
    }
    write_file(path, &out)
}

/// A sine on every channel, 24 bits.
pub fn write_tone(path: &Path, rate: u32, channels: u32, seconds: f64, freq_hz: f64, dbfs: f64) -> Result<(), String> {
    let frames = (rate as f64 * seconds) as usize;
    let amp = 10f64.powf(dbfs / 20.0);
    let ch = channels as usize;
    let pi = std::f64::consts::PI;
    let mut samples = vec![0.0; frames * ch];
    for i in 0..frames {
        let v = amp * fade_gain(i, frames, rate) * (2.0 * pi * freq_hz * i as f64 / rate as f64).sin();
        samples[i * ch..(i + 1) * ch].fill(v);
    }
    write_wav(path, rate, channels, 24, &samples)
}

/// The tone on each channel in turn: 1 s on, 0.3 s of silence; 60 Hz on the
/// LFE (a subwoofer would not reproduce 1 kHz).
pub fn write_channel_sequence(path: &Path, rate: u32, channels: u32, freq_hz: f64, dbfs: f64) -> Result<(), String> {
    let on = rate as usize;
    let gap = rate as usize * 3 / 10;
    let amp = 10f64.powf(dbfs / 20.0);
    let ch = channels as usize;
    let pi = std::f64::consts::PI;
    let mut samples = vec![0.0; ch * (on + gap) * ch];
    for c in 0..ch {
        let start = c * (on + gap);
        let f = if ch >= 6 && c == 3 { 60.0 } else { freq_hz };
        for i in 0..on {
            samples[(start + i) * ch + c] =
                amp * fade_gain(i, on, rate) * (2.0 * pi * f * i as f64 / rate as f64).sin();
        }
    }
    write_wav(path, rate, channels, 24, &samples)
}

/// The tone as DSD in a DSF (Sony) file: "DSD " + "fmt " + "data" chunks,
/// little-endian, the samples in blocks of 4096 bytes per channel, MSB first.
/// `modulator` must be set up for `dsd_multiple` and `channels`.
pub fn write_dsd_tone(
    path: &Path,
    dsd_multiple: u32,
    channels: u32,
    seconds: f64,
    freq_hz: f64,
    dbfs: f64,
    modulator: &mut dyn DsdModulator,
) -> Result<(), String> {
    // DSF "Channel Type": 1 mono, 2 stereo, 3 three channels (FL FR C), 4
    // quad, 5 four channels (FL FR C LFE), 6 five channels (FL FR C BL BR),
    // 7 5.1. The format stops at six channels.
    let channel_type = match channels {
        1 => 1,
        2 => 2,
        3 => 3,
        4 => 4,
        5 => 6,
        6 => 7,
        _ => return Err(format!("DSF holds 1 to 6 channels, not {channels}")),
    };
    let rate = 44100 * dsd_multiple;
    let frames = (rate as f64 * seconds) as usize;
    let amp = 10f64.powf(dbfs / 20.0);
    let ch = channels as usize;
    let pi = std::f64::consts::PI;

    const BLOCK: usize = 65536;
    let mut input = vec![0.0; BLOCK * ch];
    let mut words = vec![0i32; (BLOCK / 32 + 2) * ch];
    let mut chan: Vec<Vec<u8>> = vec![Vec::with_capacity(frames / 8 + 4096); ch];
    let mut done = 0;
    while done < frames {
        let n = BLOCK.min(frames - done);
        for i in 0..n {
            let v =
                amp * fade_gain(done + i, frames, rate) * (2.0 * pi * freq_hz * (done + i) as f64 / rate as f64).sin();
            input[i * ch..(i + 1) * ch].fill(v);
        }
        let mut got = modulator.process(&input[..n * ch], &mut words);
        if done + n >= frames {
            got += modulator.flush(&mut words[got * ch..]);
        }
        for w in 0..got {
            for (c, bytes) in chan.iter_mut().enumerate() {
                // The word's bytes are already oldest first.
                bytes.extend_from_slice(&words[w * ch + c].to_ne_bytes());
            }
        }
        done += n;
    }
    let bytes_per_channel = chan[0].len();
    let blocks = bytes_per_channel.div_ceil(4096);
    for c in chan.iter_mut() {
        c.resize(blocks * 4096, 0x69);
    }

    let data_size = 12 + blocks as u64 * 4096 * ch as u64;
    let total = 28 + 52 + data_size;
    let mut out = Vec::with_capacity(total as usize);
    out.extend_from_slice(b"DSD ");
    put_le(&mut out, 28, 8);
    put_le(&mut out, total, 8);
    put_le(&mut out, 0, 8); // no metadata
    out.extend_from_slice(b"fmt ");
    put_le(&mut out, 52, 8);
    put_le(&mut out, 1, 4); // format version
    put_le(&mut out, 0, 4); // DSD raw
    put_le(&mut out, channel_type, 4);
    put_le(&mut out, channels as u64, 4);
    put_le(&mut out, rate as u64, 4);
    put_le(&mut out, 8, 4); // bits per sample: 8 = MSB first
    put_le(&mut out, bytes_per_channel as u64 * 8, 8);
    put_le(&mut out, 4096, 4);
    put_le(&mut out, 0, 4);
    out.extend_from_slice(b"data");
    put_le(&mut out, data_size, 8);
    for b in 0..blocks {
        for c in chan.iter() {
            out.extend_from_slice(&c[b * 4096..(b + 1) * 4096]);
        }
    }
    write_file(path, &out)
}

/// The name of channel `index` in a `channels`-channel file ("Front
/// left"...), in English; the UI translates.
pub fn channel_name(channels: u32, index: u32) -> &'static str {
    const STEREO: [&str; 2] = ["Left", "Right"];
    const QUAD: [&str; 4] = ["Front left", "Front right", "Back left", "Back right"];
    const SURROUND: [&str; 8] = [
        "Front left",
        "Front right",
        "Centre",
        "LFE (subwoofer)",
        "Back left",
        "Back right",
        "Side left",
        "Side right",
    ];
    let i = index as usize;
    if channels == 1 {
        "Mono"
    } else if channels == 2 && i < 2 {
        STEREO[i]
    } else if channels == 4 && i < 4 {
        QUAD[i]
    } else if i < 8 {
        SURROUND[i]
    } else {
        ""
    }
}
