//! Behaviour of each stage on its own: what the stages' comments promise.

mod common;

use std::path::PathBuf;
use std::process::Command;

use common::{noise, sine, tmp};
use sonkore_dsp::convolver::{BLOCK, Convolver, ImpulseResponse};
use sonkore_dsp::crossfeed::{Crossfeed, Preset};
use sonkore_dsp::limiter::{Limiter, true_peak};
use sonkore_dsp::test_signals::{self, DsdModulator, TestSignal};

// --- Limiter ----------------------------------------------------------------------

#[test]
fn limiter_passes_quiet_audio_untouched() {
    let mut l = Limiter::new(44100, 2, -1.0);
    let x: Vec<f64> = noise(2 * 20_000, 1).iter().map(|v| v * 0.5).collect();
    let mut out = vec![0.0; x.len()];
    let n = l.process(&x, &mut out);
    assert_eq!(l.lookahead(), 66); // 1.5 ms at 44.1 kHz
    let la = l.delay();
    assert_eq!(la, 66 + 6); // and half the true-peak interpolator
    assert_eq!(n, 20_000 - la);
    assert_eq!(l.latency(), la);
    assert_eq!(&out[..n * 2], &x[..n * 2]); // bit for bit, delayed
    assert_eq!(l.last_reduction_db(), 0.0);
    let mut tail = vec![0.0; la * 2];
    assert_eq!(l.flush(&mut tail), la);
    assert_eq!(&tail[..], &x[n * 2..]);
    assert_eq!(l.latency(), 0);
}

#[test]
fn limiter_keeps_the_true_peak_under_the_ceiling() {
    for (rate, ceiling) in [(44100u32, -1.0), (48000, -3.0), (96000, 0.0)] {
        let mut l = Limiter::new(rate, 2, ceiling);
        // A quarter-rate sine at 45° phase: samples at 0.707 of a 1.4 peak.
        let x: Vec<f64> = (0..2 * rate as usize)
            .map(|i| {
                1.4 * (std::f64::consts::FRAC_PI_2 * (i / 2) as f64 + std::f64::consts::FRAC_PI_4).sin()
                    + noise(1, i as u64)[0] * 0.3
            })
            .collect();
        let mut out = vec![0.0; x.len()];
        let n = l.process(&x, &mut out);
        let mut tail = vec![0.0; l.latency() * 2];
        let t = l.flush(&mut tail);
        let mut y = out[..n * 2].to_vec();
        y.extend_from_slice(&tail[..t * 2]);
        assert_eq!(y.len(), x.len());
        let limit = 10f64.powf(ceiling / 20.0);
        let tp = true_peak(&y[2000..], 2, rate);
        println!(
            "limiter at {rate} Hz, ceiling {ceiling} dBTP: input true peak {:.3}, output {tp:.4} (limit {limit:.4})",
            true_peak(&x, 2, rate)
        );
        // The samples wait for the interpolated peaks (6 frames behind, the
        // interpolator's centre): the ceiling holds to a thousandth of a dB
        // (the recorded reference, gain 6 frames early, to a few hundredths).
        assert!(tp <= limit * 1.0001, "true peak {tp} over {limit}");
    }
}

#[test]
fn limiter_ceiling_is_clamped() {
    let mut l = Limiter::new(48000, 1, -20.0);
    assert_eq!(l.ceiling_db(), -6.0);
    l.set_ceiling_db(3.0);
    assert_eq!(l.ceiling_db(), 0.0);
    // 352.8 kHz and up: the samples are the peak (one phase); latency 529.
    let l = Limiter::new(352800, 2, -1.0);
    assert_eq!(l.lookahead(), 529);
}

// --- Crossfeed --------------------------------------------------------------------

#[test]
fn crossfeed_presets() {
    assert_eq!(Preset::Default.parameters(), (700.0, 4.5));
    assert_eq!(Preset::ChuMoy.parameters(), (700.0, 6.0));
    assert_eq!(Preset::JanMeier.parameters(), (650.0, 9.5));
    assert_eq!(Preset::from_index(3), Preset::JanMeier);
    assert!(!Crossfeed::new(Preset::Off, 44100.0).active());
    // One ear only: the other gets a low-passed copy.
    let mut x = sine(200.0, 44100.0, 44100, 2, 0.5);
    for f in x.chunks_mut(2) {
        f[1] = 0.0;
    }
    Crossfeed::new(Preset::Default, 44100.0).process(&mut x);
    let r: f64 = x.iter().skip(1).step_by(2).skip(22050).map(|v| v.abs()).fold(0.0, f64::max);
    assert!(r > 0.05 && r < 0.4, "{r}");
}

