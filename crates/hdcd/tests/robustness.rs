//! Damaged CUE sheets and damaged HDCD streams: no panic, no endless loop,
//! no absurd allocation (fuzz-lite, see tests/fuzz_lite.rs).

mod common;
#[path = "../../../tests/fuzz_lite.rs"]
mod fuzz_lite;

use std::path::Path;
use std::time::Duration;

use fuzz_lite::{Case, Rng, corpus_seeds, run, scaled};
use sonkore_hdcd::cue::{self, CueSheet, TrackRange};
use sonkore_hdcd::hdcd::{self, Hdcd};

#[global_allocator]
static GUARD: fuzz_lite::Guard = fuzz_lite::Guard;

const STACK: usize = 512 << 10;
const LIMIT: Duration = Duration::from_secs(5);

const SHEET: &str = "\u{FEFF}REM GENRE \"Classical\"\r
REM DATE 1985\r
REM REPLAYGAIN_ALBUM_GAIN -5.10 dB\r
REM REPLAYGAIN_ALBUM_PEAK 0.95\r
PERFORMER \"Cue Performer\"\r
TITLE \"Cue Album\"\r
FILE \"image.wav\" WAVE\r
  TRACK 01 AUDIO\r
    TITLE \"Opening\"\r
    PERFORMER \"Soloist\"\r
    REM REPLAYGAIN_TRACK_GAIN -3.50 dB\r
    REM REPLAYGAIN_TRACK_PEAK 0.80\r
    INDEX 01 00:00:00\r
  TRACK 02 AUDIO\r
    TITLE \"Middle\"\r
    INDEX 00 00:02:50\r
    INDEX 01 00:03:00\r
FILE \"other.flac\" WAVE\r
  TRACK 03 AUDIO\r
    INDEX 01 00:06:37\r
  TRACK 04 AUDIO\r
    INDEX 01 99:59:74\r
";

fn mutate_text(rng: &mut Rng, text: &[u8]) -> Vec<u8> {
    if rng.chance(50) {
        return fuzz_lite::mutate(rng, text);
    }
    let s = String::from_utf8_lossy(text).into_owned();
    let mut words: Vec<String> = s.split(' ').map(str::to_owned).collect();
    let crazy = [
        "0",
        "-1",
        "99999999999999999999",
        "4294967295",
        "18446744073709551615",
        "-9223372036854775808",
        "1e308",
        "NaN",
        "inf",
        "99:99:99",
        "4294967295:59:74",
        "18446744073709551615:00:00",
        "00:00:75",
        "::",
        "1:2",
        "-1:-1:-1",
        "\"",
        "\"\"",
        "TRACK",
        "INDEX",
        "FILE",
        "REM",
        "\u{0}",
        "\u{FEFF}",
        "999999999:00:00",
        "\u{e9}",
        "\u{FFFD}",
        "/../../..",
        "C:\\x.wav",
    ];
    for _ in 0..1 + rng.below(4) {
        let i = rng.below(words.len());
        match rng.below(3) {
            0 => words[i] = rng.pick(&crazy).to_string(),
            1 => {
                let w = words[i].clone();
                words.insert(i, w);
            }
            _ => {
                let lines: Vec<&str> = s.lines().collect();
                let l = rng.pick(&lines).to_string();
                words.insert(i, format!("\n{l}\n"));
            }
        }
    }
    let mut out = words.join(" ").into_bytes();
    if rng.chance(10) {
        // Latin-1 instead of UTF-8.
        out.push(0xE9);
    }
    out
}

/// Everything the engine and the library do with a sheet's tracks.
fn use_sheet(sheet: &CueSheet) {
    let _ = sheet.track(1);
    for t in &sheet.tracks {
        for rate in [1u32, 44100, 192_000, 2_822_400, u32::MAX] {
            let _ = t.start_frame(rate);
            let _ = t.end_frame(rate);
            for image in [None, Some(0), Some(1), Some(44100 * 60), Some(u64::MAX)] {
                let r = t.range(rate, image);
                let _ = r.readable(0, 4096);
                let _ = r.readable(r.frames, 4096);
                let _ = r.readable(u64::MAX, u64::MAX);
                let _ = r.seek(u64::MAX);
                let _ = r.seek(r.frames / 2);
            }
        }
        let _ = t.start_seconds();
        let _ = t.duration_seconds(0.0);
        let _ = t.duration_seconds(f64::NAN);
        let _ = t.duration_seconds(1e300);
    }
}

#[test]
fn cue_sheets_survive_damage() {
    let mut seeds: Vec<Vec<u8>> = vec![SHEET.as_bytes().to_vec()];
    seeds.extend(corpus_seeds(&["cue"]).into_iter().map(|s| s.bytes));
    let make = move |n: usize| {
        let mut rng = Rng::for_case("hdcd-cue", n);
        let seed = &seeds[n % seeds.len()];
        let bytes = if rng.chance(5) {
            let len = rng.below(512);
            rng.bytes(len)
        } else {
            mutate_text(&mut rng, seed)
        };
        Case { label: format!("sheet {} mutated (case {n})", n % seeds.len()), ext: "cue".into(), bytes }
    };
    run("hdcd-cue", scaled(20000), STACK, LIMIT, make, |case: &Case| {
        if let Some(sheet) = CueSheet::parse_bytes(&case.bytes, Path::new("/music/album")) {
            use_sheet(&sheet);
        }
    });
}

