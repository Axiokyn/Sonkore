//! The HDCD port against libhdcd: golden digests of synthetic HDCD signals
//! (taken from libhdcd) and libhdcd's own test files against the md5 sums its
//! tests.sh expects.

mod common;

use common::{Format, Rng, Signal, digest, read_wav, run_rust, test_files};
use md5::{Digest, Md5};
use sonkore_hdcd::hdcd::{self, AnalyzeMode, Detected, Hdcd, PeakExtend};
/// (name, signal, rate)
fn cases() -> Vec<(&'static str, Signal, u32)> {
    let s = |seed, frames| Signal::new(seed, frames);
    vec![
        ("b", s(1, 176_400), 44100),
        ("a", Signal { format: Format::A, ..s(2, 176_400) }, 44100),
        ("mixed", Signal { format: Format::Mixed, ..s(3, 176_400) }, 44100),
        ("errors", Signal { corrupt: 250, format: Format::Mixed, ..s(4, 176_400) }, 44100),
        ("mismatch", Signal { mismatch: 150, ..s(5, 176_400) }, 44100),
        ("dropout", Signal { dropout: 200, period: 600, ..s(6, 441_000) }, 44100),
        ("noisy-lsb", Signal { noisy_lsb: true, period: 5000, ..s(7, 176_400) }, 44100),
        ("nop", Signal { nop: true, ..s(8, 100_000) }, 44100),
        ("quiet", Signal { level: 300, ..s(9, 100_000) }, 44100),
        ("dense", Signal { period: 1, ..s(10, 50_000) }, 44100),
        ("r48000", s(11, 96_000), 48000),
        ("r88200", s(12, 176_400), 88200),
        ("r96000", Signal { dropout: 100, ..s(13, 400_000) }, 96000),
        ("r176400", s(14, 176_400), 176400),
        ("r192000", Signal { format: Format::Mixed, ..s(15, 192_000) }, 192000),
        ("bits20", Signal { bits: 20, ..s(16, 100_000) }, 44100),
        ("bits24", Signal { bits: 24, format: Format::Mixed, ..s(17, 100_000) }, 44100),
    ]
}

/// Random 16-bit noise: no HDCD, but now and then a coincidental packet.
fn plain_noise(seed: u64, frames: usize) -> Vec<i32> {
    let mut r = Rng::new(seed);
    (0..frames * 2).map(|_| (r.next() >> 48) as u16 as i16 as i32).collect()
}

/// libhdcd's digests of the cases (output and detection, 2048-frame blocks),
/// recorded from the C library itself.
const GOLDEN: &[(&str, u64)] = &[
    ("b", 0x035257a73b5c8065),
    ("a", 0x1f42c10ca1a89ad9),
    ("mixed", 0x2744a89758a3933b),
    ("errors", 0x2ee10f7838ea6b40),
    ("mismatch", 0xa414d636399dea71),
    ("dropout", 0xbd19e8a7885d3986),
    ("noisy-lsb", 0xd65980db77649133),
    ("nop", 0xbc79f704e5c6db06),
    ("quiet", 0xa5bb0a93e5f90d7a),
    ("dense", 0x366f6e9d3f5b15b6),
    ("r48000", 0x9308b8ef2976794d),
    ("r88200", 0xde80ab917dfffbbc),
    ("r96000", 0x705b6197b0947dba),
    ("r176400", 0xb9f14bf027d39257),
    ("r192000", 0x98f17f84cbe48f44),
    ("bits20", 0x514682fb466364b2),
    ("bits24", 0x5c40e918bf9a1f8f),
];

#[test]
fn golden_digests() {
    for (name, sig, rate) in cases() {
        let input = sig.generate();
        let (out, d) = run_rust(&input, rate, sig.bits, 2048, AnalyzeMode::Off);
        let want = GOLDEN.iter().find(|(n, _)| *n == name).map(|g| g.1);
        assert_eq!(Some(digest(&out, &d)), want, "{name}");
    }
}

