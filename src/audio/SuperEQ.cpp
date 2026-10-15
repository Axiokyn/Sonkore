#include "SuperEQ.hpp"

#include <algorithm>
#include <cmath>
#include <complex>
#include <numbers>

namespace sonkore {

SuperEQ::SuperEQ()
    : m_pendingBands(graphic10Band())
{
}

namespace {
constexpr double kGraphicCenters[] = {31.25, 62.5, 125, 250, 500, 1000, 2000, 4000, 8000, 16000};
constexpr double kGraphicQ = 1.41421356237; // ~1 octave bandwidth
constexpr double kGraphicRate = 48000.0;    // the matched designs barely depend on it
} // namespace

std::vector<SuperEQ::Band> SuperEQ::graphic10Band()
{
    std::vector<Band> bands;
    for (double f : kGraphicCenters) {
        Band b;
        b.type = FilterType::Peaking;
        b.freqHz = f;
        b.q = kGraphicQ;
        bands.push_back(b);
    }
    return bands;
}

std::array<double, 10> SuperEQ::graphicResponse(const std::array<double, 10>& gains)
{
    std::vector<BiquadCoeffs> sections;
    for (int j = 0; j < 10; ++j)
        if (gains[j] != 0.0)
            sections.push_back(designBiquad(FilterType::Peaking, kGraphicRate, kGraphicCenters[j], kGraphicQ, gains[j]));
    std::array<double, 10> r{};
    for (int i = 0; i < 10; ++i) {
        const std::complex<double> z1 = std::polar(1.0, -2.0 * std::numbers::pi * kGraphicCenters[i] / kGraphicRate), z2 = z1 * z1;
        for (const BiquadCoeffs& c : sections)
            r[i] += 20.0 * std::log10(std::abs((c.b0 + c.b1 * z1 + c.b2 * z2) / (1.0 + c.a1 * z1 + c.a2 * z2)));
    }
    return r;
}

std::array<double, 10> SuperEQ::graphicGains(const std::array<double, 10>& sliders)
{
    if (std::all_of(sliders.begin(), sliders.end(), [](double g) { return g == 0.0; }))
        return sliders;
    // The overlap, inverted once: column j is band j's response per dB of
    // its gain at the ten centres.
    static const auto inverse = [] {
        std::array<std::array<double, 20>, 10> m{};
        for (int j = 0; j < 10; ++j) {
            std::array<double, 10> g{};
            g[j] = 1.0;
            const auto r = graphicResponse(g);
            for (int i = 0; i < 10; ++i)
                m[i][j] = r[i];
            m[j][10 + j] = 1.0;
        }
        for (int c = 0; c < 10; ++c) { // Gauss-Jordan; diagonally dominant, no pivoting needed
            const double p = m[c][c];
            for (double& v : m[c])
                v /= p;
            for (int r = 0; r < 10; ++r)
                if (r != c) {
                    const double f = m[r][c];
                    for (int k = 0; k < 20; ++k)
                        m[r][k] -= f * m[c][k];
                }
        }
        std::array<std::array<double, 10>, 10> inv{};
        for (int i = 0; i < 10; ++i)
            for (int j = 0; j < 10; ++j)
                inv[i][j] = m[i][10 + j];
        return inv;
    }();
    // Newton steps (the overlap is nearly linear in dB; a few land within
    // 0.01 dB even at +-12 dB).
    std::array<double, 10> gains = sliders;
    for (int step = 0; step < 5; ++step) {
        const auto got = graphicResponse(gains);
        std::array<double, 10> next = gains;
        for (int i = 0; i < 10; ++i) {
            double d = 0.0;
            for (int j = 0; j < 10; ++j)
                d += inverse[i][j] * (sliders[j] - got[j]);
            next[i] = std::clamp(gains[i] + d, -36.0, 36.0);
        }
        gains = next;
    }
    return gains;
}

void SuperEQ::setEnabled(bool enabled)
{
    m_enabled.store(enabled, std::memory_order_relaxed);
    m_generation.fetch_add(1, std::memory_order_release);
}

SuperEQ::Curve SuperEQ::curve() const
{
    std::lock_guard lock(m_mutex);
    return {m_pendingBands, m_pendingPreampDb};
}

void SuperEQ::setBands(std::vector<Band> bands)
{
    if (bands.size() > static_cast<std::size_t>(kMaxBands))
        bands.resize(kMaxBands);
    std::lock_guard lock(m_mutex);
    m_pendingBands = std::move(bands);
    m_dirty.store(true, std::memory_order_release);
    m_generation.fetch_add(1, std::memory_order_release);
}

void SuperEQ::setCurve(Curve curve)
{
    if (curve.bands.size() > static_cast<std::size_t>(kMaxBands))
        curve.bands.resize(kMaxBands);
    std::lock_guard lock(m_mutex);
    m_pendingBands = std::move(curve.bands);
    m_pendingPreampDb = curve.preampDb;
    m_dirty.store(true, std::memory_order_release);
    m_generation.fetch_add(1, std::memory_order_release);
}

bool SuperEQ::setBand(int index, const Band& band)
{
    std::lock_guard lock(m_mutex);
    if (index < 0 || index >= static_cast<int>(m_pendingBands.size()))
        return false;
    m_pendingBands[static_cast<std::size_t>(index)] = band;
    m_dirty.store(true, std::memory_order_release);
    m_generation.fetch_add(1, std::memory_order_release);
    return true;
}

std::vector<SuperEQ::Band> SuperEQ::bands() const
{
    std::lock_guard lock(m_mutex);
    return m_pendingBands;
}

void SuperEQ::setPreampDb(double db)
{
    std::lock_guard lock(m_mutex);
    m_pendingPreampDb = db;
    m_dirty.store(true, std::memory_order_release);
    m_generation.fetch_add(1, std::memory_order_release);
}

double SuperEQ::preampDb() const
{
    std::lock_guard lock(m_mutex);
    return m_pendingPreampDb;
}

bool SuperEQ::bandHasEffect(const Band& b)
{
    if (!b.enabled)
        return false;
    switch (b.type) {
    case FilterType::Peaking:
    case FilterType::LowShelf:
    case FilterType::HighShelf:
        return b.gainDb != 0.0;
    default:
        return true;
    }
}

bool SuperEQ::isNeutral() const
{
    if (!isEnabled())
        return true;
    std::lock_guard lock(m_mutex);
    if (m_pendingPreampDb != 0.0)
        return false;
    return std::none_of(m_pendingBands.begin(), m_pendingBands.end(), bandHasEffect);
}

double SuperEQ::peakGainDb() const
{
    if (isNeutral())
        return 0.0;
    constexpr double kRate = 96000.0;
    std::vector<BiquadCoeffs> sections;
    double preamp = 0.0;
    {
        std::lock_guard lock(m_mutex);
        preamp = m_pendingPreampDb;
        for (const Band& b : m_pendingBands)
            if (bandHasEffect(b))
                sections.push_back(designBiquad(b.type, kRate, b.freqHz, b.q, b.gainDb));
    }
    double peak = -1e9;
    const double lo = std::log10(20.0), hi = std::log10(20000.0);
    for (int i = 0; i < 400; ++i) {
        const double w = 2.0 * std::numbers::pi * std::pow(10.0, lo + (hi - lo) * i / 399.0) / kRate;
        const std::complex<double> z1 = std::polar(1.0, -w), z2 = z1 * z1;
        double db = preamp;
        for (const BiquadCoeffs& c : sections)
            db += 20.0 * std::log10(std::abs((c.b0 + c.b1 * z1 + c.b2 * z2) / (1.0 + c.a1 * z1 + c.a2 * z2)));
        peak = std::max(peak, db);
    }
    return peak;
}

void SuperEQ::prepare(double sampleRate, int channels)
{
    m_sampleRate = sampleRate;
    m_channels = std::clamp(channels, 1, kMaxChannels);
    m_dirty.store(true, std::memory_order_release);
    reset();
}

void SuperEQ::reset()
{
    for (auto& ch : m_state)
        for (auto& s : ch)
            s.reset();
}

void SuperEQ::applyPending()
{
    std::lock_guard lock(m_mutex);
    int n = 0;
    for (const Band& b : m_pendingBands) {
        if (!bandHasEffect(b))
            continue;
        m_coeffs[static_cast<std::size_t>(n++)] = designBiquad(b.type, m_sampleRate, b.freqHz, b.q, b.gainDb);
    }
    // Sections that shift position keep their old state; the filters settle
    // within a few samples, which is inaudible compared to a full reset.
    m_activeBands = n;
    m_preampLinear = std::pow(10.0, m_pendingPreampDb / 20.0);
    m_dirty.store(false, std::memory_order_relaxed);
}

void SuperEQ::process(double* interleaved, std::size_t frames)
{
    if (m_dirty.load(std::memory_order_acquire))
        applyPending();
    if (!isEnabled())
        return;

    const int channels = m_channels;
    const int bands = m_activeBands;
    const double pre = m_preampLinear;

    for (int ch = 0; ch < channels; ++ch) {
        auto& state = m_state[static_cast<std::size_t>(ch)];
        double* p = interleaved + ch;
        for (std::size_t i = 0; i < frames; ++i, p += channels) {
            double x = *p * pre;
            for (int b = 0; b < bands; ++b)
                x = state[static_cast<std::size_t>(b)].process(m_coeffs[static_cast<std::size_t>(b)], x);
            *p = x;
        }
    }
}

} // namespace sonkore
