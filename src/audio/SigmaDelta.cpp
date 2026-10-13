#include "SigmaDelta.hpp"

#include <algorithm>
#include <cmath>
#include <cstring>

namespace sonkore {

namespace {

// NTF numerator N(z) and denominator D(z), z^0 .. z^-7, for OSR 64 / 128 / 256
// (see the class comment for how they were obtained).
constexpr double kN[3][8] = {
    {1, -6.9961081674428742, 20.980545099289039, -34.961094459511735, 34.961094459511735, -20.980545099289035, 6.9961081674428733, -0.99999999999999967},
    {1, -6.9990269383640342, 20.995134958251274, -34.990270182915815, 34.990270182915815, -20.995134958251274, 6.9990269383640333, -1},
    {1, -6.9997567281220805, 20.99878365726315, -34.997567331178772, 34.997567331178772, -20.998783657263154, 6.9997567281220823, -1.0000000000000002},
};
constexpr double kD[3][8] = {
    {1, -6.1887058310164385, 16.456608463744516, -24.370369228074001, 21.703622969060206, -11.622507739170345, 3.464935820559579, -0.44358025003306856},
    {1, -6.1901570935959294, 16.464165032170779, -24.386816248378793, 21.72277225448348, -11.635084383633133, 3.4693528441548178, -0.4442282496245793},
    {1, -6.1905198898992646, 16.466054388427281, -24.390929123696029, 21.727561609013151, -11.638230339210917, 3.4704578852425549, -0.44439038659614288},
};

constexpr double kDivergence = 8.0; // a stable loop stays within about 1.5

} // namespace

void SigmaDelta::setup(int dsdMultiple, uint32_t channels, DsdPacking packing)
{
    const int set = dsdMultiple >= 256 ? 2 : dsdMultiple >= 128 ? 1 : 0;
    for (int k = 0; k < kOrder; ++k) {
        m_fb[k] = kN[set][k + 1] - kD[set][k + 1];
        m_d[k] = kD[set][k + 1];
    }
    m_channels = channels;
    m_packing = packing;
    m_bitsPerWord = packing == DsdPacking::Dop ? 16 : 32;
    m_loops.assign(channels, Loop{});
    m_filled = 0;
    m_resets = 0;
}

void SigmaDelta::reset()
{
    m_loops.assign(m_channels, Loop{});
    m_filled = 0;
}

// Bits (oldest first, MSB) to the device word, as DsdDecoder packs a file.
int32_t SigmaDelta::pack(uint32_t bits) const
{
    uint8_t b[4];
    if (m_packing == DsdPacking::Dop) {
        const uint32_t word = 0x05000000u | (bits & 0xFFFF) << 8; // the output stamps the real marker
        return static_cast<int32_t>(word);
    }
    for (int i = 0; i < 4; ++i)
        b[i] = static_cast<uint8_t>(bits >> (24 - 8 * i));
    if (m_packing == DsdPacking::NativeU32LE)
        std::reverse(b, b + 4);
    int32_t v;
    std::memcpy(&v, b, 4);
    return v;
}

std::size_t SigmaDelta::process(const double* in, std::size_t frames, int32_t* out)
{
    std::size_t written = 0;
    for (std::size_t f = 0; f < frames; ++f) {
        for (uint32_t c = 0; c < m_channels; ++c) {
            Loop& L = m_loops[c];
            const double u = std::clamp(in[f * m_channels + c] * kInputScale, -kInputLimit, kInputLimit);
            double r = 0.0;
            for (int k = 0; k < kOrder; ++k)
                r += m_fb[k] * L.e[k] - m_d[k] * L.r[k];
            const double w = u + r;
            const double y = w >= 0.0 ? 1.0 : -1.0;
            if (std::fabs(w) > kDivergence || !std::isfinite(w)) {
                L.e.fill(0.0); // overloaded: start the loop again
                L.r.fill(0.0);
                ++m_resets;
            } else {
                std::copy_backward(L.e.begin(), L.e.end() - 1, L.e.end());
                std::copy_backward(L.r.begin(), L.r.end() - 1, L.r.end());
                L.e[0] = y - w;
                L.r[0] = r;
            }
            L.bits = L.bits << 1 | (y > 0.0 ? 1u : 0u);
        }
        if (++m_filled == m_bitsPerWord) {
            for (uint32_t c = 0; c < m_channels; ++c)
                out[written * m_channels + c] = pack(m_loops[c].bits);
            ++written;
            m_filled = 0;
        }
    }
    return written;
}

std::size_t SigmaDelta::flush(int32_t* out)
{
    if (m_filled == 0)
        return 0;
    for (uint32_t c = 0; c < m_channels; ++c) {
        uint32_t bits = m_loops[c].bits;
        for (std::size_t i = m_filled; i < m_bitsPerWord; ++i)
            bits = bits << 1 | ((0x69u >> (7 - (i % 8))) & 1u); // the rest is DSD idle pattern
        out[c] = pack(bits);
    }
    m_filled = 0;
    return 1;
}

} // namespace sonkore