#[test]
fn synthetic_signals_are_detected() {
    for (name, sig, rate) in cases() {
        let (out, d) = run_rust(&sig.generate(), rate, sig.bits, 4096, AnalyzeMode::Off);
        assert!(d.total_packets > 0, "{name}: {d:?}");
        let want = if sig.nop { Detected::NoEffect } else { Detected::Effectual };
        assert_eq!(d.detected, want, "{name}: {}", d.summary());
        assert_eq!(out.len(), sig.frames * 2);
    }
    let corrupt = cases().into_iter().find(|c| c.0 == "errors").unwrap().1;
    let (_, d) = run_rust(&corrupt.generate(), 44100, 16, 4096, AnalyzeMode::Off);
    assert!(d.errors > 0);
    let drop = cases().into_iter().find(|c| c.0 == "dropout").unwrap().1;
    let (_, d) = run_rust(&drop.generate(), 44100, 16, 4096, AnalyzeMode::Off);
    assert!(d.cdt_expirations > 0, "{d:?}");
    let mm = cases().into_iter().find(|c| c.0 == "mismatch").unwrap().1;
    let (_, d) = run_rust(&mm.generate(), 44100, 16, 4096, AnalyzeMode::Off);
    assert!(d.lle_mismatch > 0, "{d:?}");
    assert_eq!(d.peak_extend, PeakExtend::Intermittent);
    assert!(d.uses_transient_filter);
    assert!(d.max_gain_adjustment <= -6.0);

    let (_, d) = run_rust(&plain_noise(99, 100_000), 44100, 16, 4096, AnalyzeMode::Off);
    assert_eq!(d.detected, Detected::None);
    assert_eq!(d.summary(), "HDCD detected: no");
}

#[test]
fn block_size_does_not_matter() {
    let sig = Signal { format: Format::Mixed, dropout: 100, mismatch: 50, corrupt: 50, ..Signal::new(42, 300_000) };
    let input = sig.generate();
    let (whole, dw) = run_rust(&input, 44100, 16, input.len(), AnalyzeMode::Off);
    for block in [1, 2, 3, 31, 32, 33, 1000, 4096] {
        let (out, d) = run_rust(&input, 44100, 16, block, AnalyzeMode::Off);
        assert!(out == whole, "block {block}");
        assert_eq!(d, dw, "block {block}");
    }
}

#[test]
fn reset_rules() {
    let mut h = Hdcd::new();
    for (rate, bits, ok) in [
        (0, 0, true),
        (44100, 16, true),
        (48000, 20, true),
        (192000, 24, true),
        (22050, 16, false),
        (44100, 32, false),
        (44100, 8, false),
    ] {
        assert_eq!(h.reset_ext(rate, bits), ok, "{rate} {bits}");
    }
    assert_eq!((h.rate(), h.bits()), (192000, 24));
    h.reset();
    assert_eq!((h.rate(), h.bits()), (44100, 16));
    // a reset forgets the codes found
    let sig = Signal::new(5, 50_000).generate();
    let mut buf = sig.clone();
    h.process(&mut buf);
    assert_eq!(h.detected(), Detected::Effectual);
    h.reset_ext(44100, 16);
    assert_eq!(h.detection(), Hdcd::new().detection());
    // a fresh decoder after a reset decodes as a new one
    let mut a = sig.clone();
    h.process(&mut a);
    let mut b = sig;
    Hdcd::new().process(&mut b);
    assert!(a == b);
}

#[test]
fn scan_predicts_process() {
    let sig = Signal::new(77, 60_000).generate();
    let mut h = Hdcd::new();
    for chunk in sig.chunks(2 * 2048) {
        let predicted = h.scan(chunk, false);
        let mut c = chunk.to_vec();
        h.process(&mut c);
        assert_eq!(predicted, h.detected());
    }
    assert_eq!(Hdcd::new().scan(&sig, true), Detected::Effectual);
}

#[test]
fn sonkore_path_and_detection() {
    let sig = Signal::new(3, 300_000).generate();
    // the decoder's s32 (16-bit << 16) in, as the engine feeds it
    let mut s32: Vec<i32> = sig.iter().map(|&s| s << 16).collect();
    let mut a = Hdcd::new();
    a.reset_ext(44100, 16);
    a.process_s32(&mut s32);
    let mut direct = sig.clone();
    Hdcd::new().process(&mut direct);
    assert!(s32 == direct);

    let reader = |input: Vec<i32>| {
        let mut pos = 0;
        move |buf: &mut [i32]| {
            let n = (buf.len().min(input.len() - pos)) / 2;
            for (d, s) in buf.iter_mut().zip(&input[pos..pos + n * 2]) {
                *d = s << 16;
            }
            pos += n * 2;
            n
        }
    };
    assert!(hdcd::detect(44100, reader(sig.clone())));
    assert!(!hdcd::detect(44100, reader(plain_noise(5, 300_000))));
    assert!(!hdcd::detect(22050, reader(sig.clone())), "not an HDCD rate");
    // codes only after the first 5 s are not looked for
    let mut late = vec![0i32; 2 * 44100 * 6];
    late.extend_from_slice(&sig);
    assert!(!hdcd::detect(44100, reader(late)));
    let mut early = vec![0i32; 2 * 44100 * 4];
    early.extend_from_slice(&sig);
    assert!(hdcd::detect(44100, reader(early)));
    assert!(hdcd::eligible(2, 16) && !hdcd::eligible(1, 16) && !hdcd::eligible(2, 24));
}

