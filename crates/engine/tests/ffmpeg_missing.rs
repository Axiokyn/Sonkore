//! Without FFmpeg's libraries (the loader made to fail): the files only it
//! decodes do not open, with a message that says why, and everything else
//! plays as before. A test binary of its own: turning FFmpeg off is
//! process-wide.

use std::path::PathBuf;

use sonkore_engine::decode;

fn corpus() -> Option<PathBuf> {
    std::env::var_os("SONKORE_TEST_CORPUS").map(PathBuf::from).filter(|c| c.join("ffmpeg").is_dir())
}

#[test]
fn without_ffmpeg_its_formats_say_they_need_it() {
    sonkore_ffmpeg::set_disabled(true);
    let dir = std::env::temp_dir().join(format!("sonkore-noffmpeg-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let opus = dir.join("canción.opus");
    std::fs::write(&opus, b"OggS").unwrap();
    let e = decode::open(&opus).err().expect("no FFmpeg, no Opus");
    assert_eq!(e.0, "Playing canción.opus needs FFmpeg, which is not installed");
    assert_eq!(decode::ffmpeg_missing_file(&e.0), Some("canción.opus"));
    assert_eq!(decode::ffmpeg_missing_file("File not found: x.opus"), None);
    assert_eq!(decode::ffmpeg_versions(), None);

    if let Some(c) = corpus() {
        // Opus in an .ogg: symphonia opens it, has no decoder, and FFmpeg is missing.
        let e = decode::open(&c.join("ffmpeg/opus-in-ogg.ogg")).err().expect("no Opus decoder");
        assert_eq!(decode::ffmpeg_missing_file(&e.0), Some("opus-in-ogg.ogg"));
        // A WavPack CD image's tracks too.
        let track = PathBuf::from(format!("{}#2", c.join("ffmpeg/cue/imagen.cue").display()));
        assert!(decode::ffmpeg_missing_file(&decode::open(&track).err().unwrap().0).is_some());
        // What symphonia plays is untouched.
        let mut d = decode::open(&c.join("album/01.flac")).unwrap();
        let mut buf = vec![0i32; 4096];
        assert_eq!(d.read_i32(&mut buf).unwrap(), 2048);
        assert_eq!(d.info().decoder, "Symphonia");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
