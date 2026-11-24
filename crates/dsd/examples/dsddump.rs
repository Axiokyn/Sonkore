//! Raw little-endian dumps of what the decoder gives, on stdout (written to
//! compare the decoder's output with a reference, byte for byte).
//!   dsddump words <file> <dop|be|le> [seek] [block]
//!   dsddump pcm   <file> [seek] [block]
//!   dsddump mid   <file>
//!   dsddump taps  <dsdRate>
//!   dsddump sd    <multiple> <ch> <dop|be|le> <in.f64> [block]
//!   dsddump info  <file> [dop|be|le|pcm]
//!   dsddump bench <file>          DSD→PCM speed (stderr)

use std::io::Write;
use std::path::Path;
use std::time::Instant;

use sonkore_dsd::{Decoder, DsdDecoder, DsdPacking, DsdToPcm, SigmaDelta};

fn packing(s: &str) -> DsdPacking {
    match s {
        "dop" => DsdPacking::Dop,
        "le" => DsdPacking::NativeU32Le,
        _ => DsdPacking::NativeU32Be,
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arg = |i: usize| args.get(i).map(String::as_str);
    let num = |i: usize, d: u64| arg(i).map(|s| s.parse().unwrap()).unwrap_or(d);
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    match arg(1).unwrap_or("") {
        "words" => {
            let mut d = DsdDecoder::open(Path::new(arg(2).unwrap()), packing(arg(3).unwrap())).unwrap();
            if arg(4).is_some() {
                d.seek(num(4, 0)).unwrap();
            }
            let block = num(5, 1000) as usize;
            let ch = d.info().channels;
            let mut buf = vec![0i32; block * ch];
            loop {
                let n = d.read_i32(&mut buf).unwrap();
                if n == 0 {
                    break;
                }
                for w in &buf[..n * ch] {
                    out.write_all(&w.to_le_bytes()).unwrap();
                }
            }
        }
        "pcm" | "mid" => {
            let path = Path::new(arg(2).unwrap());
            let mut d =
                if arg(1) == Some("pcm") { DsdToPcm::open(path) } else { DsdToPcm::open_intermediate(path) }.unwrap();
            if arg(1) == Some("pcm") && arg(3).is_some() {
                d.seek(num(3, 0)).unwrap();
            }
            let block = num(4, 1024) as usize;
            let ch = d.info().channels;
            let mut buf = vec![0f64; block * ch];
            loop {
                let n = d.read_f64(&mut buf).unwrap();
                if n == 0 {
                    break;
                }
                for v in &buf[..n * ch] {
                    out.write_all(&v.to_le_bytes()).unwrap();
                }
            }
        }
        "taps" => {
            for v in DsdToPcm::first_stage(num(2, 2_822_400) as u32) {
                out.write_all(&v.to_le_bytes()).unwrap();
            }
        }
        "sd" => {
            let multiple = num(2, 64) as u32;
            let ch = num(3, 2) as usize;
            let raw = std::fs::read(arg(5).unwrap()).unwrap();
            let input: Vec<f64> = raw.chunks_exact(8).map(|b| f64::from_le_bytes(b.try_into().unwrap())).collect();
            let block = num(6, 4097) as usize;
            let mut sd = SigmaDelta::new();
            sd.setup(multiple, ch, packing(arg(4).unwrap()));
            let mut words = vec![0i32; (block / sd.bits_per_word() + 2) * ch];
            let start = Instant::now();
            for chunk in input.chunks(block * ch) {
                let n = sd.process(chunk, &mut words);
                for w in &words[..n * ch] {
                    out.write_all(&w.to_le_bytes()).unwrap();
                }
            }
            let n = sd.flush(&mut words);
            for w in &words[..n * ch] {
                out.write_all(&w.to_le_bytes()).unwrap();
            }
            eprintln!("resets {} ({:.3} s)", sd.resets(), start.elapsed().as_secs_f64());
        }
        "info" => {
            let path = Path::new(arg(2).unwrap());
            let mode = arg(3).unwrap_or("dop");
            let d: Box<dyn Decoder> = if mode == "pcm" {
                Box::new(DsdToPcm::open(path).unwrap())
            } else {
                Box::new(DsdDecoder::open(path, packing(mode)).unwrap())
            };
            writeln!(out, "{:?}", d.info()).unwrap();
        }
        "bench" => {
            let path = Path::new(arg(2).unwrap());
            let start = Instant::now();
            let mut d = DsdToPcm::open(path).unwrap();
            let ch = d.info().channels;
            let mut buf = vec![0f64; 4096 * ch];
            let mut frames = 0;
            while let Ok(n @ 1..) = d.read_f64(&mut buf) {
                frames += n;
            }
            let secs = frames as f64 / d.info().rate as f64;
            let took = start.elapsed().as_secs_f64();
            eprintln!("{} : {:.2} s of audio in {:.3} s ({:.0}x real time)", d.info().codec, secs, took, secs / took);
        }
        _ => {
            eprintln!("modes: words pcm mid taps sd info bench");
            std::process::exit(2);
        }
    }
}