// --- libhdcd's test files, against its tests.sh md5 sums -----------------------------

/// What `hdcd-detect -p` writes for a 16-bit input: 2048-frame blocks, the
/// 32-bit output as s24le (its top three bytes).
fn hdcd_detect_pcm(samples: &[i32], rate: u32, mode: AnalyzeMode) -> String {
    let mut h = Hdcd::new();
    assert!(h.reset_ext(rate, 16));
    h.set_analyze_mode(mode);
    let mut buf: Vec<i32> = samples.iter().map(|s| s >> 16).collect();
    for chunk in buf.chunks_mut(2 * 2048) {
        h.process(chunk);
    }
    let mut md5 = Md5::new();
    for s in &buf {
        md5.update(&s.to_le_bytes()[1..4]);
    }
    format!("{:x}", md5.finalize())
}

#[test]
fn libhdcd_test_files_match_tests_sh() {
    let Some(dir) = test_files() else {
        eprintln!("libhdcd's test files not found: skipped");
        return;
    };
    let cases: [(&str, AnalyzeMode, &str); 9] = [
        ("hdcd.wav", AnalyzeMode::Off, "5db465a58d2fd0d06ca944b883b33476"),
        ("hdcd-all.wav", AnalyzeMode::Off, "e8cdf508b7805ed49aaba2f3e12c1bfe"),
        ("hdcd-err.wav", AnalyzeMode::Off, "0f7c3581950e57564d72ad96200bb648"),
        ("hdcd-ftm.wav", AnalyzeMode::Off, "c8c094ad88f43cb9eda1fa2d9b121664"),
        ("hdcd-tgm.wav", AnalyzeMode::Off, "c512b16b00f867105c68ef93fea08fb7"),
        ("hdcd-pfa.wav", AnalyzeMode::Off, "760628f8e3c81e7f7f94fdf594decd61"),
        ("ava16.wav", AnalyzeMode::Off, "b179a5cc8d8a0b8461236807ae55bdd3"),
        ("hdcd-all.wav", AnalyzeMode::Pe, "6c180e3e734f0d670221e99109a9460d"),
        ("hdcd-all.wav", AnalyzeMode::Lle, "a097872ca7f7ffc94b133a84accc928f"),
    ];
    for (file, mode, md5) in cases {
        let (samples, bits, rate) = read_wav(&dir.join(file));
        assert_eq!(bits, 16);
        assert_eq!(hdcd_detect_pcm(&samples, rate, mode), md5, "{file} {mode:?}");
    }
    // ava16 is not HDCD despite a coincidental packet
    let (samples, _, _) = read_wav(&dir.join("ava16.wav"));
    let mut buf: Vec<i32> = samples.iter().map(|s| s >> 16).collect();
    let mut h = Hdcd::new();
    h.process(&mut buf);
    assert_eq!(h.detected(), Detected::None);

    // mkmix.sh: pfa, tgm and ava16 back to back as raw PCM
    let mut mix = Vec::new();
    for f in ["hdcd-pfa.wav", "hdcd-tgm.wav", "ava16.wav"] {
        mix.extend(read_wav(&dir.join(f)).0);
    }
    assert_eq!(hdcd_detect_pcm(&mix, 44100, AnalyzeMode::Off), "6a3cf7f920f419477ada264cc63b40da", "hdcd-mix");
    assert_eq!(hdcd_detect_pcm(&mix, 44100, AnalyzeMode::Cdt), "b10600fc87011c6cc6c048d1a4e7da8d", "hdcd-mix cdt");

    let (samples, _, _) = read_wav(&dir.join("hdcd-all.wav"));
    let mut buf: Vec<i32> = samples.iter().map(|s| s >> 16).collect();
    let mut h = Hdcd::new();
    h.process(&mut buf);
    let d = h.detection();
    assert_eq!(d.detected, Detected::Effectual);
    assert_eq!(d.peak_extend, PeakExtend::Permanent);
    assert!(d.uses_transient_filter);
    assert_eq!(d.max_gain_adjustment, -4.0);
    assert_eq!(
        d.summary(),
        "HDCD detected: yes (B:869), peak_extend: enabled permanently, max_gain_adj: -4.0 dB, transient_filter: detected, detectable errors: 0"
    );
}
