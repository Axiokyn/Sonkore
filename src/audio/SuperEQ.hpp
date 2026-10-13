#pragma once

#include "Biquad.hpp"

#include <array>
#include <atomic>
#include <cstddef>
#include <mutex>
#include <vector>

namespace sonkore {

// Multiband graphic/parametric equaliser: a cascade of RBJ biquads processed
// in double precision on interleaved buffers.
//
// Threading: setters may be called from any thread (they only touch a
// mutex-protected "pending" copy). prepare()/reset()/process() belong to the
// DSP worker thread, which picks up pending changes at block boundaries.
class SuperEQ {
public:
    static constexpr int kMaxBands = 32;
    static constexpr int kMaxChannels = 8;

    struct Band {
        FilterType type = FilterType::Peaking;
        double freqHz = 1000.0;
        double gainDb = 0.0;
        double q = 1.41421356237;
        bool enabled = true;
    };

    SuperEQ();

    // ISO 10-band graphic layout (31 Hz .. 16 kHz), all at 0 dB.
    static std::vector<Band> graphic10Band();

    // --- any thread ---
    void setEnabled(bool enabled);
    bool isEnabled() const { return m_enabled.load(std::memory_order_relaxed); }
    void setBands(std::vector<Band> bands);
    bool setBand(int index, const Band& band);
    std::vector<Band> bands() const;
    void setPreampDb(double db);
    double preampDb() const;

    // True when processing would be an identity: the bit-perfect bypass may
    // then skip the EQ entirely.
    bool isNeutral() const;
    // Highest gain of the whole curve (preamp included) across 20 Hz - 20 kHz,
    // in dB; 0 when neutral. What the EQ can add to a full-scale signal.
    double peakGainDb() const;

    // --- DSP worker thread ---
    void prepare(double sampleRate, int channels);
    void reset();
    void process(double* interleaved, std::size_t frames);

private:
    void applyPending();
    static bool bandHasEffect(const Band& b);

    mutable std::mutex m_mutex;
    std::vector<Band> m_pendingBands;
    double m_pendingPreampDb = 0.0;
    std::atomic<bool> m_dirty{true};
    std::atomic<bool> m_enabled{true};

    // Worker-owned state.
    double m_sampleRate = 44100.0;
    int m_channels = 2;
    int m_activeBands = 0;
    double m_preampLinear = 1.0;
    std::array<BiquadCoeffs, kMaxBands> m_coeffs{};
    std::array<std::array<BiquadState, kMaxBands>, kMaxChannels> m_state{};
};

} // namespace sonkore
