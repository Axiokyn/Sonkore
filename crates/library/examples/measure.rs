//! EBU R128 of files, one by one and as an album:
//!   cargo run -p sonkore-library --example measure -- <file|sheet.cue#n>…
use std::sync::atomic::AtomicBool;

fn main() {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    let m = sonkore_library::loudness::measure_album(&paths, &AtomicBool::new(false));
    for (p, t) in paths.iter().zip(&m.tracks) {
        println!(
            "{:>9} gain {:+.6} dB  peak {:.6}  {}",
            if t.ok { "measured" } else { "failed" },
            t.gain_db(),
            t.peak,
            p
        );
    }
    println!("album ok={} gain {:+.6} dB peak {:.6}", m.album.ok, m.album.gain_db(), m.album.peak);
}
