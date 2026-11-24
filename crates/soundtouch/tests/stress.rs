//! Random settings, block sizes and calls: nothing may panic (overflow
//! checks included in debug builds) and the output stays finite.

use sonkore_soundtouch::{Interpolation, SoundTouch, TimeStretch};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

fn change_something(rng: &mut Rng, ts: &TimeStretch) {
    let c = ts.control();
    match rng.below(10) {
        0 => c.set_pitch_semitones(rng.unit() * 120.0 - 60.0),
        1 => c.set_tempo(rng.unit() * 25.0),
        2 => c.set_rate(rng.unit() * 25.0),
        3 => c.set_use_aa_filter(rng.below(2) == 0),
        4 => c.set_aa_filter_length(rng.below(200) as i32 - 20),
        5 => c.set_use_quick_seek(rng.below(2) == 0),
        6 => c.set_sequence_ms(rng.below(700) as i32 - 100),
        7 => c.set_seek_window_ms(rng.below(400) as i32 - 100),
        8 => c.set_overlap_ms(rng.below(150) as i32 - 20),
        _ => c.set_enabled(rng.below(4) != 0),
    }
}

#[test]
fn random_use_of_the_stage() {
    let mut rng = Rng(0x2545F4914F6CDD1D);
    let rates = [8000, 11025, 22050, 44100, 48000, 96000, 192000, 384000];
    for round in 0..24 {
        let sr = rates[rng.below(rates.len() as u64) as usize];
        let ch = 1 + rng.below(8) as usize;
        let mut ts = TimeStretch::new(sr, ch);
        ts.control().set_enabled(true);
        for _ in 0..4 {
            change_something(&mut rng, &ts);
        }
        let mut out = vec![0.0f32; (1 + rng.below(5000) as usize) * ch];
        let mut produced = 0usize;
        for _ in 0..60 {
            if rng.below(8) == 0 {
                change_something(&mut rng, &ts);
            }
            ts.apply_pending_changes();
            if ts.is_neutral() {
                ts.release();
                continue;
            }
            match rng.below(40) {
                0 => ts.clear(),
                1 => ts.flush(),
                _ => {}
            }
            // Any length, partial frames and empty blocks included.
            let len = rng.below(3000) as usize;
            let block: Vec<f32> = (0..len).map(|_| (rng.unit() * 2.0 - 1.0) as f32).collect();
            ts.put_samples(&block);
            loop {
                let got = ts.receive_samples(&mut out);
                assert!(out[..got * ch].iter().all(|v| v.is_finite()), "round {round}");
                produced += got;
                if got == 0 || produced > 4_000_000 {
                    break;
                }
            }
            assert!(ts.held_source_frames() >= 0.0);
        }
    }
}

#[test]
fn raw_processor_edge_cases() {
    assert!(SoundTouch::new(192_001, 2).is_err());
    assert!(SoundTouch::new(44100, 0).is_err());
    assert!(SoundTouch::new(44100, 17).is_err());
    for algo in [Interpolation::Linear, Interpolation::Cubic, Interpolation::Shannon] {
        for ch in [1, 2, 3, 8, 16] {
            let mut st = SoundTouch::with_interpolation(0, ch, algo).unwrap();
            assert_eq!(st.sample_rate(), 44100);
            assert!(!st.set_aa_filter_length(12));
            assert!(!st.set_aa_filter_length(136));
            assert!(st.set_aa_filter_length(8));
            st.set_pitch_semitones(5.0);
            st.set_tempo(0.9);
            let block = vec![0.25f32; 4096 * ch];
            let mut out = vec![0.0f32; 1000 * ch];
            let mut total = 0;
            for _ in 0..8 {
                st.put_samples(&block);
                st.put_samples(&[]);
                while let n @ 1.. = st.receive_samples(&mut out) {
                    total += n;
                }
            }
            st.flush();
            while let n @ 1.. = st.receive_samples(&mut out) {
                total += n;
            }
            // Flush trims the output to what the input implies.
            let expected = (8.0 * 4096.0 / (st.effective_rate() * st.effective_tempo()) + 0.5) as usize;
            assert!(total.abs_diff(expected) <= 1, "{algo:?} {ch}ch: {total} vs {expected}");
            assert!(st.initial_latency() > 0);
            assert!(st.nominal_input_sequence() > 0 && st.nominal_output_sequence() > 0);
        }
    }
}
