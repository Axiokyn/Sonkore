//! The minimum- and intermediate-phase filters: the linear filter's
//! magnitude, no (or less) pre-ringing, and the linear filter's timing rules
//! (exact output length, DC gain 1, a click on time).

mod common;

use common::{gain_db, measure, noise, resample_all};
use sonkore_dsp::resampler::{Filter, Resampler};

const PHASES: [Filter; 2] = [Filter::Minimum, Filter::Intermediate];

/// A click at input frame `at`, converted: the output and where its peak is.
fn click(r: &mut Resampler, frames: usize, at: usize) -> (Vec<f64>, usize) {
    let mut x = vec![0.0; frames];
    x[at] = 1.0;
    r.clear();
    let y = resample_all(r, &x, 1000);
    let peak = y.iter().enumerate().fold(0, |p, (i, v)| if v.abs() > y[p].abs() { i } else { p });
    (y, peak)
}

/// Energy of a click's response more than `lower` lower-rate samples before
/// its peak, as a fraction of the whole.
fn pre_ringing(i: u32, o: u32, filter: Filter, lower: f64) -> f64 {
    let mut r = Resampler::new(i, o, 1, filter).unwrap();
    let (y, peak) = click(&mut r, 20_000, 8000);
    let before = peak.saturating_sub((lower * o as f64 / i.min(o) as f64).round() as usize);
    let e = |s: &[f64]| s.iter().map(|v| v * v).sum::<f64>();
    e(&y[..before]) / e(&y)
}

/// The linear filter's ripple, pass band and stop band, and its gain where
/// both are flat. (A few cases, one test each: a sweep takes seconds in a dev
/// build; the kernels' whole spectra are checked in polyphase.rs.)
fn same_magnitude_as_linear(i: u32, o: u32, filter: Filter) {
    let mut linear = Resampler::new(i, o, 1, Filter::Linear).unwrap();
    let low = i.min(o) as f64 / 2.0;
    let mut r = Resampler::new(i, o, 1, filter).unwrap();
    let mut convert = |x: &[f64]| {
        r.clear();
        resample_all(&mut r, x, 4096)
    };
    let q = measure(i, o, 0.9, 1.005, &mut convert);
    // Against the linear filter, where both are flat (their edges
    // differ: rubato's Blackman-Harris² vs this Kaiser prototype).
    let mut worst: f64 = 0.0;
    for f in [0.1, 0.5, 0.8, 0.9] {
        let ours = gain_db(i, o, f * low, &mut convert);
        let theirs = gain_db(i, o, f * low, &mut |x| {
            linear.clear();
            resample_all(&mut linear, x, 4096)
        });
        worst = worst.max((ours - theirs).abs());
    }
    println!(
        "{i} -> {o} {filter:?}: ripple {:.2e} dB to 0.9, flat (0.1 dB) to {:.3}, stop band {:.1} dB, \
         off the linear one by {worst:.1e} dB",
        q.ripple_db, q.flat_to, q.stopband_db
    );
    assert!(q.ripple_db < 0.001, "ripple {}", q.ripple_db);
    assert!(q.stopband_db > 175.0, "stop band {}", q.stopband_db);
    assert!(q.flat_to >= 0.94, "flat to {}", q.flat_to);
    assert!(worst < 0.001, "{worst} dB from the linear filter");
}

#[test]
fn same_magnitude_up_minimum() {
    same_magnitude_as_linear(44100, 48000, Filter::Minimum);
}

#[test]
fn same_magnitude_up_intermediate() {
    same_magnitude_as_linear(44100, 48000, Filter::Intermediate);
}

#[test]
fn same_magnitude_down_minimum() {
    same_magnitude_as_linear(96000, 44100, Filter::Minimum);
}

#[test]
fn same_magnitude_up4_intermediate() {
    same_magnitude_as_linear(44100, 192000, Filter::Intermediate);
}

#[test]
fn minimum_phase_has_no_pre_ringing() {
    for (i, o) in
        [(44100u32, 48000u32), (48000, 44100), (44100, 96000), (96000, 44100), (44100, 192000), (44100, 2822400)]
    {
        // Past the main lobe: 8 samples of the lower rate before the peak.
        let linear = pre_ringing(i, o, Filter::Linear, 8.0);
        let intermediate = pre_ringing(i, o, Filter::Intermediate, 8.0);
        let minimum = pre_ringing(i, o, Filter::Minimum, 8.0);
        println!(
            "{i} -> {o}: energy more than 8 samples before the peak: linear {linear:.2e}, \
             intermediate {intermediate:.2e}, minimum {minimum:.2e}"
        );
        assert!(minimum < 1e-15, "minimum {minimum}");
        assert!(intermediate < linear / 20.0 && intermediate > 1e-9, "intermediate {intermediate}");
    }
}

#[test]
fn click_peaks_on_time() {
    // The peak is the alignment point: a click at input frame 8000 peaks on
    // the output frame of that time.
    for (i, o) in
        [(44100u32, 48000u32), (48000, 44100), (44100, 88200), (192000, 44100), (11025, 768000), (44100, 2822400)]
    {
        for filter in PHASES {
            let mut r = Resampler::new(i, o, 1, filter).unwrap();
            let (_, peak) = click(&mut r, 20_000, 8000);
            let want = 8000.0 * o as f64 / i as f64;
            println!("{i} -> {o} {filter:?}: peak at {peak}, the click's time is {want:.2}");
            assert!((peak as f64 - want).abs() <= 1.0, "{i} -> {o} {filter:?}: peak at {peak}, want {want}");
        }
    }
}

