//! Damaged DSF / DSDIFF / DST files against the DSD reader, the DST decoder
//! and DSD to PCM: no panic, no endless loop, no absurd allocation
//! (fuzz-lite, see tests/fuzz_lite.rs).

#[path = "../../../tests/fuzz_lite.rs"]
mod fuzz_lite;

use std::io::Cursor;
use std::time::Duration;

use fuzz_lite::{Case, Plan, Rng, Seed, corpus_seeds_up_to, run, run_plan, scaled};
use sonkore_dsd::{Decoder, DsdDecoder, DsdPacking, DsdToPcm, DstDecoder};

#[global_allocator]
static GUARD: fuzz_lite::Guard = fuzz_lite::Guard;

const STACK: usize = 512 << 10;
const LIMIT: Duration = Duration::from_secs(5);

// --- Synthetic seeds -----------------------------------------------------------------

fn dsf(bits_per_sample: u32, channels: u32) -> Vec<u8> {
    const BLOCK: usize = 4096;
    let blocks = 2;
    let mut data = Vec::new();
    for b in 0..blocks * channels as usize * BLOCK {
        data.push((b * 37 % 251) as u8 ^ 0x5A);
    }
    let tag = b"ID3\x03\0\0\0\0\0\x0bTIT2\0\0\0\x01\0\0\0".to_vec();
    let data_size = 12 + data.len() as u64;
    let total = 28 + 52 + data_size + tag.len() as u64;
    let mut f = b"DSD ".to_vec();
    f.extend_from_slice(&28u64.to_le_bytes());
    f.extend_from_slice(&total.to_le_bytes());
    f.extend_from_slice(&(28 + 52 + data_size).to_le_bytes());
    f.extend_from_slice(b"fmt ");
    f.extend_from_slice(&52u64.to_le_bytes());
    for v in [1u32, 0, 2, channels, 2_822_400, bits_per_sample] {
        f.extend_from_slice(&v.to_le_bytes());
    }
    f.extend_from_slice(&((blocks * BLOCK * 8) as u64 - 100).to_le_bytes());
    f.extend_from_slice(&(BLOCK as u32).to_le_bytes());
    f.extend_from_slice(&0u32.to_le_bytes());
    f.extend_from_slice(b"data");
    f.extend_from_slice(&data_size.to_le_bytes());
    f.extend(data);
    f.extend(tag);
    f
}

fn chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut c = id.to_vec();
    c.extend_from_slice(&(body.len() as u64).to_be_bytes());
    c.extend_from_slice(body);
    if body.len() % 2 == 1 {
        c.push(0);
    }
    c
}

fn dff_with(sound: Vec<u8>, compression: &[u8]) -> Vec<u8> {
    let mut prop = b"SND ".to_vec();
    prop.extend(chunk(b"FS  ", &2_822_400u32.to_be_bytes()));
    prop.extend(chunk(b"CHNL", b"\0\x02SLFTSRGT"));
    prop.extend(chunk(b"CMPR", compression));
    let mut body = b"DSD ".to_vec();
    body.extend(chunk(b"FVER", &[1, 5, 0, 0]));
    body.extend(chunk(b"PROP", &prop));
    body.extend(sound);
    chunk(b"FRM8", &body)
}

fn dff() -> Vec<u8> {
    let data: Vec<u8> = (0..8192).map(|i| (i * 13 % 251) as u8 ^ 0x33).collect();
    dff_with(chunk(b"DSD ", &data), b"DSD \x0fnot compressed\0")
}

