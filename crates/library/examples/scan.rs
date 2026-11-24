//! Scans folders into a library kept in a data folder of its own, and says
//! how long it took:
//!   cargo run --release -p sonkore-library --example scan -- <data dir> <settings file> [folder|file…]
//! Folders and files given are added first (as the interface's "add" does);
//! without any, the library's own folders are scanned again.
use std::path::Path;
use std::time::{Duration, Instant};

use sonkore_library::{Library, LibraryFiles};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let files = LibraryFiles::in_dir(Path::new(&args[0]), Path::new(&args[1]));
    if let Ok(label) = std::env::var("SONKORE_TRACK_LABEL") {
        sonkore_library::metadata::set_untitled_track_label(&label); // "Pista {0}" in Spanish
    }
    let t0 = Instant::now();
    let mut lib = Library::open(files);
    println!("loaded {} tracks in {:.3} s", lib.count(), t0.elapsed().as_secs_f64());
    let t1 = Instant::now();
    if args.len() > 2 {
        lib.add_urls(&args[2..]);
    } else {
        lib.rescan();
    }
    lib.wait_idle(Duration::from_secs(3600));
    let secs = t1.elapsed().as_secs_f64();
    println!("scanned {} tracks in {:.3} s ({:.0} files/s)", lib.count(), secs, lib.count() as f64 / secs.max(1e-9));
    if std::env::var_os("SONKORE_ANALYZE").is_some() {
        let t2 = Instant::now();
        lib.analyze_loudness();
        lib.wait_idle(Duration::from_secs(3600));
        println!("measured loudness in {:.3} s", t2.elapsed().as_secs_f64());
    }
}
