#include "SigmaDelta.hpp"

#include <algorithm>
#include <cmath>
#include <cstring>

namespace sonkore {

namespace {

// NTF numerator N(z) and denominator D(z), z^0 .. z^-8, for OSR 64 / 128 /
// 256 / 512 (see the class comment for how they were obtained).
constexpr double kN[4][9] = {
    {1, -7.99550277716134, 27.973022913961728, -55.93256665865651, 69.91009304371242, -55.93256665865652, 27.973022913961728, -7.99550277716134, 0.9999999999999998},
    {1, -7.998875573589071, 27.99325383229928, -55.983135166852676, 69.97751381628494, -55.983135166852676, 27.99325383229928, -7.99887557358907, 0.9999999999999998},
    {1, -7.999718885852985, 27.99831333954193, -55.995783385490185, 69.99437786360247, -55.995783385490185, 27.998313339541934, -7.999718885852987, 1.0},
    {1, -7.999929720991721, 27.999578327476847, -55.99894582098188, 69.99859442899351, -55.99894582098189, 27.999578327476854, -7.999929720991721, 1.0},
};
constexpr double kD[4][9] = {
    {1, -7.188018653886824, 22.64220562070373, -40.82087305319691, 46.067024930450735, -33.32089641893887, 15.084749485485242, -3.9076376140463376, 0.4434459698181196},
    {1, -7.189698064780384, 22.652621453416984, -40.848614520270345, 46.10815184956516, -33.357546121936366, 15.104379832951356, -3.9134887952392416, 0.44419462851719965},
    {1, -7.1901178941445245, 22.655225692998858, -40.8555517339486, 46.11843789208569, -33.36671375074137, 15.109290913235506, -3.9149528363122164, 0.4443819780187692},
    {1, -7.190222850024472, 22.655876770472716, -40.85728615278703, 46.12100967228441, -33.36900598320961, 15.110518901725289, -3.915318924479541, 0.4444288269528381},
};

constexpr double kDivergence = 8.0; // a stable loop stays within about 1.9

} // namespace

SigmaDelta::~SigmaDelta()
{
    stopWorkers();
}

void SigmaDelta::setup(int dsdMultiple, uint32_t channels, DsdPacking packing)
{
    stopWorkers();
    const int set = dsdMultiple >= 512 ? 3 : dsdMultiple >= 256 ? 2 : dsdMultiple >= 128 ? 1 : 0;
    for (int k = 0; k < kOrder; ++k) {
        m_fb[k] = kN[set][k + 1] - kD[set][k + 1];
        m_d[k] = kD[set][k + 1];
    }
    m_channels = channels;
    m_packing = packing;
    m_bitsPerWord = packing == DsdPacking::Dop ? 16 : 32;
    m_loops.assign(channels, Loop{});
    m_filled = 0;

    // The channels after the first on their own threads.
    m_quit = false;
    m_generation = 0;
    m_pending = 0;
    for (uint32_t c = 1; c < channels; ++c)
        m_workers.emplace_back([this, c] {
            uint64_t seen = 0;
            for (;;) {
                const double* in;
                std::size_t frames;
                {
                    std::unique_lock lock(m_mutex);
                    m_wake.wait(lock, [&] { return m_quit || m_generation != seen; });
                    if (m_quit)
                        return;
                    seen = m_generation;
                    in = m_jobIn;
                    frames = m_jobFrames;
                }
                modulate(c, in, frames);
                {
                    std::lock_guard lock(m_mutex);
                    if (--m_pending == 0)
                        m_done.notify_one();
                }
            }
        });
}

void SigmaDelta::stopWorkers()
{
    {
        std::lock_guard lock(m_mutex);
        m_quit = true;
    }
    m_wake.notify_all();
    for (std::thread& t : m_workers)
        t.join();
    m_workers.clear();
}

void SigmaDelta::reset()
{
    for (Loop& L : m_loops) {
        L.e.fill(0.0);
        L.r.fill(0.0);
        L.bits = 0;
    }
    m_filled = 0;
}

uint64_t SigmaDelta::resets() const
{
    uint64_t n = 0;
    for (const Loop& L : m_loops)
        n += L.resets;
    return n;
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

void SigmaDelta::modulate(uint32_t c, const double* in, std::size_t frames)
{
    Loop& L = m_loops[c];
    L.words.clear();
    const uint32_t ch = m_channels;
    const std::size_t bitsPerWord = m_bitsPerWord;
    std::size_t filled = m_filled;
    uint32_t bits = L.bits;
    // The histories in rings written twice (at p and p + kOrder), so the
    // newest-first window is always the contiguous run [p, p + kOrder): no
    // shifting per sample, and the feedback is a plain dot product.
    alignas(32) double he[2 * kOrder], hr[2 * kOrder];
    for (int k = 0; k < kOrder; ++k) {
        he[k] = he[k + kOrder] = L.e[k];
        hr[k] = hr[k + kOrder] = L.r[k];
    }
    const double* fbc = m_fb.data();
    const double* dc = m_d.data();
    int p = 0;
    uint64_t resets = 0;
    for (std::size_t f = 0; f < frames; ++f) {
        const double u = std::clamp(in[f * ch + c] * kInputScale, -kInputLimit, kInputLimit);
        const double* e = he + p;
        const double* r = hr + p;
        // Summed as a tree: the sixteen products are independent, so the
        // chain the next sample has to wait for is four additions deep
        // instead of sixteen (the loop is serial from sample to sample).
        const double fb = (((fbc[0] * e[0] - dc[0] * r[0]) + (fbc[1] * e[1] - dc[1] * r[1]))
                           + ((fbc[2] * e[2] - dc[2] * r[2]) + (fbc[3] * e[3] - dc[3] * r[3])))
                        + (((fbc[4] * e[4] - dc[4] * r[4]) + (fbc[5] * e[5] - dc[5] * r[5]))
                           + ((fbc[6] * e[6] - dc[6] * r[6]) + (fbc[7] * e[7] - dc[7] * r[7])));
        static_assert(kOrder == 8, "the feedback sum is written out for eight taps");
        const double w = u + fb;
        const double y = w >= 0.0 ? 1.0 : -1.0;
        if (std::fabs(w) > kDivergence || !std::isfinite(w)) {
            std::fill(he, he + 2 * kOrder, 0.0); // overloaded: start the loop again
            std::fill(hr, hr + 2 * kOrder, 0.0);
            ++resets;
        } else {
            p = (p + kOrder - 1) % kOrder;
            he[p] = he[p + kOrder] = y - w;
            hr[p] = hr[p + kOrder] = fb;
        }
        bits = bits << 1 | (y > 0.0 ? 1u : 0u);
        if (++filled == bitsPerWord) {
            L.words.push_back(pack(bits));
            filled = 0;
        }
    }
    for (int k = 0; k < kOrder; ++k) {
        L.e[k] = he[p + k];
        L.r[k] = hr[p + k];
    }
    L.bits = bits;
    L.resets += resets;
}

std::size_t SigmaDelta::process(const double* in, std::size_t frames, int32_t* out)
{
    const uint32_t ch = m_channels;
    if (ch == 0 || frames == 0)
        return 0;
    if (ch > 1) {
        {
            std::lock_guard lock(m_mutex);
            m_jobIn = in;
            m_jobFrames = frames;
            m_pending = ch - 1;
            ++m_generation;
        }
        m_wake.notify_all();
        modulate(0, in, frames);
        std::unique_lock lock(m_mutex);
        m_done.wait(lock, [&] { return m_pending == 0; });
    } else {
        modulate(0, in, frames);
    }
    const std::size_t written = m_loops[0].words.size();
    for (uint32_t c = 0; c < ch; ++c)
        for (std::size_t w = 0; w < written; ++w)
            out[w * ch + c] = m_loops[c].words[w];
    m_filled = (m_filled + frames) % m_bitsPerWord;
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
