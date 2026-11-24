//! The engine against the test corpus (tests/make_corpus.sh):
//! SONKORE_TEST_CORPUS=<dir> cargo test -p sonkore-engine
//! Without the variable these tests pass without checking anything.
//!
//! References come from FFmpeg (`-f s32le`: integer sources left-justified
//! to 32 bits, as the engine delivers them).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sonkore_engine::Engine;
use sonkore_engine::decode::{self, Decoder as _};
use sonkore_engine::output::{DeviceFormat, MemoryOutput, Output, Render, Request};

fn corpus() -> Option<PathBuf> {
    std::env::var_os("SONKORE_TEST_CORPUS").map(PathBuf::from)
}

/// FFmpeg's decoding of `path` as left-justified s32.
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
    let mut d = decode::open(path).unwrap();
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
fn lossless_decoding_is_bit_exact() {
    let Some(c) = corpus() else { return };
    for name in [
        "album/01.flac",
        "formats/hires-24-96.flac",
        "formats/hires-24-192.wav",
        "formats/alac.m4a",
        "formats/aiff16.aiff",
        "formats/mono16.wav",
    ] {
        let path = c.join(name);
        let (info, ours) = decode_all(&path);
        let theirs = reference(&path);
        assert!(info.lossless, "{name}: lossless");
        assert_eq!(ours.len(), theirs.len(), "{name}: length");
        assert!(ours == theirs, "{name}: samples differ");
        if let Some(total) = info.total_frames {
            assert_eq!(total as usize, ours.len() / info.channels, "{name}: total frames");
        }
    }
}

#[test]
fn seeking_lands_on_the_sample() {
    let Some(c) = corpus() else { return };
    for name in ["album/01.flac", "formats/hires-24-96.flac", "formats/alac.m4a"] {
        let path = c.join(name);
        let all = reference(&path);
        let mut d = decode::open(&path).unwrap();
        let ch = d.info().channels;
        for &frame in &[0u64, 1, 4095, 4096, 50_001, 123_457] {
            if frame as usize * ch >= all.len() {
                continue;
            }
            assert_eq!(d.seek(frame).unwrap(), frame);
            let mut buf = vec![0i32; 2000 * ch];
            let n = d.read_i32(&mut buf).unwrap();
            let want = &all[frame as usize * ch..(frame as usize * ch + n * ch).min(all.len())];
            assert_eq!(&buf[..want.len()], want, "{name}: after a seek to {frame}");
        }
    }
}

