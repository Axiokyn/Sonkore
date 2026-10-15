#include "Limiter.hpp"

#include <algorithm>
#include <cmath>
#include <numbers>

namespace sonkore {

namespace {

// 4x windowed-sinc interpolator (Blackman window), 12 taps per phase: the
// BS.1770 true-peak measurement filter.
std::vector<double> interpolator()
{
    constexpr int phases = Limiter::kOversample, n = Limiter::kTapsPerPhase;
    constexpr int total = phases * n;
    std::vector<double> taps(total);
    const double centre = (total - 1) / 2.0;
    double sum = 0.0;
    for (int i = 0; i < total; ++i) {
        const double x = (i - centre) / phases;
        const double sinc = x == 0.0 ? 1.0 : std::sin(std::numbers::pi * x) / (std::numbers::pi * x);
        const double w = 0.42 - 0.5 * std::cos(2.0 * std::numbers::pi * i / (total - 1)) + 0.08 * std::cos(4.0 * std::numbers::pi * i / (total - 1));
        taps[i] = sinc * w;
        sum += taps[i];
    }
    for (double& t : taps) // unity gain per phase (DC through each phase sums to 1)
        t *= phases / sum;
    return taps;
}

} // namespace

void Limiter::setup(uint32_t sampleRate, uint32_t channels, double ceilingDb)
{
    m_rate = sampleRate;
    m_channels = channels;
    m_lookahead = std::max<std::size_t>(8, static_cast<std::size_t>(std::lround(kLookaheadSeconds * sampleRate)));
    m_releaseCoef = 1.0 - std::exp(-1.0 / (kReleaseSeconds * sampleRate));
    m_phases = sampleRate >= 352800 ? 1 : kOversample;
    m_taps = interpolator();
    m_history.assign(static_cast<std::size_t>(channels) * kTapsPerPhase, 0.0);
    m_delay.assign(m_lookahead * channels, 0.0);
    m_box.assign(m_lookahead, 1.0);
    setCeilingDb(ceilingDb);
    reset();
}

void Limiter::setCeilingDb(double db)
{
    m_ceilingDb = std::min(0.0, db);
    m_ceiling = std::pow(10.0, m_ceilingDb / 20.0);
}

void Limiter::reset()
{
    std::fill(m_history.begin(), m_history.end(), 0.0);
    std::fill(m_delay.begin(), m_delay.end(), 0.0);
    std::fill(m_box.begin(), m_box.end(), 1.0);
    m_historyPos = m_delayPos = m_boxPos = 0;
    m_held = 0;
    m_minWindow.clear();
    m_index = 0;
    m_boxSum = static_cast<double>(m_lookahead);
    m_envelope = 1.0;
    m_lastReductionDb = 0.0;
}

void Limiter::release()
{
    m_rate = m_channels = 0;
    m_taps = {};
    m_history = {};
    m_delay = {};
    m_box = {};
    m_minWindow.clear();
    m_held = 0;
}

double Limiter::gainFor(double peak) const
{
    return peak > m_ceiling ? m_ceiling / peak : 1.0;
}

// The true peak of one frame: the frame's samples and the three interpolated
// values that follow each (the interpolator is centred, so the peak found
// belongs to the samples kTapsPerPhase / 2 frames back; the look-ahead is
// long enough for that not to matter).
void Limiter::pushFrame(const double* frame)
{
    const uint32_t ch = m_channels;
    double peak = 0.0;
    for (uint32_t c = 0; c < ch; ++c) {
        double* hist = m_history.data() + static_cast<std::size_t>(c) * kTapsPerPhase;
        hist[m_historyPos] = frame[c];
        peak = std::max(peak, std::fabs(frame[c]));
        for (int phase = 1; phase < m_phases; ++phase) {
            double acc = 0.0;
            const double* taps = m_taps.data() + phase; // taps[phase + 4k]
            for (int k = 0; k < kTapsPerPhase; ++k)
                acc += taps[static_cast<std::size_t>(k) * kOversample] * hist[(m_historyPos + kTapsPerPhase - k) % kTapsPerPhase];
            peak = std::max(peak, std::fabs(acc));
        }
    }
    m_historyPos = (m_historyPos + 1) % kTapsPerPhase;

    // Sliding minimum of the target gain over the next m_lookahead frames.
    const double target = gainFor(peak);
    while (!m_minWindow.empty() && m_minWindow.back().second >= target)
        m_minWindow.pop_back();
    m_minWindow.emplace_back(m_index, target);
    while (m_minWindow.front().first + m_lookahead <= m_index)
        m_minWindow.pop_front();
    ++m_index;
}

std::size_t Limiter::process(const double* in, std::size_t frames, std::vector<double>& out)
{
    const uint32_t ch = m_channels;
    out.resize(frames * ch);
    std::size_t produced = 0;
    double worst = 1.0;
    for (std::size_t f = 0; f < frames; ++f) {
        pushFrame(in + f * ch);
        // The envelope for the frame leaving the delay line: minimum over the
        // window that ends at the frame just pushed, released exponentially,
        // then averaged over the window length (a linear attack).
        const double minimum = m_minWindow.front().second;
        m_envelope = minimum < m_envelope ? minimum : m_envelope + (minimum - m_envelope) * m_releaseCoef;
        m_boxSum += m_envelope - m_box[m_boxPos];
        m_box[m_boxPos] = m_envelope;
        m_boxPos = (m_boxPos + 1) % m_lookahead;
        double gain = std::min(1.0, m_boxSum / static_cast<double>(m_lookahead));
        if (gain > 1.0 - 1e-12)
            gain = 1.0; // exact pass-through, not 0.999999999
        worst = std::min(worst, gain);

        double* slot = m_delay.data() + m_delayPos * ch;
        if (m_held == m_lookahead) {
            double* dst = out.data() + produced * ch;
            for (uint32_t c = 0; c < ch; ++c)
                dst[c] = slot[c] * gain;
            ++produced;
        } else {
            ++m_held;
        }
        std::copy_n(in + f * ch, ch, slot);
        m_delayPos = (m_delayPos + 1) % m_lookahead;
    }
    out.resize(produced * ch);
    m_lastReductionDb = worst < 1.0 ? 20.0 * std::log10(worst) : 0.0;
    return produced;
}

std::size_t Limiter::flush(std::vector<double>& out)
{
    // Push silence through: the held frames come out with their gains.
    const uint32_t ch = m_channels;
    const std::size_t held = m_held;
    std::vector<double> zeros(m_lookahead * ch, 0.0);
    std::vector<double> tail;
    process(zeros.data(), m_lookahead, tail);
    // Of what came out, only `held` frames were real input (the rest is the
    // silence pushed in, which went through the fill-up).
    out.assign(tail.begin(), tail.begin() + static_cast<std::ptrdiff_t>(std::min(held, tail.size() / ch) * ch));
    const double ceilingDb = m_ceilingDb;
    reset();
    setCeilingDb(ceilingDb);
    return out.size() / ch;
}

double Limiter::truePeak(const double* in, std::size_t frames, uint32_t channels, uint32_t sampleRate)
{
    Limiter l;
    l.setup(sampleRate, channels, 0.0);
    double peak = 0.0;
    const std::vector<double> taps = interpolator();
    std::vector<double> hist(static_cast<std::size_t>(channels) * kTapsPerPhase, 0.0);
    std::size_t pos = 0;
    for (std::size_t f = 0; f < frames; ++f) {
        for (uint32_t c = 0; c < channels; ++c) {
            double* h = hist.data() + static_cast<std::size_t>(c) * kTapsPerPhase;
            h[pos] = in[f * channels + c];
            peak = std::max(peak, std::fabs(h[pos]));
            for (int phase = 1; phase < kOversample; ++phase) {
                double acc = 0.0;
                for (int k = 0; k < kTapsPerPhase; ++k)
                    acc += taps[static_cast<std::size_t>(phase) + static_cast<std::size_t>(k) * kOversample] * h[(pos + kTapsPerPhase - k) % kTapsPerPhase];
                peak = std::max(peak, std::fabs(acc));
            }
        }
        pos = (pos + 1) % kTapsPerPhase;
    }
    return peak;
}

} // namespace sonkore
