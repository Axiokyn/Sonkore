#include "TimeStretch.hpp"

#include <SoundTouch.h>
#include <rubberband/RubberBandStretcher.h>

#include <algorithm>
#include <cmath>
#include <vector>

namespace sonkore {

// One pitch / tempo engine, owned and driven by the DSP worker thread.
class StretchBackend {
public:
    virtual ~StretchBackend() = default;
    virtual void setRatios(double semitones, double tempo, double rate) = 0;
    virtual void applySoundTouch(const TimeStretch::SoundTouchSettings&) {}
    virtual void setFormantsPreserved(bool) {}
    virtual void applyR2(const TimeStretch::RubberBandSettings&) {}
    virtual void put(const float* interleaved, uint32_t frames) = 0;
    virtual uint32_t receive(float* interleaved, uint32_t maxFrames) = 0;
    virtual void flush() = 0;
    virtual void clear() = 0;
};

namespace {

// --- SoundTouch ---------------------------------------------------------------

class SoundTouchBackend final : public StretchBackend {
public:
    SoundTouchBackend(uint32_t sampleRate, uint32_t channels)
    {
        // SoundTouch throws above 192 kHz (352.8 / 384 kHz PCM, DSD read as
        // PCM). The rate only turns its millisecond settings into samples, so
        // it is capped there, as Rubber Band R3 does: the windows get shorter
        // in time, the pitch and tempo ratios stay exact.
        m_st.setSampleRate(std::min<uint32_t>(sampleRate, 192000));
        m_st.setChannels(channels);
    }

    void setRatios(double semitones, double tempo, double rate) override
    {
        m_st.setPitchSemiTones(semitones); // double overload
        m_st.setTempo(tempo);
        m_st.setRate(rate);
    }

    void applySoundTouch(const TimeStretch::SoundTouchSettings& s) override
    {
        m_st.setSetting(SETTING_USE_AA_FILTER, s.useAAFilter ? 1 : 0);
        m_st.setSetting(SETTING_AA_FILTER_LENGTH, s.aaFilterLength);
        m_st.setSetting(SETTING_USE_QUICKSEEK, s.useQuickSeek ? 1 : 0);
        m_st.setSetting(SETTING_SEQUENCE_MS, s.sequenceMs);
        m_st.setSetting(SETTING_SEEKWINDOW_MS, s.seekWindowMs);
        m_st.setSetting(SETTING_OVERLAP_MS, s.overlapMs);
    }

    void put(const float* interleaved, uint32_t frames) override { m_st.putSamples(interleaved, frames); }
    uint32_t receive(float* interleaved, uint32_t maxFrames) override { return m_st.receiveSamples(interleaved, maxFrames); }
    void flush() override { m_st.flush(); }
    void clear() override { m_st.clear(); }

private:
    soundtouch::SoundTouch m_st;
};

// --- Rubber Band ----------------------------------------------------------------

class RubberBandBackend final : public StretchBackend {
public:
    using Stretcher = RubberBand::RubberBandStretcher;

    RubberBandBackend(uint32_t sampleRate, uint32_t channels, uint32_t maxBlockFrames,
                      const TimeStretch::RubberBandSettings& s)
        : m_channels(channels)
        , m_maxBlock(std::max<uint32_t>(1, maxBlockFrames))
    {
        Stretcher::Options options = Stretcher::OptionProcessRealTime | Stretcher::OptionThreadingNever;
        options |= s.finer ? Stretcher::OptionEngineFiner : Stretcher::OptionEngineFaster;
        options |= s.preserveFormants ? Stretcher::OptionFormantPreserved : Stretcher::OptionFormantShifted;
        options |= s.pitchMethod == TimeStretch::PitchConsistency ? Stretcher::OptionPitchHighConsistency
                 : s.pitchMethod == TimeStretch::PitchSpeed       ? Stretcher::OptionPitchHighSpeed
                                                                  : Stretcher::OptionPitchHighQuality;
        options |= s.channelsTogether ? Stretcher::OptionChannelsTogether : Stretcher::OptionChannelsApart;
        options |= s.window == TimeStretch::WindowShort ? Stretcher::OptionWindowShort
                 : s.window == TimeStretch::WindowLong  ? Stretcher::OptionWindowLong
                                                        : Stretcher::OptionWindowStandard;
        options |= s.smoothing ? Stretcher::OptionSmoothingOn : Stretcher::OptionSmoothingOff;
        options |= transientsOption(s.transients) | detectorOption(s.detector);
        options |= s.phaseIndependent ? Stretcher::OptionPhaseIndependent : Stretcher::OptionPhaseLaminar;
        m_rb = std::make_unique<Stretcher>(sampleRate, channels, options);
        m_rb->setMaxProcessSize(m_maxBlock);

        m_in.assign(static_cast<std::size_t>(m_maxBlock) * channels, 0.0f);
        m_out.assign(static_cast<std::size_t>(kOutChunk) * channels, 0.0f);
        for (uint32_t c = 0; c < channels; ++c) {
            m_inPtrs.push_back(m_in.data() + static_cast<std::size_t>(c) * m_maxBlock);
            m_outPtrs.push_back(m_out.data() + static_cast<std::size_t>(c) * kOutChunk);
        }
    }

