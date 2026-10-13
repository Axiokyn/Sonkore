#pragma once

#include <atomic>
#include <cstdint>
#include <memory>

namespace sonkore {

class StretchBackend; // SoundTouch itself, private to TimeStretch.cpp

// Pitch / tempo / rate processor on SoundTouch (WSOLA): light on CPU, with
// its classic tuning parameters.
//
// Thread-safety: every setter only stores the new value in an atomic and
// raises a dirty bit. The DSP worker calls applyPendingChanges() at each block
// boundary and forwards the values to the engine. Pitch, tempo and rate stay
// `double` end to end (SoundTouch takes the pitch as double semitones).
//
// Memory: the engine is only built while it has something to do, and
// release() frees it again (the DSP worker calls it once the values are back
// to neutral), so a bit-perfect session never holds stretcher buffers.
class TimeStretch {
public:
    struct SoundTouchSettings {
        bool useAAFilter = true;   // SETTING_USE_AA_FILTER
        int aaFilterLength = 64;   // SETTING_AA_FILTER_LENGTH (8..128, multiple of 8)
        bool useQuickSeek = false; // SETTING_USE_QUICKSEEK
        int sequenceMs = 0;        // SETTING_SEQUENCE_MS   (0 = automatic)
        int seekWindowMs = 0;      // SETTING_SEEKWINDOW_MS (0 = automatic)
        int overlapMs = 8;         // SETTING_OVERLAP_MS
    };

    static constexpr double kMinSemitones = -48.0;
    static constexpr double kMaxSemitones = 48.0;
    static constexpr double kMinRatio = 0.05;
    static constexpr double kMaxRatio = 20.0;

    TimeStretch();
    ~TimeStretch();
    TimeStretch(const TimeStretch&) = delete;
    TimeStretch& operator=(const TimeStretch&) = delete;

    // --- any thread -------------------------------------------------------
    void setPitchSemiTones(double semitones);
    void setTempo(double ratio);
    void setRate(double ratio);
    double pitchSemiTones() const { return m_pitchSemitones.load(std::memory_order_relaxed); }
    double tempo() const { return m_tempo.load(std::memory_order_relaxed); }
    double rate() const { return m_rate.load(std::memory_order_relaxed); }

    void setUseAAFilter(bool on);
    void setAAFilterLength(int taps);
    void setUseQuickSeek(bool on);
    void setSequenceMs(int ms);
    void setSeekWindowMs(int ms);
    void setOverlapMs(int ms);
    SoundTouchSettings soundTouchSettings() const;

    // Off: the values are kept but not applied (the path is bypassed as if
    // they were neutral).
    void setEnabled(bool on) { m_enabled.store(on, std::memory_order_relaxed); }
    bool enabled() const { return m_enabled.load(std::memory_order_relaxed); }

    // The values differ from neutral, whether or not the processor is on.
    // Exact comparison on purpose: -0.0000001 semitones is a real change.
    bool hasEffect() const;
    // Nothing to apply: switched off, or neutral values.
    bool isNeutral() const { return !enabled() || !hasEffect(); }

    // Source frames consumed per output frame (tempo * rate; 1 while neutral).
    double speedFactor() const { return isNeutral() ? 1.0 : tempo() * rate(); }

    // --- DSP worker thread ------------------------------------------------
    void prepare(uint32_t sampleRate, uint32_t channels, uint32_t maxBlockFrames);
    // Builds the engine if needed and forwards the pending values to it.
    void applyPendingChanges();
    void putSamples(const float* interleaved, uint32_t frames);
    uint32_t receiveSamples(float* interleaved, uint32_t maxFrames);
    void flush();
    void clear();
    void release();

    // --- any thread -------------------------------------------------------
    // Source frames given to the engine that have not come out yet (its
    // latency plus what waits for a full analysis window), so the playback
    // position can leave them out: ~50-100 ms.
    double heldSourceFrames() const { return m_heldSource.load(std::memory_order_relaxed); }

private:
    enum DirtyBits : uint32_t {
        DirtyRatios = 1u << 0,
        DirtySoundTouch = 1u << 1,
        DirtyAll = 0x3u,
    };
    void markDirty(uint32_t bits) { m_dirty.fetch_or(bits, std::memory_order_release); }

    // Worker-owned.
    std::unique_ptr<StretchBackend> m_backend;
    uint32_t m_sampleRate = 44100;
    uint32_t m_channels = 2;
    uint32_t m_maxBlockFrames = 1024;

    std::atomic<double> m_pitchSemitones{0.0};
    std::atomic<double> m_tempo{1.0};
    std::atomic<double> m_rate{1.0};
    std::atomic<bool> m_enabled{false}; // a fresh install starts bit-perfect

    std::atomic<bool> m_useAAFilter{true};
    std::atomic<int> m_aaFilterLength{64};
    std::atomic<bool> m_useQuickSeek{false};
    std::atomic<int> m_sequenceMs{0};
    std::atomic<int> m_seekWindowMs{0};
    std::atomic<int> m_overlapMs{8};

    std::atomic<uint32_t> m_dirty{DirtyAll};

    // Worker-written: frames put in minus frames received times the speed.
    std::atomic<double> m_heldSource{0.0};
    double m_appliedSpeed = 1.0; // tempo * rate the engine runs at

    static_assert(std::atomic<double>::is_always_lock_free, "lock-free atomic<double> required");
};

} // namespace sonkore
