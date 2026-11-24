//! What FFmpeg makes of a file: `cargo run -p sonkore-ffmpeg --example ffinfo FILE…`
fn main() {
    println!("{:?}", sonkore_ffmpeg::versions());
    for path in std::env::args().skip(1) {
        match sonkore_ffmpeg::Stream::open_path(std::path::Path::new(&path)) {
            Ok(mut s) => {
                let info = s.info().clone();
                let mut buf = vec![0f64; 4096 * info.channels];
                let mut n = 0u64;
                loop {
                    let got = s.read_f64(&mut buf).unwrap();
                    if got == 0 {
                        break;
                    }
                    n += got as u64;
                }
                println!("{path}: {info:?} decoded {n}");
            }
            Err(e) => println!("{path}: {e}"),
        }
    }
}
