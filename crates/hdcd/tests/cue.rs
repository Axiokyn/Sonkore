//! The CUE port: the corpus sheet (tests/make_corpus.sh), edge cases, the
//! library's helpers.

use std::fs;
use std::path::{Path, PathBuf};

use sonkore_hdcd::cue::{self, CueSheet, TrackRange};

fn scratch(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join("cue").join(name);
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn touch(p: &Path) {
    fs::write(p, b"").unwrap();
}

/// The CUE section of tests/make_corpus.sh: the sheet names a .wav, the
/// image is a .flac (converted after ripping).
const CORPUS: &str = r#"REM GENRE Rock
REM DATE 1999
PERFORMER "Banda CUE"
TITLE "Disco CUE"
FILE "imagen.wav" WAVE
  TRACK 01 AUDIO
    TITLE "Uno"
    INDEX 01 00:00:00
  TRACK 02 AUDIO
    TITLE "Dos"
    INDEX 00 00:03:50
    INDEX 01 00:04:00
  TRACK 03 AUDIO
    TITLE "Tres"
    INDEX 01 00:08:37
"#;

#[test]
fn corpus_sheet() {
    let dir = scratch("corpus");
    touch(&dir.join("imagen.flac"));
    let sheet = dir.join("imagen.cue");
    fs::write(&sheet, CORPUS).unwrap();

    let s = CueSheet::parse(&sheet).unwrap();
    assert_eq!(
        (s.title.as_str(), s.performer.as_str(), s.genre.as_str(), s.date.as_str()),
        ("Disco CUE", "Banda CUE", "Rock", "1999")
    );
    assert_eq!(s.album_gain, None);
    assert_eq!(s.tracks.len(), 3);
    let image = dir.join("imagen.flac");
    let want = [(1, "Uno", 0, Some(300)), (2, "Dos", 300, Some(637)), (3, "Tres", 637, None)];
    for (t, (n, title, start, end)) in s.tracks.iter().zip(want) {
        assert_eq!((t.number, t.title.as_str(), t.start, t.end), (n, title, start, end));
        assert_eq!(t.file, image);
        assert_eq!(t.performer, "");
        assert_eq!(t.gain, None);
    }
    // the pregap (INDEX 00 at 00:03:50) stays with track 1
    let t2 = s.track(2).unwrap();
    assert_eq!(t2.start_frame(44100), 176_400);
    assert_eq!(t2.end_frame(44100), Some((637 * 44100 + 37) / 75));
    assert_eq!(
        s.track(1).unwrap().end_frame(44100),
        Some(t2.start_frame(44100)),
        "consecutive tracks share their boundary"
    );
    assert_eq!(s.track(3).unwrap().end_frame(44100), None);
    assert!((t2.duration_seconds(12.0) - 337.0 / 75.0).abs() < 1e-12);
    assert!((s.track(3).unwrap().duration_seconds(12.0) - (12.0 - 637.0 / 75.0)).abs() < 1e-12);

    // as a file of its own (RangeDecoder), over the 12 s image
    let total = 12 * 44100;
    let r1 = s.track(1).unwrap().range(44100, Some(total));
    let r2 = t2.range(44100, Some(total));
    let r3 = s.track(3).unwrap().range(44100, Some(total));
    assert_eq!(r1, TrackRange { start: 0, frames: 176_400 });
    assert_eq!(r1.start + r1.frames, r2.start);
    assert_eq!(r2.start + r2.frames, r3.start);
    assert_eq!(r3.start + r3.frames, total);
    assert_eq!(r2.readable(0, 4096), 4096);
    assert_eq!(r2.readable(r2.frames - 10, 4096), 10);
    assert_eq!(r2.readable(r2.frames + 10, 4096), 0);
    assert_eq!(r2.seek(1000), (1000, 176_400 + 1000));
    assert_eq!(r2.seek(u64::MAX), (r2.frames, r2.start + r2.frames));
    // an image of unknown length: the last track gets nothing
    assert_eq!(s.track(3).unwrap().range(44100, None).frames, 0);
    assert_eq!(t2.range(44100, None), r2);
    assert_eq!(TrackRange::new(10, Some(5), Some(100)).frames, 0);
    assert_eq!(TrackRange::new(10, Some(500), Some(100)).frames, 90);

    // track paths and the library's view
    let p2 = cue::track_path(&sheet, 2);
    assert_eq!(p2.to_str().unwrap(), format!("{}#2", sheet.display()));
    assert!(cue::is_track_path(&p2));
    assert_eq!(cue::split_track_path(&p2), Some((sheet.clone(), 2)));
    assert_eq!(cue::open_track(&p2).unwrap().unwrap(), t2.clone());
    assert_eq!(cue::open_track(cue::track_path(&sheet, 9)).unwrap().unwrap_err(), "Track 9 not found in imagen.cue");
    assert!(cue::open_track(&image).is_none());
    let (paths, images) = cue::cue_tracks(&sheet, |_| true);
    assert_eq!(paths, (1..=3).map(|n| cue::track_path(&sheet, n)).collect::<Vec<_>>());
    assert_eq!(images, vec![image.clone()]);
    let (paths, images) = cue::cue_tracks(&sheet, |p| p.extension().is_some_and(|e| e == "mp3"));
    assert!(paths.is_empty() && images.is_empty());
    let stamp = cue::track_stamp(&p2).unwrap();
    assert_eq!(stamp, cue::modified_ms(&sheet).unwrap().max(cue::modified_ms(&image).unwrap()));
    assert_eq!(cue::track_stamp(dir.join("nada.cue#1")), None);
}

#[test]
fn encodings_and_bom() {
    let dir = scratch("enc");
    touch(&dir.join("a.wav"));
    // Windows-1252: é, €, “ ”, and the undefined 0x81 kept as U+0081
    let mut cp = b"TITLE \"Caf\xe9 \x80 \x93x\x94 \x81\"\nFILE a.wav WAVE\nTRACK 1 AUDIO\nINDEX 01 00:00:00\n".to_vec();
    let s = CueSheet::parse_bytes(&cp, &dir).unwrap();
    assert_eq!(s.title, "Café € “x” \u{81}");
    // UTF-8 with a BOM
    let utf8 = "\u{feff}PERFORMER \"Ñandú\"\nFILE \"a.wav\" WAVE\nTRACK 1 AUDIO\nINDEX 01 00:00:00\n";
    let s = CueSheet::parse_bytes(utf8.as_bytes(), &dir).unwrap();
    assert_eq!(s.performer, "Ñandú");
    // a BOM, then CP1252
    cp.splice(0..0, *b"\xEF\xBB\xBF");
    assert_eq!(CueSheet::parse_bytes(&cp, &dir).unwrap().title, "Café € “x” \u{81}");
    // a character at the very end is fine; a truncated one makes it CP1252
    let tail = "FILE a.wav WAVE\nTRACK 1 AUDIO\nINDEX 01 00:00:00\nTITLE é";
    assert_eq!(CueSheet::parse_bytes(tail.as_bytes(), &dir).unwrap().tracks[0].title, "é");
    let cut = &tail.as_bytes()[..tail.len() - 1];
    assert_eq!(CueSheet::parse_bytes(cut, &dir).unwrap().tracks[0].title, "Ã");
    // CRLF, tabs, lower case keywords
    let crlf = "file \"a.wav\" WAVE\r\n\ttrack 01 AUDIO\r\n\t\ttitle \"Uno\"\r\n\t\tindex 01 01:02:03\r\n";
    let s = CueSheet::parse_bytes(crlf.as_bytes(), &dir).unwrap();
    assert_eq!((s.tracks[0].title.as_str(), s.tracks[0].start), ("Uno", (62 * 75) + 3));
    // over 1 MiB is not a sheet
    let mut big = crlf.as_bytes().to_vec();
    big.resize((1 << 20) + 1, b' ');
    assert!(CueSheet::parse_bytes(&big, &dir).is_none());
}

#[test]
fn files_tracks_and_gains() {
    let dir = scratch("files");
    touch(&dir.join("one.wav"));
    touch(&dir.join("Two.FLAC")); // the sheet says two.wav
    touch(&dir.join("three.txt")); // not audio: three.wav stays missing
    fs::create_dir(dir.join("four.flac")).unwrap(); // not a file
    let text = r#"REM REPLAYGAIN_ALBUM_GAIN -7.25 dB
REM REPLAYGAIN_ALBUM_PEAK 0.988
TITLE "Album"
FILE "one.wav" WAVE
TITLE "After FILE, before TRACK: still the album"
  TRACK 01 AUDIO
    PERFORMER "Ana"
    REM REPLAYGAIN_TRACK_GAIN "+1.50 dB"
    REM REPLAYGAIN_TRACK_PEAK 0.5
    INDEX 01 00:00:00
  TRACK 02 AUDIO
    REM REPLAYGAIN_TRACK_GAIN dB
    REM REPLAYGAIN_TRACK_PEAK x
    INDEX 01 00:10:00
FILE "sub/dir/two.wav" WAVE
  TRACK 03 AUDIO
    INDEX 00 00:00:00
    INDEX 01 00:00:32
  TRACK 04 AUDIO
    INDEX 01 00:00:10
  TRACK 05 AUDIO
    INDEX 01 00:20:00
FILE "three.wav" WAVE
  TRACK 06 AUDIO
    INDEX 01 00:00:00
FILE "four.wav" WAVE
  TRACK 07 AUDIO
    INDEX 01 00:00:00
FILE "one.wav" WAVE
  TRACK 08 AUDIO
    INDEX 00 00:30:00
  TRACK 08 AUDIO
    INDEX 01 00:30:00
"#;
    let s = CueSheet::parse_bytes(text.as_bytes(), &dir).unwrap();
    assert_eq!(s.title, "After FILE, before TRACK: still the album");
    assert_eq!(s.album_gain, Some(-7.25));
    assert_eq!(s.album_peak, 0.988);
    let nums: Vec<i32> = s.tracks.iter().map(|t| t.number).collect();
    assert_eq!(nums, [1, 2, 3, 4, 5, 8], "no file for 6 and 7; the first 8 has no INDEX 01");
    let t = |n| s.track(n).unwrap();
    assert_eq!(t(1).performer, "Ana");
    assert_eq!(t(1).gain, Some(1.5));
    assert_eq!(t(1).peak, 0.5);
    assert_eq!(t(2).gain, None);
    assert!(t(2).peak.is_nan(), "an unreadable peak is NaN");
    assert_eq!(t(3).file, dir.join("Two.FLAC"), "another audio extension, any case");
    assert_eq!(t(1).file, dir.join("one.wav"));
    assert_eq!(t(8).file, dir.join("one.wav"));
    assert_eq!(t(1).end, Some(750));
    assert_eq!(t(2).end, None, "the next track is in another file");
    assert_eq!(t(3).end, None, "the next one starts earlier");
    assert_eq!(t(4).end, Some(1500));
    assert_eq!(t(5).end, None);
    assert_eq!(t(8).start, 2250);
}

#[test]
fn windows_paths_resolve_by_stem() {
    let dir = scratch("winpath");
    touch(&dir.join("disc.flac"));
    let s = CueSheet::parse_bytes(b"FILE \"D:\\Music\\disc.wav\" WAVE\nTRACK 1 AUDIO\nINDEX 01 00:00:00\n", &dir);
    if cfg!(windows) {
        // '\' separates there: the sheet's "disc", in the sheet's folder
        assert_eq!(s.unwrap().track(1).unwrap().file, dir.join("disc.flac"));
    } else {
        // the whole "D:\Music\disc.wav" is taken as a file name: its stem
        // is "D:\Music\disc", which no file has
        assert!(s.is_none());
    }
}

#[test]
fn track_path_helpers() {
    assert!(cue::is_track_path("/m/disc.cue#1"));
    assert!(cue::is_track_path("/m/DISC.CUE#x"));
    assert!(!cue::is_track_path("/m/disc.cue#"));
    assert!(!cue::is_track_path("/m/disc.cue"));
    assert_eq!(cue::split_track_path("/m/Disc.Cue#12"), Some((PathBuf::from("/m/Disc.Cue"), 12)));
    assert_eq!(cue::split_track_path("/m/a.cue#1/b.cue#3"), Some((PathBuf::from("/m/a.cue#1/b.cue"), 3)));
    assert_eq!(cue::split_track_path("/m/disc.cue#1x"), None);
    assert_eq!(cue::split_track_path("/m/disc.cue#-1"), None);
    assert_eq!(cue::split_track_path("/m/disc.cue#"), None);
    assert!(cue::is_sheet("/m/disc.CUE"));
    assert!(!cue::is_sheet("/m/.cue"));
    assert!(!cue::is_sheet("/m/disc.cue#1"));
    assert_eq!(cue::track_path("/m/disc.cue", 7), PathBuf::from("/m/disc.cue#7"));
}
