#include "TestSignals.hpp"

#include "SigmaDelta.hpp"
#if defined(SONKORE_HAVE_FFMPEG)
#include "FfmpegDecoder.hpp"
#endif

#include <algorithm>
#include <cmath>
#include <cstdio>
#include <cstring>
#include <numbers>

namespace sonkore::testsignals {

namespace {

std::FILE* create(const std::filesystem::path& path, std::string* error)
{
    std::error_code ec;
    std::filesystem::create_directories(path.parent_path(), ec);
#if defined(_WIN32)
    std::FILE* f = _wfopen(path.c_str(), L"wb");
#else
    std::FILE* f = std::fopen(path.c_str(), "wb");
#endif
    if (!f && error)
        *error = "Could not write " + path.string();
    return f;
}

void putLe(std::vector<uint8_t>& out, uint64_t v, int bytes)
{
    for (int i = 0; i < bytes; ++i)
        out.push_back(static_cast<uint8_t>(v >> (8 * i)));
}

// The WAV channel mask for the usual layouts (front left, front right, centre,
// LFE, back left, back right, side left, side right).
uint32_t channelMask(uint32_t channels)
{
    switch (channels) {
    case 1: return 0x4;
    case 2: return 0x3;
    case 4: return 0x33;
    case 6: return 0x3F;
    case 8: return 0x63F;
    default: return 0;
    }
}

bool writeWavHeader(std::vector<uint8_t>& out, uint32_t rate, uint32_t channels, int bits, uint64_t dataBytes)
{
    const bool extensible = channels > 2 || bits > 16;
    const uint32_t blockAlign = channels * static_cast<uint32_t>(bits / 8);
    const uint32_t fmtSize = extensible ? 40 : 16;
    out.clear();
    out.insert(out.end(), {'R', 'I', 'F', 'F'});
    putLe(out, 4 + 8 + fmtSize + 8 + dataBytes, 4);
    out.insert(out.end(), {'W', 'A', 'V', 'E', 'f', 'm', 't', ' '});
    putLe(out, fmtSize, 4);
    putLe(out, extensible ? 0xFFFE : 1, 2); // WAVE_FORMAT_EXTENSIBLE / PCM
    putLe(out, channels, 2);
    putLe(out, rate, 4);
    putLe(out, static_cast<uint64_t>(rate) * blockAlign, 4);
    putLe(out, blockAlign, 2);
    putLe(out, static_cast<uint64_t>(bits), 2);
    if (extensible) {
        putLe(out, 22, 2);
        putLe(out, static_cast<uint64_t>(bits), 2);
        putLe(out, channelMask(channels), 4);
        // KSDATAFORMAT_SUBTYPE_PCM
        static const uint8_t pcmGuid[16] = {0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00,
                                            0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71};
        out.insert(out.end(), pcmGuid, pcmGuid + 16);
    }
    out.insert(out.end(), {'d', 'a', 't', 'a'});
    putLe(out, dataBytes, 4);
    return true;
}

// 10 ms raised-cosine fades at both ends of [start, start + frames).
double fadeGain(std::size_t i, std::size_t frames, uint32_t rate)
{
    const std::size_t fade = std::max<std::size_t>(1, rate / 100);
    if (i < fade)
        return 0.5 - 0.5 * std::cos(std::numbers::pi * static_cast<double>(i) / static_cast<double>(fade));
    if (frames - i <= fade)
        return 0.5 - 0.5 * std::cos(std::numbers::pi * static_cast<double>(frames - i) / static_cast<double>(fade));
    return 1.0;
}

} // namespace

bool writeWav(const std::filesystem::path& path, uint32_t rate, uint32_t channels, int bits, const std::vector<double>& samples,
              std::string* error)
{
    if (bits != 16 && bits != 24) {
        if (error)
            *error = "WAV: 16 or 24 bits";
        return false;
    }
    std::vector<uint8_t> out;
    writeWavHeader(out, rate, channels, bits, samples.size() * static_cast<std::size_t>(bits / 8));
    const double full = bits == 16 ? 32768.0 : 8388608.0;
    for (double s : samples) {
        const double v = std::clamp(std::round(s * full), -full, full - 1.0);
        putLe(out, static_cast<uint64_t>(static_cast<int64_t>(v)), bits / 8);
    }
    std::FILE* f = create(path, error);
    if (!f)
        return false;
    const bool ok = std::fwrite(out.data(), 1, out.size(), f) == out.size();
    std::fclose(f);
    if (!ok && error)
        *error = "Could not write " + path.string();
    return ok;
}

bool writeWavBytes(const std::filesystem::path& path, uint32_t rate, const std::vector<uint8_t>& bytes, std::string* error)
{
    std::vector<uint8_t> out;
    writeWavHeader(out, rate, 2, 16, bytes.size());
    out.insert(out.end(), bytes.begin(), bytes.end());
    if (out.size() & 1)
        out.push_back(0);
    std::FILE* f = create(path, error);
    if (!f)
        return false;
    const bool ok = std::fwrite(out.data(), 1, out.size(), f) == out.size();
    std::fclose(f);
    if (!ok && error)
        *error = "Could not write " + path.string();
    return ok;
}

bool writeTone(const std::filesystem::path& path, uint32_t rate, uint32_t channels, double seconds, double freqHz, double dbfs,
               std::string* error)
{
    const auto frames = static_cast<std::size_t>(rate * seconds);
    const double amp = std::pow(10.0, dbfs / 20.0);
    std::vector<double> samples(frames * channels);
    for (std::size_t i = 0; i < frames; ++i) {
        const double v = amp * fadeGain(i, frames, rate) * std::sin(2.0 * std::numbers::pi * freqHz * static_cast<double>(i) / rate);
        for (uint32_t c = 0; c < channels; ++c)
            samples[i * channels + c] = v;
    }
    return writeWav(path, rate, channels, 24, samples, error);
}

bool writeChannelSequence(const std::filesystem::path& path, uint32_t rate, uint32_t channels, double freqHz, double dbfs,
                          std::string* error)
{
    const std::size_t on = rate, gap = rate * 3 / 10; // 1 s of tone, 0.3 s of silence
    const double amp = std::pow(10.0, dbfs / 20.0);
    std::vector<double> samples(static_cast<std::size_t>(channels) * (on + gap) * channels, 0.0);
    for (uint32_t c = 0; c < channels; ++c) {
        const std::size_t start = c * (on + gap);
        // The LFE gets a low tone, as a subwoofer would not reproduce 1 kHz.
        const double f = (channels >= 6 && c == 3) ? 60.0 : freqHz;
        for (std::size_t i = 0; i < on; ++i)
            samples[(start + i) * channels + c] = amp * fadeGain(i, on, rate) * std::sin(2.0 * std::numbers::pi * f * static_cast<double>(i) / rate);
    }
    return writeWav(path, rate, channels, 24, samples, error);
}

// DSF (Sony): "DSD " + "fmt " + "data" chunks, little-endian, the samples in
// blocks of 4096 bytes per channel, MSB first.
bool writeDsdTone(const std::filesystem::path& path, int dsdMultiple, uint32_t channels, double seconds, double freqHz,
                  double dbfs, std::string* error)
{
    const uint32_t rate = 44100u * static_cast<uint32_t>(dsdMultiple);
    const auto frames = static_cast<std::size_t>(rate * seconds);
    const double amp = std::pow(10.0, dbfs / 20.0);
    SigmaDelta modulator;
    modulator.setup(dsdMultiple, channels, DsdPacking::NativeU32BE);

    const std::size_t block = 65536;
    std::vector<double> in(block * channels);
    std::vector<int32_t> words((block / 32 + 2) * channels);
    std::vector<std::vector<uint8_t>> chan(channels);
    for (std::size_t done = 0; done < frames; done += block) {
        const std::size_t n = std::min(block, frames - done);
        for (std::size_t i = 0; i < n; ++i) {
            const double v = amp * fadeGain(done + i, frames, rate) * std::sin(2.0 * std::numbers::pi * freqHz * static_cast<double>(done + i) / rate);
            for (uint32_t c = 0; c < channels; ++c)
                in[i * channels + c] = v;
        }
        std::size_t got = modulator.process(in.data(), n, words.data());
        if (done + n >= frames)
            got += modulator.flush(words.data() + got * channels);
        for (std::size_t w = 0; w < got; ++w)
            for (uint32_t c = 0; c < channels; ++c) {
                uint8_t b[4];
                std::memcpy(b, &words[w * channels + c], 4);
                chan[c].insert(chan[c].end(), b, b + 4); // the word's bytes are already oldest first
            }
    }
    const std::size_t bytesPerChannel = chan[0].size();
    const std::size_t blocks = (bytesPerChannel + 4095) / 4096;
    for (auto& c : chan)
        c.resize(blocks * 4096, 0x69);

    std::vector<uint8_t> out;
    const uint64_t dataSize = 12 + static_cast<uint64_t>(blocks) * 4096 * channels;
    const uint64_t total = 28 + 52 + dataSize;
    out.insert(out.end(), {'D', 'S', 'D', ' '});
    putLe(out, 28, 8);
    putLe(out, total, 8);
    putLe(out, 0, 8); // no metadata
    out.insert(out.end(), {'f', 'm', 't', ' '});
    putLe(out, 52, 8);
    putLe(out, 1, 4); // format version
    putLe(out, 0, 4); // DSD raw
    putLe(out, channels == 1 ? 1 : channels == 2 ? 2 : channels == 6 ? 6 : channels == 4 ? 4 : 7, 4);
    putLe(out, channels, 4);
    putLe(out, rate, 4);
    putLe(out, 8, 4); // bits per sample: 8 = MSB first
    putLe(out, static_cast<uint64_t>(bytesPerChannel) * 8, 8);
    putLe(out, 4096, 4);
    putLe(out, 0, 4);
    out.insert(out.end(), {'d', 'a', 't', 'a'});
    putLe(out, dataSize, 8);
    for (std::size_t b = 0; b < blocks; ++b)
        for (uint32_t c = 0; c < channels; ++c)
            out.insert(out.end(), chan[c].begin() + static_cast<std::ptrdiff_t>(b * 4096), chan[c].begin() + static_cast<std::ptrdiff_t>((b + 1) * 4096));

    std::FILE* f = create(path, error);
    if (!f)
        return false;
    const bool ok = std::fwrite(out.data(), 1, out.size(), f) == out.size();
    std::fclose(f);
    if (!ok && error)
        *error = "Could not write " + path.string();
    return ok;
}

bool dtsAvailable()
{
#if defined(SONKORE_HAVE_FFMPEG)
    return ffmpeg::dtsEncoderAvailable();
#else
    return false;
#endif
}

bool writeDtsTest(const std::filesystem::path& path, std::string* error)
{
#if defined(SONKORE_HAVE_FFMPEG)
    // 1 kHz at -20 dBFS: 2 s on the left, 2 s on the right, 2 s on both.
    const uint32_t rate = 44100;
    const std::size_t part = rate * 2, frames = part * 3;
    std::vector<int32_t> pcm(frames * 2, 0);
    const double amp = 0.1 * 2147483647.0;
    for (std::size_t i = 0; i < frames; ++i) {
        const double v = amp * fadeGain(i % part, part, rate) * std::sin(2.0 * std::numbers::pi * 1000.0 * static_cast<double>(i) / rate);
        const std::size_t section = i / part;
        if (section != 1)
            pcm[i * 2] = static_cast<int32_t>(v);
        if (section != 0)
            pcm[i * 2 + 1] = static_cast<int32_t>(v);
    }
    std::vector<uint8_t> stream;
    if (!ffmpeg::encodeDts(pcm.data(), frames, 2, rate, stream, error))
        return false;
    return writeWavBytes(path, rate, stream, error);
#else
    (void)path;
    if (error)
        *error = "Built without FFmpeg";
    return false;
#endif
}

const char* channelName(uint32_t channels, uint32_t index)
{
    static const char* const stereo[] = {"Left", "Right"};
    static const char* const quad[] = {"Front left", "Front right", "Back left", "Back right"};
    static const char* const surround[] = {"Front left", "Front right", "Centre", "LFE (subwoofer)",
                                           "Back left", "Back right", "Side left", "Side right"};
    if (channels == 1)
        return "Mono";
    if (channels == 2 && index < 2)
        return stereo[index];
    if (channels == 4 && index < 4)
        return quad[index];
    if (index < 8)
        return surround[index];
    return "";
}

} // namespace sonkore::testsignals
