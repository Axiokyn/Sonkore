//! FFmpeg decoding against the sources the corpus encoded
//! (tests/make_corpus.sh: `SONKORE_TEST_CORPUS=<dir>/ffmpeg/…`). Skipped
//! (passing, with a note) without the corpus or without FFmpeg's libraries.

use std::path::{Path, PathBuf};
use std::process::Command;

use sonkore_ffmpeg::{Stream, available};

fn corpus() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("SONKORE_TEST_CORPUS")?).join("ffmpeg");
    if !dir.is_dir() {
        eprintln!("skipped: no {} (tests/make_corpus.sh)", dir.display());
        return None;
    }
    if !available() {
        eprintln!("skipped: FFmpeg's libraries are not installed");
        return None;
    }
    Some(dir)
}

/// A PCM WAV's samples, left-justified to 32 bits, and its channels.
fn wav(path: &Path) -> (Vec<i32>, usize) {
    let b = std::fs::read(path).unwrap();
    assert_eq!(&b[..4], b"RIFF");
    let (mut p, mut channels, mut bits) = (12, 0, 0);
    while p + 8 <= b.len() {
        let size = u32::from_le_bytes(b[p + 4..p + 8].try_into().unwrap()) as usize;
        let body = &b[p + 8..(p + 8 + size).min(b.len())];
        match &b[p..p + 4] {
            b"fmt " => {
                channels = u16::from_le_bytes([body[2], body[3]]) as usize;
                bits = u16::from_le_bytes([body[14], body[15]]) as usize;
            }
            b"data" => {
                let width = bits / 8;
                let samples = body
                    .chunks_exact(width)
                    .map(|s| {
                        let mut v = [0u8; 4];
                        v[4 - width..].copy_from_slice(s);
                        i32::from_le_bytes(v)
                    })
                    .collect();
                return (samples, channels);
            }
            _ => {}
        }
        p += 8 + size + (size & 1);
    }
    panic!("no data in {}", path.display());
}

fn read_all_i32(s: &mut Stream) -> Vec<i32> {
    let ch = s.info().channels;
    let mut out = Vec::new();
    let mut buf = vec![0i32; 3000 * ch];
    loop {
        let n = s.read_i32(&mut buf).unwrap();
        if n == 0 {
            return out;
        }
        out.extend_from_slice(&buf[..n * ch]);
    }
}

fn read_all_f64(s: &mut Stream) -> Vec<f64> {
    let ch = s.info().channels;
    let mut out = Vec::new();
    let mut buf = vec![0f64; 3000 * ch];
    loop {
        let n = s.read_f64(&mut buf).unwrap();
        if n == 0 {
            return out;
        }
        out.extend_from_slice(&buf[..n * ch]);
    }
}

fn read_f64(s: &mut Stream, frames: usize) -> Vec<f64> {
    let ch = s.info().channels;
    let mut out = vec![0f64; frames * ch];
    let mut done = 0;
    while done < frames {
        let n = s.read_f64(&mut out[done * ch..]).unwrap();
        if n == 0 {
            break;
        }
        done += n;
    }
    out.truncate(done * ch);
    out
}

/// (file, its source WAV, bits)
const LOSSLESS: [(&str, &str, u32); 5] = [
    ("wavpack-16.wv", "src-16-44.wav", 16),
    ("wavpack-24.wv", "src-24-96.wav", 24),
    ("tta-16.tta", "src-16-44.wav", 16),
    ("tta-24.tta", "src-24-96.wav", 24),
    ("pcm-24.w64", "src-24-96.wav", 24),
];

