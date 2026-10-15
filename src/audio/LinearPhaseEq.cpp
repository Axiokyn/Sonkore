#include "LinearPhaseEq.hpp"

#include "Biquad.hpp"
#include "SampleConvert.hpp"

#include <pocketfft_hdronly.h>

#include <algorithm>
#include <cmath>
#include <complex>
#include <numbers>

namespace sonkore {

namespace {

bool bandHasEffect(const SuperEQ::Band& b)
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

} // namespace

std::size_t LinearPhaseEq::tapsFor(double sampleRate)
{
    constexpr std::size_t kUnit = 2 * Convolver::kBlock; // whole partitions, an even count
    const auto units = static_cast<std::size_t>(std::ceil(sampleRate * kSeconds / kUnit));
    return std::max<std::size_t>(units, 1) * kUnit;
}

std::size_t LinearPhaseEq::tapsFor(double sampleRate, const std::vector<SuperEQ::Band>& bands)
{
    // A resonance at f0 with quality Q decays as exp(-pi f0 t / Q): 80 dB
    // down after ln(10^4) Q / (pi f0) s. That must fit in the flat part of
    // each half of the window.
    double seconds = kSeconds;
    for (const SuperEQ::Band& b : bands)
        if (bandHasEffect(b) && b.freqHz > 0.0)
            seconds = std::max(seconds, 2.0 * std::log(1e4) * b.q / (std::numbers::pi * b.freqHz) / (1.0 - kTaper));
    seconds = std::min(seconds, kMaxSeconds);
    constexpr std::size_t kUnit = 2 * Convolver::kBlock;
    const auto units = static_cast<std::size_t>(std::ceil(sampleRate * seconds / kUnit));
    return std::max<std::size_t>(units, 1) * kUnit;
}

std::vector<double> LinearPhaseEq::design(const std::vector<SuperEQ::Band>& bands, double preampDb, double sampleRate,
                                          std::size_t taps)
{
    const std::size_t n = taps, half = taps / 2;
    std::vector<BiquadCoeffs> sections;
    for (const SuperEQ::Band& b : bands)
        if (bandHasEffect(b))
            sections.push_back(designBiquad(b.type, sampleRate, b.freqHz, b.q, b.gainDb));
    const double preamp = std::pow(10.0, preampDb / 20.0);

    // The curve's magnitude on the FFT grid, zero phase, in pocketfft's
    // packed layout [r0, r1, i1, ..., r(n/2)]: its inverse is the impulse
    // response centred on sample 0, symmetric.
    std::vector<double> h(n, 0.0);
    for (std::size_t k = 0; k <= half; ++k) {
        const double w = 2.0 * std::numbers::pi * static_cast<double>(k) / static_cast<double>(n);
        const std::complex<double> z1 = std::polar(1.0, -w), z2 = z1 * z1;
        double m = preamp;
        for (const BiquadCoeffs& c : sections)
            m *= std::abs((c.b0 + c.b1 * z1 + c.b2 * z2) / (1.0 + c.a1 * z1 + c.a2 * z2));
        if (k == 0)
            h[0] = m;
        else if (k == half)
            h[n - 1] = m;
        else
            h[2 * k - 1] = m; // imaginary part 0
    }
    pocketfft::detail::pocketfft_r<double> fft(n);
    fft.exec(h.data(), 1.0 / static_cast<double>(n), false);

    // Centred at taps / 2 and windowed: flat over most of the length, a
    // raised-cosine taper only at the ends.
    std::vector<double> fir(n, 0.0);
    for (std::size_t i = 0; i < n; ++i) {
        const auto offset = static_cast<std::ptrdiff_t>(i) - static_cast<std::ptrdiff_t>(half);
        const double dist = std::abs(static_cast<double>(offset)) / static_cast<double>(half);
        double w = 1.0;
        if (dist > 1.0 - kTaper)
            w = 0.5 * (1.0 + std::cos(std::numbers::pi * (dist - (1.0 - kTaper)) / kTaper));
        const std::size_t src = static_cast<std::size_t>((offset + static_cast<std::ptrdiff_t>(n)) % static_cast<std::ptrdiff_t>(n));
        fir[i] = h[src] * w;
    }
    return fir;
}

void LinearPhaseEq::prepare(uint32_t sampleRate, uint32_t channels)
{
    if (sampleRate != m_rate || channels != m_channels)
        release();
    m_rate = sampleRate;
    m_channels = channels;
    m_taps = tapsFor(sampleRate);
    m_latency = m_taps / 2 + Convolver::kBlock;
    if (m_historySize < m_taps / 2) {
        m_history = {};
        m_historySize = m_taps / 2;
    }
    reset();
}

// A longer filter wants more of the past: the ring grows, what it holds kept
// (oldest first, at the start of the new ring).
void LinearPhaseEq::growHistory(std::size_t frames)
{
    const std::size_t ch = m_channels;
    std::vector<double> grown(frames * ch, 0.0);
    for (std::size_t i = 0; i < m_historyFill && !m_history.empty(); ++i) {
        const std::size_t from = (m_historyPos + m_historySize - m_historyFill + i) % m_historySize;
        std::copy_n(m_history.data() + from * ch, ch, grown.data() + i * ch);
    }
    m_history = std::move(grown);
    m_historySize = frames;
    m_historyPos = m_historyFill % frames;
}

void LinearPhaseEq::release()
{
    reset();
    m_history = {};
    m_historySize = 0;
    m_held = {};
    m_work = {};
    m_out = {};
    m_pass = {};
}

void LinearPhaseEq::reset()
{
    m_state = State::Off;
    busy.store(false, std::memory_order_relaxed);
    m_conv.reset();
    m_generation = ~uint64_t(0);
    m_discard = m_drainLeft = 0;
    m_historyPos = m_historyFill = 0;
    m_held.clear();
}

void LinearPhaseEq::remember(const double* samples, std::size_t frames)
{
    const std::size_t ch = m_channels, size = m_historySize;
    if (ch == 0 || size == 0)
        return;
    if (m_history.empty())
        m_history.assign(size * ch, 0.0);
    // Only the last `size` frames matter.
    if (frames > size) {
        samples += (frames - size) * ch;
        frames = size;
    }
    while (frames > 0) {
        const std::size_t n = std::min(frames, size - m_historyPos);
        std::copy_n(samples, n * ch, m_history.data() + m_historyPos * ch);
        m_historyPos = (m_historyPos + n) % size;
        m_historyFill = std::min(size, m_historyFill + n);
        samples += n * ch;
        frames -= n;
    }
}

void LinearPhaseEq::remember(const int32_t* samples, std::size_t frames)
{
    m_work.resize(frames * m_channels);
    s32ToDouble(samples, m_work.data(), m_work.size());
    remember(m_work.data(), frames);
}

void LinearPhaseEq::update(bool want, const SuperEQ& eq)
{
    switch (m_state) {
    case State::Off:
        if (want)
            engage(eq);
        break;
    case State::Engaged:
        if (!want) {
            m_state = State::Draining;
            m_drainLeft = m_latency;
            m_held.clear();
        } else if (const uint64_t generation = eq.generation(); generation != m_generation) {
            const SuperEQ::Curve curve = eq.curve();
            m_conv->replaceTaps(design(curve.bands, curve.preampDb, m_rate, m_taps));
            m_generation = generation;
        }
        break;
    case State::Draining:
        break; // it engages again (from its history) once drained
    }
    busy.store(m_state != State::Off, std::memory_order_relaxed);
}

// The filter starts from the input that went just before, so its first output
// worth keeping is the next input sample, filtered with its full context.
void LinearPhaseEq::engage(const SuperEQ& eq)
{
    const SuperEQ::Curve curve = eq.curve();
    m_generation = eq.generation();
    m_taps = tapsFor(m_rate, curve.bands);
    m_latency = m_taps / 2 + Convolver::kBlock;
    if (m_historySize < m_taps / 2)
        growHistory(m_taps / 2);
    ImpulseResponse ir;
    ir.sampleRate = m_rate;
    ir.channels = 1;
    ir.taps = {design(curve.bands, curve.preampDb, m_rate, m_taps)};
    m_conv = Convolver::create(ir, m_rate, m_channels, nullptr);
    if (!m_conv)
        return;
    const std::size_t ch = m_channels, size = m_taps / 2, ring = m_historySize;
    // The last taps/2 input frames, oldest first (silence before a seek).
    std::vector<double> prime(size * ch, 0.0);
    const std::size_t have = std::min(m_historyFill, size);
    if (!m_history.empty())
        for (std::size_t i = 0; i < have; ++i) {
            const std::size_t from = (m_historyPos + ring - have + i) % ring;
            std::copy_n(m_history.data() + from * ch, ch, prime.data() + (size - have + i) * ch);
        }
    m_conv->process(prime.data(), size); // outputs of what was already heard: dropped
    m_discard = m_latency;
    m_state = State::Engaged;
}

void LinearPhaseEq::feed(const double* samples, std::size_t frames)
{
    const std::size_t ch = m_channels;
    m_work.assign(samples, samples + frames * ch);
    m_conv->process(m_work.data(), frames);
    const std::size_t drop = std::min(m_discard, frames);
    m_discard -= drop;
    m_out.insert(m_out.end(), m_work.begin() + static_cast<std::ptrdiff_t>(drop * ch), m_work.end());
}

void LinearPhaseEq::process(const double* samples, std::size_t frames)
{
    const std::size_t ch = m_channels;
    m_out.clear();
    m_pass.clear();
    switch (m_state) {
    case State::Off:
        m_pass.assign(samples, samples + frames * ch);
        return;
    case State::Engaged:
        feed(samples, frames);
        return;
    case State::Draining: {
        const std::size_t n = std::min(frames, m_drainLeft);
        m_held.insert(m_held.end(), samples, samples + n * ch);
        feed(samples, n);
        m_drainLeft -= n;
        if (m_drainLeft == 0) {
            // Everything it held is out: the input fed meanwhile goes on
            // unfiltered, then the rest of this block.
            m_state = State::Off;
            busy.store(false, std::memory_order_relaxed);
            m_conv.reset();
            m_pass = std::move(m_held);
            m_held = {};
            m_pass.insert(m_pass.end(), samples + n * ch, samples + frames * ch);
        }
        return;
    }
    }
}

} // namespace sonkore
