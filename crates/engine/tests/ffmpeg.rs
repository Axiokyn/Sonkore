//! The formats the engine decodes through FFmpeg (`sonkore_ffmpeg`), against
//! the corpus of tests/make_corpus.sh (`SONKORE_TEST_CORPUS=<dir>`, files in
//! <dir>/ffmpeg). Without the corpus or without FFmpeg's libraries these
//! tests pass without checking anything.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sonkore_engine::Engine;
use sonkore_engine::decode;
use sonkore_engine::output::{DeviceFormat, MemoryOutput, Output, Render, Request};

fn corpus() -> Option<PathBuf> {
    let c = PathBuf::from(std::env::var_os("SONKORE_TEST_CORPUS")?);
    if !c.join("ffmpeg").is_dir() || !sonkore_ffmpeg::available() {
        eprintln!("skipped: no corpus/ffmpeg (tests/make_corpus.sh) or no FFmpeg libraries");
        return None;
    }
    Some(c)
}

/// FFmpeg's (the command's) decoding of `path` as left-justified s32.
fn reference(path: &Path) -> Vec<i32> {
    let out = Command::new("ffmpeg")
        .args(["-nostdin", "-loglevel", "error", "-i"])
        .arg(path)
        .args(["-f", "s32le", "-c:a", "pcm_s32le", "-"])
        .output()
        .expect("ffmpeg");
    out.stdout.chunks_exact(4).map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect()
}

