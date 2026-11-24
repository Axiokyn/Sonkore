//! The formats only FFmpeg decodes, in the library: listed, their tags,
//! length and format read (by the library's own readers), and their
//! loudness measured through FFmpeg. Against the corpus of
//! tests/make_corpus.sh (`SONKORE_TEST_CORPUS=<dir>`, files in <dir>/ffmpeg);
//! without it, or without FFmpeg's libraries, these tests check nothing.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use sonkore_library::loudness::{Measurement, measure_album};
use sonkore_library::metadata;

fn corpus() -> Option<PathBuf> {
    let c = PathBuf::from(std::env::var_os("SONKORE_TEST_CORPUS")?);
    if !c.join("ffmpeg").is_dir() || !sonkore_ffmpeg::available() {
        eprintln!("skipped: no corpus/ffmpeg (tests/make_corpus.sh) or no FFmpeg libraries");
        return None;
    }
    Some(c)
}

fn s(p: &Path) -> String {
    sonkore_core::paths::stored_path(&p.to_string_lossy())
}

fn measure(path: &str) -> Measurement {
    measure_album(&[path.to_string()], &AtomicBool::new(false)).tracks[0]
}

#[test]
fn ffmpeg_formats_are_listed_with_tags_length_and_format() {
    for ext in ["opus", "ape", "wv", "tta", "tak", "mpc", "wma", "w64", "ac3", "dts"] {
        assert!(metadata::is_audio_file(&format!("/music/x.{ext}")), "{ext}");
    }
    let Some(c) = corpus() else { return };
    let f = |name: &str| s(&c.join("ffmpeg").join(name));
    for (name, codec, rate, bits) in [
        ("wavpack-16.wv", "WavPack", 44100, 16),
        ("tta-16.tta", "TTA", 44100, 16),
        ("opus.opus", "Opus", 48000, 0),
        ("wma.wma", "WMA", 44100, 0),
    ] {
        let t = metadata::read(&f(name)).unwrap_or_else(|| panic!("{name} not read"));
        assert_eq!(
            (t.title.as_str(), &*t.artist, &*t.album, t.track_no, t.year, &*t.genre),
            ("Título FFmpeg", "Artista FF", "Álbum FF", 3, 2021, "Jazz"),
            "{name}"
        );
        assert_eq!((&*t.codec, t.sample_rate, t.bits, t.channels), (codec, rate, bits, 2), "{name}");
        // The length FFmpeg plays, to the millisecond (WMA's header is in
        // milliseconds and leaves out the encoder's delay).
        let mut stream = sonkore_ffmpeg::Stream::open_path(Path::new(&f(name))).unwrap();
        let mut buf = vec![0f64; 8192];
        let mut frames = 0;
        loop {
            let n = stream.read_f64(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            frames += n;
        }
        let played = frames as f64 / rate as f64;
        let slack = if codec == "WMA" { 0.1 } else { 0.001 };
        assert!((t.duration - played).abs() <= slack, "{name}: {} s listed, {played} s played", t.duration);
    }
    for (name, label) in [("wavpack-24.wv", "WavPack 24/96"), ("tta-24.tta", "TTA 24/96"), ("ac3.ac3", "AC-3 384k")] {
        assert_eq!(metadata::read(&f(name)).unwrap().format_label(), label, "{name}");
    }
}

#[test]
fn ffmpeg_formats_are_measured() {
    let Some(c) = corpus() else { return };
    let f = |name: &str| s(&c.join("ffmpeg").join(name));
    // Lossless: the very same measurement as the source WAV's (symphonia's decoding).
    for (name, source) in [
        ("wavpack-16.wv", "src-16-44.wav"),
        ("tta-16.tta", "src-16-44.wav"),
        ("wavpack-24.wv", "src-24-96.wav"),
        ("tta-24.tta", "src-24-96.wav"),
    ] {
        let (m, want) = (measure(&f(name)), measure(&f(source)));
        assert!(m.ok && want.ok, "{name}");
        assert_eq!((m.lufs, m.peak), (want.lufs, want.peak), "{name}");
    }
    // Lossy: close to the source.
    let src48 = measure(&f("src-16-48.wav"));
    for name in ["opus.opus", "opus-in-ogg.ogg", "opus.webm", "ac3.ac3"] {
        let m = measure(&f(name));
        assert!(m.ok, "{name}");
        assert!((m.lufs - src48.lufs).abs() < 0.3, "{name}: {} LUFS, the source {}", m.lufs, src48.lufs);
    }
    let m = measure(&f("wma.wma"));
    assert!(m.ok && (m.lufs - measure(&f("src-16-44.wav")).lufs).abs() < 0.3);
    // A WavPack CD image's track: its stretch, as the FLAC image's.
    for n in [1, 3] {
        let wv = measure(&format!("{}#{n}", f("cue/imagen.cue")));
        let flac = measure(&format!("{}#{n}", s(&c.join("cue/imagen.cue"))));
        assert!(wv.ok, "track {n}");
        assert_eq!((wv.lufs, wv.peak), (flac.lufs, flac.peak), "track {n}");
    }
}
