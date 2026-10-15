#pragma once

#include <algorithm>
#include <cmath>
#include <complex>
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
inline BiquadCoeffs designRbj(FilterType type, double sampleRate, double freqHz, double q, double gainDb)
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

namespace biquad_detail {

// The analog prototype of an RBJ peaking / shelving filter, s normalised to
// the corner frequency: (n2 s^2 + n1 s + n0) / (d2 s^2 + d1 s + d0).
struct Analog {
    double n2, n1, n0, d2, d1, d0;
    double magnitude2(double x) const // |H(jx)|^2
    {
        const double nr = n0 - n2 * x * x, ni = n1 * x, dr = d0 - d2 * x * x, di = d1 * x;
        return (nr * nr + ni * ni) / (dr * dr + di * di);
    }
};

inline Analog analog(FilterType type, double A, double q)
{
    const double sA = std::sqrt(A);
    switch (type) {
    case FilterType::LowShelf:
        return {A, sA * A / q, A * A, A, sA / q, 1.0};
    case FilterType::HighShelf:
        return {A * A, sA * A / q, A, 1.0, sA / q, A};
    default: // Peaking
        return {1.0, A / q, 1.0, 1.0, 1.0 / (A * q), 1.0};
    }
}

inline double digitalMagnitude2(const BiquadCoeffs& c, double w)
{
    const std::complex<double> z1 = std::polar(1.0, -w), z2 = z1 * z1;
    return std::norm((c.b0 + c.b1 * z1 + c.b2 * z2) / (1.0 + c.a1 * z1 + c.a2 * z2));
}

// Vicanek's matched design ("Matched Second Order Digital Filters", 2016):
// poles from the prototype's by impulse invariance, zeros chosen so the
// magnitude equals the prototype's at DC, at the corner and at Nyquist.
// Unlike the bilinear transform it does not squeeze the response toward
// Nyquist (a 16 kHz +6 dB band at 44.1 kHz stays a 16 kHz +6 dB band).
// Cuts are boosts inverted (the prototypes are exact reciprocals), which
// keeps the poles where the matching works.
inline bool designMatched(FilterType type, double sampleRate, double freqHz, double q, double gainDb, BiquadCoeffs& out)
{
    const bool cut = gainDb < 0.0;
    const double A = std::pow(10.0, std::fabs(gainDb) / 40.0);
    const Analog h = analog(type, A, q);
    const double w0 = 2.0 * std::numbers::pi * freqHz / sampleRate;
    const std::complex<double> disc = std::sqrt(std::complex<double>(h.d1 * h.d1 - 4.0 * h.d2 * h.d0));
    const std::complex<double> p1 = std::exp((-h.d1 + disc) / (2.0 * h.d2) * w0);
    const std::complex<double> p2 = std::exp((-h.d1 - disc) / (2.0 * h.d2) * w0);
    const double a1 = -(p1 + p2).real(), a2 = (p1 * p2).real();

    const double A0 = (1.0 + a1 + a2) * (1.0 + a1 + a2), A1 = (1.0 - a1 + a2) * (1.0 - a1 + a2), A2 = -4.0 * a2;
    const double s2 = std::sin(w0 / 2.0) * std::sin(w0 / 2.0);
    const double phi0 = 1.0 - s2, phi1 = s2, phi2 = 4.0 * phi0 * phi1;
    if (phi2 < 1e-12)
        return false;
    const double B0 = A0 * h.magnitude2(0.0);
    const double B1 = A1 * h.magnitude2(std::numbers::pi / w0);
    const double B2 = (h.magnitude2(1.0) * (A0 * phi0 + A1 * phi1 + A2 * phi2) - B0 * phi0 - B1 * phi1) / phi2;
    const double sB0 = std::sqrt(B0), sB1 = std::sqrt(B1);
    const double W = 0.5 * (sB0 + sB1);
    const double b0 = 0.5 * (W + std::sqrt(std::max(W * W + B2, 0.0)));
    if (!(b0 > 0.0) || !std::isfinite(b0))
        return false;
    const double b1 = 0.5 * (sB0 - sB1), b2 = -B2 / (4.0 * b0);
    if (!cut) {
        out = {b0, b1, b2, a1, a2};
    } else {
        const double inv = 1.0 / b0;
        out = {inv, a1 * inv, a2 * inv, b1 * inv, b2 * inv};
    }
    // Finite and stable (poles inside the unit circle), else the caller
    // keeps the RBJ design.
    return std::isfinite(out.b1) && std::isfinite(out.b2) && std::isfinite(out.a1) && std::isfinite(out.a2)
        && std::fabs(out.a2) < 1.0 && std::fabs(out.a1) < 1.0 + out.a2;
}

// Largest deviation (dB) from the prototype over the audible band.
inline double prototypeError(const BiquadCoeffs& c, FilterType type, double sampleRate, double freqHz, double q, double gainDb)
{
    const Analog h = analog(type, std::pow(10.0, gainDb / 40.0), q);
    const double top = std::min(20000.0, sampleRate * 0.45);
    double worst = 0.0;
    constexpr int kPoints = 48;
    for (int i = 0; i < kPoints; ++i) {
        const double f = 20.0 * std::pow(top / 20.0, i / double(kPoints - 1));
        const double want = h.magnitude2(f / freqHz), got = digitalMagnitude2(c, 2.0 * std::numbers::pi * f / sampleRate);
        worst = std::max(worst, std::fabs(10.0 * std::log10(got / want)));
    }
    return worst;
}

} // namespace biquad_detail

// Peaking and shelving bands: the matched design, or the RBJ one where that
// lands closer to the analog prototype (a high shelf whose corner sits near
// Nyquist); the other types are RBJ's.
inline BiquadCoeffs designBiquad(FilterType type, double sampleRate, double freqHz, double q, double gainDb)
{
    const BiquadCoeffs rbj = designRbj(type, sampleRate, freqHz, q, gainDb);
    if ((type != FilterType::Peaking && type != FilterType::LowShelf && type != FilterType::HighShelf) || gainDb == 0.0)
        return rbj;
    freqHz = std::clamp(freqHz, 1.0, sampleRate * 0.4999);
    q = std::max(q, 1e-4);
    BiquadCoeffs matched;
    if (!biquad_detail::designMatched(type, sampleRate, freqHz, q, gainDb, matched))
        return rbj;
    using biquad_detail::prototypeError;
    return prototypeError(matched, type, sampleRate, freqHz, q, gainDb) <= prototypeError(rbj, type, sampleRate, freqHz, q, gainDb)
        ? matched : rbj;
}

} // namespace sonkore