// --- Convolver --------------------------------------------------------------------

fn ir(rate: u32, taps: Vec<Vec<f64>>) -> ImpulseResponse {
    ImpulseResponse { path: PathBuf::new(), sample_rate: rate, channels: taps.len(), taps }
}

#[test]
fn convolver_unit_impulse_is_a_delay() {
    let mut c = Convolver::new(&ir(48000, vec![vec![1.0]]), 48000, 3).unwrap();
    assert_eq!((c.latency(), c.tail(), c.taps()), (BLOCK, BLOCK + 1, 1));
    let x = noise(3 * 5000, 2);
    let mut y = x.clone();
    c.process(&mut y);
    assert!(y[..3 * BLOCK].iter().all(|&v| v == 0.0));
    let worst = y[3 * BLOCK..].iter().zip(&x).map(|(a, b)| (a - b).abs()).fold(0.0, f64::max);
    // FFT rounding (1.05e-15 with Windows' sin and cos for the twiddles).
    assert!(worst < 1e-14, "{worst}");
    c.reset();
    let mut z = x.clone();
    c.process(&mut z);
    assert_eq!(z, y);
}

#[test]
fn convolver_true_stereo_routing() {
    // LL = 1, LR = 0.5 (delayed 3), RL = 0.25, RR = -1.
    let taps =
        vec![vec![1.0, 0.0, 0.0, 0.0], vec![0.0, 0.0, 0.0, 0.5], vec![0.25, 0.0, 0.0, 0.0], vec![-1.0, 0.0, 0.0, 0.0]];
    let mut c = Convolver::new(&ir(44100, taps), 44100, 2).unwrap();
    let x = noise(2 * 4096, 3);
    let mut y = x.clone();
    c.process(&mut y);
    for f in 8..(4096 - BLOCK) {
        let o = f + BLOCK;
        let (l, r) = (x[2 * f], x[2 * f + 1]);
        let want_l = l + 0.25 * r;
        let want_r = 0.5 * x[2 * (f - 3)] - r;
        assert!((y[2 * o] - want_l).abs() < 1e-14 && (y[2 * o + 1] - want_r).abs() < 1e-14);
    }
    assert!(Convolver::new(&ir(44100, vec![vec![1.0]; 4]), 44100, 1).is_err());
    assert!(Convolver::new(&ir(44100, vec![vec![1.0]; 2]), 44100, 6).is_err());
}

#[test]
fn convolver_resampled_ir_keeps_its_gain() {
    // A 4.41 kHz-wide low-pass IR at 44.1 kHz, used at 96 kHz: DC gain kept.
    let taps: Vec<f64> = (0..801)
        .map(|n| {
            let t = n as f64 - 400.0;
            let s =
                if t == 0.0 { 1.0 } else { (std::f64::consts::PI * 0.2 * t).sin() / (std::f64::consts::PI * 0.2 * t) };
            0.2 * s * (0.5 + 0.5 * (std::f64::consts::PI * t / 400.0).cos())
        })
        .collect();
    let dc: f64 = taps.iter().sum();
    let mut c = Convolver::new(&ir(44100, vec![taps]), 96000, 1).unwrap();
    assert_eq!(c.resampled_from(), 44100);
    assert_eq!(c.taps(), (801.0f64 * 96000.0 / 44100.0).round() as usize);
    let mut y = vec![1.0; 20_000];
    c.process(&mut y);
    assert!((y[19_000] - dc).abs() < 1e-6, "{} vs {dc}", y[19_000]);
}

#[test]
fn convolver_refuses_a_response_resampled_past_the_tap_limit() {
    // 500k taps at 44.1 kHz fit; at 96 kHz they are 1.09M, over 2^20.
    let long = ir(44100, vec![vec![1e-6; 500_000]]);
    let err = Convolver::new(&long, 96000, 2).err().expect("a 1.09M-tap filter was accepted");
    assert!(err.contains("taps"), "{err}");
}

