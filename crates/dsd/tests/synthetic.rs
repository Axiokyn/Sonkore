//! Self-contained tests: DSF / DSDIFF files written here from the crate's own
//! modulator, read back and converted.

mod common;

use common::*;
use sonkore_dsd::{Decoder, DsdDecoder, DsdMode, DsdPacking, DsdToPcm, SigmaDelta, probe, silence};

const DSD64: u32 = 2_822_400;

#[test]
fn dsf_and_dff_give_the_same_bits() {
    // 10001 bytes per channel: not a whole number of words or blocks.
    let chans = sine_dsd(DSD64, 2, 10_001, 1000.0, 0.5);
    let dsf1 = tmp("same-1bit.dsf");
    let dsf8 = tmp("same-8bit.dsf");
    let dff = tmp("same.dff");
    write_dsf(&dsf1, DSD64, &chans, 1, 4096);
    write_dsf(&dsf8, DSD64, &chans, 8, 4096);
    write_dff(&dff, DSD64, &chans);
    for path in [&dsf1, &dsf8, &dff] {
        let fmt = probe(path).unwrap();
        assert_eq!((fmt.dsd_rate, fmt.channels, fmt.bytes_per_channel), (DSD64, 2, 10_001));
        let mut d = DsdDecoder::open(path, DsdPacking::NativeU32Be).unwrap();
        assert_eq!(d.info().total_frames, 2501); // ceil(10001 / 4)
        assert_eq!(d.info().rate, 88_200);
        assert_eq!(d.info().dsd, DsdMode::Native);
        let got = read_native(&mut d, 333);
        for c in 0..2 {
            assert_eq!(&got[c][..10_001], &chans[c][..], "{path:?} channel {c}");
            assert_eq!(&got[c][10_001..], &[0x69; 3], "padded with silence");
        }
    }
}

#[test]
fn packings() {
    let chans = sine_dsd(DSD64, 2, 4000, 700.0, 0.4);
    let path = tmp("packings.dff");
    write_dff(&path, DSD64, &chans);
    let be = read_all_i32(&mut DsdDecoder::open(&path, DsdPacking::NativeU32Be).unwrap(), 1000);
    let le = read_all_i32(&mut DsdDecoder::open(&path, DsdPacking::NativeU32Le).unwrap(), 1000);
    for (b, l) in be.iter().zip(&le) {
        let mut r = b.to_ne_bytes();
        r.reverse();
        assert_eq!(r, l.to_ne_bytes());
    }
    let mut d = DsdDecoder::open(&path, DsdPacking::Dop).unwrap();
    assert_eq!(d.info().rate, 176_400);
    assert_eq!(d.info().bits, 24);
    assert_eq!(d.info().total_frames, 2000);
    assert!(d.info().codec.ends_with("DoP"));
    // Odd block sizes: the marker alternates across reads.
    let dop = read_all_i32(&mut d, 7);
    for (f, pair) in dop.chunks(2).enumerate() {
        for (c, w) in pair.iter().enumerate() {
            let w = *w as u32;
            assert_eq!(w >> 24, if f % 2 == 0 { 0x05 } else { 0xFA });
            assert_eq!(w & 0xFF, 0);
            assert_eq!(((w >> 16) & 0xFF) as u8, chans[c][2 * f]);
            assert_eq!(((w >> 8) & 0xFF) as u8, chans[c][2 * f + 1]);
        }
    }
    // A seek restarts the phase; the phase can be carried over a join.
    d.seek(11).unwrap();
    assert!(!d.dop_phase());
    d.set_dop_phase(true);
    let mut one = [0i32; 2];
    d.read_i32(&mut one).unwrap();
    assert_eq!(one[0] as u32 >> 24, 0xFA);
    assert_eq!(((one[0] as u32 >> 16) & 0xFF) as u8, chans[0][22]);
    assert_eq!(silence(DsdPacking::Dop, false) as u32, 0x0569_6900);
}

#[test]
fn seeking_lands_on_the_same_words() {
    let chans = sine_dsd(DSD64, 2, 20_000, 1000.0, 0.5);
    let path = tmp("seek.dsf");
    write_dsf(&path, DSD64, &chans, 1, 4096);
    for packing in [DsdPacking::NativeU32Be, DsdPacking::Dop] {
        let all = read_all_i32(&mut DsdDecoder::open(&path, packing).unwrap(), 1024);
        let mut d = DsdDecoder::open(&path, packing).unwrap();
        for at in [0u64, 1, 1023, 1024, 2047, 3333] {
            assert_eq!(d.seek(at).unwrap(), at);
            let mut buf = vec![0i32; 64 * 2];
            let n = d.read_i32(&mut buf).unwrap();
            let mut want = all[at as usize * 2..(at as usize + n) * 2].to_vec();
            if packing == DsdPacking::Dop {
                // From a seek the marker starts at 0x05 again.
                for (f, w) in want.iter_mut().enumerate() {
                    let marker = if (f / 2) % 2 == 0 { 0x05u32 } else { 0xFA };
                    *w = ((*w as u32 & 0x00FF_FFFF) | marker << 24) as i32;
                }
            }
            assert_eq!(&buf[..n * 2], &want[..]);
        }
        assert_eq!(d.seek(u64::MAX).unwrap(), d.info().total_frames);
        assert_eq!(d.read_i32(&mut [0; 2]).unwrap(), 0);
    }
}

