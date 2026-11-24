//! Without FFmpeg (the loader made to fail, as on a system without its
//! libraries): opening says so, and nothing crashes. A test binary of its
//! own: turning FFmpeg off is process-wide.

use sonkore_ffmpeg::{Error, Stream, available, set_disabled, versions};

#[test]
fn without_ffmpeg_files_do_not_open_and_say_why() {
    set_disabled(true);
    assert!(!available());
    assert_eq!(versions(), None);
    let path = std::env::temp_dir().join(format!("sonkore-missing-{}.opus", std::process::id()));
    std::fs::write(&path, b"OggS not really").unwrap();
    match Stream::open_path(&path) {
        Err(Error::Missing(why)) => assert!(!why.is_empty()),
        Err(e) => panic!("{e}"),
        Ok(_) => panic!("opened without FFmpeg"),
    }
    let source = Box::new(std::io::Cursor::new(vec![0u8; 16]));
    assert!(matches!(Stream::open(source, "x.wv"), Err(Error::Missing(_))));
    let _ = std::fs::remove_file(&path);
}
