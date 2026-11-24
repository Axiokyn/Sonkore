//! Damaged files against the engine's decoders (symphonia's demuxers and
//! codecs, the DSD conversion, HDCD detection, CD image tracks): opening
//! and reading a few blocks never panics, never hangs, never allocates
//! absurdly (fuzz-lite, see tests/fuzz_lite.rs). decode::open turns a
//! decoder's panic into an error (the track is skipped), in release builds
//! too: they unwind.

#[path = "../../../tests/fuzz_lite.rs"]
mod fuzz_lite;

use std::fs;
use std::path::Path;
use std::time::Duration;

use fuzz_lite::{Case, Plan, Rng, Seed, corpus_seeds, run, run_plan, scaled};
use sonkore_engine::decode::{self, HdcdDecoder};

#[global_allocator]
static GUARD: fuzz_lite::Guard = fuzz_lite::Guard;

// symphonia's decoders keep big structures on the stack: 1 MiB, still well
// under a thread's default 2 MiB.
const STACK: usize = 1 << 20;
const LIMIT: Duration = Duration::from_secs(10);

fn wav(rate: u32, channels: u16, bits: u16, frames: usize) -> Vec<u8> {
    let bytes = bits as usize / 8;
    let mut data = Vec::new();
    for f in 0..frames {
        for c in 0..channels as usize {
            let v = ((f as f64 * (0.05 + 0.01 * c as f64)).sin() * 8000.0) as i32;
            data.extend_from_slice(&(v << (bits - 16)).to_le_bytes()[..bytes]);
        }
    }
    let block = channels as u32 * bytes as u32;
    let mut w = b"RIFF".to_vec();
    w.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&channels.to_le_bytes());
    w.extend_from_slice(&rate.to_le_bytes());
    w.extend_from_slice(&(rate * block).to_le_bytes());
    w.extend_from_slice(&(block as u16).to_le_bytes());
    w.extend_from_slice(&bits.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&(data.len() as u32).to_le_bytes());
    w.extend_from_slice(&data);
    w
}

/// What the engine does with a file: open it (and HDCD detection for 16-bit
/// stereo), read blocks as the worker does, seek, read again.
fn play_a_little(path: &Path) {
    let Ok(d) = decode::open(path) else { return };
    let mut d = HdcdDecoder::wrap(d);
    let info = d.info().clone();
    let _ = info.duration_seconds();
    let ch = info.channels.clamp(1, 32);
    let mut floats = vec![0f64; 4096 * ch];
    let mut ints = vec![0i32; 1024 * ch];
    for _ in 0..3 {
        if !matches!(d.read_f64(&mut floats), Ok(n) if n > 0) {
            break;
        }
    }
    let total = info.total_frames.unwrap_or(0);
    for to in [total / 2, total.saturating_sub(1), u64::MAX, 0] {
        if d.seek(to).is_ok() {
            let _ = d.read_i32(&mut ints);
        }
    }
    if path.extension().is_some_and(|e| e == "m4a" || e == "mp4" || e == "m4b") {
        let _ = decode::mp4_edit(path, info.rate);
    }
}

fn file_target(target: &'static str, exts: &[&str], extra: Vec<Seed>, mutations: usize, truncations: usize) {
    let mut seeds = corpus_seeds(exts);
    seeds.extend(extra);
    let mut plan = Plan::new(seeds, mutations, truncations);
    plan.other_exts =
        ["flac", "wav", "mp3", "m4a", "ogg", "aiff", "caf", "mka", "dsf", "dff"].map(String::from).to_vec();
    let dir = fuzz_lite::scratch(target);
    run_plan(target, plan, STACK, LIMIT, move |case: &Case| {
        let path = dir.join(format!("case.{}", case.ext));
        fs::write(&path, &case.bytes).unwrap();
        play_a_little(&path);
    });
}

#[test]
fn flac_decoding_survives_damage() {
    file_target("dec-flac", &["flac", "oga"], vec![], 1200, 40);
}

