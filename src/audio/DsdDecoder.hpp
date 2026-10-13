#pragma once

#include "Decoder.hpp"
#include "DstDecoder.hpp"

#include <cstdio>
#include <memory>

namespace sonkore {

// How the 1-bit stream reaches the DAC.
enum class DsdPacking {
    Dop,         // DSD over PCM: 16 bits per channel in a 24-bit sample, 0x05/0xFA marker
    NativeU32BE, // ALSA DSD_U32_BE: 32 bits per channel, oldest bit first
    NativeU32LE, // ALSA DSD_U32_LE: the same 4 bytes in reverse order
};

// Reads DSF and DSDIFF, DST-compressed DSDIFF included (decoded frame by
// frame with DstDecoder), and hands the bitstream to the engine untouched,
// packed for the DAC. The samples it produces are not PCM: the engine sends
// them straight to the device.
class DsdDecoder : public Decoder {
public:
    struct Format {
        uint32_t dsdRate = 0;       // 2822400 for DSD64
        uint32_t channels = 0;
        uint64_t bytesPerChannel = 0;
    };
    static bool isDsdExtension(const std::filesystem::path& path);
    // Header only. False when the file is not DSD (or DST with more channels
    // than the decoder takes).
    static bool probe(const std::filesystem::path& path, Format& out);
    static std::unique_ptr<DsdDecoder> open(const std::filesystem::path& path, DsdPacking packing, std::string* error);
    // Frame rate of the packed stream: DSD rate / 16 (DoP) or / 32 (native).
    static uint32_t frameRate(uint32_t dsdRate, DsdPacking packing);
    // One sample of DSD silence (the 0x69 idle pattern) in this packing; for
    // DoP, `phase` picks the marker (false = 0x05, true = 0xFA).
    static int32_t silence(DsdPacking packing, bool phase);
    // "DSD64", "DSD128"...
    static std::string rateLabel(uint32_t dsdRate);

    ~DsdDecoder() override;
    uint64_t readS32(int32_t* out, uint64_t frames) override;
    bool seek(uint64_t frame) override;

    DsdPacking packing() const { return m_packing; }
    // DoP marker of the next frame: carried across a gapless join so the DAC
    // never sees two equal markers in a row.
    bool dopPhase() const { return m_phase; }
    void setDopPhase(bool phase) { m_phase = phase; }

private:
    DsdDecoder() = default;
    bool readBytes(uint64_t first, std::size_t count); // fills m_bytes (count per channel)

    std::FILE* m_file = nullptr;
    DsdPacking m_packing = DsdPacking::Dop;
    Format m_format;
    bool m_dsf = false;
    bool m_lsbFirst = false;      // DSF with 1 bit per sample: bit order reversed in each byte
    uint64_t m_dataStart = 0;
    uint32_t m_blockSize = 0;     // DSF: bytes per channel per block
    uint64_t m_position = 0;      // frames
    bool m_phase = false;
    std::vector<uint8_t> m_bytes; // per channel, channel after channel
    std::vector<uint8_t> m_scratch;

    // DST: the frames (1/75 s each) are found as they are needed, through
    // the file's index (DSTI) when it has one or by walking the frame chunks,
    // and decoded on demand; the last two decoded frames are kept.
    bool m_dst = false;
    std::size_t m_frameBytes = 0;         // per channel
    uint64_t m_dstEnd = 0;                // end of the DST chunk's body
    uint64_t m_scanPos = 0;               // the next frame chunk not yet indexed
    std::vector<uint64_t> m_frameOffsets; // each frame's data in the file
    std::vector<uint32_t> m_frameSizes;
    std::unique_ptr<DstDecoder> m_dstDecoder;
    struct Cached {
        uint64_t index = ~uint64_t(0);
        std::vector<uint8_t> bytes;       // per channel, channel after channel
    };
    Cached m_cache[2];
    unsigned m_cacheNext = 0;
    std::vector<uint8_t> m_packet;
    bool m_dstError = false;              // a frame failed to decode (reported once)
    bool loadIndex(uint64_t offset, uint64_t size, uint32_t frames);
    bool locateFrame(uint64_t index, uint64_t& offset, uint32_t& size);
    const uint8_t* dstFrame(uint64_t index); // null: silence (missing or broken)
};

} // namespace sonkore
