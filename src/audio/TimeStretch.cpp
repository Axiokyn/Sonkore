#include "TimeStretch.hpp"

#include <SoundTouch.h>

#include <algorithm>
#include <cmath>

namespace sonkore {

// SoundTouch, owned and driven by the DSP worker thread.
class StretchBackend {
public:
    StretchBackend(uint32_t sampleRate, uint32_t channels)
    {
        // SoundTouch throws above 192 kHz (352.8 / 384 kHz PCM, DSD read as
        // PCM). The rate only turns its millisecond settings into samples, so
        // it is capped there: the windows get shorter in time, the pitch and
        // tempo ratios stay exact.
        m_st.setSampleRate(std::min<uint32_t>(sampleRate, 192000));
        m_st.setChannels(channels);
    }

    void setRatios(double semitones, double tempo, double rate)
    {
        m_st.setPitchSemiTones(semitones); // double overload
        m_st.setTempo(tempo);
        m_st.setRate(rate);
    }

    void applySettings(const TimeStretch::SoundTouchSettings& s)
    {
        m_st.setSetting(SETTING_USE_AA_FILTER, s.useAAFilter ? 1 : 0);
        m_st.setSetting(SETTING_AA_FILTER_LENGTH, s.aaFilterLength);
        m_st.setSetting(SETTING_USE_QUICKSEEK, s.useQuickSeek ? 1 : 0);
        m_st.setSetting(SETTING_SEQUENCE_MS, s.sequenceMs);
        m_st.setSetting(SETTING_SEEKWINDOW_MS, s.seekWindowMs);
        m_st.setSetting(SETTING_OVERLAP_MS, s.overlapMs);
    }

    void put(const float* interleaved, uint32_t frames) { m_st.putSamples(interleaved, frames); }
    uint32_t receive(float* interleaved, uint32_t maxFrames) { return m_st.receiveSamples(interleaved, maxFrames); }
    void flush() { m_st.flush(); }
    void clear() { m_st.clear(); }

private:
    soundtouch::SoundTouch m_st;
};

TimeStretch::TimeStretch() = default;
TimeStretch::~TimeStretch() = default;

// --- Parameters (any thread) ------------------------------------------------------

void TimeStretch::setPitchSemiTones(double semitones)
{
    m_pitchSemitones.store(std::clamp(semitones, kMinSemitones, kMaxSemitones), std::memory_order_relaxed);
    markDirty(DirtyRatios);
}

void TimeStretch::setTempo(double ratio)
{
    m_tempo.store(std::clamp(ratio, kMinRatio, kMaxRatio), std::memory_order_relaxed);
    markDirty(DirtyRatios);
}

void TimeStretch::setRate(double ratio)
{
    m_rate.store(std::clamp(ratio, kMinRatio, kMaxRatio), std::memory_order_relaxed);
    markDirty(DirtyRatios);
}

void TimeStretch::setUseAAFilter(bool on)
{
    m_useAAFilter.store(on, std::memory_order_relaxed);
    markDirty(DirtySoundTouch);
}

void TimeStretch::setAAFilterLength(int taps)
{
    // SoundTouch requires a multiple of 8 in [8, 128].
    taps = std::clamp((taps + 4) / 8 * 8, 8, 128);
    m_aaFilterLength.store(taps, std::memory_order_relaxed);
    markDirty(DirtySoundTouch);
}

void TimeStretch::setUseQuickSeek(bool on)
{
    m_useQuickSeek.store(on, std::memory_order_relaxed);
    markDirty(DirtySoundTouch);
}

void TimeStretch::setSequenceMs(int ms)
{
    m_sequenceMs.store(std::clamp(ms, 0, 500), std::memory_order_relaxed);
    markDirty(DirtySoundTouch);
}

void TimeStretch::setSeekWindowMs(int ms)
{
    m_seekWindowMs.store(std::clamp(ms, 0, 250), std::memory_order_relaxed);
    markDirty(DirtySoundTouch);
}

void TimeStretch::setOverlapMs(int ms)
{
    m_overlapMs.store(std::clamp(ms, 1, 100), std::memory_order_relaxed);
    markDirty(DirtySoundTouch);
}

TimeStretch::SoundTouchSettings TimeStretch::soundTouchSettings() const
{
    SoundTouchSettings s;
    s.useAAFilter = m_useAAFilter.load(std::memory_order_relaxed);
    s.aaFilterLength = m_aaFilterLength.load(std::memory_order_relaxed);
    s.useQuickSeek = m_useQuickSeek.load(std::memory_order_relaxed);
    s.sequenceMs = m_sequenceMs.load(std::memory_order_relaxed);
    s.seekWindowMs = m_seekWindowMs.load(std::memory_order_relaxed);
    s.overlapMs = m_overlapMs.load(std::memory_order_relaxed);
    return s;
}

bool TimeStretch::hasEffect() const
{
    return !(pitchSemiTones() == 0.0 && tempo() == 1.0 && rate() == 1.0);
}

// --- DSP worker -------------------------------------------------------------------

void TimeStretch::prepare(uint32_t sampleRate, uint32_t channels, uint32_t maxBlockFrames)
{
    if (sampleRate != m_sampleRate || channels != m_channels || maxBlockFrames != m_maxBlockFrames) {
        release(); // rebuilt for the new format when it is next needed
        m_sampleRate = sampleRate;
        m_channels = channels;
        m_maxBlockFrames = maxBlockFrames;
    } else {
        clear();
    }
}

void TimeStretch::applyPendingChanges()
{
    // Nothing runs while neutral; the pending values wait for the engine.
    if (isNeutral())
        return;

    uint32_t dirty = m_dirty.exchange(0, std::memory_order_acquire);
    if (!m_backend) {
        m_backend = std::make_unique<StretchBackend>(m_sampleRate, m_channels);
        dirty = DirtyAll;
    }

    if (dirty & DirtyRatios) {
        m_backend->setRatios(pitchSemiTones(), tempo(), rate());
        m_appliedSpeed = tempo() * rate();
    }
    if (dirty & DirtySoundTouch)
        m_backend->applySettings(soundTouchSettings());
}

void TimeStretch::putSamples(const float* interleaved, uint32_t frames)
{
    if (!m_backend)
        return;
    m_backend->put(interleaved, frames);
    m_heldSource.store(m_heldSource.load(std::memory_order_relaxed) + frames, std::memory_order_relaxed);
}

uint32_t TimeStretch::receiveSamples(float* interleaved, uint32_t maxFrames)
{
    if (!m_backend)
        return 0;
    const uint32_t got = m_backend->receive(interleaved, maxFrames);
    if (got > 0) {
        const double held = m_heldSource.load(std::memory_order_relaxed) - got * m_appliedSpeed;
        m_heldSource.store(std::max(0.0, held), std::memory_order_relaxed);
    }
    return got;
}

void TimeStretch::flush()
{
    if (m_backend)
        m_backend->flush();
}

void TimeStretch::clear()
{
    if (m_backend)
        m_backend->clear();
    m_heldSource.store(0.0, std::memory_order_relaxed);
}

void TimeStretch::release()
{
    m_backend.reset();
    m_heldSource.store(0.0, std::memory_order_relaxed);
    markDirty(DirtyAll);
}

} // namespace sonkore
