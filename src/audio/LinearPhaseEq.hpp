#pragma once

#include "Convolver.hpp"
#include "SuperEQ.hpp"

#include <atomic>
#include <cstddef>
#include <cstdint>
#include <memory>
#include <vector>

namespace sonkore {

// The equaliser's curve as a linear-phase FIR: the same magnitude response as
// SuperEQ's biquads, but every frequency delayed alike, so the EQ leaves the
// waveform's timing (transients, the stereo image) untouched.
//
// Design: the curve sampled on a grid of fs/N, made zero-phase, centred and
// shaped by a Tukey window (flat over 3/4 of its length, so the slow decay of
// a narrow bass band is kept): within 0.01 dB of the biquad curve from 20 Hz
// up for ordinary settings. N is about 0.5 s of taps at any rate, so the
// filter delays the music by about a quarter of a second; run with the
// engine's FFT convolver (partitioned overlap-save).
//
// The delay is hidden rather than heard:
// * engaging (the curve leaves flat, the mode is chosen, a seek): the filter
//   is primed with the input that went just before (kept here all along) and
//   its first outputs are dropped, so the music carries on from the same
//   sample, filtered, with no gap and no repeat;
// * a new curve while it runs: the filter is swapped with a one-block
//   crossfade;
// * disengaging (back to flat): the filter is drained with the input that
//   follows, which is then handed back to go on unfiltered.
// Worker thread only.
class LinearPhaseEq {
public:
    static constexpr double kSeconds = 0.5;   // filter length
    static constexpr double kTaper = 0.25;    // Tukey window: tapered fraction of each half

    // Taps for a sample rate: whole convolver partitions, about kSeconds.
    static std::size_t tapsFor(double sampleRate);
    // The FIR for `bands` and `preampDb` at `sampleRate`, centred at taps / 2.
    static std::vector<double> design(const std::vector<SuperEQ::Band>& bands, double preampDb, double sampleRate,
                                      std::size_t taps);

    void prepare(uint32_t sampleRate, uint32_t channels);
    void release();
    // Seek or a new track: no history, not engaged.
    void reset();

    // The chain's input at the filter's place, kept while the mode is on so
    // the filter can engage seamlessly (blocks that bypass it included).
    void remember(const double* samples, std::size_t frames);
    void remember(const int32_t* samples, std::size_t frames);

    // Once per block, before process(): `want` whether the curve should be
    // applied now; `eq` its bands. Engages, swaps or starts draining.
    void update(bool want, const SuperEQ& eq);
    // Engaged or draining: the frames have to come through process().
    bool active() const { return m_state != State::Off; }
    // Frames of music held inside (what was fed and not handed out yet).
    std::size_t latency() const { return active() ? m_latency : 0; }
    // Frames still owed after the input ends.
    std::size_t tail() const { return m_conv ? m_conv->tail() : 0; }
    std::size_t taps() const { return m_taps; }

    // Interleaved input; afterwards filtered() holds the frames that come out
    // of the filter (fewer while it engages) and passed() those handed back
    // unfiltered (once a drain ends: the input it held, then the rest).
    void process(const double* samples, std::size_t frames);
    std::vector<double>& filtered() { return m_out; }
    std::vector<double>& passed() { return m_pass; }

    // Read from other threads (the worker sets it).
    std::atomic<bool> busy{false};

private:
    enum class State { Off, Engaged, Draining };
    void engage(const SuperEQ& eq);
    void feed(const double* samples, std::size_t frames); // through the filter, dropping what must be

    uint32_t m_rate = 0, m_channels = 0;
    std::size_t m_taps = 0, m_latency = 0;
    State m_state = State::Off;
    std::unique_ptr<Convolver> m_conv;
    uint64_t m_generation = ~uint64_t(0); // SuperEQ generation the filter is for
    std::size_t m_discard = 0;            // outputs still to drop (engaging)
    std::size_t m_drainLeft = 0;          // input still to feed (draining)
    std::vector<double> m_history;       // ring of the last taps/2 input frames
    std::size_t m_historyPos = 0, m_historyFill = 0;
    std::vector<double> m_held;           // draining: the input fed, to hand back
    std::vector<double> m_work, m_out, m_pass;
};

} // namespace sonkore