#[test]
fn convolver_replace_taps_needs_the_same_partitions() {
    let mut c = Convolver::new(&ir(48000, vec![vec![0.5; 3000]]), 48000, 2).unwrap();
    assert!(!c.replace_taps(&[1.0; 5000]));
    assert!(c.replace_taps(&[0.25; 2049]));
    assert_eq!(c.taps(), 2049);
}

// --- Impulse response files -------------------------------------------------------

#[test]
fn impulse_response_files() {
    let rate = 48000;
    // 24-bit stereo WAV (written with the test-signal writer).
    let samples: Vec<f64> = (0..2000).map(|i| ((i * 37 % 101) as f64 - 50.0) / 64.0).collect();
    let wav = tmp("ir-24.wav");
    test_signals::write_wav(&wav, rate, 2, 24, &samples).unwrap();
    let ir = ImpulseResponse::load(&wav).unwrap();
    assert_eq!((ir.sample_rate, ir.channels, ir.taps[0].len()), (rate, 2, 1000));
    for (i, &s) in samples.iter().enumerate() {
        let want = (s * 8388608.0).round().clamp(-8388608.0, 8388607.0) / 8388608.0;
        assert_eq!(ir.taps[i % 2][i / 2], want);
    }
    // 16-bit, 4 channels (true stereo).
    let wav = tmp("ir-16-4ch.wav");
    test_signals::write_wav(&wav, 44100, 4, 16, &samples).unwrap();
    let ir = ImpulseResponse::load(&wav).unwrap();
    assert_eq!((ir.channels, ir.taps[3].len()), (4, 500));
    assert_eq!(ir.taps[1][0], (samples[1] * 32768.0).round() / 32768.0);
    // 3 channels: refused.
    let wav = tmp("ir-3ch.wav");
    test_signals::write_wav(&wav, 44100, 3, 16, &samples[..1500]).unwrap();
    assert_eq!(ImpulseResponse::load(&wav).unwrap_err(), "An impulse response has 1, 2 or 4 (true stereo) channels");
    // FLAC, when the encoder is installed.
    let wav = tmp("ir-flac-src.wav");
    test_signals::write_wav(&wav, rate, 1, 24, &samples).unwrap();
    let flac = tmp("ir.flac");
    let _ = std::fs::remove_file(&flac);
    if Command::new("flac")
        .arg("-s")
        .arg("-f")
        .arg("-o")
        .arg(&flac)
        .arg(&wav)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        let ir = ImpulseResponse::load(&flac).unwrap();
        assert_eq!(ir.taps[0].len(), 2000);
        assert_eq!(ir.taps[0][7], (samples[7] * 8388608.0).round() / 8388608.0);
    } else {
        eprintln!("no flac encoder: FLAC impulse response not tested");
    }
}

// --- Test signals -----------------------------------------------------------------

/// A first-order sigma-delta, enough to check the DSF framing.
struct Toy {
    channels: usize,
    integrator: Vec<f64>,
    fed: Vec<f64>,
    word: Vec<u32>,
    bits: usize,
}

impl DsdModulator for Toy {
    fn process(&mut self, input: &[f64], out: &mut [i32]) -> usize {
        let ch = self.channels;
        let mut n = 0;
        for frame in input.chunks_exact(ch) {
            for c in 0..ch {
                self.integrator[c] += frame[c] * 0.5 - self.fed[c];
                let bit = self.integrator[c] >= 0.0;
                self.fed[c] = if bit { 1.0 } else { -1.0 };
                self.word[c] = (self.word[c] << 1) | bit as u32;
            }
            self.bits += 1;
            if self.bits == 32 {
                for c in 0..ch {
                    out[n * ch + c] = i32::from_ne_bytes(self.word[c].to_be_bytes());
                }
                n += 1;
                self.bits = 0;
            }
        }
        n
    }

    fn flush(&mut self, out: &mut [i32]) -> usize {
        if self.bits == 0 {
            return 0;
        }
        let pad = 32 - self.bits;
        for c in 0..self.channels {
            let w = (self.word[c] << pad) | (0x6969_6969u32 >> self.bits);
            out[c] = i32::from_ne_bytes(w.to_be_bytes());
        }
        self.bits = 0;
        1
    }
}

