#pragma once

#include <cstddef>
#include <cstdint>
#include <string>
#include <vector>

namespace sonkore {

// Decoder for DST (Direct Stream Transfer, ISO/IEC 14496-3 subpart 10), the
// lossless coding of DSD on SACDs and in DSDIFF files: each frame (1/75 s)
// carries, for every channel, a prediction filter of up to 128 taps over the
// previous bits and a table of probabilities indexed by the prediction's
// confidence, and the bits as arithmetic-coded residuals of the filter's
// guess. The frame comes out as the DSD it was made from, bit for bit.
//
// Written after the standard's description and the layout FFmpeg's dstdec
// reads (every SACD rip uses one segment per channel; a frame with more is
// refused rather than guessed at). One instance per stream; not thread-safe.
class DstDecoder {
public:
    static constexpr uint32_t kMaxChannels = 6;
    static constexpr uint32_t kMaxElements = 2 * kMaxChannels; // tables a frame may define

    // Bits per channel in one frame at a DSD rate (588 * rate / 44100).
    static std::size_t samplesPerFrame(uint32_t dsdRate);

    // Decodes one frame of `size` bytes into `out`: `channels` runs of
    // samplesPerFrame / 8 bytes, MSB first (the oldest bit first). False
    // (with `error`) when the frame is malformed; `out` then holds DSD
    // silence.
    bool decode(const uint8_t* data, std::size_t size, uint32_t channels, std::size_t samples, uint8_t* out,
                std::string* error = nullptr);

private:
    struct Table {
        unsigned elements = 1;
        unsigned length[kMaxElements] = {};
        int coeff[kMaxElements][128] = {};
    };
    class BitReader;
    struct ArithDecoder;
    bool readMap(BitReader& gb, Table& t, unsigned map[kMaxChannels], uint32_t channels);
    bool readTable(BitReader& gb, Table& t, const int8_t pred[3][3], int lengthBits, int coeffBits, bool isSigned, int offset);
    bool buildFilter(const Table& fsets);

    Table m_fsets, m_probs;
    // For each filter, history byte and byte value: the sum of the filter's
    // coefficients with the sign of each bit.
    std::vector<int16_t> m_filter; // [element][16][256]
    uint8_t m_status[kMaxChannels][16] = {};
};

} // namespace sonkore