/// A DST file whose frames are plain DSD (DST allows that: a 0 bit first),
/// with a frame index.
fn dst_plain() -> Vec<u8> {
    let per_frame = 588 * 64 / 8 * 2; // DSD64, 2 channels, bytes per frame
    let frames = 3u32;
    let mut dst = b"FRTE".to_vec();
    dst.extend_from_slice(&8u64.to_be_bytes());
    dst.extend_from_slice(&frames.to_be_bytes());
    dst.extend_from_slice(&75u16.to_be_bytes());
    dst.extend_from_slice(&[0, 0]);
    let mut offsets = Vec::new();
    for f in 0..frames {
        let mut frame = vec![0u8]; // plain DSD: bit 0 = 0, then 7 zero bits
        frame.extend((0..per_frame).map(|i| (i as u32 * 7 + f) as u8 | 1));
        offsets.push(dst.len() as u64);
        dst.extend(chunk(b"DSTF", &frame));
    }
    let mut sound = chunk(b"DST ", &dst);
    let mut index = Vec::new();
    for o in offsets {
        index.extend_from_slice(&o.to_be_bytes());
        index.extend_from_slice(&((per_frame + 1) as u32).to_be_bytes());
    }
    sound.extend(chunk(b"DSTI", &index));
    dff_with(sound, b"DST \x0eDST Encoded\0")
}

fn seeds() -> Vec<Seed> {
    let mut seeds = corpus_seeds_up_to(&["dsf", "dff"], 4 << 20);
    seeds.push(Seed::new("synthetic-1bit.dsf", "dsf", dsf(1, 2)));
    seeds.push(Seed::new("synthetic-8bit.dsf", "dsf", dsf(8, 2)));
    seeds.push(Seed::new("synthetic-mono.dsf", "dsf", dsf(1, 1)));
    seeds.push(Seed::new("synthetic.dff", "dff", dff()));
    seeds.push(Seed::new("synthetic-dst.dff", "dff", dst_plain()));
    seeds
}

/// What the engine does with a DSD file: open, read blocks, seek, read.
fn exercise(d: &mut dyn Decoder) {
    let info = d.info().clone();
    let _ = info.duration_seconds();
    let ch = info.channels.clamp(1, 16);
    let mut ints = vec![0i32; 1024 * ch];
    let mut floats = vec![0f64; 333 * ch];
    for _ in 0..3 {
        if d.read_i32(&mut ints).unwrap_or(0) == 0 {
            break;
        }
    }
    let _ = d.read_f64(&mut floats);
    for to in [info.total_frames / 2, info.total_frames.saturating_sub(1), u64::MAX, 0] {
        if d.seek(to).is_ok() {
            let _ = d.read_i32(&mut ints);
        }
    }
}

#[test]
fn dsd_reader_survives_damage() {
    let plan = Plan::new(seeds(), 2500, 8);
    let mut n = 0usize;
    run_plan("dsd-reader", plan, STACK, LIMIT, move |case: &Case| {
        n += 1;
        let packing = [DsdPacking::Dop, DsdPacking::NativeU32Be, DsdPacking::NativeU32Le][n % 3];
        if let Ok(mut d) = DsdDecoder::from_source(Box::new(Cursor::new(case.bytes.clone())), packing) {
            let _ = d.format();
            let _ = d.is_dst();
            exercise(&mut d);
        }
    });
}

#[test]
fn dsd_to_pcm_survives_damage() {
    // Fewer cases: building the conversion's filters is the slow part.
    let plan = Plan::new(seeds(), 200, 0);
    run_plan("dsd-to-pcm", plan, STACK, LIMIT, move |case: &Case| {
        if let Ok(mut d) = DsdToPcm::from_source(Box::new(Cursor::new(case.bytes.clone()))) {
            exercise(&mut d);
        }
    });
}

#[test]
fn dsd_probe_survives_damage() {
    let dir = fuzz_lite::scratch("dsd-probe");
    let plan = Plan::new(
        seeds()
            .into_iter()
            .map(|mut s| {
                s.bytes.truncate(64 << 10);
                s
            })
            .collect(),
        2000,
        40,
    );
    run_plan("dsd-probe", plan, STACK, LIMIT, move |case: &Case| {
        let path = dir.join(format!("case.{}", case.ext));
        std::fs::write(&path, &case.bytes).unwrap();
        let _ = sonkore_dsd::probe(&path);
    });
}