#[test]
fn dsf_channel_types_follow_the_spec() {
    // DSF "Channel Type": 1 mono, 2 stereo, 3 three channels (FL FR C),
    // 4 quad, 5 four channels (FL FR C LFE), 6 five channels, 7 5.1.
    let dir = tmp("dsf-channel-types");
    std::fs::create_dir_all(&dir).unwrap();
    for (channels, kind) in [(1u32, 1u32), (2, 2), (3, 3), (4, 4), (5, 6), (6, 7)] {
        let path = dir.join(format!("{channels}.dsf"));
        let ch = channels as usize;
        let mut toy = Toy { channels: ch, integrator: vec![0.0; ch], fed: vec![0.0; ch], word: vec![0; ch], bits: 0 };
        test_signals::write_dsd_tone(&path, 64, channels, 0.01, 1000.0, -20.0, &mut toy).unwrap();
        let b = std::fs::read(&path).unwrap();
        assert_eq!(u32::from_le_bytes(b[48..52].try_into().unwrap()), kind, "{channels} channels: type");
        assert_eq!(u32::from_le_bytes(b[52..56].try_into().unwrap()), channels, "{channels} channels: count");
    }
    // DSF stops at 6 channels.
    let mut toy = Toy { channels: 8, integrator: vec![0.0; 8], fed: vec![0.0; 8], word: vec![0; 8], bits: 0 };
    assert!(test_signals::write_dsd_tone(&dir.join("8.dsf"), 64, 8, 0.01, 1000.0, -20.0, &mut toy).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_signal_files() {
    let dir = tmp("cache-test-signals");
    let _ = std::fs::remove_dir_all(&dir);
    let tone = TestSignal::Tone { rate: 1_000_000, channels: 9 };
    assert_eq!(tone.file_name(), "tone-1khz-768000-8ch.wav");
    assert_eq!(TestSignal::Channels { rate: 44100, channels: 6 }.file_name(), "channels-44100-6ch.wav");
    assert_eq!(TestSignal::Dsd { multiple: 200, channels: 2 }.file_name(), "tone-1khz-dsd128-2ch.dsf");
    assert!(test_signals::cache_dir().ends_with("sonkore/test-signals"));

    let path = TestSignal::Tone { rate: 48000, channels: 2 }.make_in(&dir, None).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(bytes.len(), 68 + 48000 * 10 * 2 * 3);
    // Starts silent (fade-in), reaches -20 dBFS (0.1 · 2^23 ≈ 838861).
    assert_eq!(&bytes[68..74], &[0, 0, 0, 0, 0, 0]);
    let peak = bytes[68..].chunks(3).map(|b| ((i32::from_le_bytes([0, b[0], b[1], b[2]])) >> 8).abs()).max().unwrap();
    assert!((838_800..=838_861).contains(&peak), "{peak}");
    // Kept: not made again.
    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
    TestSignal::Tone { rate: 48000, channels: 2 }.make_in(&dir, None).unwrap();
    assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), modified);

    assert!(TestSignal::Dsd { multiple: 64, channels: 2 }.make_in(&dir, None).is_err());
    let mut toy = Toy { channels: 2, integrator: vec![0.0; 2], fed: vec![0.0; 2], word: vec![0; 2], bits: 0 };
    let dsf = TestSignal::Dsd { multiple: 64, channels: 2 }.make_in(&dir, Some(&mut toy)).unwrap();
    let b = std::fs::read(dsf).unwrap();
    assert_eq!(&b[0..4], b"DSD ");
    assert_eq!(&b[28..32], b"fmt ");
    assert_eq!(u32::from_le_bytes(b[56..60].try_into().unwrap()), 2_822_400);
    let samples = u64::from_le_bytes(b[64..72].try_into().unwrap());
    assert_eq!(samples, 2_822_400 * 5);
    assert_eq!(&b[80..84], b"data");
    assert_eq!(u64::from_le_bytes(b[12..20].try_into().unwrap()) as usize, b.len());
    // The toy modulator's ones density follows the tone: ~50 % overall.
    let ones: u32 = b[92..92 + 4096].iter().map(|v| v.count_ones()).sum();
    assert!((14_000..18_800).contains(&ones), "{ones}");

    assert_eq!(test_signals::channel_name(6, 3), "LFE (subwoofer)");
    assert_eq!(test_signals::channel_name(2, 1), "Right");
    assert_eq!(test_signals::channel_name(1, 0), "Mono");
    assert_eq!(test_signals::channel_name(4, 2), "Back left");
}