#[test]
fn exact_output_length_and_delay() {
    for (i, o) in
        [(44100u32, 48000u32), (48000, 44100), (44100, 705600), (192000, 48000), (32000, 44100), (44100, 2822400)]
    {
        for filter in PHASES {
            let mut r = Resampler::new(i, o, 1, filter).unwrap();
            for n in [0usize, 1, 7, 1000, 12345, 100_000] {
                r.clear();
                let x = noise(n, 3);
                let mut out = Vec::new();
                let got = r.process(&x, &mut out);
                let expect_delay = n as f64 * o as f64 / i as f64 - got as f64;
                assert!((r.delay_frames() - expect_delay).abs() < 1e-6);
                let mut total = got;
                loop {
                    let d = r.drain(&mut out);
                    if d == 0 {
                        break;
                    }
                    total += d;
                }
                let want = ((n as u128 * o as u128 * 2 + i as u128) / (2 * i as u128)) as usize;
                assert_eq!(total, want, "{i} -> {o} {filter:?}, {n} frames");
                assert_eq!(r.delay_frames(), 0.0);
            }
        }
    }
}

#[test]
fn dc_gain_is_one() {
    for (i, o) in [(44100u32, 48000u32), (48000, 44100), (44100, 192000), (11025, 768000), (48000, 6144000)] {
        for filter in PHASES {
            let mut r = Resampler::new(i, o, 2, filter).unwrap();
            let y = resample_all(&mut r, &vec![0.25; 2 * 20_000], 999);
            let frames = y.len() / 2;
            let worst = y[frames / 4 * 2..frames * 3 / 4 * 2].iter().fold(0.0f64, |w, v| w.max((v - 0.25).abs()));
            println!("{i} -> {o} {filter:?}: DC off by {worst:.1e}");
            assert!(worst < 1e-12, "{i} -> {o} {filter:?}: {worst}");
        }
    }
}

#[test]
fn blocks_do_not_matter() {
    let x = noise(3 * 30_000, 11);
    for (i, o) in [(44100u32, 48000u32), (96000, 44100), (44100, 2822400)] {
        for filter in PHASES {
            let mut r = Resampler::new(i, o, 3, filter).unwrap();
            let reference = resample_all(&mut r, &x, 30_000);
            for block in [1usize, 17, 1024, 4099] {
                r.clear();
                assert_eq!(resample_all(&mut r, &x, block), reference, "block {block}");
            }
        }
    }
}

#[test]
fn reserved_output_never_grows() {
    for (filter, o) in [(Filter::Minimum, 192000), (Filter::Intermediate, 192000), (Filter::Minimum, 2822400)] {
        let mut r = Resampler::new(44100, o, 8, filter).unwrap();
        for block in [1usize, 1024, 5000] {
            let mut out = Vec::with_capacity(r.max_output_frames(block) * 8);
            let cap = out.capacity();
            let x = noise(block * 8, 5);
            for _ in 0..20 {
                r.process(&x, &mut out);
                assert_eq!(out.capacity(), cap);
            }
            r.drain(&mut out);
            assert_eq!(out.capacity(), cap);
            r.clear();
        }
    }
}

/// A sine through the converter: its gain (dB) and what is left once the
/// sine is fitted out (dB re the sine: images, aliases, errors).
fn sine_through(r: &mut Resampler, hz: f64) -> (f64, f64) {
    let (i, o) = (r.in_rate() as f64, r.out_rate() as f64);
    let x = common::sine(hz, i, i as usize, 1, 0.5);
    r.clear();
    let y = resample_all(r, &x, 4096);
    let part = &y[y.len() / 4..y.len() * 3 / 4];
    let at = y.len() / 4;
    // Least squares a·sin + b·cos.
    let (mut ss, mut sc, mut cc, mut ys, mut yc) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (n, v) in part.iter().enumerate() {
        let w = 2.0 * std::f64::consts::PI * hz * (at + n) as f64 / o;
        let (s, c) = w.sin_cos();
        ss += s * s;
        sc += s * c;
        cc += c * c;
        ys += v * s;
        yc += v * c;
    }
    let det = ss * cc - sc * sc;
    let (a, b) = ((ys * cc - yc * sc) / det, (yc * ss - ys * sc) / det);
    let mut rest = 0.0;
    for (n, v) in part.iter().enumerate() {
        let w = 2.0 * std::f64::consts::PI * hz * (at + n) as f64 / o;
        rest += (v - a * w.sin() - b * w.cos()).powi(2);
    }
    let amp = (a * a + b * b).sqrt();
    let rms = (rest / part.len() as f64).sqrt();
    (20.0 * (amp / 0.5).log10(), 20.0 * (rms * 2f64.sqrt() / amp).log10())
}

#[test]
fn rates_of_no_family_and_dsd() {
    // 44100 -> 47999 Hz: too many phases (47999) for an exact table, so the
    // rows are interpolated; past 16× (11025 -> 768 kHz, 44.1 kHz -> DSD64's
    // rate) two stages. Against an exact table in one stage (44.1 -> 48
    // kHz): the same gain, and nothing else above -170 dB.
    for (i, o) in [(44100u32, 48000u32), (44100, 47999), (11025, 768000), (44100, 2822400)] {
        for filter in PHASES {
            let mut r = Resampler::new(i, o, 1, filter).unwrap();
            for f in [0.1, 0.5, 0.9] {
                let hz = f * i as f64 / 2.0;
                let (gain, rest) = sine_through(&mut r, hz);
                println!("{i} -> {o} {filter:?}, {hz:.0} Hz: gain {gain:.1e} dB, the rest {rest:.1} dB");
                assert!(gain.abs() < 1e-5 && rest < -170.0, "{i} -> {o} {filter:?} {hz} Hz: {gain} {rest}");
            }
        }
    }
}