#[test]
fn multichannel_and_higher_rates() {
    for (rate, ch) in [(5_644_800u32, 6usize), (11_289_600, 1), (22_579_200, 3)] {
        let chans = sine_dsd(rate, ch, 6000, 1000.0, 0.5);
        let path = tmp(&format!("multi-{rate}-{ch}.dff"));
        write_dff(&path, rate, &chans);
        let mut d = DsdDecoder::open(&path, DsdPacking::NativeU32Be).unwrap();
        assert_eq!(d.info().channels, ch);
        assert_eq!(d.info().rate, rate / 32);
        let got = read_native(&mut d, 500);
        for c in 0..ch {
            assert_eq!(&got[c][..6000], &chans[c][..]);
        }
    }
}

#[test]
fn not_dsd() {
    let path = tmp("not.dsf");
    std::fs::write(&path, b"RIFF....WAVEfmt nothing to see here, not DSD at all............................").unwrap();
    assert!(probe(&path).is_none());
    assert!(DsdDecoder::open(&path, DsdPacking::Dop).is_err());
    assert!(DsdToPcm::open(&path).is_err());
    // A DSF below DSD64, or with 9 channels, is refused.
    let chans = vec![vec![0x69u8; 100]; 1];
    let slow = tmp("slow.dsf");
    write_dsf(&slow, 1_411_200, &chans, 1, 4096);
    assert!(probe(&slow).is_none());
}

#[test]
fn pcm_level_and_silence() {
    // All ones = PCM full scale; all zeros = -full scale; 0x69 = silence.
    for (byte, want) in [(0xFFu8, 1.0f64), (0x00, -1.0), (0x69, 0.0)] {
        let chans = vec![vec![byte; 40_000]; 2];
        let path = tmp(&format!("level-{byte:02x}.dff"));
        write_dff(&path, DSD64, &chans);
        let mut d = DsdToPcm::open(&path).unwrap();
        assert_eq!(d.info().rate, 88_200);
        assert_eq!(d.info().dsd, DsdMode::None);
        assert!(!d.info().lossless);
        let pcm = read_all_f64(&mut d, 1000);
        assert_eq!(pcm.len(), 2 * 10_000);
        // Away from the edges (the file ends into DSD silence).
        for v in &pcm[2 * 500..2 * 9_400] {
            assert!((v - want).abs() < 1e-6, "{byte:02x}: {v}");
        }
    }
    // read_i32 clips: full scale is i32::MAX.
    let path = tmp("level-ff.dff");
    let mut d = DsdToPcm::open(&path).unwrap();
    let mut buf = vec![0i32; 2 * 4000];
    d.read_i32(&mut buf).unwrap();
    assert!(buf[2 * 3000] > i32::MAX - 4096);
}

#[test]
fn pcm_round_trip_of_a_sine() {
    // PCM -> DSD (input scaled by 0.45) -> PCM: the sine comes back at 45 %.
    for rate in [DSD64, 5_644_800] {
        let amp = 0.8;
        let freq = 1000.0;
        let chans = sine_dsd(rate, 1, (rate / 8) as usize / 4, freq, amp); // 0.25 s
        let path = tmp(&format!("sine-{rate}.dff"));
        write_dff(&path, rate, &chans);
        let mut d = DsdToPcm::open(&path).unwrap();
        let out_rate = d.info().rate as f64;
        let pcm = read_all_f64(&mut d, 4096);
        // Least-squares fit of a sine and cosine at `freq`, away from the edges.
        let (mut ss, mut sc, mut cc, mut ys, mut yc) = (0.0, 0.0, 0.0, 0.0, 0.0);
        let range = pcm.len() / 10..pcm.len() * 9 / 10;
        for i in range.clone() {
            let t = 2.0 * std::f64::consts::PI * freq * i as f64 / out_rate;
            let (s, c) = t.sin_cos();
            ss += s * s;
            sc += s * c;
            cc += c * c;
            ys += pcm[i] * s;
            yc += pcm[i] * c;
        }
        let det = ss * cc - sc * sc;
        let a = (ys * cc - yc * sc) / det;
        let b = (yc * ss - ys * sc) / det;
        let got = (a * a + b * b).sqrt();
        assert!((got - 0.45 * amp).abs() < 1e-3, "amplitude {got}");
        // Time-aligned (linear phase, delays taken out): phase ~0.
        assert!(b.abs() < 2e-3, "phase: {b}");
        // The residue (modulator noise) is far below the tone in band.
        let mut res = 0.0;
        for i in range.clone() {
            let t = 2.0 * std::f64::consts::PI * freq * i as f64 / out_rate;
            res += (pcm[i] - a * t.sin() - b * t.cos()).powi(2);
        }
        let res_db = 10.0 * (res / range.len() as f64).log10();
        assert!(res_db < -60.0, "residue {res_db} dB");
    }
}