    void setRatios(double semitones, double tempo, double rate) override
    {
        // Rate is resampling: speed and pitch move together.
        m_rb->setTimeRatio(1.0 / (tempo * rate));
        m_rb->setPitchScale(std::exp2(semitones / 12.0) * rate);
    }

    void setFormantsPreserved(bool on) override
    {
        m_rb->setFormantOption(on ? Stretcher::OptionFormantPreserved : Stretcher::OptionFormantShifted);
    }

    void applyR2(const TimeStretch::RubberBandSettings& s) override
    {
        m_rb->setTransientsOption(transientsOption(s.transients));
        m_rb->setDetectorOption(detectorOption(s.detector));
        m_rb->setPhaseOption(s.phaseIndependent ? Stretcher::OptionPhaseIndependent : Stretcher::OptionPhaseLaminar);
    }

    void put(const float* interleaved, uint32_t frames) override
    {
        if (m_padPending)
            padStart();
        const std::size_t ch = m_channels;
        while (frames > 0) {
            const uint32_t n = std::min(frames, m_maxBlock);
            for (uint32_t f = 0; f < n; ++f)
                for (std::size_t c = 0; c < ch; ++c)
                    m_inPtrs[c][f] = interleaved[f * ch + c];
            m_rb->process(m_inPtrs.data(), n, false);
            interleaved += static_cast<std::size_t>(n) * ch;
            frames -= n;
        }
    }

    uint32_t receive(float* interleaved, uint32_t maxFrames) override
    {
        const std::size_t ch = m_channels;
        uint32_t produced = 0;
        while (produced < maxFrames) {
            const int available = m_rb->available();
            if (available <= 0)
                break;
            const auto wanted = std::min<std::size_t>({static_cast<std::size_t>(available), kOutChunk,
                                                      static_cast<std::size_t>(maxFrames - produced)});
            const std::size_t got = m_rb->retrieve(m_outPtrs.data(), wanted);
            if (got == 0)
                break;
            // The stretcher's own delay: its first output frames are the
            // silent start pad, not music.
            const std::size_t skip = std::min(got, m_discard);
            m_discard -= skip;
            float* out = interleaved + static_cast<std::size_t>(produced) * ch;
            for (std::size_t f = skip; f < got; ++f)
                for (std::size_t c = 0; c < ch; ++c)
                    *out++ = m_outPtrs[c][f];
            produced += static_cast<uint32_t>(got - skip);
        }
        return produced;
    }

    void flush() override
    {
        if (!m_padPending) // something was processed
            m_rb->process(m_inPtrs.data(), 0, true);
    }

    void clear() override
    {
        m_rb->reset(); // keeps the ratios
        m_padPending = true;
        m_discard = 0;
    }

private:
    static constexpr uint32_t kOutChunk = 4096;

    static Stretcher::Options transientsOption(int t)
    {
        return t == TimeStretch::TransientsMixed  ? Stretcher::OptionTransientsMixed
             : t == TimeStretch::TransientsSmooth ? Stretcher::OptionTransientsSmooth
                                                  : Stretcher::OptionTransientsCrisp;
    }
    static Stretcher::Options detectorOption(int d)
    {
        return d == TimeStretch::DetectorPercussive ? Stretcher::OptionDetectorPercussive
             : d == TimeStretch::DetectorSoft       ? Stretcher::OptionDetectorSoft
                                                    : Stretcher::OptionDetectorCompound;
    }

    // Real-time mode does no latency compensation by itself: feed the
    // preferred amount of silence first and drop the matching output, so the
    // music starts at once and stays aligned with the source position.
    void padStart()
    {
        m_padPending = false;
        std::fill(m_in.begin(), m_in.end(), 0.0f);
        std::size_t pad = m_rb->getPreferredStartPad();
        while (pad > 0) {
            const std::size_t n = std::min<std::size_t>(pad, m_maxBlock);
            m_rb->process(m_inPtrs.data(), n, false);
            pad -= n;
        }
        m_discard = m_rb->getStartDelay();
    }

