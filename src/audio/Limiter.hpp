#pragma once

#include <cstddef>
#include <cstdint>
#include <deque>
#include <vector>

namespace sonkore {

// True-peak limiter, double precision, at the end of the processed path
// (after the EQ, the volume and the resampler, before the quantiser): what
// a boost or an inter-sample peak pushes above the ceiling is brought down
// to it, instead of being clipped at full scale by the quantiser.
//
// * The peak is the true peak (ITU-R BS.1770: 4x oversampled with a 48-tap
//   windowed-sinc interpolator), so the waveform the DAC reconstructs
//   between the samples stays under the ceiling too (-1 dBTP by default,
//   EBU R128's ceiling).
// * Look-ahead: the gain ramps down over `lookahead` frames (1.5 ms) and
//   reaches its minimum exactly at the peak (a sliding minimum over the
//   look-ahead window, then a box average of the same length), so the
//   attack is a linear fade, not a step. The release back to unity is
//   exponential (50 ms). All channels share one gain: the stereo image is
//   not shifted.
// * Below the ceiling the gain is exactly 1.0 and the samples pass through
//   untouched (a multiplication by 1.0 is exact), delayed by `lookahead`.
//
// Worker thread only. process() hands back fewer frames than it took while
// the delay line fills; flush() returns what is still held (end of track,
// or the path going back to untouched).
class Limiter {
public:
    static constexpr double kLookaheadSeconds = 0.0015;
    static constexpr double kReleaseSeconds = 0.05;
    static constexpr int kOversample = 4;
    static constexpr int kTapsPerPhase = 12; // 48 taps

    void setup(uint32_t sampleRate, uint32_t channels, double ceilingDb);
    void setCeilingDb(double db); // without a reset
    void reset();                 // empties the delay line and the envelope
    void release();               // frees everything (not set up any more)
    bool ready() const { return m_channels != 0; }
    uint32_t sampleRate() const { return m_rate; }
    uint32_t channels() const { return m_channels; }
    double ceilingDb() const { return m_ceilingDb; }

    std::size_t process(const double* in, std::size_t frames, std::vector<double>& out);
    std::size_t flush(std::vector<double>& out);
    std::size_t latency() const { return m_held; } // frames held in the delay line
    // Most reduction applied in the last block, in dB (<= 0; 0 = none).
    double lastReductionDb() const { return m_lastReductionDb; }
    // True peak (linear) of one interleaved block, as the limiter measures it.
    static double truePeak(const double* in, std::size_t frames, uint32_t channels, uint32_t sampleRate);

private:
    void pushFrame(const double* frame);
    double gainFor(double peak) const;

    uint32_t m_rate = 0, m_channels = 0;
    double m_ceilingDb = -1.0, m_ceiling = 1.0;
    std::size_t m_lookahead = 0;
    int m_phases = kOversample; // 1 at rates >= 352.8 kHz: the samples are the true peak there
    double m_releaseCoef = 0.0;
    // 4x interpolator: per phase, kTapsPerPhase coefficients.
    std::vector<double> m_taps;
    std::vector<double> m_history; // per channel, the last kTapsPerPhase inputs (ring)
    std::size_t m_historyPos = 0;
    // Delay line: m_lookahead frames (ring) and the per-frame target gains.
    std::vector<double> m_delay;
    std::size_t m_delayPos = 0;
    std::size_t m_held = 0;
    std::deque<std::pair<std::size_t, double>> m_minWindow; // (index, gain) monotonic queue
    std::size_t m_index = 0;
    std::vector<double> m_box; // last m_lookahead envelope values (ring)
    std::size_t m_boxPos = 0;
    double m_boxSum = 0.0;
    double m_envelope = 1.0;
    double m_lastReductionDb = 0.0;
};

} // namespace sonkore
