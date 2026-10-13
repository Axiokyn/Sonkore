#pragma once

#include <cstddef>
#include <cstdint>
#include <filesystem>
#include <memory>
#include <string>
#include <vector>

namespace sonkore {

// An impulse response as read from its file, before it is fitted to a track.
struct ImpulseResponse {
    std::filesystem::path path;
    uint32_t sampleRate = 0;
    uint32_t channels = 0;               // 1 (all channels), 2 (one per channel) or 4 (true stereo: LL LR RL RR)
    std::vector<std::vector<double>> taps; // per IR channel

    // WAV / FLAC / any format the decoders read. Up to 2^20 taps.
    static bool load(const std::filesystem::path& path, ImpulseResponse& out, std::string* error);
};

// FIR convolution in double precision: uniformly partitioned overlap-save
// (FFT size 2 x 1024), so filters of tens of thousands of taps (room or
// headphone correction from REW, Dirac, Acourate...) run cheaply with 1024
// frames of latency. Built for one sample rate and channel count; the
// impulse response is resampled (SoX VHQ, gain kept) when its rate differs.
// Worker-thread only once built.
class Convolver {
public:
    static constexpr std::size_t kBlock = 1024;

    // False (with a reason) when the response does not fit the stream.
    static std::unique_ptr<Convolver> create(const ImpulseResponse& ir, uint32_t sampleRate, uint32_t channels,
                                             std::string* error);
    ~Convolver();

    // Interleaved frames in place; the output is the input convolved, delayed
    // by kBlock frames.
    void process(double* samples, std::size_t frames);
    void reset();
    // A new single-channel response of the same length, for every channel:
    // the input already seen is kept, and the next block crossfades from the
    // old filter's output to the new one's (no gap, no click). False when the
    // length differs.
    bool replaceTaps(const std::vector<double>& taps);

    std::size_t taps() const { return m_taps; }
    std::size_t latency() const { return kBlock; }
    // Frames still owed after the input ends (delay plus the filter's tail).
    std::size_t tail() const { return kBlock + m_taps; }
    bool resampled() const { return m_resampledFrom != 0; }
    uint32_t resampledFrom() const { return m_resampledFrom; }

private:
    Convolver() = default;
    void step(); // one block through every output channel

    struct Impl;
    std::unique_ptr<Impl> d;
    uint32_t m_channels = 0;
    std::size_t m_taps = 0;
    uint32_t m_resampledFrom = 0;
};

} // namespace sonkore
