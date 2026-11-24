//! Decodes a file to stdout as f32 little-endian interleaved (for comparisons).
use std::io::Write;
fn main() {
    let path = std::env::args().nth(1).expect("a file");
    let mut d = sonkore_engine::decode::open(std::path::Path::new(&path)).expect("open");
    let ch = d.info().channels;
    eprintln!("{:?}", d.info());
    let mut buf = vec![0f64; 4096 * ch];
    let mut out = std::io::stdout().lock();
    loop {
        let n = d.read_f64(&mut buf).expect("read");
        if n == 0 {
            break;
        }
        for s in &buf[..n * ch] {
            out.write_all(&(*s as f32).to_le_bytes()).unwrap();
        }
    }
}
