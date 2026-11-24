//! Shared by the HDCD tests: an HDCD packet encoder for synthetic signals,
//! a WAV reader and a digest.

#![allow(dead_code)]

use std::path::PathBuf;

use sonkore_hdcd::hdcd::{Detection, Hdcd};

/// xorshift64*: the same numbers everywhere.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    pub fn chance(&mut self, per_mille: u64) -> bool {
        self.below(1000) < per_mille
    }
}

/// Scrambles a channel's HDCD bit stream into sample LSBs: the decoder reads
/// `w ^ w >> 5 ^ w >> 23` of its LSB window, so LSB(t) = bit(t) ^ LSB(t-5) ^ LSB(t-23).
#[derive(Default)]
pub struct Scrambler {
    raw: Vec<u8>,
}

impl Scrambler {
    pub fn push(&mut self, bit: u8) -> i32 {
        let n = self.raw.len();
        let back = |k: usize| if n >= k { self.raw[n - k] } else { 0 };
        let r = bit ^ back(5) ^ back(23);
        self.raw.push(r);
        r as i32
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Format {
    A,
    B,
    Mixed,
}

/// How a synthetic HDCD signal is made.
#[derive(Clone, Debug)]
pub struct Signal {
    pub seed: u64,
    pub frames: usize,
    pub bits: u32,
    pub format: Format,
    /// Frames between packets (a random value up to this, at least 40).
    pub period: u64,
    /// Per mille of packets with a corrupted code.
    pub corrupt: u64,
    /// Per mille of packets where the right channel gets another code.
    pub mismatch: u64,
    /// Per mille of packets followed by a silence longer than the code
    /// detect timer (2 s at 44.1 kHz).
    pub dropout: u64,
    /// Random LSBs between packets instead of zeros.
    pub noisy_lsb: bool,
    /// Peak amplitude, as a fraction (‰) of full scale.
    pub level: u64,
    /// Packets carry no gain, peak extend or filter.
    pub nop: bool,
}

impl Signal {
    pub fn new(seed: u64, frames: usize) -> Self {
        Signal {
            seed,
            frames,
            bits: 16,
            format: Format::B,
            period: 2000,
            corrupt: 0,
            mismatch: 0,
            dropout: 0,
            noisy_lsb: false,
            level: 1000,
            nop: false,
        }
    }

    /// Interleaved stereo, LSB in bit 0 (what `Hdcd::process` takes).
    pub fn generate(&self) -> Vec<i32> {
        let mut rng = Rng::new(self.seed);
        let mut queue: [Vec<u8>; 2] = [Vec::new(), Vec::new()];
        let mut scr = [Scrambler::default(), Scrambler::default()];
        let mut next_packet = 0u64;
        let mut out = Vec::with_capacity(self.frames * 2);
        let full: i64 = (1i64 << (self.bits - 1)) - 1;
        let peak = full * self.level as i64 / 1000;
        let mut phase = [0i64, 0i64];
        let step = [peak / 37 + 1, peak / 91 + 1];
        let mut dir = [1i64, -1i64];

        for t in 0..self.frames as u64 {
            if t >= next_packet && queue[0].is_empty() && queue[1].is_empty() {
                let (c0, c1) = self.codes(&mut rng);
                let fmt = match self.format {
                    Format::Mixed => {
                        if rng.chance(500) {
                            Format::A
                        } else {
                            Format::B
                        }
                    }
                    f => f,
                };
                let corrupt = rng.chance(self.corrupt);
                queue[0] = packet(fmt, c0, corrupt.then(|| rng.below(16) as u32));
                queue[1] = packet(fmt, c1, None);
                queue[0].reverse();
                queue[1].reverse();
                let gap =
                    if rng.chance(self.dropout) { 100_000 + rng.below(50_000) } else { 40 + rng.below(self.period) };
                next_packet = t + gap;
            }
            for ch in 0..2 {
                let bit = match queue[ch].pop() {
                    Some(b) => b,
                    None => (self.noisy_lsb && rng.chance(500)) as u8,
                };
                let lsb = scr[ch].push(bit);
                // a triangle wave with noise, now and then at full scale
                phase[ch] += step[ch] * dir[ch];
                if phase[ch] > peak || phase[ch] < -peak {
                    dir[ch] = -dir[ch];
                    phase[ch] = phase[ch].clamp(-peak, peak);
                }
                let noise = (rng.below(2049) as i64 - 1024) * (full / 16384).max(1);
                let v = (phase[ch] + noise).clamp(-full - 1, full);
                out.push(((v & !1) | lsb as i64) as i32);
            }
        }
        out
    }

    fn codes(&self, rng: &mut Rng) -> (u8, u8) {
        if self.nop {
            return (0, 0);
        }
        let code = |rng: &mut Rng| -> u8 {
            let gain = rng.below(16) as u8;
            let pe = rng.chance(600) as u8;
            let tf = rng.chance(200) as u8;
            gain | pe << 4 | tf << 5
        };
        let c0 = code(rng);
        let c1 = if rng.chance(self.mismatch) { code(rng) } else { c0 };
        (c0, c1)
    }
}

/// A packet's bits, first first: the sync word and the code. Format A keeps
/// 3 gain bits (doubled by the decoder); `flip` corrupts a code bit.
pub fn packet(fmt: Format, control: u8, flip: Option<u32>) -> Vec<u8> {
    let word: u64 = match fmt {
        Format::A => {
            let xx = (control & 7) | (control & 0x30);
            let mut w = (0x7e0f_a005u64 << 8) | xx as u64;
            if let Some(b) = flip {
                w ^= 1 << (b % 8);
            }
            w
        }
        _ => {
            let mut w = (0x7e0f_a006u64 << 16) | (control as u64) << 8 | (!control) as u64;
            if let Some(b) = flip {
                w ^= 1 << (b % 16);
            }
            w
        }
    };
    let n = if fmt == Format::A { 40 } else { 48 };
    (0..n).rev().map(|k| ((word >> k) & 1) as u8).collect()
}

/// FNV-1a over the samples and what the decoder reports.
pub fn digest(samples: &[i32], d: &Detection) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |b: &[u8]| {
        for &x in b {
            h ^= x as u64;
            h = h.wrapping_mul(0x100_0000_01b3);
        }
    };
    for s in samples {
        eat(&s.to_le_bytes());
    }
    for v in [
        d.detected as i32,
        d.packet_type as i32,
        d.total_packets,
        d.errors,
        d.peak_extend as i32,
        d.uses_transient_filter as i32,
        d.cdt_expirations,
        d.lle_mismatch,
    ] {
        eat(&v.to_le_bytes());
    }
    eat(&d.max_gain_adjustment.to_bits().to_le_bytes());
    h
}

/// Runs the port over `input` in blocks of `block` frames.
pub fn run_rust(
    input: &[i32],
    rate: u32,
    bits: u32,
    block: usize,
    mode: sonkore_hdcd::hdcd::AnalyzeMode,
) -> (Vec<i32>, Detection) {
    let mut h = Hdcd::new();
    assert!(h.reset_ext(rate, bits));
    h.set_analyze_mode(mode);
    let mut out = input.to_vec();
    for chunk in out.chunks_mut(block * 2) {
        h.process(chunk);
    }
    (out, h.detection())
}

/// libhdcd's test files (the test/ folder of https://github.com/bp0/libhdcd),
/// when they are around: $SONKORE_HDCD_TEST_FILES, else the corpus's
/// libhdcd/ ($SONKORE_TEST_CORPUS).
pub fn test_files() -> Option<PathBuf> {
    let dir = std::env::var_os("SONKORE_HDCD_TEST_FILES")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("SONKORE_TEST_CORPUS").map(|c| PathBuf::from(c).join("libhdcd")))?;
    dir.join("hdcd.wav").exists().then_some(dir)
}