#[test]
fn track_ranges_take_any_numbers() {
    let make = |n: usize| {
        let mut rng = Rng::for_case("ranges", n);
        let mut v = || -> u64 {
            match rng.below(4) {
                0 => *rng.pick(&[0, 1, u64::MAX, u64::MAX - 1, u32::MAX as u64, 1 << 63]),
                1 => rng.next() >> rng.below(64),
                _ => rng.below(1 << 20) as u64,
            }
        };
        let bytes = [v(), v(), v(), v(), v()].iter().flat_map(|x| x.to_le_bytes()).collect();
        Case { label: format!("numbers (case {n})"), ext: "bin".into(), bytes }
    };
    run("ranges", scaled(20000), STACK, LIMIT, make, |case: &Case| {
        let v: Vec<u64> = case.bytes.chunks(8).map(|c| u64::from_le_bytes(c.try_into().unwrap())).collect();
        let end = if v[1] % 3 == 0 { None } else { Some(v[1]) };
        let image = if v[2] % 3 == 0 { None } else { Some(v[2]) };
        let r = TrackRange::new(v[0], end, image);
        let _ = r.readable(v[3], v[4]);
        let _ = r.seek(v[3]);
        let _ = cue::cd_to_frames(v[0], v[4] as u32);
        let _ = cue::split_track_path(format!("/a/b.cue#{}", v[3]));
    });
}

// --- HDCD ------------------------------------------------------------------------------

/// What the engine does: detection over the first seconds, then decoding
/// in blocks of left-justified s32 samples.
fn decode_like_the_engine(rate: u32, s32: &[i32]) {
    let mut pos = 0;
    let _ = hdcd::detect(rate, |buf| {
        let n = (buf.len().min(s32.len() - pos)) / 2 * 2;
        buf[..n].copy_from_slice(&s32[pos..pos + n]);
        pos += n;
        n / 2
    });
    let mut h = Hdcd::new();
    if !h.reset_ext(rate, 16) {
        return;
    }
    let mut block = Vec::new();
    for chunk in s32.chunks(4096 * 2 + 6) {
        block.clear();
        block.extend_from_slice(chunk);
        h.process_s32(&mut block);
    }
    let _ = h.detection();
    let _ = h.detect_str();
    let _ = h.channel_stats(0);
}

#[test]
fn hdcd_streams_survive_damage() {
    // Streams with HDCD packets (the reference tests' generator), then
    // damaged: bits flipped anywhere (codes and sync words included),
    // extreme values, runs of garbage.
    let make = |n: usize| {
        let mut rng = Rng::for_case("hdcd", n);
        let mut signal = common::Signal::new(n as u64, 6000 + rng.below(20000));
        signal.format = *rng.pick(&[common::Format::A, common::Format::B, common::Format::Mixed]);
        signal.corrupt = rng.below(400) as u64;
        signal.mismatch = rng.below(400) as u64;
        signal.noisy_lsb = rng.chance(50);
        signal.level = 1 + rng.below(1000) as u64;
        signal.period = 40 + rng.below(4000) as u64;
        let mut samples = signal.generate();
        for _ in 0..rng.below(64) {
            let i = rng.below(samples.len());
            samples[i] = match rng.below(4) {
                0 => samples[i] ^ (1 << rng.below(16)),
                1 => *rng.pick(&[-32768, 32767, 0, -1]),
                2 => rng.next() as i16 as i32,
                _ => samples[i] ^ 1,
            };
        }
        let bytes = samples.iter().flat_map(|s| (*s as i16).to_le_bytes()).collect();
        Case { label: format!("hdcd stream (case {n})"), ext: "raw".into(), bytes }
    };
    run("hdcd", scaled(2000), STACK, Duration::from_secs(10), make, |case: &Case| {
        let s32: Vec<i32> =
            case.bytes.chunks_exact(2).map(|b| (i16::from_le_bytes([b[0], b[1]]) as i32) << 16).collect();
        for rate in [44100, 48000, 88200] {
            decode_like_the_engine(rate, &s32);
        }
    });
}

#[test]
fn hdcd_takes_any_s32_samples() {
    // A 16-bit source's decoder only gives multiples of 65536, but the
    // engine passes whatever comes: full 32-bit noise and extremes too.
    let make = |n: usize| {
        let mut rng = Rng::for_case("hdcd-s32", n);
        let frames = 1 + rng.below(20000);
        let mode = rng.below(3);
        let bytes = (0..frames * 2)
            .flat_map(|_| {
                let v: i32 = match mode {
                    0 => rng.next() as i32,
                    1 => *rng.pick(&[i32::MIN, i32::MAX, 0, -1, 1 << 16, -(1 << 16)]),
                    _ => (rng.next() as i16 as i32) << 16,
                };
                v.to_le_bytes()
            })
            .collect();
        Case { label: format!("s32 samples (case {n})"), ext: "raw".into(), bytes }
    };
    run("hdcd-s32", scaled(2000), STACK, Duration::from_secs(10), make, |case: &Case| {
        let s32: Vec<i32> = case.bytes.chunks_exact(4).map(|b| i32::from_le_bytes(b.try_into().unwrap())).collect();
        decode_like_the_engine(44100, &s32);
        // And Hdcd::process at the other depths, values within them.
        for bits in [16u32, 20, 24] {
            let mut h = Hdcd::new();
            assert!(h.reset_ext(96000, bits));
            let mut v: Vec<i32> = s32.iter().map(|&s| s >> (32 - bits)).collect();
            h.process(&mut v);
        }
    });
}
