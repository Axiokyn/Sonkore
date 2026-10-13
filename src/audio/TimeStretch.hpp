#pragma once

#include <atomic>
#include <cstdint>
#include <memory>

namespace sonkore {

class StretchBackend; // the engine itself, private to TimeStretch.cpp

// Pitch / tempo / rate processor with two interchangeable engines:
//
// * Rubber Band (default): phase-vocoder; the R3 "finer" engine (default)
//   gives the cleanest result, and formants can be preserved.
// * SoundTouch: WSOLA; light on CPU, with its classic tuning parameters.
//
// Thread-safety: every setter only stores the new value in an atomic and
// raises a dirty bit. The DSP worker calls applyPendingChanges() at each block
// boundary and forwards the values to the engine. Pitch, tempo and rate stay
// `double` end to end (Rubber Band takes the pitch as a double frequency
// ratio, 2^(semitones/12); SoundTouch as double semitones).
//
// Memory: the engine is only built while it has something to do, and
// release() frees it again (the DSP worker calls it once the values are back
// to neutral), so a bit-perfect session never holds stretcher buffers.
class TimeStretch {
public:
    enum class Engine : int { RubberBand = 0, SoundTouch = 1 };

    struct SoundTouchSettings {
        bool useAAFilter = true;   // SETTING_USE_AA_FILTER
        int aaFilterLength = 64;   // SETTING_AA_FILTER_LENGTH (8..128, multiple of 8)
        bool useQuickSeek = false; // SETTING_USE_QUICKSEEK
        int sequenceMs = 0;        // SETTING_SEQUENCE_MS   (0 = automatic)
        int seekWindowMs = 0;      // SETTING_SEEKWINDOW_MS (0 = automatic)
        int overlapMs = 8;         // SETTING_OVERLAP_MS
    };

    // Defaults: R3 with the channels together. In real-time mode R2 keeps
    // resetting phase on steady tones (a 1 kHz sine shifted -0.32 semitones
    // comes out warbling, ~7 dB SINAD, vs ~55 dB for R3) and smears attacks
    // more than R3. R3 with the channels apart narrows the stereo image
    // (L/R correlation 0.62 -> 0.43 on a mix); together it is kept. R3 costs
    // ~10% of one core for 44.1 kHz stereo. A plain shift needs no formant
    // processing.
    // Rubber Band choices, stored as ints (settings, QML).
    // Pitch: quality for a fixed shift; consistency for no discontinuity
    // while the pitch is being edited; speed for the least CPU.
    enum PitchMethod : int { PitchQuality = 0, PitchConsistency = 1, PitchSpeed = 2 };
    // FFT window: short = less delay (and, in R3, much less CPU) at some
    // quality; long (R2 only) = smoother, less precise in time.
    enum Window : int { WindowStandard = 0, WindowShort = 1, WindowLong = 2 };
    // R2 phase resets at attacks: crisp, only outside the range of musical
    // fundamentals, or never. Smooth is the default: in real time, crisp and
    // mixed keep resetting phase on steady tones too (a 1 kHz sine shifted
    // -0.32 semitones warbles, ~7 dB SINAD, vs ~61 dB smooth).
    enum Transients : int { TransientsCrisp = 0, TransientsMixed = 1, TransientsSmooth = 2 };
    enum Detector : int { DetectorCompound = 0, DetectorPercussive = 1, DetectorSoft = 2 };

    struct RubberBandSettings {
        bool finer = true;              // R3 engine instead of R2
        bool preserveFormants = false;  // keep voices' timbre when shifting pitch
        int pitchMethod = PitchQuality;
        bool channelsTogether = true;   // stereo pair kept phase-coherent (mono-compatible)
        int window = WindowStandard;
        // R2 only (R3 ignores them):
        int transients = TransientsSmooth;
        int detector = DetectorCompound;
        bool phaseIndependent = false;  // each frequency bin on its own: softer, phasier
        bool smoothing = false;         // time-domain smoothing: softer, artifacts at attacks
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
    void setEngine(Engine engine);
    Engine engine() const { return m_engine.load(std::memory_order_relaxed); }

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

    void setRubberBandFiner(bool on);
    void setPreserveFormants(bool on);
    void setPitchMethod(int method);
    void setChannelsTogether(bool on);
    void setRubberBandWindow(int window);
    void setTransients(int transients);
    void setDetector(int detector);
    void setPhaseIndependent(bool on);
    void setRubberBandSmoothing(bool on);
    RubberBandSettings rubberBandSettings() const;

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
    // position can leave them out: ~50-140 ms with Rubber Band.
    double heldSourceFrames() const { return m_heldSource.load(std::memory_order_relaxed); }

private:
    enum DirtyBits : uint32_t {
        DirtyRatios = 1u << 0,
        DirtySoundTouch = 1u << 1,
        DirtyFormants = 1u << 2,
        DirtyEngine = 1u << 3, // the engine must be rebuilt
        DirtyR2 = 1u << 4,     // Rubber Band R2 options that change on the fly
        DirtyAll = 0x1Fu,
    };
    void markDirty(uint32_t bits) { m_dirty.fetch_or(bits, std::memory_order_release); }

    // Worker-owned.
    std::unique_ptr<StretchBackend> m_backend;
    uint32_t m_sampleRate = 44100;
    uint32_t m_channels = 2;
    uint32_t m_maxBlockFrames = 1024;

    std::atomic<Engine> m_engine{Engine::RubberBand};
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

    std::atomic<bool> m_rbFiner{true};
    std::atomic<bool> m_rbFormants{false};
    std::atomic<int> m_rbPitchMethod{PitchQuality};
    std::atomic<bool> m_rbChannelsTogether{true};
    std::atomic<int> m_rbWindow{WindowStandard};
    std::atomic<int> m_rbTransients{TransientsSmooth};
    std::atomic<int> m_rbDetector{DetectorCompound};
    std::atomic<bool> m_rbPhaseIndependent{false};
    std::atomic<bool> m_rbSmoothing{false};

    std::atomic<uint32_t> m_dirty{DirtyAll};

    // Worker-written: frames put in minus frames received times the speed.
    std::atomic<double> m_heldSource{0.0};
    double m_appliedSpeed = 1.0; // tempo * rate the engine runs at

    static_assert(std::atomic<double>::is_always_lock_free, "lock-free atomic<double> required");
};

} // namespace sonkore
