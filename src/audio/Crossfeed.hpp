#pragma once

#include <cmath>
#include <cstddef>
#include <numbers>

namespace sonkore {

// Headphone crossfeed after Benjamin Bauer's stereophonic-to-binaural design
// (the one bs2b implements): each ear also gets the other channel, low-passed
// and lowered, as it would from loudspeakers; the direct signal gets a high
// shelf so the overall balance stays flat. Centred (mono) sound keeps its
// level in the bass and comes out up to ~1.75 dB lower in the treble (Default
// preset; less with the others), as with libbs2b.
// Double precision, stereo only. Worker-thread only.
// Coefficients and presets as in bs2b, (c) 2005 Boris Mikhaylov, MIT license
// (see THIRD-PARTY.md); output identical to libbs2b's bs2b_cross_feed_d().
class Crossfeed {
public:
    enum class Preset { Off, Default, ChuMoy, JanMeier };

    // Cut-off (Hz) and how much lower the crossfed signal is (dB).
    static void parameters(Preset preset, double& cutoffHz, double& feedDb)
    {
        switch (preset) {
        case Preset::ChuMoy: cutoffHz = 700.0; feedDb = 6.0; break;
        case Preset::JanMeier: cutoffHz = 650.0; feedDb = 9.5; break;
        default: cutoffHz = 700.0; feedDb = 4.5; break;
        }
    }

    void setup(Preset preset, double sampleRate)
    {
        m_preset = preset;
        if (preset == Preset::Off || sampleRate <= 0.0)
            return;
        double fcLo = 0.0, feed = 0.0;
        parameters(preset, fcLo, feed);
        const double gbLo = feed * -5.0 / 6.0 - 3.0; // dB
        const double gbHi = feed / 6.0 - 3.0;
        const double gLo = std::pow(10.0, gbLo / 20.0);
        const double gHi = 1.0 - std::pow(10.0, gbHi / 20.0);
        const double fcHi = fcLo * std::pow(2.0, (gbLo - 20.0 * std::log10(gHi)) / 12.0);

        double x = std::exp(-2.0 * std::numbers::pi * fcLo / sampleRate);
        m_b1Lo = x;
        m_a0Lo = gLo * (1.0 - x);
        x = std::exp(-2.0 * std::numbers::pi * fcHi / sampleRate);
        m_b1Hi = x;
        m_a0Hi = 1.0 - gHi * (1.0 - x);
        m_a1Hi = -x;
        m_gain = 1.0 / (1.0 - gHi + gLo);
        reset();
    }
    Preset preset() const { return m_preset; }
    bool active() const { return m_preset != Preset::Off; }
    void reset() { m_lo[0] = m_lo[1] = m_hi[0] = m_hi[1] = m_in[0] = m_in[1] = 0.0; }

    // Interleaved stereo, in place.
    void process(double* samples, std::size_t frames)
    {
        for (std::size_t f = 0; f < frames; ++f) {
            double* s = samples + 2 * f;
            for (int c = 0; c < 2; ++c) {
                m_lo[c] = m_a0Lo * s[c] + m_b1Lo * m_lo[c];
                m_hi[c] = m_a0Hi * s[c] + m_a1Hi * m_in[c] + m_b1Hi * m_hi[c];
                m_in[c] = s[c];
            }
            s[0] = (m_hi[0] + m_lo[1]) * m_gain;
            s[1] = (m_hi[1] + m_lo[0]) * m_gain;
        }
    }

private:
    Preset m_preset = Preset::Off;
    double m_a0Lo = 0.0, m_b1Lo = 0.0;
    double m_a0Hi = 1.0, m_a1Hi = 0.0, m_b1Hi = 0.0;
    double m_gain = 1.0;
    double m_lo[2] = {}, m_hi[2] = {}, m_in[2] = {};
};

} // namespace sonkore