    std::unique_ptr<Stretcher> m_rb;
    uint32_t m_channels;
    uint32_t m_maxBlock;
    std::vector<float> m_in;  // planar, m_maxBlock frames per channel
    std::vector<float> m_out; // planar, kOutChunk frames per channel
    std::vector<float*> m_inPtrs;
    std::vector<float*> m_outPtrs;
    bool m_padPending = true;
    std::size_t m_discard = 0;
};

} // namespace

TimeStretch::TimeStretch() = default;
TimeStretch::~TimeStretch() = default;

// --- Parameters (any thread) ------------------------------------------------------

void TimeStretch::setEngine(Engine engine)
{
    if (m_engine.exchange(engine, std::memory_order_relaxed) != engine)
        markDirty(DirtyEngine);
}

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

// Engine, pitch method, channel handling, window and smoothing are fixed when
// Rubber Band is built, so changing them rebuilds it; formants and the R2
// transients, detector and phase change on the fly.
void TimeStretch::setRubberBandFiner(bool on)
{
    if (m_rbFiner.exchange(on, std::memory_order_relaxed) != on && engine() == Engine::RubberBand)
        markDirty(DirtyEngine);
}

void TimeStretch::setPreserveFormants(bool on)
{
    m_rbFormants.store(on, std::memory_order_relaxed);
    markDirty(DirtyFormants);
}

void TimeStretch::setPitchMethod(int method)
{
    method = std::clamp(method, int(PitchQuality), int(PitchSpeed));
    if (m_rbPitchMethod.exchange(method, std::memory_order_relaxed) != method && engine() == Engine::RubberBand)
        markDirty(DirtyEngine);
}

void TimeStretch::setRubberBandWindow(int window)
{
    window = std::clamp(window, int(WindowStandard), int(WindowLong));
    if (m_rbWindow.exchange(window, std::memory_order_relaxed) != window && engine() == Engine::RubberBand)
        markDirty(DirtyEngine);
}

void TimeStretch::setRubberBandSmoothing(bool on)
{
    if (m_rbSmoothing.exchange(on, std::memory_order_relaxed) != on && engine() == Engine::RubberBand)
        markDirty(DirtyEngine);
}

// R2 transients, detector and phase change while it runs.
void TimeStretch::setTransients(int transients)
{
    m_rbTransients.store(std::clamp(transients, int(TransientsCrisp), int(TransientsSmooth)), std::memory_order_relaxed);
    markDirty(DirtyR2);
}

void TimeStretch::setDetector(int detector)
{
    m_rbDetector.store(std::clamp(detector, int(DetectorCompound), int(DetectorSoft)), std::memory_order_relaxed);
    markDirty(DirtyR2);
}

void TimeStretch::setPhaseIndependent(bool on)
{
    m_rbPhaseIndependent.store(on, std::memory_order_relaxed);
    markDirty(DirtyR2);
}

void TimeStretch::setChannelsTogether(bool on)
{
    if (m_rbChannelsTogether.exchange(on, std::memory_order_relaxed) != on && engine() == Engine::RubberBand)
        markDirty(DirtyEngine);
}

TimeStretch::RubberBandSettings TimeStretch::rubberBandSettings() const
{
    RubberBandSettings s;
    s.finer = m_rbFiner.load(std::memory_order_relaxed);
    s.preserveFormants = m_rbFormants.load(std::memory_order_relaxed);
    s.pitchMethod = m_rbPitchMethod.load(std::memory_order_relaxed);
    s.window = m_rbWindow.load(std::memory_order_relaxed);
    s.transients = m_rbTransients.load(std::memory_order_relaxed);
    s.detector = m_rbDetector.load(std::memory_order_relaxed);
    s.phaseIndependent = m_rbPhaseIndependent.load(std::memory_order_relaxed);
    s.smoothing = m_rbSmoothing.load(std::memory_order_relaxed);
    s.channelsTogether = m_rbChannelsTogether.load(std::memory_order_relaxed);
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
    if (!m_backend || (dirty & DirtyEngine)) {
        m_backend.reset(); // free the old engine before building the new one
        if (engine() == Engine::SoundTouch)
            m_backend = std::make_unique<SoundTouchBackend>(m_sampleRate, m_channels);
        else
            m_backend = std::make_unique<RubberBandBackend>(m_sampleRate, m_channels, m_maxBlockFrames,
                                                            rubberBandSettings());
        dirty = DirtyAll;
    }

    if (dirty & DirtyRatios) {
        m_backend->setRatios(pitchSemiTones(), tempo(), rate());
        m_appliedSpeed = tempo() * rate();
    }
    if (dirty & DirtySoundTouch)
        m_backend->applySoundTouch(soundTouchSettings());
    if (dirty & DirtyFormants)
        m_backend->setFormantsPreserved(m_rbFormants.load(std::memory_order_relaxed));
    if (dirty & DirtyR2)
        m_backend->applyR2(rubberBandSettings());
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