#[test]
fn pcm_decoding_survives_damage() {
    let extra = vec![
        Seed::new("synthetic-16.wav", "wav", wav(44100, 2, 16, 3000)),
        Seed::new("synthetic-24-mono.wav", "wav", wav(96000, 1, 24, 2000)),
    ];
    file_target("dec-pcm", &["wav", "wave", "aif", "aiff", "aifc", "caf"], extra, 1200, 40);
}

#[test]
fn lossy_decoding_survives_damage() {
    file_target("dec-lossy", &["mp3", "mp2", "aac"], vec![], 1200, 40);
}

#[test]
fn mp4_decoding_survives_damage() {
    file_target("dec-mp4", &["m4a", "m4b", "mp4"], vec![], 1200, 40);
}

#[test]
fn ogg_and_matroska_decoding_survive_damage() {
    file_target("dec-ogg-mkv", &["ogg", "opus", "mka", "webm"], vec![], 1200, 40);
}

#[test]
fn dsd_decoding_survives_damage() {
    // DSD to PCM: the slow one (the filters are built per file).
    file_target("dec-dsd", &["dsf", "dff"], vec![], 150, 0);
}

#[test]
fn random_bytes_decode_as_nothing() {
    let dir = fuzz_lite::scratch("dec-random");
    let exts = ["flac", "wav", "mp3", "m4a", "aac", "caf", "ogg", "aiff", "mka", "webm", "dsf", "dff"];
    let make = move |n: usize| {
        let mut rng = Rng::for_case("dec-random", n);
        let long = rng.chance(20);
        let len = rng.below(if long { 8192 } else { 96 });
        let mut bytes = rng.bytes(len);
        let magic: &[&[u8]] = &[
            b"fLaC",
            b"RIFF\xff\xff\xff\xffWAVE",
            b"FORM\0\0\x10\0AIFF",
            b"OggS\0\x02",
            b"ID3\x04\0\0\0\0\0\0",
            b"\xFF\xFB",
            b"\0\0\0\x18ftypM4A ",
            b"caff\0\x01\0\0",
            b"\x1A\x45\xDF\xA3",
            b"DSD \x1c\0\0\0\0\0\0\0",
            b"FRM8",
        ];
        if rng.chance(70) {
            bytes.splice(0..0, rng.pick(magic).to_vec());
        }
        Case { label: format!("random {len} bytes"), ext: rng.pick(&exts).to_string(), bytes }
    };
    run("dec-random", scaled(2000), STACK, LIMIT, make, move |case: &Case| {
        let path = dir.join(format!("case.{}", case.ext));
        fs::write(&path, &case.bytes).unwrap();
        play_a_little(&path);
    });
}

#[test]
fn cd_image_tracks_survive_damaged_sheets() {
    let dir = fuzz_lite::scratch("dec-cue");
    fs::write(dir.join("image.wav"), wav(44100, 2, 16, 44100)).unwrap();
    let sheet = "FILE \"image.wav\" WAVE\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n  TRACK 02 AUDIO\n    INDEX 00 00:00:30\n    INDEX 01 00:00:40\n  TRACK 03 AUDIO\n    INDEX 01 00:00:74\n";
    let make = move |n: usize| {
        let mut rng = Rng::for_case("dec-cue", n);
        let mut bytes = fuzz_lite::mutate(&mut rng, sheet.as_bytes());
        if rng.chance(30) {
            // An index far past the image, or times at their limits.
            let t = *rng.pick(&["99:59:74", "00:00:74", "4294967295:00:00", "18446744073709551615:59:74", "0:0:0"]);
            bytes = sheet.replace("00:00:40", t).into_bytes();
        }
        Case { label: format!("sheet (case {n})"), ext: "cue".into(), bytes }
    };
    run("dec-cue", scaled(1500), STACK, LIMIT, make, move |case: &Case| {
        let path = dir.join("case.cue");
        fs::write(&path, &case.bytes).unwrap();
        for n in [0, 1, 2, 3, 4] {
            play_a_little(&dir.join(format!("case.cue#{n}")));
        }
    });
}

