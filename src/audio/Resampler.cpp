#include "Resampler.hpp"

#include <soxr.h>

#include <algorithm>
#include <cmath>

namespace sonkore {

Resampler::~Resampler()
{
    release();
}

bool Resampler::prepare(uint32_t inRate, uint32_t outRate, uint32_t channels, std::string* error, Filter filter)
{
    release();
    unsigned long phase = SOXR_LINEAR_PHASE;
    switch (filter) {
    case Filter::Intermediate: phase = SOXR_INTERMEDIATE_PHASE; break;
    case Filter::Minimum: phase = SOXR_MINIMUM_PHASE; break;
    case Filter::LinearSteep: phase = SOXR_LINEAR_PHASE | SOXR_STEEP_FILTER; break;
    default: break;
    }
    // Phase response and steepness belong to the recipe, with the quality.
    const soxr_quality_spec_t quality = soxr_quality_spec(SOXR_VHQ | phase, 0);
    return create(inRate, outRate, channels, quality, error);
}

bool Resampler::prepareLowpass(uint32_t inRate, uint32_t outRate, uint32_t channels, double passbandEnd, double stopbandBegin,
                               std::string* error)
{
    release();
    soxr_quality_spec_t quality = soxr_quality_spec(SOXR_VHQ | SOXR_LINEAR_PHASE, 0);
    quality.passband_end = passbandEnd;
    quality.stopband_begin = stopbandBegin;
    return create(inRate, outRate, channels, quality, error);
}

bool Resampler::create(uint32_t inRate, uint32_t outRate, uint32_t channels, const struct ::soxr_quality_spec& quality, std::string* error)
{
    const soxr_io_spec_t io = soxr_io_spec(SOXR_FLOAT64_I, SOXR_FLOAT64_I);
    const soxr_runtime_spec_t runtime = soxr_runtime_spec(1);          // the worker is the only thread
    soxr_error_t err = nullptr;
    m_soxr = soxr_create(inRate, outRate, channels, &err, &io, &quality, &runtime);
    if (err || !m_soxr) {
        if (error)
            *error = std::string("Resampler: ") + (err ? err : "unknown error");
        m_soxr = nullptr;
        return false;
    }
    m_channels = channels;
    m_ratio = static_cast<double>(outRate) / inRate;
    return true;
}

void Resampler::release()
{
    if (m_soxr) {
        soxr_delete(m_soxr);
        m_soxr = nullptr;
    }
    m_out.clear();
    m_out.shrink_to_fit();
}

std::size_t Resampler::process(const double* in, std::size_t frames)
{
    return frames > 0 ? run(in, frames) : 0;
}

std::size_t Resampler::drain()
{
    return run(nullptr, 0);
}

// One call consumes all the input (in == nullptr: flush), growing the output
// buffer when the converter has more to give than expected.
std::size_t Resampler::run(const double* in, std::size_t frames)
{
    if (!m_soxr)
        return 0;
    const std::size_t ch = m_channels;
    const std::size_t expected = static_cast<std::size_t>(std::ceil(static_cast<double>(frames) * m_ratio)) + 256;
    if (m_out.size() < expected * ch)
        m_out.resize(expected * ch);

    std::size_t consumed = 0, produced = 0;
    for (;;) {
        const std::size_t room = m_out.size() / ch - produced;
        std::size_t used = 0, made = 0;
        const double* src = in ? in + consumed * ch : nullptr;
        soxr_process(m_soxr, src, in ? frames - consumed : 0, &used, m_out.data() + produced * ch, room, &made);
        consumed += used;
        produced += made;
        if (made < room && consumed >= frames)
            return produced; // all input taken and the output was not the limit
        if (made == room)
            m_out.resize(m_out.size() * 2);
        else if (used == 0 && made == 0)
            return produced; // no progress possible
    }
}

void Resampler::clear()
{
    if (m_soxr)
        soxr_clear(m_soxr);
}

double Resampler::delayFrames() const
{
    return m_soxr ? soxr_delay(m_soxr) : 0.0;
}

} // namespace sonkore