/// The DSTF frames of a DSDIFF file (DST compressed), up to `max`.
fn dst_frames(file: &[u8], max: usize) -> Vec<Vec<u8>> {
    let be64 = |p: usize| u64::from_be_bytes(file[p..p + 8].try_into().unwrap()) as usize;
    let mut out = Vec::new();
    let mut pos = 16;
    while pos + 12 <= file.len() && out.len() < max {
        let size = be64(pos + 4);
        if &file[pos..pos + 4] == b"DST " {
            let end = (pos + 12 + size).min(file.len());
            let mut p = pos + 12;
            while p + 12 <= end && out.len() < max {
                let s = be64(p + 4);
                if &file[p..p + 4] == b"DSTF" {
                    out.push(file[p + 12..(p + 12 + s).min(end)].to_vec());
                }
                p += 12 + s + (s & 1);
            }
        }
        pos += 12 + size + (size & 1);
    }
    out
}

#[test]
fn dst_frames_survive_damage() {
    // Real DST frames (the corpus' DST files) mutated, and random bytes.
    let mut frames: Vec<(Vec<u8>, u32, usize)> = Vec::new();
    for s in corpus_seeds_up_to(&["dff"], 64 << 20) {
        let rate = if s.name.contains("128") { 5_644_800 } else { 2_822_400 };
        for f in dst_frames(&s.bytes, 12) {
            frames.push((f, rate, 2));
        }
    }
    let with_corpus = !frames.is_empty();
    // A plain-DSD frame (always there).
    frames.push(([vec![0u8], vec![0x55; 588 * 64 / 8 * 2]].concat(), 2_822_400, 2));
    let make = move |n: usize| {
        let mut rng = Rng::for_case("dst", n);
        let (frame, rate, ch) = &frames[n % frames.len()];
        let bytes = if rng.chance(10) {
            let len = rng.below(4096);
            rng.bytes(len)
        } else {
            fuzz_lite::mutate(&mut rng, frame)
        };
        // The frame, then its rate and channels (sometimes not the frame's).
        let rate = if rng.chance(10) { *rng.pick(&[2_822_400u32, 5_644_800, 11_289_600, 44100, 0]) } else { *rate };
        let ch = if rng.chance(10) { rng.below(10) } else { *ch };
        let mut b = bytes;
        b.extend_from_slice(&rate.to_le_bytes());
        b.push(ch as u8);
        Case { label: format!("DST frame {} ({n})", n % frames.len()), ext: "dst".into(), bytes: b }
    };
    let count = scaled(if with_corpus { 1200 } else { 500 });
    let mut decoder = DstDecoder::new();
    let mut out = Vec::new();
    run("dst", count, STACK, LIMIT, make, move |case: &Case| {
        let n = case.bytes.len();
        let ch = case.bytes[n - 1] as usize;
        let rate = u32::from_le_bytes(case.bytes[n - 5..n - 1].try_into().unwrap());
        let samples = DstDecoder::samples_per_frame(rate);
        out.resize(samples / 8 * ch.max(1), 0);
        let _ = decoder.decode(&case.bytes[..n - 5], ch, samples, &mut out);
    });
}

// --- Bugs the fuzzing found ------------------------------------------------------------

#[test]
fn dff_sound_chunk_longer_than_the_file() {
    let mut f = dff();
    let at = f.windows(4).rposition(|w| w == b"DSD ").unwrap();
    f[at + 4..at + 12].copy_from_slice(&u64::MAX.to_be_bytes());
    f.truncate(at + 12 + 64);
    let mut d = DsdDecoder::from_source(Box::new(Cursor::new(f)), DsdPacking::NativeU32Be).unwrap();
    exercise(&mut d);
}

#[test]
fn absurd_dsd_rate() {
    let mut f = dsf(1, 2);
    // fmt: version, format id, channel type, channels, then the rate.
    let fmt = f.windows(4).position(|w| w == b"fmt ").unwrap();
    let rate = fmt + 12 + 16;
    f[rate..rate + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    if let Ok(mut d) = DsdDecoder::from_source(Box::new(Cursor::new(f)), DsdPacking::Dop) {
        exercise(&mut d);
    }
}
