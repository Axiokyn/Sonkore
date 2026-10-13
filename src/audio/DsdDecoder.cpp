#include "DsdDecoder.hpp"

#include <algorithm>
#include <array>
#include <cstring>

namespace sonkore {

namespace {

constexpr uint8_t kSilenceByte = 0x69; // DSD idle pattern: 0 V average, no click
constexpr uint8_t kDopMarkers[2] = {0x05, 0xFA};

std::FILE* openStream(const std::filesystem::path& path)
{
#if defined(_WIN32)
    return _wfopen(path.c_str(), L"rb");
#else
    return std::fopen(path.c_str(), "rb");
#endif
}

bool seekTo(std::FILE* f, uint64_t offset)
{
#if defined(_WIN32)
    return _fseeki64(f, static_cast<long long>(offset), SEEK_SET) == 0;
#else
    return fseeko(f, static_cast<off_t>(offset), SEEK_SET) == 0;
#endif
}

bool readExact(std::FILE* f, void* dst, std::size_t n)
{
    return std::fread(dst, 1, n, f) == n;
}

uint64_t le(const uint8_t* p, int n)
{
    uint64_t v = 0;
    for (int i = n - 1; i >= 0; --i)
        v = (v << 8) | p[i];
    return v;
}

uint64_t be(const uint8_t* p, int n)
{
    uint64_t v = 0;
    for (int i = 0; i < n; ++i)
        v = (v << 8) | p[i];
    return v;
}

uint8_t reverseBits(uint8_t b)
{
    b = static_cast<uint8_t>((b & 0xF0) >> 4 | (b & 0x0F) << 4);
    b = static_cast<uint8_t>((b & 0xCC) >> 2 | (b & 0x33) << 2);
    return static_cast<uint8_t>((b & 0xAA) >> 1 | (b & 0x55) << 1);
}

int32_t fromBytes(const std::array<uint8_t, 4>& b)
{
    int32_t v;
    std::memcpy(&v, b.data(), 4); // memory order is what the device receives
    return v;
}

struct Layout {
    DsdDecoder::Format format;
    bool dsf = false;
    bool lsbFirst = false;
    uint64_t dataStart = 0;
    uint32_t blockSize = 0;
};

// DSF (Sony): little-endian chunks "DSD ", "fmt ", "data"; per channel blocks
// of `blockSize` bytes, LSB-first when the file stores 1 bit per sample.
bool parseDsf(std::FILE* f, Layout& out)
{
    uint8_t h[28 + 52 + 12];
    if (!seekTo(f, 0) || !readExact(f, h, sizeof h))
        return false;
    if (std::memcmp(h, "DSD ", 4) != 0 || std::memcmp(h + 28, "fmt ", 4) != 0)
        return false;
    const uint8_t* fmt = h + 28 + 12;
    const uint64_t fmtSize = le(h + 28 + 4, 8);
    if (le(fmt + 4, 4) != 0) // format id 0: DSD raw
        return false;
    const uint32_t channels = static_cast<uint32_t>(le(fmt + 12, 4));
    const uint32_t rate = static_cast<uint32_t>(le(fmt + 16, 4));
    const uint32_t bits = static_cast<uint32_t>(le(fmt + 20, 4));
    const uint64_t samples = le(fmt + 24, 8);
    const uint32_t block = static_cast<uint32_t>(le(fmt + 32, 4));
    const uint64_t dataChunk = 28 + fmtSize;
    uint8_t d[12];
    if (!seekTo(f, dataChunk) || !readExact(f, d, sizeof d) || std::memcmp(d, "data", 4) != 0)
        return false;
    if (channels == 0 || channels > 8 || rate < 2822400 || block == 0 || (bits != 1 && bits != 8))
        return false;
    out.dsf = true;
    out.lsbFirst = bits == 1;
    out.dataStart = dataChunk + 12;
    out.blockSize = block;
    out.format = {rate, channels, samples / 8};
    return true;
}

// DSDIFF (Philips): big-endian "FRM8"/"DSD " form; "PROP"/"SND " holds the
// rate ("FS  "), channels ("CHNL") and compression ("CMPR"); the "DSD "
// chunk holds the samples, one byte per channel in turn, MSB first.
bool parseDff(std::FILE* f, Layout& out)
{
    uint8_t h[16];
    if (!seekTo(f, 0) || !readExact(f, h, sizeof h) || std::memcmp(h, "FRM8", 4) != 0 || std::memcmp(h + 12, "DSD ", 4) != 0)
        return false;
    const uint64_t end = 12 + be(h + 4, 8);
    uint32_t rate = 0, channels = 0;
    bool compressed = false;
    uint64_t pos = 16;
    while (pos + 12 <= end) {
        uint8_t c[12];
        if (!seekTo(f, pos) || !readExact(f, c, sizeof c))
            return false;
        const uint64_t size = be(c + 4, 8);
        const uint64_t body = pos + 12;
        if (std::memcmp(c, "PROP", 4) == 0) {
            uint64_t sub = body + 4; // after "SND "
            while (sub + 12 <= body + size) {
                uint8_t s[12 + 4];
                if (!seekTo(f, sub) || !readExact(f, s, sizeof s))
                    return false;
                const uint64_t subSize = be(s + 4, 8);
                if (std::memcmp(s, "FS  ", 4) == 0)
                    rate = static_cast<uint32_t>(be(s + 12, 4));
                else if (std::memcmp(s, "CHNL", 4) == 0)
                    channels = static_cast<uint32_t>(be(s + 12, 2));
                else if (std::memcmp(s, "CMPR", 4) == 0)
                    compressed = std::memcmp(s + 12, "DSD ", 4) != 0;
                sub += 12 + subSize + (subSize & 1);
            }
        } else if (std::memcmp(c, "DSD ", 4) == 0) {
            if (channels == 0 || channels > 8 || rate < 2822400 || compressed)
                return false;
            out.dsf = false;
            out.lsbFirst = false;
            out.dataStart = body;
            out.format = {rate, channels, size / channels};
            return true;
        } else if (std::memcmp(c, "DST ", 4) == 0) {
            return false;
        }
        pos = body + size + (size & 1);
    }
    return false;
}

bool parse(std::FILE* f, Layout& out)
{
    return parseDsf(f, out) || parseDff(f, out);
}

} // namespace

bool DsdDecoder::isDsdExtension(const std::filesystem::path& path)
{
    std::string ext = path.extension().string();
    std::transform(ext.begin(), ext.end(), ext.begin(), [](unsigned char c) { return static_cast<char>(std::tolower(c)); });
    return ext == ".dsf" || ext == ".dff";
}

bool DsdDecoder::probe(const std::filesystem::path& path, Format& out)
{
    std::FILE* f = openStream(path);
    if (!f)
        return false;
    Layout layout;
    const bool ok = parse(f, layout);
    std::fclose(f);
    if (ok)
        out = layout.format;
    return ok;
}

uint32_t DsdDecoder::frameRate(uint32_t dsdRate, DsdPacking packing)
{
    return packing == DsdPacking::Dop ? dsdRate / 16 : dsdRate / 32;
}

int32_t DsdDecoder::silence(DsdPacking packing, bool phase)
{
    if (packing == DsdPacking::Dop)
        return static_cast<int32_t>(static_cast<uint32_t>(kDopMarkers[phase ? 1 : 0]) << 24 | kSilenceByte << 16 | kSilenceByte << 8);
    return fromBytes({kSilenceByte, kSilenceByte, kSilenceByte, kSilenceByte});
}

std::string DsdDecoder::rateLabel(uint32_t dsdRate)
{
    return "DSD" + std::to_string((dsdRate + 22050) / 44100);
}

std::unique_ptr<DsdDecoder> DsdDecoder::open(const std::filesystem::path& path, DsdPacking packing, std::string* error)
{
    std::unique_ptr<DsdDecoder> d(new DsdDecoder);
    d->m_file = openStream(path);
    Layout layout;
    if (!d->m_file || !parse(d->m_file, layout)) {
        if (error)
            *error = "Not an uncompressed DSF / DSDIFF file";
        return nullptr;
    }
    d->m_packing = packing;
    d->m_format = layout.format;
    d->m_dsf = layout.dsf;
    d->m_lsbFirst = layout.lsbFirst;
    d->m_dataStart = layout.dataStart;
    d->m_blockSize = layout.blockSize;

    const uint32_t bytesPerFrame = packing == DsdPacking::Dop ? 2 : 4;
    StreamInfo& info = d->m_info;
    info.sampleRate = frameRate(layout.format.dsdRate, packing);
    info.channels = layout.format.channels;
    info.bitsPerSample = packing == DsdPacking::Dop ? 24 : 32;
    info.totalFrames = (layout.format.bytesPerChannel + bytesPerFrame - 1) / bytesPerFrame;
    info.lossless = true;
    info.dsdRate = layout.format.dsdRate;
    info.dsd = packing == DsdPacking::Dop ? StreamInfo::Dsd::Dop : StreamInfo::Dsd::Native;
    info.codec = rateLabel(layout.format.dsdRate) + (packing == DsdPacking::Dop ? " DoP" : " native");
    info.decoder = "DSD reader";
    return d;
}

DsdDecoder::~DsdDecoder()
{
    if (m_file)
        std::fclose(m_file);
}

// Bytes [first, first + count) of every channel into m_bytes, in time order,
// MSB first; past the end of the stream they are DSD silence.
bool DsdDecoder::readBytes(uint64_t first, std::size_t count)
{
    const uint32_t ch = m_format.channels;
    m_bytes.assign(static_cast<std::size_t>(ch) * count, kSilenceByte);
    const uint64_t total = m_format.bytesPerChannel;
    const std::size_t valid = first >= total ? 0 : static_cast<std::size_t>(std::min<uint64_t>(count, total - first));

    if (m_dsf) {
        // Channel blocks: [ch0 block][ch1 block]... per group of blockSize bytes.
        std::size_t done = 0;
        while (done < valid) {
            const uint64_t at = first + done;
            const uint64_t group = at / m_blockSize;
            const std::size_t offset = static_cast<std::size_t>(at % m_blockSize);
            const std::size_t n = std::min<std::size_t>(valid - done, m_blockSize - offset);
            for (uint32_t c = 0; c < ch; ++c) {
                const uint64_t pos = m_dataStart + (group * ch + c) * m_blockSize + offset;
                if (!seekTo(m_file, pos) || !readExact(m_file, m_bytes.data() + c * count + done, n))
                    return false;
            }
            done += n;
        }
        if (m_lsbFirst)
            for (uint32_t c = 0; c < ch; ++c)
                for (std::size_t i = 0; i < valid; ++i)
                    m_bytes[c * count + i] = reverseBits(m_bytes[c * count + i]);
        return true;
    }
    // DSDIFF: one byte per channel in turn.
    m_scratch.resize(valid * ch);
    if (valid > 0 && (!seekTo(m_file, m_dataStart + first * ch) || !readExact(m_file, m_scratch.data(), m_scratch.size())))
        return false;
    for (std::size_t i = 0; i < valid; ++i)
        for (uint32_t c = 0; c < ch; ++c)
            m_bytes[c * count + i] = m_scratch[i * ch + c];
    return true;
}

uint64_t DsdDecoder::readS32(int32_t* out, uint64_t frames)
{
    frames = std::min(frames, m_info.totalFrames - std::min(m_info.totalFrames, m_position));
    if (frames == 0)
        return 0;
    const uint32_t ch = m_format.channels;
    const std::size_t width = m_packing == DsdPacking::Dop ? 2 : 4;
    const std::size_t count = static_cast<std::size_t>(frames) * width;
    if (!readBytes(m_position * width, count))
        return 0;

    for (std::size_t f = 0; f < frames; ++f) {
        for (uint32_t c = 0; c < ch; ++c) {
            const uint8_t* b = m_bytes.data() + c * count + f * width;
            int32_t& s = out[f * ch + c];
            switch (m_packing) {
            case DsdPacking::Dop:
                s = static_cast<int32_t>(static_cast<uint32_t>(kDopMarkers[m_phase ? 1 : 0]) << 24
                                         | static_cast<uint32_t>(b[0]) << 16 | static_cast<uint32_t>(b[1]) << 8);
                break;
            case DsdPacking::NativeU32BE:
                s = fromBytes({b[0], b[1], b[2], b[3]});
                break;
            case DsdPacking::NativeU32LE:
                s = fromBytes({b[3], b[2], b[1], b[0]});
                break;
            }
        }
        m_phase = !m_phase;
    }
    m_position += frames;
    return frames;
}

bool DsdDecoder::seek(uint64_t frame)
{
    m_position = std::min(frame, m_info.totalFrames);
    m_phase = false; // the engine's pre-roll before it ends on 0xFA
    return true;
}

} // namespace sonkore