#[test]
fn pcm_seek_matches_playing_from_the_start() {
    let chans = sine_dsd(DSD64, 2, 200_000, 3000.0, 0.7);
    let path = tmp("pcmseek.dsf");
    write_dsf(&path, DSD64, &chans, 1, 4096);
    let all = read_all_f64(&mut DsdToPcm::open(&path).unwrap(), 1000);
    let mut d = DsdToPcm::open(&path).unwrap();
    let total = d.info().total_frames;
    assert_eq!(all.len() as u64, total * 2);
    for at in [0u64, 1, 2047, 2048, 3001, 33_333, total - 5] {
        d.seek(at).unwrap();
        let mut buf = vec![0f64; 2 * 4000];
        let n = d.read_f64(&mut buf).unwrap();
        assert_eq!(n as u64, (total - at).min(4000));
        assert_eq!(&buf[..n * 2], &all[at as usize * 2..(at as usize + n) * 2], "seek {at}");
    }
}

#[test]
fn sigma_delta_threads_match_one_channel() {
    // Each channel runs its own loop (on a worker thread after the first):
    // stereo with the same signal on both equals mono.
    let frames: usize = 100_003;
    let mono: Vec<f64> = (0..frames).map(|i| 0.9 * (i as f64 * 0.003).sin()).collect();
    let stereo: Vec<f64> = mono.iter().flat_map(|v| [*v, *v]).collect();
    for packing in [DsdPacking::NativeU32Be, DsdPacking::NativeU32Le, DsdPacking::Dop] {
        let mut a = SigmaDelta::new();
        a.setup(128, 1, packing);
        let mut b = SigmaDelta::new();
        b.setup(128, 2, packing);
        let bpw = a.bits_per_word();
        let (mut wa, mut wb) = (Vec::new(), Vec::new());
        for (ca, cb) in mono.chunks(999).zip(stereo.chunks(1998)) {
            let mut oa = vec![0i32; 999 / bpw + 1];
            let mut ob = vec![0i32; 2 * (999 / bpw + 1)];
            let na = a.process(ca, &mut oa);
            let nb = b.process(cb, &mut ob);
            assert_eq!(na, nb);
            wa.extend_from_slice(&oa[..na]);
            wb.extend_from_slice(&ob[..2 * nb]);
        }
        let mut oa = [0i32; 1];
        let mut ob = [0i32; 2];
        assert_eq!(a.flush(&mut oa), 1); // 100003 bits: a partial word
        assert_eq!(b.flush(&mut ob), 1);
        wa.push(oa[0]);
        wb.extend_from_slice(&ob);
        assert_eq!(wa.len(), frames.div_ceil(bpw));
        for (i, w) in wa.iter().enumerate() {
            assert_eq!(*w, wb[2 * i]);
            assert_eq!(*w, wb[2 * i + 1]);
        }
        if packing == DsdPacking::Dop {
            assert!(wa.iter().all(|w| (*w as u32) >> 24 == 0x05 && *w & 0xFF == 0));
        }
        assert_eq!(a.resets(), 0);
        // The DSD has the input's sign on average.
        let ones: u32 = wa.iter().map(|w| (*w as u32).count_ones()).sum();
        assert!(ones > 0);
    }
}

#[test]
fn sigma_delta_overload_resets() {
    let mut sd = SigmaDelta::new();
    sd.setup(64, 1, DsdPacking::NativeU32Be);
    let mut out = vec![0i32; 1000];
    let nan = vec![f64::NAN; 64];
    sd.process(&nan, &mut out);
    assert_eq!(sd.resets(), 64); // NaN: every sample restarts the loop
    sd.reset();
    let n = sd.process(&vec![0.0; 3200], &mut out);
    assert_eq!(n, 100);
    // Zero input gives a balanced stream.
    let ones: u32 = out[..n].iter().map(|w| (*w as u32).count_ones()).sum();
    assert!((ones as i64 - 1600).abs() < 40, "{ones}");
    // Re-setup with other channels (workers restarted).
    sd.setup(256, 4, DsdPacking::Dop);
    let n = sd.process(&vec![0.1; 4 * 160], &mut out);
    assert_eq!(n, 10);
    assert_eq!(sd.channels(), 4);
}
