#include "DstDecoder.hpp"

#include <algorithm>
#include <bit>
#include <cstring>

namespace sonkore {

namespace {

constexpr uint8_t kSilenceByte = 0x69;

// Predictors for the coded filter coefficients and probability tables (10.12,
// 10.13): each coefficient from the one to three before it.
constexpr int8_t kFsetsPred[3][3] = {{-8, 0, 0}, {-16, 8, 0}, {-9, -5, 6}};
constexpr int8_t kProbsPred[3][3] = {{-8, 0, 0}, {-16, 8, 0}, {-24, 24, -8}};

uint8_t reverse7(unsigned c)
{
    unsigned r = 0;
    for (int i = 0; i < 7; ++i)
        r |= ((c >> i) & 1u) << (6 - i);
    return static_cast<uint8_t>(r);
}

} // namespace

// MSB-first bit reader; past the end it returns zeros and remembers it.
class DstDecoder::BitReader {
public:
    BitReader(const uint8_t* data, std::size_t size) : m_data(data), m_bits(size * 8) {}
    unsigned bit()
    {
        if (m_pos >= m_bits) {
            ++m_pos;
            m_overrun = true;
            return 0;
        }
        const unsigned v = (m_data[m_pos >> 3] >> (7 - (m_pos & 7))) & 1u;
        ++m_pos;
        return v;
    }
    unsigned bits(int n)
    {
        unsigned v = 0;
        for (int i = 0; i < n; ++i)
            v = v << 1 | bit();
        return v;
    }
    int sbits(int n)
    {
        const unsigned v = bits(n);
        return v & (1u << (n - 1)) ? static_cast<int>(v) - (1 << n) : static_cast<int>(v);
    }
    // Rice code of the standard's tables: zeros, a one, k bits, then a sign
    // bit when the value is not zero.
    int rice(unsigned k)
    {
        unsigned q = 0;
        while (bit() == 0) {
            if (++q > 4096 || m_overrun)
                return 0;
        }
        int v = static_cast<int>(q << k | bits(static_cast<int>(k)));
        if (v != 0 && bit())
            v = -v;
        return v;
    }
    bool overrun() const { return m_overrun; }
    std::size_t left() const { return m_pos >= m_bits ? 0 : m_bits - m_pos; }

private:
    const uint8_t* m_data;
    std::size_t m_bits, m_pos = 0;
    bool m_overrun = false;
};

// The binary arithmetic decoder of 10.11: a 12-bit interval `a`, the code
// value `c` inside it; p (1..128, in 256ths) is the probability of the
// symbol 0, which takes the top of the interval.
struct DstDecoder::ArithDecoder {
    unsigned a = 4095, c = 0;
    void init(BitReader& gb) { c = gb.bits(12); }
    unsigned get(BitReader& gb, unsigned p)
    {
        const unsigned k = (a >> 8) | ((a >> 7) & 1u);
        const unsigned q = k * p;
        const unsigned aq = a - q;
        unsigned e;
        if (c < aq) {
            e = 1;
            a = aq;
        } else {
            e = 0;
            a = q;
            c -= aq;
        }
        while (a < 2048) {
            a <<= 1;
            c = c << 1 | gb.bit();
        }
        return e;
    }
};

std::size_t DstDecoder::samplesPerFrame(uint32_t dsdRate)
{
    return static_cast<std::size_t>(588) * (dsdRate / 44100);
}

// Which filter (or probability table) each channel uses (10.7-10.9): either
// all the first one, or an index per channel, a new table being announced by
// the first channel that names it.
bool DstDecoder::readMap(BitReader& gb, Table& t, unsigned map[kMaxChannels], uint32_t channels)
{
    t.elements = 1;
    map[0] = 0;
    if (gb.bit()) {
        std::fill_n(map, kMaxChannels, 0u);
        return true;
    }
    for (uint32_t ch = 1; ch < channels; ++ch) {
        const int bits = std::bit_width(t.elements);
        map[ch] = gb.bits(bits);
        if (map[ch] == t.elements) {
            if (++t.elements >= kMaxElements)
                return false;
        } else if (map[ch] > t.elements) {
            return false;
        }
    }
    return true;
}

bool DstDecoder::readTable(BitReader& gb, Table& t, const int8_t pred[3][3], int lengthBits, int coeffBits, bool isSigned,
                           int offset)
{
    for (unsigned i = 0; i < t.elements; ++i) {
        t.length[i] = gb.bits(lengthBits) + 1;
        const auto uncoded = [&](unsigned count) {
            for (unsigned j = 0; j < count; ++j)
                t.coeff[i][j] = (isSigned ? gb.sbits(coeffBits) : static_cast<int>(gb.bits(coeffBits))) + offset;
        };
        if (!gb.bit()) {
            uncoded(t.length[i]);
            continue;
        }
        const unsigned method = gb.bits(2);
        if (method == 3)
            return false;
        uncoded(method + 1);
        const unsigned lsb = gb.bits(3);
        for (unsigned j = method + 1; j < t.length[i]; ++j) {
            int x = 0;
            for (unsigned k = 0; k <= method; ++k)
                x += pred[method][k] * t.coeff[i][j - k - 1];
            int c = gb.rice(lsb);
            c += x >= 0 ? -((x + 4) / 8) : (-x + 3) / 8;
            if (!isSigned && (c < offset || c >= offset + (1 << coeffBits)))
                return false;
            t.coeff[i][j] = c;
        }
    }
    return !gb.overrun();
}

// The prediction of a filter is the sum over its taps of a coefficient with
// the sign of the bit it looks at; tabulated per byte of history, so a
// prediction costs 16 lookups.
bool DstDecoder::buildFilter(const Table& fsets)
{
    m_filter.assign(static_cast<std::size_t>(fsets.elements) * 16 * 256, 0);
    for (unsigned i = 0; i < fsets.elements; ++i) {
        const int length = static_cast<int>(fsets.length[i]);
        for (int j = 0; j < 16; ++j) {
            const int total = std::clamp(length - j * 8, 0, 8);
            int16_t* row = m_filter.data() + (static_cast<std::size_t>(i) * 16 + j) * 256;
            for (int k = 0; k < 256; ++k) {
                int v = 0;
                for (int l = 0; l < total; l++)
                    v += (((k >> l) & 1) * 2 - 1) * fsets.coeff[i][j * 8 + l];
                if (v != static_cast<int16_t>(v))
                    return false;
                row[k] = static_cast<int16_t>(v);
            }
        }
    }
    return true;
}

bool DstDecoder::decode(const uint8_t* data, std::size_t size, uint32_t channels, std::size_t samples, uint8_t* out,
                        std::string* error)
{
    const std::size_t bytesPerChannel = samples / 8;
    const auto fail = [&](const char* why) {
        std::memset(out, kSilenceByte, bytesPerChannel * channels);
        if (error)
            *error = why;
        return false;
    };
    if (channels == 0 || channels > kMaxChannels || samples % 8 != 0 || size < 1)
        return fail("DST frame: unsupported channel count or frame length");
    BitReader gb(data, size);

    if (!gb.bit()) {
        // A frame of plain DSD (10.3): the bytes, one per channel in turn.
        gb.bit();
        if (gb.bits(6) != 0 || size < 1 + bytesPerChannel * channels)
            return fail("DST frame: bad uncompressed frame");
        const uint8_t* p = data + 1;
        for (std::size_t i = 0; i < bytesPerChannel; ++i)
            for (uint32_t ch = 0; ch < channels; ++ch)
                out[ch * bytesPerChannel + i] = *p++;
        return true;
    }

    // Segmentation (10.4-10.6): one segment per channel is what every encoder
    // writes; anything else is not decoded.
    if (!gb.bit() || !gb.bit() || !gb.bit())
        return fail("DST frame: segmented frames are not supported");

    // Mapping (10.7-10.9).
    unsigned fmap[kMaxChannels] = {}, pmap[kMaxChannels] = {};
    const bool sameMap = gb.bit();
    if (!readMap(gb, m_fsets, fmap, channels))
        return fail("DST frame: bad filter map");
    if (sameMap) {
        m_probs.elements = m_fsets.elements;
        std::copy_n(fmap, kMaxChannels, pmap);
    } else if (!readMap(gb, m_probs, pmap, channels)) {
        return fail("DST frame: bad probability map");
    }

    // Half probability (10.10): the first bits of a channel, as many as its
    // filter has taps, coded at 50 %.
    bool halfProb[kMaxChannels] = {};
    for (uint32_t ch = 0; ch < channels; ++ch)
        halfProb[ch] = gb.bit();

    // Filter coefficient sets (10.12) and probability tables (10.13).
    if (!readTable(gb, m_fsets, kFsetsPred, 7, 9, true, 0))
        return fail("DST frame: bad filter coefficients");
    if (!readTable(gb, m_probs, kProbsPred, 6, 7, false, 1))
        return fail("DST frame: bad probability table");

    // Arithmetic coded data (10.11).
    if (gb.bit())
        return fail("DST frame: reserved bit set");
    ArithDecoder ac;
    ac.init(gb);
    if (!buildFilter(m_fsets))
        return fail("DST frame: filter overflow");

    std::memset(m_status, 0xAA, sizeof m_status);
    std::memset(out, 0, bytesPerChannel * channels);
    ac.get(gb, reverse7(static_cast<unsigned>(m_fsets.coeff[0][0]) & 127u) + 1); // DST_X_bit

    for (std::size_t i = 0; i < samples; ++i) {
        for (uint32_t ch = 0; ch < channels; ++ch) {
            const unsigned felem = fmap[ch];
            const int16_t* filter = m_filter.data() + static_cast<std::size_t>(felem) * 16 * 256;
            uint8_t* status = m_status[ch];
            int sum = 0;
            for (int j = 0; j < 16; ++j)
                sum += filter[j * 256 + status[j]];
            const auto predict = static_cast<int16_t>(sum);

            unsigned prob;
            if (!halfProb[ch] || i >= m_fsets.length[felem]) {
                const unsigned pelem = pmap[ch];
                const unsigned index = static_cast<unsigned>(predict < 0 ? -predict : predict) >> 3;
                prob = static_cast<unsigned>(m_probs.coeff[pelem][std::min(index, m_probs.length[pelem] - 1)]);
            } else {
                prob = 128;
            }
            const unsigned residual = ac.get(gb, prob);
            const unsigned v = ((predict < 0 ? 1u : 0u) ^ residual) & 1u;
            out[ch * bytesPerChannel + (i >> 3)] |= static_cast<uint8_t>(v << (7 - (i & 7)));

            // 128 bits of history, the newest in bit 0 of byte 0.
            unsigned carry = v;
            for (int j = 0; j < 16; ++j) {
                const unsigned next = status[j] >> 7;
                status[j] = static_cast<uint8_t>(status[j] << 1 | carry);
                carry = next;
            }
        }
    }
    if (gb.overrun())
        return fail("DST frame: truncated");
    return true;
}

} // namespace sonkore