#[test]
fn lossless_files_decode_bit_exact_with_their_length() {
    let Some(dir) = corpus() else { return };
    for (file, source, bits) in LOSSLESS {
        let (want, channels) = wav(&dir.join(source));
        let frames = want.len() / channels;
        let mut s = Stream::open_path(&dir.join(file)).unwrap();
        let info = s.info().clone();
        assert!(info.lossless && !info.float, "{file}: {info:?}");
        assert_eq!((info.bits, info.channels), (bits, channels), "{file}");
        let got = read_all_i32(&mut s);
        if file.ends_with(".w64") {
            // FFmpeg's Wave64 muxer pads the data chunk to 8 bytes and its
            // demuxer reads the padding as a frame of silence.
            assert!(got.len() / channels - frames <= 1, "{file}: {} frames", got.len() / channels);
            assert!(got[want.len()..].iter().all(|&v| v == 0));
        } else {
            assert_eq!(info.total_frames, Some(frames as u64), "{file}");
            assert_eq!(got.len(), want.len(), "{file}: frames decoded");
        }
        let first_bad = got.iter().zip(&want).position(|(a, b)| a != b);
        assert_eq!(first_bad, None, "{file}: differs from {source}");
        // The f64 view is the same samples over 2^31, exactly.
        let mut s = Stream::open_path(&dir.join(file)).unwrap();
        let f = read_f64(&mut s, 5000);
        assert!(f.iter().zip(&want).all(|(&x, &w)| x == w as f64 / 2_147_483_648.0), "{file}");
    }
}

#[test]
fn lossless_seeks_land_on_the_exact_frame() {
    let Some(dir) = corpus() else { return };
    for (file, source, _) in LOSSLESS {
        let (mut want, ch) = wav(&dir.join(source));
        let mut s = Stream::open_path(&dir.join(file)).unwrap();
        // (Wave64's padding frame, silence: see above.)
        let frames = s.info().total_frames.unwrap();
        want.resize(frames as usize * ch, 0);
        for target in [frames / 2, 1, 4095, 4096, 77_777, frames - 10, 0, 123_457, frames] {
            assert_eq!(s.seek(target).unwrap(), target, "{file}");
            let mut buf = vec![0i32; 700 * ch];
            let n = s.read_i32(&mut buf).unwrap();
            let expect = (frames - target).min(700) as usize;
            assert_eq!(n, expect, "{file} at {target}");
            let at = target as usize * ch;
            assert_eq!(&buf[..n * ch], &want[at..at + n * ch], "{file}: seek to {target}");
        }
    }
}

/// The lag (in frames, within ±`span`) at which `b` best matches `a`, on channel 0.
fn best_lag(a: &[f64], b: &[f64], ch: usize, span: i64) -> i64 {
    let frames = (a.len().min(b.len()) / ch) as i64;
    let mut best = (f64::MIN, 0);
    for lag in -span..=span {
        let mut sum = 0.0;
        for i in span..frames - span {
            sum += a[i as usize * ch] * b[(i + lag) as usize * ch];
        }
        if sum > best.0 {
            best = (sum, lag);
        }
    }
    best.1
}

#[test]
fn opus_drops_its_pre_skip_and_end_padding() {
    let Some(dir) = corpus() else { return };
    let (src, ch) = wav(&dir.join("src-16-48.wav"));
    let frames = src.len() / ch;
    let src: Vec<f64> = src.iter().map(|&v| v as f64 / 2_147_483_648.0).collect();
    for file in ["opus.opus", "opus-in-ogg.ogg", "opus.webm"] {
        let mut s = Stream::open_path(&dir.join(file)).unwrap();
        let info = s.info().clone();
        assert_eq!((info.rate, info.channels, info.codec.as_str()), (48000, 2, "Opus"), "{file}");
        assert!(info.float && !info.lossless && info.bits == 0);
        if !file.ends_with(".webm") {
            assert_eq!(info.total_frames, Some(frames as u64), "{file}");
        }
        let got = read_all_f64(&mut s);
        assert_eq!(got.len() / ch, frames, "{file}: every source frame, no more");
        // In time with the source: the encoder's delay is gone.
        assert_eq!(best_lag(&src[..96_000], &got[..96_000], ch, 400), 0, "{file}");
    }
}

