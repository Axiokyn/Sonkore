//! Headphone crossfeed after Benjamin Bauer's
//! stereophonic-to-binaural design, the one bs2b implements: each ear also
//! gets the other channel, low-passed and lowered, as it would from
//! loudspeakers; the direct signal gets a high shelf so the overall balance
//! stays flat. Centred (mono) sound keeps its level in the bass and comes out
//! up to ~1.75 dB lower in the treble (Default preset; less with the others),
//! as with libbs2b.
//!
//! Double precision, stereo only, worker thread only. Coefficients and
//! presets as in bs2b, (c) 2005 Boris Mikhaylov, MIT license; the output is
//! identical to libbs2b's `bs2b_cross_feed_d()`.

/// The presets, in the settings' order (`playback/crossfeed` 0–3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Preset {
    #[default]
    Off,
    /// 700 Hz, 4.5 dB.
    Default,
    /// 700 Hz, 6 dB.
    ChuMoy,
    /// 650 Hz, 9.5 dB.
    JanMeier,
}

impl Preset {
    /// From the settings' index (0 off, 1 default, 2 Chu Moy, 3 Jan Meier).
    pub fn from_index(i: i64) -> Self {
        match i {
            1 => Preset::Default,
            2 => Preset::ChuMoy,
            3 => Preset::JanMeier,
            _ => Preset::Off,
        }
    }

    /// The cut-off (Hz) and how much lower the crossfed signal is (dB).
    pub fn parameters(self) -> (f64, f64) {
        match self {
            Preset::ChuMoy => (700.0, 6.0),
            Preset::JanMeier => (650.0, 9.5),
            _ => (700.0, 4.5),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Crossfeed {
    preset: Preset,
    a0_lo: f64,
    b1_lo: f64,
    a0_hi: f64,
    a1_hi: f64,
    b1_hi: f64,
    gain: f64,
    lo: [f64; 2],
    hi: [f64; 2],
    last_in: [f64; 2],
}

impl Default for Crossfeed {
    fn default() -> Self {
        Crossfeed {
            preset: Preset::Off,
            a0_lo: 0.0,
            b1_lo: 0.0,
            a0_hi: 1.0,
            a1_hi: 0.0,
            b1_hi: 0.0,
            gain: 1.0,
            lo: [0.0; 2],
            hi: [0.0; 2],
            last_in: [0.0; 2],
        }
    }
}

impl Crossfeed {
    pub fn new(preset: Preset, sample_rate: f64) -> Self {
        let mut c = Crossfeed::default();
        c.setup(preset, sample_rate);
        c
    }

    /// The filters for a preset at a rate (bs2b's `init`). `Off` (or no
    /// rate) only records the preset.
    pub fn setup(&mut self, preset: Preset, sample_rate: f64) {
        self.preset = preset;
        if preset == Preset::Off || sample_rate <= 0.0 {
            return;
        }
        let (fc_lo, feed) = preset.parameters();
        let gb_lo = feed * -5.0 / 6.0 - 3.0; // dB
        let gb_hi = feed / 6.0 - 3.0;
        let g_lo = 10f64.powf(gb_lo / 20.0);
        let g_hi = 1.0 - 10f64.powf(gb_hi / 20.0);
        let fc_hi = fc_lo * 2f64.powf((gb_lo - 20.0 * g_hi.log10()) / 12.0);

        let pi = std::f64::consts::PI;
        let x = (-2.0 * pi * fc_lo / sample_rate).exp();
        self.b1_lo = x;
        self.a0_lo = g_lo * (1.0 - x);
        let x = (-2.0 * pi * fc_hi / sample_rate).exp();
        self.b1_hi = x;
        self.a0_hi = 1.0 - g_hi * (1.0 - x);
        self.a1_hi = -x;
        self.gain = 1.0 / (1.0 - g_hi + g_lo);
        self.reset();
    }

    pub fn preset(&self) -> Preset {
        self.preset
    }

    pub fn active(&self) -> bool {
        self.preset != Preset::Off
    }

    pub fn reset(&mut self) {
        self.lo = [0.0; 2];
        self.hi = [0.0; 2];
        self.last_in = [0.0; 2];
    }

    /// Interleaved stereo, in place. (The caller skips it when off or not
    /// stereo, as the engine does.)
    pub fn process(&mut self, samples: &mut [f64]) {
        for s in samples.as_chunks_mut::<2>().0 {
            for (c, &x) in s.iter().enumerate() {
                self.lo[c] = self.a0_lo * x + self.b1_lo * self.lo[c];
                self.hi[c] = self.a0_hi * x + self.a1_hi * self.last_in[c] + self.b1_hi * self.hi[c];
                self.last_in[c] = x;
            }
            s[0] = (self.hi[0] + self.lo[1]) * self.gain;
            s[1] = (self.hi[1] + self.lo[0]) * self.gain;
        }
    }
}