/// A PCM WAV's samples as hdcd-detect reads them (left-justified s32), its
/// bits and rate.
pub fn read_wav(path: &std::path::Path) -> (Vec<i32>, u32, u32) {
    let b = std::fs::read(path).unwrap();
    assert_eq!(&b[0..4], b"RIFF");
    let mut i = 12;
    let (mut bits, mut rate, mut data) = (0u32, 0u32, None);
    while i + 8 <= b.len() {
        let id = &b[i..i + 4];
        let len = u32::from_le_bytes(b[i + 4..i + 8].try_into().unwrap()) as usize;
        let body = &b[i + 8..(i + 8 + len).min(b.len())];
        if id == b"fmt " {
            rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
            bits = u16::from_le_bytes(body[14..16].try_into().unwrap()) as u32;
        } else if id == b"data" {
            data = Some(body);
        }
        i += 8 + len + (len & 1);
    }
    let data = data.unwrap();
    (pcm_to_s32(data, bits), bits, rate)
}

#[allow(clippy::chunks_exact_to_as_chunks)]
pub fn pcm_to_s32(data: &[u8], bits: u32) -> Vec<i32> {
    match bits {
        16 => data.chunks_exact(2).map(|c| (i16::from_le_bytes([c[0], c[1]]) as i32) << 16).collect(),
        24 => data.chunks_exact(3).map(|c| i32::from_le_bytes([0, c[0], c[1], c[2]])).collect(),
        _ => panic!("{bits}-bit"),
    }
}
