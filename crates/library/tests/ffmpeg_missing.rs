//! Without FFmpeg's libraries (the loader made to fail): FFmpeg's formats
//! are still listed with their tags (the library reads those itself) and
//! come back not measured. A test binary of its own: turning FFmpeg off is
//! process-wide.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use sonkore_library::loudness::measure_album;
use sonkore_library::metadata;

#[test]
fn without_ffmpeg_its_formats_are_listed_but_not_measured() {
    sonkore_ffmpeg::set_disabled(true);
    let Some(c) = std::env::var_os("SONKORE_TEST_CORPUS").map(PathBuf::from).filter(|c| c.join("ffmpeg").is_dir())
    else {
        return;
    };
    let f = |name: &str| sonkore_core::paths::stored_path(&c.join("ffmpeg").join(name).to_string_lossy());
    let t = metadata::read(&f("opus.opus")).unwrap();
    assert_eq!((t.title.as_str(), &*t.codec), ("Título FFmpeg", "Opus"));
    let paths = [f("opus.opus"), f("wavpack-16.wv"), f("opus-in-ogg.ogg"), f("src-16-44.wav")];
    let m = measure_album(&paths, &AtomicBool::new(false));
    let ok: Vec<bool> = m.tracks.iter().map(|t| t.ok).collect();
    assert_eq!(ok, [false, false, false, true]);
}