fn decode_all(path: &Path) -> (decode::StreamInfo, Vec<i32>) {
    let mut d = decode::open(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let info = d.info().clone();
    let mut all = Vec::new();
    let mut buf = vec![0i32; 4096 * info.channels];
    loop {
        let n = d.read_i32(&mut buf).unwrap();
        if n == 0 {
            break;
        }
        all.extend_from_slice(&buf[..n * info.channels]);
    }
    (info, all)
}

#[test]
fn lossless_ffmpeg_formats_are_bit_exact_at_their_depth() {
    let Some(c) = corpus() else { return };
    let f = c.join("ffmpeg");
    for (name, source, bits) in [
        ("wavpack-16.wv", "src-16-44.wav", 16),
        ("wavpack-24.wv", "src-24-96.wav", 24),
        ("tta-16.tta", "src-16-44.wav", 16),
        ("tta-24.tta", "src-24-96.wav", 24),
    ] {
        let (info, ours) = decode_all(&f.join(name));
        let want = reference(&f.join(source));
        assert!(info.lossless && !info.float, "{name}: {info:?}");
        assert_eq!(info.bits, bits, "{name}");
        assert_eq!(info.source_bits(), bits, "{name}");
        assert!(info.decoder.starts_with("FFmpeg (libavcodec "), "{name}: {}", info.decoder);
        assert_eq!(info.total_frames, Some((want.len() / 2) as u64), "{name}");
        assert_eq!(ours.len(), want.len(), "{name}");
        assert_eq!(ours.iter().zip(&want).position(|(a, b)| a != b), None, "{name}");
    }
}

#[test]
fn play_from_memory_reads_the_same() {
    let Some(c) = corpus() else { return };
    let path = c.join("ffmpeg/tta-24.tta");
    let (_, disk) = decode_all(&path);
    decode::set_from_memory(true);
    let (_, memory) = decode_all(&path);
    decode::set_from_memory(false);
    assert!(memory == disk);
}

#[test]
fn lossy_ffmpeg_formats_are_float() {
    let Some(c) = corpus() else { return };
    let f = c.join("ffmpeg");
    for (name, codec, rate) in [
        ("opus.opus", "Opus", 48000),
        // Opus where symphonia opens the container and has no decoder.
        ("opus-in-ogg.ogg", "Opus", 48000),
        ("opus.webm", "Opus", 48000),
        ("wma.wma", "WMA", 44100),
        ("ac3.ac3", "AC-3", 48000),
    ] {
        let mut d = decode::open(&f.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        let info = d.info().clone();
        assert!(info.float && !info.lossless && info.bits == 0, "{name}: {info:?}");
        assert_eq!((info.codec.as_str(), info.rate, info.channels), (codec, rate, 2), "{name}");
        assert!(info.decoder.starts_with("FFmpeg"), "{name}");
        let mut frames = 0;
        let mut buf = vec![0f64; 4096 * 2];
        loop {
            let n = d.read_f64(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            frames += n;
        }
        if codec == "Opus" {
            // Pre-skip and end padding trimmed: the source's 240013 frames.
            assert_eq!(frames, 240_013, "{name}");
        } else {
            assert!(frames > 200_000, "{name}: {frames}");
        }
    }
}

#[test]
fn seeks_land_on_the_sample() {
    let Some(c) = corpus() else { return };
    let path = c.join("ffmpeg/wavpack-24.wv");
    let (_, all) = decode_all(&path);
    let mut d = decode::open(&path).unwrap();
    for target in [150_000u64, 3, 299_000, 0, 77_777] {
        assert_eq!(d.seek(target).unwrap(), target);
        let mut buf = vec![0i32; 512 * 2];
        let n = d.read_i32(&mut buf).unwrap();
        let at = target as usize * 2;
        assert_eq!(&buf[..n * 2], &all[at..at + n * 2], "seek to {target}");
    }
}

#[test]
fn cue_tracks_of_a_wavpack_image_are_its_parts() {
    let Some(c) = corpus() else { return };
    // The same sheet over a FLAC image and over a WavPack one.
    for n in 1..=3 {
        let (flac_info, flac) = decode_all(&PathBuf::from(format!("{}#{n}", c.join("cue/imagen.cue").display())));
        let (wv_info, wv) = decode_all(&PathBuf::from(format!("{}#{n}", c.join("ffmpeg/cue/imagen.cue").display())));
        assert_eq!(wv_info.total_frames, flac_info.total_frames, "track {n}");
        assert_eq!(wv.len(), flac.len(), "track {n}");
        assert!(wv == flac, "track {n}");
    }
}

/// An engine on a memory device at `speed` × real time, keeping what it plays.
fn memory_engine(speed: f64, bits: u32) -> (Engine, Arc<Mutex<Option<Arc<Mutex<Vec<i32>>>>>>) {
    let captured = Arc::new(Mutex::new(None));
    let slot = captured.clone();
    let engine = Engine::with_outputs(Box::new(move |_key: &str, req: Request, render: Arc<dyn Render>| {
        let format = DeviceFormat {
            rate: req.rate,
            channels: req.channels,
            bits,
            float: false,
            mono_to_stereo: false,
            exclusive: true,
            period_frames: 1024,
            buffer_frames: 3072,
            dsd: req.dsd,
        };
        let out = MemoryOutput::open(format, render, speed, usize::MAX);
        *slot.lock().unwrap() = Some(out.captured());
        Ok(Box::new(out) as Box<dyn Output>)
    }));
    (engine, captured)
}

fn find(haystack: &[i32], needle: &[i32]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[test]
fn a_wavpack_album_plays_gapless_and_bit_perfect() {
    let Some(c) = corpus() else { return };
    let album = c.join("ffmpeg/album");
    let (mut engine, captured) = memory_engine(6.0, 32);
    engine.load(&album.join("01.wv"), 0.0).unwrap();
    engine.set_next(Some(&album.join("02.wv")));
    engine.play();
    assert!(engine.bit_perfect());
    let mut changes = 0;
    let deadline = Instant::now() + Duration::from_secs(20);
    while !engine.finished() && Instant::now() < deadline {
        if engine.take_track_change() {
            changes += 1;
            engine.set_next(if changes == 1 { Some(album.join("03.wv")) } else { None }.as_deref());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(changes, 2, "two joins");
    assert!(engine.finished(), "played to the end");
    let played = captured.lock().unwrap().clone().unwrap().lock().unwrap().clone();
    let long = reference(&c.join("album/long.wav"));
    // The start fades in (de-click): compare from past the fade.
    let skip = 64.max(44100 * 8 / 1000) * 2;
    let start = find(&played, &long[skip..skip + 4096]).expect("the music in the output");
    let ours = &played[start..start + long.len() - skip];
    assert_eq!(ours.iter().zip(&long[skip..]).position(|(a, b)| a != b), None, "the long file, bit for bit");
}