/// The first 136 bytes of a Matroska file (EBML header, Segment, SeekHead):
/// an interrupted download or copy.
const TRUNCATED_MKA: [u8; 136] = [
    0x1A, 0x45, 0xDF, 0xA3, 0xA3, 0x42, 0x86, 0x81, 0x01, 0x42, 0xF7, 0x81, 0x01, 0x42, 0xF2, 0x81, 0x04, 0x42, 0xF3,
    0x81, 0x08, 0x42, 0x82, 0x88, 0x6D, 0x61, 0x74, 0x72, 0x6F, 0x73, 0x6B, 0x61, 0x42, 0x87, 0x81, 0x04, 0x42, 0x85,
    0x81, 0x02, 0x18, 0x53, 0x80, 0x67, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0xA7, 0xD7, 0x11, 0x4D, 0x9B, 0x74, 0xCF,
    0xBF, 0x84, 0x33, 0xF1, 0x8E, 0x5C, 0x4D, 0xBB, 0x8B, 0x53, 0xAB, 0x84, 0x15, 0x49, 0xA9, 0x66, 0x53, 0xAC, 0x81,
    0xA1, 0x4D, 0xBB, 0x8B, 0x53, 0xAB, 0x84, 0x16, 0x54, 0xAE, 0x6B, 0x53, 0xAC, 0x81, 0xFD, 0x4D, 0xBB, 0x8C, 0x53,
    0xAB, 0x84, 0x19, 0x41, 0xA4, 0x69, 0x53, 0xAC, 0x82, 0x01, 0x7B, 0x4D, 0xBB, 0x8C, 0x53, 0xAB, 0x84, 0x12, 0x54,
    0xC3, 0x67, 0x53, 0xAC, 0x82, 0x08, 0x24, 0x4D, 0xBB, 0x8C, 0x53, 0xAB, 0x84, 0x1C, 0x53, 0xBB, 0x6B, 0x53, 0xAC,
    0x82, 0xA7, 0xBB,
];

#[test]
fn truncated_matroska_does_not_panic() {
    let dir = fuzz_lite::scratch("bug-mka");
    for ext in ["mka", "webm"] {
        let path = dir.join(format!("truncated.{ext}"));
        fs::write(&path, TRUNCATED_MKA).unwrap();
        play_a_little(&path);
    }
}

/// Runs `body` in a child process (this test binary again, only this test):
/// an allocation failure aborts the process, which a test cannot catch.
fn in_child(test: &str, body: impl FnOnce()) {
    if std::env::var("SONKORE_CHILD_TEST").as_deref() == Ok(test) {
        body();
        return;
    }
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([test, "--exact", "--include-ignored", "--nocapture", "--test-threads=1"])
        .env("SONKORE_CHILD_TEST", test)
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    let tail: Vec<&str> = err.lines().rev().take(6).collect();
    assert!(
        out.status.success(),
        "{test} ended with {}:\n{}",
        out.status,
        tail.into_iter().rev().collect::<Vec<_>>().join("\n")
    );
}

#[test]
fn caf_packet_table_does_not_reserve_what_it_claims() {
    in_child("caf_packet_table_does_not_reserve_what_it_claims", || {
        let mut f = b"caff\0\x01\0\0".to_vec();
        f.extend_from_slice(b"desc");
        f.extend_from_slice(&32i64.to_be_bytes());
        f.extend_from_slice(&44100f64.to_be_bytes());
        f.extend_from_slice(b"lpcm");
        for v in [0x0Cu32, 4, 1, 2, 16] {
            f.extend_from_slice(&v.to_be_bytes()); // flags, bytes and frames per packet, channels, bits
        }
        f.extend_from_slice(b"pakt");
        f.extend_from_slice(&24i64.to_be_bytes());
        f.extend_from_slice(&(1i64 << 40).to_be_bytes()); // packets
        f.extend_from_slice(&[0; 16]);
        f.extend_from_slice(b"data");
        f.extend_from_slice(&68i64.to_be_bytes());
        f.extend_from_slice(&[0; 68]);
        let path = fuzz_lite::scratch("bug-caf").join("pakt.caf");
        fs::write(&path, f).unwrap();
        play_a_little(&path);
    });
}