#[test]
fn lossy_sources_are_float_and_trimmed() {
    let Some(c) = corpus() else { return };
    // part1 is 177777 frames: gapless MP3 / AAC come back that long.
    for (name, slack) in [("formats/mp3.mp3", 0usize), ("formats/aac.m4a", 0), ("formats/vorbis.ogg", 0)] {
        let path = c.join(name);
        if !path.exists() {
            continue;
        }
        let mut d = decode::open(&path).unwrap();
        let info = d.info().clone();
        assert!(info.float && !info.lossless, "{name}");
        let mut total = 0;
        let mut buf = vec![0f64; 4096 * info.channels];
        loop {
            let n = d.read_f64(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            total += n;
        }
        assert!(total.abs_diff(177_777) <= slack, "{name}: {total} frames");
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

/// Where `needle` starts in `haystack`.
fn find(haystack: &[i32], needle: &[i32]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[test]
fn gapless_album_plays_bit_perfect() {
    let Some(c) = corpus() else { return };
    let album = c.join("album");
    let (mut engine, captured) = memory_engine(6.0, 32);
    engine.load(&album.join("01.flac"), 0.0).unwrap();
    engine.set_next(Some(&album.join("02.flac")));
    engine.play();
    assert!(engine.bit_perfect());
    let mut changes = 0;
    let deadline = Instant::now() + Duration::from_secs(20);
    while !engine.finished() && Instant::now() < deadline {
        if engine.take_track_change() {
            changes += 1;
            engine.set_next(if changes == 1 { Some(album.join("03.flac")) } else { None }.as_deref());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(changes, 2, "two joins");
    assert!(engine.finished(), "played to the end");
    assert_eq!(engine.underruns(), 0);
    let played = captured.lock().unwrap().clone().unwrap().lock().unwrap().clone();
    let long = reference(&album.join("long.wav"));
    // The start fades in (de-click): compare from past the fade.
    let skip = 64.max(44100 * 8 / 1000) * 2;
    let start = find(&played, &long[skip..skip + 4096]).expect("the music in the output");
    let ours = &played[start..start + long.len() - skip];
    let first_diff = ours.iter().zip(&long[skip..]).position(|(a, b)| a != b);
    assert_eq!(first_diff, None, "the three tracks joined give the long file back");
}

#[test]
fn volume_goes_through_the_quantizer() {
    let Some(c) = corpus() else { return };
    let path = c.join("album/01.flac");
    let (mut engine, captured) = memory_engine(8.0, 24);
    engine.set_volume(0.5);
    engine.load(&path, 0.0).unwrap();
    engine.play();
    assert!(!engine.bit_perfect());
    std::thread::sleep(Duration::from_millis(1200));
    engine.pause();
    let played = captured.lock().unwrap().clone().unwrap().lock().unwrap().clone();
    let reference = reference(&path);
    // Past the fade in, each sample is half the original on the 24-bit grid
    // (±1 LSB of TPDF dither).
    let lsb = 256i64;
    let start = played.iter().position(|&s| s != 0).unwrap() + 2 * 2048;
    let half = &reference[..];
    let probe: Vec<i32> = half[4096..4096 + 64].iter().map(|&s| s / 2).collect();
    let at = played[start..]
        .windows(64)
        .position(|w| w.iter().zip(&probe).all(|(a, b)| (*a as i64 - *b as i64).abs() <= 2 * lsb))
        .expect("halved music");
    let base = start + at - 4096;
    for i in 4096..20000 {
        let want = reference[i] as i64 / 2;
        assert!((played[base + i] as i64 - want).abs() <= 2 * lsb, "sample {i}");
        assert_eq!(played[base + i] as i64 % lsb, 0, "on the 24-bit grid");
    }
}

#[test]
fn seek_and_position() {
    let Some(c) = corpus() else { return };
    let (mut engine, _captured) = memory_engine(1.0, 32);
    engine.load(&c.join("album/02.flac"), 0.0).unwrap();
    engine.play();
    std::thread::sleep(Duration::from_millis(300));
    let p = engine.position();
    assert!((0.1..0.6).contains(&p), "position {p}");
    engine.seek(2.5).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    let p = engine.position();
    assert!((2.5..3.0).contains(&p), "position after seek {p}");
    engine.pause();
    let paused = engine.position();
    std::thread::sleep(Duration::from_millis(200));
    assert!((engine.position() - paused).abs() < 1e-9, "paused");
    assert!((engine.duration() - 177_536.0 / 44100.0).abs() < 1e-6, "duration {}", engine.duration());
}

#[test]
#[cfg(target_os = "linux")]
fn plays_through_alsa_null_device() {
    // ALSA's "null" device: the whole ALSA path, and nothing is heard.
    let Some(c) = corpus() else { return };
    let mut engine = Engine::new();
    engine.set_device("null");
    match engine.load(&c.join("formats/hires-24-96.flac"), 0.0) {
        Ok(info) => assert_eq!(info.rate, 96000),
        Err(e) => {
            eprintln!("no ALSA null device here: {e}");
            return;
        }
    }
    let format = engine.device_format().unwrap();
    assert_eq!((format.rate, format.channels), (96000, 2));
    engine.play();
    std::thread::sleep(Duration::from_millis(700));
    // ("null" has no clock: it takes everything at once.)
    let p = engine.position();
    assert!(p >= 0.3, "position {p} with format {format:?}");
    assert_eq!(format.bits, 32);
    assert_eq!(format.period_frames, 96000 * 40 / 1000, "40 ms periods");
    engine.stop();
}

#[test]
fn player_plays_the_queue_gapless_and_skips_bad_files() {
    use sonkore_engine::{Player, Repeat};
    let Some(c) = corpus() else { return };
    let album = c.join("album");
    let (engine, captured) = memory_engine(8.0, 32);
    let mut player = Player::new(engine);
    let s = |p: PathBuf| p.to_string_lossy().into_owned();
    let paths = vec![
        s(album.join("01.flac")),
        s(album.join("missing.flac")),
        s(album.join("02.flac")),
        s(album.join("03.flac")),
    ];
    player.play_paths(paths, 0);
    assert!(player.playing());
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut seen = vec![player.queue().current()];
    let mut finished = false;
    while Instant::now() < deadline {
        let changes = player.poll();
        if changes.track && seen.last() != Some(&player.queue().current()) {
            seen.push(player.queue().current());
        }
        if changes.finished {
            finished = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(finished, "the queue ran out");
    // 01 ended for real (02 was not armed: the missing file came first and
    // was skipped), then 02 → 03 joined without a gap.
    assert_eq!(seen, vec![Some(0), Some(2), Some(3)]);
    assert!(!player.playing());
    // Repeat one: the track joins itself.
    player.set_repeat(Repeat::One);
    player.play_index(3);
    std::thread::sleep(Duration::from_millis(400));
    assert!(player.playing());
    drop(captured);
}

#[test]
fn cue_tracks_are_their_part_of_the_image() {
    let Some(c) = corpus() else { return };
    let sheet = c.join("cue/imagen.cue");
    if !sheet.exists() {
        return;
    }
    let long = reference(&c.join("album/long.wav"));
    // Track 2 starts at 00:04:00 (4 s) and ends where track 3 starts (8 s 37 frames).
    let (info, ours) = decode_all(&PathBuf::from(format!("{}#2", sheet.display())));
    let start = 4 * 44100 * 2;
    let end = ((8 * 75 + 37) * 44100 + 37) / 75 * 2;
    assert_eq!(ours.len(), end - start, "length");
    assert_eq!(info.total_frames, Some(((end - start) / 2) as u64));
    assert!(ours == long[start..end], "samples");
    // A seek inside the track.
    let mut d = decode::open(&PathBuf::from(format!("{}#2", sheet.display()))).unwrap();
    d.seek(1000).unwrap();
    let mut buf = vec![0i32; 200];
    d.read_i32(&mut buf).unwrap();
    assert_eq!(&buf[..], &long[start + 2000..start + 2200]);
}

/// An engine whose device only takes `device_rate`.
fn fixed_rate_engine(device_rate: u32, speed: f64) -> (Engine, Arc<Mutex<Option<Arc<Mutex<Vec<i32>>>>>>) {
    let captured = Arc::new(Mutex::new(None));
    let slot = captured.clone();
    let engine = Engine::with_outputs(Box::new(move |_key: &str, req: Request, render: Arc<dyn Render>| {
        let format = DeviceFormat {
            rate: device_rate,
            channels: req.channels,
            bits: 32,
            float: false,
            mono_to_stereo: false,
            exclusive: true,
            period_frames: 1024,
            buffer_frames: 3072,
            dsd: None,
        };
        let out = MemoryOutput::open(format, render, speed, usize::MAX);
        *slot.lock().unwrap() = Some(out.captured());
        Ok(Box::new(out) as Box<dyn Output>)
    }));
    (engine, captured)
}

fn play_to_end(engine: &mut Engine, seconds: u64) {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while !engine.finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(engine.finished(), "played to the end");
}

fn music_frames(captured: &Arc<Mutex<Option<Arc<Mutex<Vec<i32>>>>>>, channels: usize) -> usize {
    let played = captured.lock().unwrap().clone().unwrap().lock().unwrap().clone();
    let first = played.iter().position(|&s| s != 0).unwrap_or(0) / channels;
    let last = played.iter().rposition(|&s| s != 0).unwrap_or(0) / channels;
    last - first + 1
}

#[test]
fn resamples_when_the_device_refuses_the_rate() {
    let Some(c) = corpus() else { return };
    // (At 2× real time: the worker paces itself for a real device.)
    let (mut engine, captured) = fixed_rate_engine(48000, 2.0);
    engine.load(&c.join("album/02.flac"), 0.0).unwrap();
    assert_eq!(engine.device_format().unwrap().rate, 48000);
    assert!(!engine.bit_perfect());
    engine.play();
    play_to_end(&mut engine, 20);
    assert_eq!(engine.underruns(), 0);
    // The span of the output includes the filter's faint ringing at both ends.
    let frames = music_frames(&captured, 2);
    let want = 177_536.0 * 48000.0 / 44100.0;
    assert!((frames as f64 - want).abs() < 6000.0, "{frames} frames, want ~{want}");
}

#[test]
fn tempo_shortens_the_track() {
    let Some(c) = corpus() else { return };
    let (mut engine, captured) = fixed_rate_engine(44100, 8.0);
    engine.dsp().stretch.set_enabled(true);
    engine.dsp().stretch.set_tempo(2.0);
    engine.load(&c.join("album/02.flac"), 0.0).unwrap();
    engine.play();
    play_to_end(&mut engine, 20);
    let frames = music_frames(&captured, 2);
    assert!((frames as f64 - 177_536.0 / 2.0).abs() < 3000.0, "{frames} frames at tempo 2");
}

#[test]
fn equalizer_takes_the_path_off_bit_perfect() {
    use sonkore_eq::super_eq::Band;
    let Some(c) = corpus() else { return };
    let (mut engine, captured) = fixed_rate_engine(44100, 8.0);
    engine.dsp().eq.set_bands(vec![Band {
        freq_hz: 1000.0,
        gain_db: 6.0,
        q: 1.0,
        enabled: true,
        ..Default::default()
    }]);
    engine.dsp().eq.set_enabled(true);
    engine.load(&c.join("album/01.flac"), 0.0).unwrap();
    assert!(!engine.bit_perfect());
    engine.play();
    play_to_end(&mut engine, 20);
    let frames = music_frames(&captured, 2);
    assert!(frames.abs_diff(177_777) < 64, "{frames} frames");
}

/// A stereo DSD64 DSF file of `seconds` with a pattern that is never the
/// idle byte (0x69), so the music is easy to find in what the device got.
fn write_dsf(path: &Path, seconds: f64) {
    const BLOCK: usize = 4096;
    let channels = 2usize;
    let samples = (2_822_400.0 * seconds) as u64; // bits per channel
    let bytes_per_ch = samples.div_ceil(8) as usize;
    let blocks = bytes_per_ch.div_ceil(BLOCK);
    let mut data = Vec::with_capacity(blocks * BLOCK * channels);
    for b in 0..blocks {
        for c in 0..channels {
            for i in 0..BLOCK {
                let n = b * BLOCK + i;
                let v = if n < bytes_per_ch { ((n * 7 + c * 13) % 251) as u8 ^ 0x5A } else { 0x69 };
                data.push(if v == 0x69 { 0x6A } else { v });
            }
        }
    }
    let data_size = 12 + data.len() as u64;
    let total = 28 + 52 + data_size;
    let mut f = Vec::new();
    f.extend_from_slice(b"DSD ");
    f.extend_from_slice(&28u64.to_le_bytes());
    f.extend_from_slice(&total.to_le_bytes());
    f.extend_from_slice(&0u64.to_le_bytes());
    f.extend_from_slice(b"fmt ");
    f.extend_from_slice(&52u64.to_le_bytes());
    for v in [1u32, 0, 2, channels as u32, 2_822_400, 1] {
        f.extend_from_slice(&v.to_le_bytes());
    }
    f.extend_from_slice(&samples.to_le_bytes());
    f.extend_from_slice(&(BLOCK as u32).to_le_bytes());
    f.extend_from_slice(&0u32.to_le_bytes());
    f.extend_from_slice(b"data");
    f.extend_from_slice(&data_size.to_le_bytes());
    f.extend_from_slice(&data);
    std::fs::write(path, f).unwrap();
}

fn dsd_words(path: &Path, packing: sonkore_dsd::DsdPacking) -> Vec<i32> {
    let mut d = decode::DsdRaw::open(path, packing).unwrap();
    let mut all = Vec::new();
    let mut buf = vec![0i32; 4096 * 2];
    loop {
        let n = d.read_i32(&mut buf).unwrap();
        if n == 0 {
            break;
        }
        all.extend_from_slice(&buf[..n * 2]);
    }
    all
}

#[test]
fn dsd_goes_out_untouched_native_and_dop() {
    use sonkore_dsd::DsdPacking;
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let a = dir.join("dsd-a.dsf");
    let b = dir.join("dsd-b.dsf");
    write_dsf(&a, 0.5);
    write_dsf(&b, 0.3);
    for (mode, packing) in [(1, DsdPacking::NativeU32Be), (2, DsdPacking::Dop)] {
        let (mut engine, captured) = memory_engine(4.0, 32);
        engine.set_device("alsa|hw:CARD=Test,DEV=0");
        engine.set_dsd_output(mode);
        engine.set_volume(0.5); // DSD ignores it
        let info = engine.load(&a, 0.0).unwrap();
        assert_eq!(info.rate, sonkore_dsd::frame_rate(2_822_400, packing), "{packing:?}: frame rate");
        engine.set_next(Some(&b));
        engine.play();
        assert!(engine.bit_perfect(), "{packing:?}: bit-perfect");
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut changes = 0;
        while !engine.finished() && Instant::now() < deadline {
            if engine.take_track_change() {
                changes += 1;
                engine.set_next(None);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(changes, 1, "{packing:?}: gapless into the second file");
        assert_eq!(engine.underruns(), 0, "{packing:?}: underruns");
        let played = captured.lock().unwrap().clone().unwrap().lock().unwrap().clone();
        let silence = sonkore_dsd::silence(packing, false);
        let mut music = dsd_words(&a, packing);
        music.extend(dsd_words(&b, packing));
        // DoP: the markers are the device thread's; compare the DSD bits.
        let mask = |v: i32| if packing == DsdPacking::Dop { v & 0x00FF_FF00 } else { v };
        let start = played.iter().position(|&v| mask(v) != mask(silence)).expect("music");
        assert!(start >= sonkore_dsd::output::preroll_frames(info.rate) * 2, "{packing:?}: preroll first");
        assert!(played[..start].iter().all(|&v| mask(v) == mask(silence)), "{packing:?}: silence is the idle pattern");
        let ours: Vec<i32> = played[start..start + music.len()].iter().map(|&v| mask(v)).collect();
        let theirs: Vec<i32> = music.iter().map(|&v| mask(v)).collect();
        assert!(ours == theirs, "{packing:?}: the words are the files'");
        if packing == DsdPacking::Dop {
            for (f, frame) in played.chunks_exact(2).enumerate().skip(1) {
                let prev = (played[(f - 1) * 2] as u32) >> 24;
                let now = (frame[0] as u32) >> 24;
                assert!(
                    prev != now && (now == 0x05 || now == 0xFA) && frame[0] >> 24 == frame[1] >> 24,
                    "DoP markers alternate (frame {f})"
                );
            }
        }
    }
}

#[test]
fn a_rate_change_starts_with_the_resync_silence() {
    // playback/resyncDelayMs: after the device changes rate, silence first
    // (the DAC locks on it); the position waits for the music.
    let Some(c) = corpus() else { return };
    let (mut engine, _captured) = memory_engine(1.0, 32);
    engine.set_resync_delay_ms(500);
    engine.load(&c.join("album/02.flac"), 0.0).unwrap();
    engine.play();
    std::thread::sleep(Duration::from_millis(300));
    assert!(engine.position() < 0.02, "silent first, position {}", engine.position());
    std::thread::sleep(Duration::from_millis(600));
    assert!(engine.position() > 0.15, "then the music, position {}", engine.position());

    // Another rate: the device reopens, silence again.
    engine.load(&c.join("formats/hires-24-96.flac"), 0.0).unwrap();
    engine.play();
    std::thread::sleep(Duration::from_millis(300));
    assert!(engine.position() < 0.02, "silent after the rate change, position {}", engine.position());
    std::thread::sleep(Duration::from_millis(600));
    assert!(engine.position() > 0.15, "then the music again, position {}", engine.position());
    engine.stop();
}

// --- Generated files (no corpus needed) ------------------------------------------

/// A 16-bit PCM WAV of `frames` frames: a quiet sine, a different pitch per channel.
fn write_wav(path: &Path, rate: u32, channels: u16, frames: usize) {
    let mut data = Vec::with_capacity(frames * channels as usize * 2);
    for f in 0..frames {
        for c in 0..channels as usize {
            let v = ((f as f64 * (0.05 + 0.01 * c as f64)).sin() * 8000.0) as i16;
            data.extend_from_slice(&v.to_le_bytes());
        }
    }
    let block = channels as u32 * 2;
    let mut w = Vec::new();
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&channels.to_le_bytes());
    w.extend_from_slice(&rate.to_le_bytes());
    w.extend_from_slice(&(rate.wrapping_mul(block)).to_le_bytes());
    w.extend_from_slice(&(block as u16).to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&(data.len() as u32).to_le_bytes());
    w.extend_from_slice(&data);
    std::fs::write(path, w).unwrap();
}

fn tmp(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name)
}

#[test]
fn a_low_rate_file_plays_on_a_faster_device() {
    // The ring was sized for the track's rate before the device opened: at
    // 8 kHz on a 48 kHz device a block never fitted, and nothing played.
    for (rate, channels) in [(8000u32, 2u16), (11025, 2), (8000, 1)] {
        let path = tmp(&format!("low-{rate}-{channels}.wav"));
        write_wav(&path, rate, channels, rate as usize);
        let (mut engine, captured) = fixed_rate_engine(48000, 4.0);
        engine.load(&path, 0.0).unwrap();
        engine.play();
        play_to_end(&mut engine, 10);
        let frames = music_frames(&captured, channels as usize);
        assert!((frames as f64 - 48000.0).abs() < 2000.0, "{rate} Hz × {channels}: {frames} frames");
    }
}

/// A device that plays for `alive` and then stops for good, as an unplugged one.
struct DyingOutput {
    format: DeviceFormat,
    failed: Arc<std::sync::atomic::AtomicBool>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl DyingOutput {
    fn open(format: DeviceFormat, render: Arc<dyn Render>, alive: Duration) -> Self {
        use std::sync::atomic::{AtomicBool, Ordering};
        let failed = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let (f, s) = (failed.clone(), stop.clone());
        let thread = std::thread::spawn(move || {
            let start = Instant::now();
            let mut block = vec![0i32; format.period_frames * format.channels];
            let period = Duration::from_secs_f64(format.period_frames as f64 / format.rate as f64);
            while !s.load(Ordering::Acquire) {
                if start.elapsed() > alive {
                    f.store(true, Ordering::Release);
                    return;
                }
                render.render(&mut block);
                std::thread::sleep(period);
            }
        });
        DyingOutput { format, failed, stop, thread: Some(thread) }
    }
}

impl Output for DyingOutput {
    fn format(&self) -> DeviceFormat {
        self.format
    }
    fn latency_frames(&self) -> u64 {
        self.format.period_frames as u64
    }
    fn key(&self) -> &str {
        "dying"
    }
    fn failed(&self) -> bool {
        self.failed.load(std::sync::atomic::Ordering::Acquire)
    }
}

impl Drop for DyingOutput {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn plain_format(rate: u32, channels: usize) -> DeviceFormat {
    DeviceFormat {
        rate,
        channels,
        bits: 32,
        float: false,
        mono_to_stereo: false,
        exclusive: true,
        period_frames: 1024,
        buffer_frames: 3072,
        dsd: None,
    }
}

#[test]
fn a_lost_output_is_reported_and_closed() {
    use sonkore_engine::Player;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let path = tmp("lost-output.wav");
    write_wav(&path, 44100, 2, 44100 * 5);
    let opens = Arc::new(AtomicUsize::new(0));
    let counted = opens.clone();
    let engine = Engine::with_outputs(Box::new(move |_key: &str, req: Request, render: Arc<dyn Render>| {
        counted.fetch_add(1, Ordering::Relaxed);
        Ok(Box::new(DyingOutput::open(plain_format(req.rate, req.channels), render, Duration::from_millis(300)))
            as Box<dyn Output>)
    }));
    let mut player = Player::new(engine);
    player.play_paths(vec![path.to_string_lossy().into_owned()], 0);
    assert!(player.playing());
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut error = false;
    while Instant::now() < deadline && !error {
        error = player.poll().error;
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(error, "the lost output is reported");
    assert!(!player.playing() && !player.loaded(), "and the track stops there");
    assert!(!player.last_error().is_empty());
    let at = player.position();
    assert!(at > 0.05, "the position is kept ({at})");
    // Nothing waits on the dead device, and the next play opens it again.
    let t = Instant::now();
    player.pause();
    player.play();
    assert!(t.elapsed() < Duration::from_millis(450), "{:?}", t.elapsed());
    assert_eq!(opens.load(Ordering::Relaxed), 2, "opened again");
    assert!(player.playing());
    assert!((player.position() - at).abs() < 0.2, "from where it was: {} vs {at}", player.position());
}

#[test]
fn an_output_error_does_not_try_the_whole_queue() {
    use sonkore_engine::Player;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let paths: Vec<String> = (0..5)
        .map(|i| {
            let p = tmp(&format!("queue-{i}.wav"));
            write_wav(&p, 44100, 2, 4410);
            p.to_string_lossy().into_owned()
        })
        .collect();
    let opens = Arc::new(AtomicUsize::new(0));
    let counted = opens.clone();
    let engine = Engine::with_outputs(Box::new(move |_key: &str, _req: Request, _render: Arc<dyn Render>| {
        counted.fetch_add(1, Ordering::Relaxed);
        Err(sonkore_engine::output::OutputError("device busy".into()))
    }));
    let mut player = Player::new(engine);
    let c = player.play_paths(paths, 0);
    assert!(c.error && !player.loaded());
    assert_eq!(opens.load(Ordering::Relaxed), 1);
    let c = player.next();
    assert!(c.error && !player.loaded());
    assert_eq!(opens.load(Ordering::Relaxed), 2, "one try: the device fails for every file alike");
    assert_eq!(player.queue().current(), Some(1));
}

#[test]
fn the_bit_perfect_badge_follows_the_whole_chain() {
    let path = tmp("badge.wav");
    write_wav(&path, 44100, 2, 44100);
    let (mut engine, _captured) = memory_engine(4.0, 32);
    engine.load(&path, 0.0).unwrap();
    assert!(engine.bit_perfect(), "neutral");
    engine.dsp().update(|s| s.crossfeed = 1);
    assert!(!engine.bit_perfect(), "crossfeed");
    engine.dsp().update(|s| s.crossfeed = 0);
    assert!(engine.bit_perfect());
    engine.dsp().stretch.set_enabled(true);
    engine.dsp().stretch.set_pitch_semitones(1.0);
    assert!(!engine.bit_perfect(), "pitch");
    engine.dsp().stretch.set_pitch_semitones(0.0);
    assert!(engine.bit_perfect());
    let ir = sonkore_dsp::convolver::ImpulseResponse {
        path: PathBuf::from("ir.wav"),
        sample_rate: 44100,
        channels: 1,
        taps: vec![vec![1.0, 0.0, 0.0, 0.0]],
    };
    engine.dsp().update(|s| {
        s.impulse = Some(Arc::new(ir));
        s.impulse_generation += 1;
    });
    assert!(!engine.bit_perfect(), "convolver");
    engine.dsp().update(|s| {
        s.impulse = None;
        s.impulse_generation += 1;
    });
    assert!(engine.bit_perfect());
    // HDCD decoding changes the samples.
    if let Some(hdcd) = corpus().map(|c| c.join("libhdcd/hdcd.wav")).filter(|p| p.exists()) {
        engine.set_hdcd(true);
        let info = engine.load(&hdcd, 0.0).unwrap();
        assert!(info.codec.ends_with("HDCD"), "{}", info.codec);
        assert!(!engine.bit_perfect(), "HDCD");
    }
}

#[test]
fn a_long_reverb_tail_does_not_hold_the_engine() {
    // The convolver's tail was computed in one go at the end of the track,
    // with the worker's lock held: every call that takes it waited.
    let path = tmp("tail.wav");
    write_wav(&path, 44100, 2, 4410);
    let (mut engine, _captured) = memory_engine(4.0, 32);
    let taps: Vec<f64> = (0..1 << 15).map(|i| if i == 0 { 0.5 } else { 1e-6 }).collect();
    let ir = sonkore_dsp::convolver::ImpulseResponse {
        path: PathBuf::from("ir.wav"),
        sample_rate: 44100,
        channels: 1,
        taps: vec![taps],
    };
    engine.dsp().update(|s| {
        s.impulse = Some(Arc::new(ir));
        s.impulse_generation += 1;
    });
    engine.load(&path, 0.0).unwrap();
    engine.play();
    let mut worst = Duration::ZERO;
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline && !engine.finished() {
        let t = Instant::now();
        engine.take_clipped();
        worst = worst.max(t.elapsed());
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(engine.finished(), "played to the end");
    // (One block of a 32768-tap convolver takes ~20 ms in a debug build; the
    // whole tail at once, ~0.6 s.)
    assert!(worst < Duration::from_millis(150), "the lock was held {worst:?}");
}

#[test]
fn absurd_formats_are_refused() {
    // A corrupt header's rate sized the ring (and overflowed with upsampling).
    let path = tmp("absurd-rate.wav");
    write_wav(&path, 4_000_000_000, 2, 64);
    let (mut engine, _captured) = memory_engine(4.0, 32);
    engine.set_rate_mode(3, 0);
    if let Ok(info) = engine.load(&path, 0.0) {
        panic!("loaded at {} Hz", info.rate);
    }
}

#[test]
fn replaygain_lands_on_the_first_sample_of_a_joined_track() {
    // The next track's gain came only when its start was heard (the app sets
    // it on the track change): a ring's worth of it played at the previous
    // track's gain, then glided. Given with the next track, it lands at the join.
    let (a, b) = (tmp("rg-a.wav"), tmp("rg-b.wav"));
    write_wav(&a, 44100, 2, 44100);
    write_wav(&b, 44100, 2, 44100);
    let (mut engine, captured) = memory_engine(6.0, 32);
    engine.set_track_gain(1.0);
    engine.load(&a, 0.0).unwrap();
    engine.set_next(Some(&b));
    gain_with_next(&mut engine, 0.5);
    engine.play();
    let deadline = Instant::now() + Duration::from_secs(20);
    while !engine.finished() && Instant::now() < deadline {
        if engine.take_track_change() {
            engine.set_track_gain(0.5); // what the app does when it hears the change
            engine.set_next(None);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(engine.finished());
    let played = captured.lock().unwrap().clone().unwrap().lock().unwrap().clone();
    let a_ref = reference(&a);
    let skip = 64.max(44100 * 8 / 1000) * 2;
    let start = find(&played, &a_ref[skip..skip + 4096]).expect("track A in the output") - skip;
    let b_start = start + a_ref.len();
    let peak = |from: usize| played[from..from + 4096].iter().map(|v| v.unsigned_abs()).max().unwrap() as f64;
    let (head, tail) = (peak(b_start), peak(b_start + 30000 * 2));
    assert!((head / tail - 1.0).abs() < 0.02, "B starts at {head}, goes on at {tail}");
}

/// The next track's gain, as the player gives it with the track.
fn gain_with_next(engine: &mut Engine, gain: f64) {
    engine.set_next_track_gain(Some(gain));
}
