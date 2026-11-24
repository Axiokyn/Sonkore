//! The resampler's frequency response, measured with sines: pass-band
//! ripple, where the pass band ends, and the stop-band attenuation (images
//! when upsampling, aliases when downsampling).

mod common;

use common::{gain_db, measure, resample_all};
use sonkore_dsp::resampler::{Filter, Resampler};

#[test]
fn frequency_response() {
    for (i, o) in
        [(44100u32, 48000u32), (48000, 44100), (44100, 96000), (96000, 44100), (44100, 88200), (192000, 48000)]
    {
        for filter in [Filter::Linear, Filter::LinearSteep] {
            let q = measure(i, o, 0.9, 1.005, &mut |x| {
                resample_all(&mut Resampler::new(i, o, 1, filter).unwrap(), x, 4096)
            });
            println!(
                "{i} -> {o} {filter:?}: ripple {:.2e} dB to 0.9, flat (0.1 dB) to {:.3}, stop band {:.1} dB",
                q.ripple_db, q.flat_to, q.stopband_db
            );
            assert!(q.ripple_db < 0.001, "ripple {}", q.ripple_db);
            assert!(q.stopband_db > 175.0, "stop band {}", q.stopband_db);
            assert!(q.flat_to >= if filter == Filter::LinearSteep { 0.98 } else { 0.94 }, "flat to {}", q.flat_to);
        }
    }
}

#[test]
fn lowpass_response() {
    // DSD→PCM's second stages: DSD64 (flat to 30 kHz, 4:1), DSD128 (50 kHz,
    // stop at 70 kHz, 2:1).
    for (i, o, pass, stop) in
        [(352800u32, 88200u32, 30000.0 / 44100.0, 1.0), (352800, 176400, 50000.0 / 88200.0, 70000.0 / 88200.0)]
    {
        let q = measure(i, o, pass * 0.95, stop + 0.005, &mut |x| {
            resample_all(&mut Resampler::new_lowpass(i, o, 1, pass, stop).unwrap(), x, 4096)
        });
        println!(
            "low-pass {i} -> {o} pass {pass:.3} stop {stop:.3}: ripple {:.2e} dB to 0.95 of the pass band, flat to {:.3}, stop band {:.1} dB",
            q.ripple_db, q.flat_to, q.stopband_db
        );
        let edge = gain_db(i, o, pass * o.min(i) as f64 / 2.0, &mut |x| {
            resample_all(&mut Resampler::new_lowpass(i, o, 1, pass, stop).unwrap(), x, 4096)
        });
        println!("  at the pass-band edge: {edge:.4} dB");
        assert!(q.stopband_db > 175.0, "stop band {}", q.stopband_db);
        assert!(edge.abs() < 0.05, "edge {edge}");
    }
}
