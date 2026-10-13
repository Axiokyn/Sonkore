#pragma once

#include <algorithm>
#include <cmath>
#include <numbers>

namespace sonkore {

enum class FilterType {
    Peaking,
    LowShelf,
    HighShelf,
    LowPass,
    HighPass,
    BandPass,
    Notch,
};

// Normalised (a0 == 1) second-order IIR coefficients, double precision.
struct BiquadCoeffs {
    double b0 = 1.0, b1 = 0.0, b2 = 0.0;
    double a1 = 0.0, a2 = 0.0;
};

// Transposed Direct Form II state. Double precision keeps low-frequency,
// high-Q sections stable and quiet even at 192/384 kHz.
struct BiquadState {
    double z1 = 0.0, z2 = 0.0;

    double process(const BiquadCoeffs& c, double x) noexcept
    {
        const double y = c.b0 * x + z1;
        z1 = c.b1 * x - c.a1 * y + z2;
        z2 = c.b2 * x - c.a2 * y;
        return y;
    }

    void reset() noexcept { z1 = z2 = 0.0; }
};

// RBJ "Audio EQ Cookbook" designs.
inline BiquadCoeffs designBiquad(FilterType type, double sampleRate, double freqHz, double q, double gainDb)
{
    freqHz = std::clamp(freqHz, 1.0, sampleRate * 0.4999);
    q = std::max(q, 1e-4);

    const double A = std::pow(10.0, gainDb / 40.0);
    const double w0 = 2.0 * std::numbers::pi * freqHz / sampleRate;
    const double cw = std::cos(w0);
    const double sw = std::sin(w0);
    const double alpha = sw / (2.0 * q);
    const double sqA2alpha = 2.0 * std::sqrt(A) * alpha;

    double b0 = 1, b1 = 0, b2 = 0, a0 = 1, a1 = 0, a2 = 0;

    switch (type) {
    case FilterType::Peaking:
        b0 = 1.0 + alpha * A;
        b1 = -2.0 * cw;
        b2 = 1.0 - alpha * A;
        a0 = 1.0 + alpha / A;
        a1 = -2.0 * cw;
        a2 = 1.0 - alpha / A;
        break;
    case FilterType::LowShelf:
        b0 = A * ((A + 1.0) - (A - 1.0) * cw + sqA2alpha);
        b1 = 2.0 * A * ((A - 1.0) - (A + 1.0) * cw);
        b2 = A * ((A + 1.0) - (A - 1.0) * cw - sqA2alpha);
        a0 = (A + 1.0) + (A - 1.0) * cw + sqA2alpha;
        a1 = -2.0 * ((A - 1.0) + (A + 1.0) * cw);
        a2 = (A + 1.0) + (A - 1.0) * cw - sqA2alpha;
        break;
    case FilterType::HighShelf:
        b0 = A * ((A + 1.0) + (A - 1.0) * cw + sqA2alpha);
        b1 = -2.0 * A * ((A - 1.0) + (A + 1.0) * cw);
        b2 = A * ((A + 1.0) + (A - 1.0) * cw - sqA2alpha);
        a0 = (A + 1.0) - (A - 1.0) * cw + sqA2alpha;
        a1 = 2.0 * ((A - 1.0) - (A + 1.0) * cw);
        a2 = (A + 1.0) - (A - 1.0) * cw - sqA2alpha;
        break;
    case FilterType::LowPass:
        b0 = (1.0 - cw) * 0.5;
        b1 = 1.0 - cw;
        b2 = (1.0 - cw) * 0.5;
        a0 = 1.0 + alpha;
        a1 = -2.0 * cw;
        a2 = 1.0 - alpha;
        break;
    case FilterType::HighPass:
        b0 = (1.0 + cw) * 0.5;
        b1 = -(1.0 + cw);
        b2 = (1.0 + cw) * 0.5;
        a0 = 1.0 + alpha;
        a1 = -2.0 * cw;
        a2 = 1.0 - alpha;
        break;
    case FilterType::BandPass: // constant 0 dB peak gain
        b0 = alpha;
        b1 = 0.0;
        b2 = -alpha;
        a0 = 1.0 + alpha;
        a1 = -2.0 * cw;
        a2 = 1.0 - alpha;
        break;
    case FilterType::Notch:
        b0 = 1.0;
        b1 = -2.0 * cw;
        b2 = 1.0;
        a0 = 1.0 + alpha;
        a1 = -2.0 * cw;
        a2 = 1.0 - alpha;
        break;
    }

    const double inv = 1.0 / a0;
    return {b0 * inv, b1 * inv, b2 * inv, a1 * inv, a2 * inv};
}

} // namespace sonkore
