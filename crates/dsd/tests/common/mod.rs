//! DSF / DSDIFF writers and test signals for the tests.
#![allow(dead_code)]

use std::path::PathBuf;

use sonkore_dsd::{DsdPacking, SigmaDelta};

pub fn tmp(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("sonkore-dsd");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

/// DSD bytes per channel (MSB first, oldest bit first) of a sine through the
/// crate's own modulator.
pub fn sine_dsd(dsd_rate: u32, channels: usize, bytes: usize, freq: f64, amp: f64) -> Vec<Vec<u8>> {
    let frames = bytes * 8;
    let mut input = vec![0.0; frames * channels];
    for f in 0..frames {
        for c in 0..channels {
            input[f * channels + c] =
                amp * (2.0 * std::f64::consts::PI * freq * (c + 1) as f64 * f as f64 / dsd_rate as f64).sin();
        }
    }
    let mut sd = SigmaDelta::new();
    sd.setup(dsd_rate / 44100, channels, DsdPacking::NativeU32Be);
    let mut words = vec![0i32; (frames / 32 + 1) * channels];
    let mut n = sd.process(&input, &mut words);
    n += sd.flush(&mut words[n * channels..]);
    let mut out = vec![Vec::with_capacity(bytes); channels];
    for w in 0..n {
        for c in 0..channels {
            out[c].extend_from_slice(&words[w * channels + c].to_ne_bytes());
        }
    }
    for o in &mut out {
        o.truncate(bytes);
    }
    out
}

/// A DSF file (1 bit per sample = LSB first, or 8 = MSB first), blocks of
/// `block` bytes, the last one padded with zeros as the spec says.
pub fn write_dsf(path: &PathBuf, rate: u32, chans: &[Vec<u8>], bits: u32, block: usize) {
    let ch = chans.len();
    let nbytes = chans[0].len();
    let blocks = nbytes.div_ceil(block);
    let mut data = Vec::new();
    for b in 0..blocks {
        for c in chans {
            for i in 0..block {
                let v = c.get(b * block + i).copied().unwrap_or(0);
                data.push(if bits == 1 { v.reverse_bits() } else { v });
            }
        }
    }
    let mut fmt = Vec::new();
    fmt.extend_from_slice(b"fmt ");
    fmt.extend_from_slice(&52u64.to_le_bytes());
    for v in [1u32, 0, if ch == 2 { 2 } else { ch as u32 }, ch as u32, rate, bits] {
        fmt.extend_from_slice(&v.to_le_bytes());
    }
    fmt.extend_from_slice(&((nbytes * 8) as u64).to_le_bytes());
    fmt.extend_from_slice(&(block as u32).to_le_bytes());
    fmt.extend_from_slice(&0u32.to_le_bytes());
    let mut file = Vec::new();
    let total = 28 + fmt.len() + 12 + data.len();
    file.extend_from_slice(b"DSD ");
    file.extend_from_slice(&28u64.to_le_bytes());
    file.extend_from_slice(&(total as u64).to_le_bytes());
    file.extend_from_slice(&0u64.to_le_bytes());
    file.extend_from_slice(&fmt);
    file.extend_from_slice(b"data");
    file.extend_from_slice(&((12 + data.len()) as u64).to_le_bytes());
    file.extend_from_slice(&data);
    std::fs::write(path, file).unwrap();
}

fn chunk(id: &[u8], body: &[u8]) -> Vec<u8> {
    let mut v = id.to_vec();
    v.extend_from_slice(&(body.len() as u64).to_be_bytes());
    v.extend_from_slice(body);
    if body.len() & 1 == 1 {
        v.push(0);
    }
    v
}

/// An uncompressed DSDIFF file.
pub fn write_dff(path: &PathBuf, rate: u32, chans: &[Vec<u8>]) {
    let ch = chans.len();
    let mut chnl = (ch as u16).to_be_bytes().to_vec();
    for c in 0..ch {
        chnl.extend_from_slice(format!("C{c:03}").as_bytes());
    }
    let mut prop = b"SND ".to_vec();
    prop.extend(chunk(b"FS  ", &rate.to_be_bytes()));
    prop.extend(chunk(b"CHNL", &chnl));
    prop.extend(chunk(b"CMPR", b"DSD \x0enot compressed\x00"));
    let mut raw = Vec::new();
    for i in 0..chans[0].len() {
        for c in chans {
            raw.push(c[i]);
        }
    }
    let mut body = b"DSD ".to_vec();
    body.extend(chunk(b"FVER", &0x0105_0000u32.to_be_bytes()));
    body.extend(chunk(b"PROP", &prop));
    body.extend(chunk(b"DSD ", &raw));
    let mut file = b"FRM8".to_vec();
    file.extend_from_slice(&(body.len() as u64).to_be_bytes());
    file.extend(body);
    std::fs::write(path, file).unwrap();
}

/// The per-channel DSD bytes a decoder gives in native BE packing.
pub fn read_native(d: &mut dyn sonkore_dsd::Decoder, block: usize) -> Vec<Vec<u8>> {
    let ch = d.info().channels;
    let mut out = vec![Vec::new(); ch];
    let mut buf = vec![0i32; block * ch];
    loop {
        let n = d.read_i32(&mut buf).unwrap();
        if n == 0 {
            break;
        }
        for f in 0..n {
            for c in 0..ch {
                out[c].extend_from_slice(&buf[f * ch + c].to_ne_bytes());
            }
        }
    }
    out
}

pub fn read_all_i32(d: &mut dyn sonkore_dsd::Decoder, block: usize) -> Vec<i32> {
    let ch = d.info().channels;
    let mut out = Vec::new();
    let mut buf = vec![0i32; block * ch];
    loop {
        let n = d.read_i32(&mut buf).unwrap();
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n * ch]);
    }
    out
}

pub fn read_all_f64(d: &mut dyn sonkore_dsd::Decoder, block: usize) -> Vec<f64> {
    let ch = d.info().channels;
    let mut out = Vec::new();
    let mut buf = vec![0f64; block * ch];
    loop {
        let n = d.read_f64(&mut buf).unwrap();
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n * ch]);
    }
    out
}
