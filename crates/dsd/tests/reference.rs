//! Comparisons with FFmpeg, on the corpus that tests/reference/make_reference.sh
//! builds. Skipped (passing) unless SONKORE_DSD_CORPUS is set; the FFmpeg
//! ones also need `ffmpeg` on the PATH.

mod common;

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;

use common::*;
use sonkore_dsd::{Decoder, DsdDecoder, DsdPacking};

fn corpus() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("SONKORE_DSD_CORPUS")?);
    Some(dir)
}

fn run(cmd: &Path, args: &[&str]) -> Vec<u8> {
    let out = Command::new(cmd).args(args).output().unwrap();
    assert!(out.status.success(), "{cmd:?} {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    out.stdout
}

const PACKINGS: [(DsdPacking, &str); 3] =
    [(DsdPacking::Dop, "dop"), (DsdPacking::NativeU32Be, "be"), (DsdPacking::NativeU32Le, "le")];

fn words(path: &Path, packing: DsdPacking, seek: Option<u64>, block: usize) -> Vec<i32> {
    let mut d = DsdDecoder::open(path, packing).unwrap();
    if let Some(s) = seek {
        d.seek(s).unwrap();
    }
    read_all_i32(&mut d, block)
}

/// DSDIFF top-level chunks: (id, header offset, body size).
fn chunks(file: &[u8]) -> Vec<([u8; 4], usize, usize)> {
    let mut v = Vec::new();
    let mut pos = 16;
    while pos + 12 <= file.len() {
        let size = u64::from_be_bytes(file[pos + 4..pos + 12].try_into().unwrap()) as usize;
        v.push((file[pos..pos + 4].try_into().unwrap(), pos, size));
        pos += 12 + size + (size & 1);
    }
    v
}

#[test]
fn dst_is_bit_exact() {
    let Some(dir) = corpus() else { return };
    for (dst, plain) in
        [("dsd64-dst.dff", "dsd64.dff"), ("dsd64-dst-mixed.dff", "dsd64.dff"), ("dsd128-dst.dff", "dsd128.dff")]
    {
        let (dst, plain) = (dir.join(dst), dir.join(plain));
        if !dst.exists() {
            continue;
        }
        for (packing, _) in PACKINGS {
            assert_eq!(words(&dst, packing, None, 1000), words(&plain, packing, None, 1000), "{dst:?}");
            for at in [1u64, 4703, 12_345, 50_000] {
                assert_eq!(words(&dst, packing, Some(at), 777), words(&plain, packing, Some(at), 777), "{dst:?} @{at}");
            }
        }
        let d = DsdDecoder::open(&dst, DsdPacking::Dop).unwrap();
        assert!(d.is_dst());
        assert_eq!(d.info().decoder, "DST decoder");
        assert!(d.info().codec.contains(" DST "));
    }
}

#[test]
fn dst_without_index_or_with_frame_offsets() {
    let Some(dir) = corpus() else { return };
    let path = dir.join("dsd64-dst.dff");
    let want = words(&dir.join("dsd64.dff"), DsdPacking::NativeU32Be, None, 1000);
    let file = std::fs::read(&path).unwrap();
    let list = chunks(&file);
    let (_, index_at, index_size) = *list.iter().find(|c| &c.0 == b"DSTI").unwrap();

    // No DSTI: the frames are found by walking the chunks.
    let mut no_index = file[..index_at].to_vec();
    let form = (no_index.len() - 12) as u64;
    no_index[4..12].copy_from_slice(&form.to_be_bytes());
    // An index whose entries point at the frame data rather than the chunk.
    let mut data_index = file.clone();
    for e in 0..index_size / 12 {
        let at = index_at + 12 + e * 12;
        let v = u64::from_be_bytes(data_index[at..at + 8].try_into().unwrap()) + 12;
        data_index[at..at + 8].copy_from_slice(&v.to_be_bytes());
    }
    // A broken index (an offset out of range): ignored, frames walked.
    let mut bad_index = file.clone();
    bad_index[index_at + 12..index_at + 20].copy_from_slice(&u64::MAX.to_be_bytes());
    for bytes in [no_index, data_index, bad_index] {
        let mut d = DsdDecoder::from_source(Box::new(Cursor::new(bytes.clone())), DsdPacking::NativeU32Be).unwrap();
        assert_eq!(read_all_i32(&mut d, 1000), want);
        // Seeking far ahead first, then back (the walk resumes where it stopped).
        let mut d = DsdDecoder::from_source(Box::new(Cursor::new(bytes)), DsdPacking::NativeU32Be).unwrap();
        d.seek(80_000).unwrap();
        let mut buf = vec![0i32; 200];
        d.read_i32(&mut buf).unwrap();
        assert_eq!(&buf[..], &want[160_000..160_200]);
        d.seek(0).unwrap();
        assert_eq!(read_all_i32(&mut d, 4096), want);
    }
}

#[test]
fn dst_corrupt_frame_gives_silence() {
    let Some(dir) = corpus() else { return };
    let mut file = std::fs::read(dir.join("dsd64-dst.dff")).unwrap();
    let list = chunks(&file);
    let (_, dst_at, _) = *list.iter().find(|c| &c.0 == b"DST ").unwrap();
    // The first DSTF chunk after FRTE: zero its frame (a "segmented" header).
    let frte = u64::from_be_bytes(file[dst_at + 16..dst_at + 24].try_into().unwrap()) as usize;
    let dstf = dst_at + 12 + 12 + frte + (frte & 1);
    assert_eq!(&file[dstf..dstf + 4], b"DSTF");
    file[dstf + 12] = 0x80;
    let mut d = DsdDecoder::from_source(Box::new(Cursor::new(file)), DsdPacking::NativeU32Be).unwrap();
    let got = read_native(&mut d, 1000);
    assert!(got[0][..4704].iter().all(|&b| b == 0x69));
    let want = read_native(&mut DsdDecoder::open(&dir.join("dsd64.dff"), DsdPacking::NativeU32Be).unwrap(), 1000);
    assert_eq!(&got[0][4704..], &want[0][4704..]);
}

#[test]
fn bitstream_matches_ffmpeg_demux() {
    let Some(dir) = corpus() else { return };
    if Command::new("ffmpeg").arg("-version").output().is_err() {
        return;
    }
    for name in ["dsd64.dsf", "dsd128.dsf", "dsd256.dsf", "dsd512.dsf", "dsd64.dff", "dsd128.dff"] {
        let path = dir.join(name);
        if !path.exists() {
            continue;
        }
        let raw = run(
            Path::new("ffmpeg"),
            &["-nostdin", "-v", "error", "-i", path.to_str().unwrap(), "-map", "0:a", "-c", "copy", "-f", "data", "-"],
        );
        let mut d = DsdDecoder::open(&path, DsdPacking::NativeU32Be).unwrap();
        let fmt = d.format();
        let ours = read_native(&mut d, 4096);
        let ch = fmt.channels as usize;
        let mut theirs = vec![Vec::new(); ch];
        if name.ends_with(".dsf") {
            // dsd_lsbf_planar packets: a block per channel, LSB first. The
            // last packet FFmpeg cuts to the audio's length (not planar any
            // more): only whole packets are compared.
            let block = 4096;
            for group in raw.chunks_exact(block * ch) {
                for c in 0..ch {
                    theirs[c].extend(group[c * block..(c + 1) * block].iter().map(|b| b.reverse_bits()));
                }
            }
        } else {
            for (i, b) in raw.iter().enumerate() {
                theirs[i % ch].push(*b);
            }
            assert_eq!(theirs[0].len() as u64, fmt.bytes_per_channel);
        }
        for c in 0..ch {
            let n = theirs[c].len().min(fmt.bytes_per_channel as usize);
            assert!(n as u64 + 8192 >= fmt.bytes_per_channel);
            assert_eq!(&ours[c][..n], &theirs[c][..n], "{name} channel {c}");
        }
        // FFmpeg's rate is bytes per second (8 bits per sample).
        let probe = run(
            Path::new("ffprobe"),
            &["-v", "error", "-show_entries", "stream=sample_rate,channels", "-of", "csv=p=0", path.to_str().unwrap()],
        );
        let probe = String::from_utf8(probe).unwrap();
        assert_eq!(probe.trim(), format!("{},{}", fmt.dsd_rate / 8, ch), "{name}");
    }
}