#[test]
fn lossy_seeks_land_on_the_exact_frame() {
    let Some(dir) = corpus() else { return };
    for file in ["opus.opus", "opus.webm", "wma.wma", "ac3.ac3", "eac3.eac3", "dts.dts"] {
        let path = dir.join(file);
        if !path.exists() {
            continue; // (an encoder this ffmpeg lacks)
        }
        let mut s = Stream::open_path(&path).unwrap();
        let ch = s.info().channels;
        let whole = read_all_f64(&mut s);
        let total = whole.len() / ch;
        for target in [total / 2, 30_011, 4_801, 0, total - 3000] {
            assert_eq!(s.seek(target as u64).unwrap(), target as u64);
            let got = read_f64(&mut s, 2400);
            assert_eq!(got.len(), 2400.min(total - target) * ch, "{file} at {target}");
            let at = target * ch;
            // The decoder settled during the preroll: the same samples as
            // decoded straight through, at the same place.
            let want = &whole[at..at + got.len()];
            let err = got.iter().zip(want).map(|(a, b)| (a - b).abs()).fold(0.0, f64::max);
            assert!(err < 0.02, "{file} at {target}: off by {err}");
            if got.len() >= 2000 * ch {
                assert_eq!(best_lag(want, &got, ch, 200), 0, "{file} at {target}");
            }
        }
    }
}

#[test]
fn reads_through_any_source_and_reports_failures() {
    let Some(dir) = corpus() else { return };
    // From memory, as Settings › Audio › Play from memory reads files.
    let bytes = std::fs::read(dir.join("tta-16.tta")).unwrap();
    let mut s = Stream::open(Box::new(std::io::Cursor::new(bytes)), "tta-16.tta").unwrap();
    let (want, _) = wav(&dir.join("src-16-44.wav"));
    assert_eq!(read_all_i32(&mut s), want);
    // Not audio at all.
    let junk = Stream::open(Box::new(std::io::Cursor::new(vec![0x55u8; 10_000])), "junk.ape");
    assert!(matches!(junk, Err(sonkore_ffmpeg::Error::Failed(_))));
    // A truncated file plays what it holds.
    let mut half = std::fs::read(dir.join("wavpack-16.wv")).unwrap();
    half.truncate(half.len() / 2);
    let mut s = Stream::open(Box::new(std::io::Cursor::new(half)), "half.wv").unwrap();
    let got = read_all_i32(&mut s);
    assert!(!got.is_empty() && got.len() < want.len());
    assert_eq!(got[..], want[..got.len()]);
}

#[test]
fn offsets_match_the_installed_headers() {
    // abi.rs against the headers of this system and of every release
    // tools/ffmpeg-offsets.sh has fetched (target/ffmpeg-headers/*.txt).
    let mut tables = Vec::new();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    if Path::new("/usr/include/libavformat/avformat.h").exists() {
        let exe = std::env::temp_dir().join(format!("sonkore-ffmpeg-offsets-{}", std::process::id()));
        let built = Command::new("cc")
            .args(["-w", "-o"])
            .arg(&exe)
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/offsets.c"))
            .status();
        if built.is_ok_and(|s| s.success()) {
            let out = Command::new(&exe).output().unwrap();
            tables.push(("system".to_string(), String::from_utf8(out.stdout).unwrap()));
            let _ = std::fs::remove_file(&exe);
        } else {
            eprintln!("no C compiler: the system's headers are not compared");
        }
    }
    if let Ok(dir) = std::fs::read_dir(root.join("target/ffmpeg-headers")) {
        for e in dir.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "txt") {
                tables.push((p.display().to_string(), std::fs::read_to_string(&p).unwrap()));
            }
        }
    }
    if tables.is_empty() {
        eprintln!("skipped: no FFmpeg headers");
        return;
    }
    for (name, table) in tables {
        let rows: Vec<(&str, &str)> = table.lines().filter_map(|l| l.split_once(' ')).collect();
        let v: Vec<u32> = rows[0].1.split(' ').map(|x| x.parse().unwrap()).collect();
        let ours = sonkore_ffmpeg::offsets(v[0], v[1], v[2])
            .unwrap_or_else(|| panic!("{name}: FFmpeg {v:?} is not in abi.rs (tools/ffmpeg-offsets.sh)"));
        let mut compared = 0;
        for (field, offset) in &ours {
            let theirs = rows.iter().find(|r| r.0 == field).unwrap_or_else(|| panic!("{name}: no {field}"));
            assert_eq!(theirs.1.parse::<usize>().unwrap(), *offset, "{name}: {field}");
            compared += 1;
        }
        assert_eq!(compared, ours.len());
    }
}
