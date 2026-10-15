#pragma once

#include <cmath>
#include <cstddef>
#include <cstdint>

namespace sonkore {

// Full-scale reference for left-justified 32-bit PCM (dr_flac's s32 output).
// Scaling by an exact power of two means that int -> float -> int round-trips
// exactly for sources up to 24 bit (float) and 32 bit (double).
inline constexpr double kS32FullScale = 2147483648.0; // 2^31
inline constexpr double kS32InvFullScale = 1.0 / kS32FullScale;

inline void s32ToFloat(const int32_t* in, float* out, std::size_t count) noexcept
{
    for (std::size_t i = 0; i < count; ++i)
        out[i] = static_cast<float>(static_cast<double>(in[i]) * kS32InvFullScale);
}

inline void s32ToDouble(const int32_t* in, double* out, std::size_t count) noexcept
{
    for (std::size_t i = 0; i < count; ++i)
        out[i] = static_cast<double>(in[i]) * kS32InvFullScale;
}

inline void floatToDouble(const float* in, double* out, std::size_t count) noexcept
{
    for (std::size_t i = 0; i < count; ++i)
        out[i] = static_cast<double>(in[i]);
}

// xorshift64* - cheap, allocation-free noise source for TPDF dither.
class DitherRng {
public:
    double uniform() noexcept // [0, 1)
    {
        m_state ^= m_state >> 12;
        m_state ^= m_state << 25;
        m_state ^= m_state >> 27;
        return static_cast<double>((m_state * 0x2545F4914F6CDD1DULL) >> 11) * (1.0 / 9007199254740992.0);
    }

private:
    uint64_t m_state = 0x9E3779B97F4A7C15ULL;
};

// Requantisation dither (see Quantizer).
enum class DitherMode { Off, Tpdf, NoiseShaped };

// Quantises double samples to left-justified s32 on the grid of the device's
// real bit depth (16/24/32), so the downstream truncation performed by
// miniaudio / the driver is exact.
//
// Dither: TPDF (+-1 LSB, flat), or TPDF with F-weighted noise shaping (SoX's
// 9-tap error-feedback filter for 44.1/48 kHz): the requantisation noise is
// moved out of the 1-6 kHz region where the ear is most sensitive. At other
// rates, and on outputs wider than 20 bits (where the flat floor, ~-144 dBFS
// at 24 bits, is already far below any DAC's own noise and shaping would only
// raise it by ~18 dB), noise shaping falls back to flat TPDF. Samples beyond
// full scale are clamped and counted.
class Quantizer {
public:
    static constexpr int kMaxChannels = 8;

    void setTargetBits(int bits) noexcept
    {
        m_bits = (bits <= 0 || bits > 32) ? 32 : bits;
        m_lsb = std::ldexp(1.0, 32 - m_bits); // size of one target LSB in s32 units
    }
    void setFormat(uint32_t sampleRate, uint32_t channels) noexcept
    {
        m_channels = channels == 0 || channels > kMaxChannels ? 2 : channels;
        m_shapingRate = sampleRate == 44100 || sampleRate == 48000;
        reset();
    }
    void setDither(DitherMode mode) noexcept { m_mode = mode; }
    void setDither(bool on) noexcept { m_mode = on ? DitherMode::Tpdf : DitherMode::Off; }
    void reset() noexcept
    {
        for (auto& e : m_error)
            for (double& v : e)
                v = 0.0;
        m_pos = 0;
    }
    // Samples clamped at full scale since the last call (clipping).
    uint64_t takeClipped() noexcept
    {
        const uint64_t n = m_clipped;
        m_clipped = 0;
        return n;
    }

    void process(const double* in, int32_t* out, std::size_t count) noexcept
    {
        const double maxValue = kS32FullScale - m_lsb; // largest on-grid positive value
        const bool dither = m_mode != DitherMode::Off && m_bits < 32;
        const bool shaped = m_mode == DitherMode::NoiseShaped && m_bits <= 20 && m_shapingRate;
        const std::size_t ch = m_channels;
        for (std::size_t i = 0; i < count; ++i) {
            double v = in[i] * kS32FullScale / m_lsb; // in target LSB units
            const std::size_t c = i % ch;
            if (shaped) {
                // w = x - sum(h[k] * e[n-k]); y = Q(w + d); e[n] = y - w
                double fb = 0.0;
                for (int k = 0; k < kTaps; ++k)
                    fb += kFWeighted[k] * m_error[c][(m_pos + kTaps - k) % kTaps];
                v -= fb;
            }
            const double w = v;
            if (dither)
                v += m_rng.uniform() - m_rng.uniform(); // TPDF, +-1 LSB peak
            v = std::nearbyint(v);
            if (shaped) {
                // Clamped (clipping) errors stay bounded so the loop cannot run away.
                const double e = v - w;
                m_error[c][(m_pos + 1) % kTaps] = e > 1.5 ? 1.5 : e < -1.5 ? -1.5 : e;
                if (c == ch - 1)
                    m_pos = (m_pos + 1) % kTaps;
            }
            v *= m_lsb;
            if (v > maxValue) {
                v = maxValue;
                ++m_clipped;
            } else if (v < -kS32FullScale) {
                v = -kS32FullScale;
                ++m_clipped;
            }
            out[i] = static_cast<int32_t>(v);
        }
    }

private:
    static constexpr int kTaps = 9;
    // SoX "f-weighted" error-feedback coefficients (Wannamaker), 44.1 / 48 kHz.
    static constexpr double kFWeighted[kTaps] = {2.412, -3.370, 3.937, -4.174, 3.353, -2.205, 1.281, -0.569, 0.0847};

    DitherRng m_rng;
    int m_bits = 24;
    double m_lsb = 256.0;
    DitherMode m_mode = DitherMode::Tpdf;
    std::size_t m_channels = 2;
    bool m_shapingRate = true;
    double m_error[kMaxChannels][kTaps] = {};
    int m_pos = 0;
    uint64_t m_clipped = 0;
};

} // namespace sonkore
