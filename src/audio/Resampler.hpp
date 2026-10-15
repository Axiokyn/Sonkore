#pragma once

#include <cstddef>
#include <cstdint>
#include <string>
#include <vector>

struct soxr; // libsoxr
struct soxr_quality_spec;

namespace sonkore {

// Sample-rate converter for outputs that refuse the track's rate (WASAPI
// shared mode, DACs without 44.1 kHz...): SoX Resampler "very high quality"
// (linear phase, 175 dB stop band, 28-bit precision), double in and out.
//
// The engine only builds one when it is needed, so a bit-perfect session
// carries no resampler at all. Worker-thread only.
class Resampler {
public:
    // Phase response of the anti-imaging / anti-aliasing filter (all SoX VHQ):
    // linear (symmetric, pre- and post-ringing), intermediate, minimum (no
    // pre-ringing, phase shift near the top of the band), or linear with a
    // steeper transition (flat to 98.2 % of Nyquist, 21.7 kHz at 44.1 kHz,
    // instead of 91.1 %, 20.1 kHz).
    enum class Filter { Linear, Intermediate, Minimum, LinearSteep };

    Resampler() = default;
    ~Resampler();
    Resampler(const Resampler&) = delete;
    Resampler& operator=(const Resampler&) = delete;

    bool prepare(uint32_t inRate, uint32_t outRate, uint32_t channels, std::string* error = nullptr,
                 Filter filter = Filter::Linear);
    // A linear-phase low-pass of its own (same precision): flat to
    // `passbandEnd` and fully attenuated from `stopbandBegin`, both as
    // fractions of the lower rate's Nyquist frequency.
    bool prepareLowpass(uint32_t inRate, uint32_t outRate, uint32_t channels, double passbandEnd, double stopbandBegin,
                        std::string* error = nullptr);
    void release();
    bool active() const { return m_soxr != nullptr; }

    // Converts interleaved frames; the result stays valid until the next call.
    std::size_t process(const double* in, std::size_t frames);
    // End of input: returns what is still buffered, call until it returns 0.
    std::size_t drain();
    const double* output() const { return m_out.data(); }
    void clear();
    // Output frames the converter holds (its filter's delay plus what is
    // buffered): heard later than the input that went in.
    double delayFrames() const;

private:
    bool create(uint32_t inRate, uint32_t outRate, uint32_t channels, const struct ::soxr_quality_spec& quality, std::string* error);
    std::size_t run(const double* in, std::size_t frames);

    soxr* m_soxr = nullptr;
    uint32_t m_channels = 2;
    double m_ratio = 1.0;
    std::vector<double> m_out;
};

} // namespace sonkore
