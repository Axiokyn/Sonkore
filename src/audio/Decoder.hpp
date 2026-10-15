#pragma once

#include <climits>
#include <cmath>
#include <cstdint>
#include <filesystem>
#include <memory>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

namespace sonkore {

struct StreamInfo {
    uint32_t sampleRate = 0;
    uint32_t channels = 0;
    uint32_t bitsPerSample = 0; // 0 for lossy sources
    uint64_t totalFrames = 0;
    std::string codec;          // "FLAC", "WAV", "MP3", "ALAC", "AAC", "Opus"...
    bool lossless = true;
    // DSD sent as is (DoP or native words): the samples are not PCM and must
    // reach the device untouched (no DSP, volume or resampling).
    enum class Dsd { None, Dop, Native };
    Dsd dsd = Dsd::None;
    uint32_t dsdRate = 0;       // 1-bit rate (2822400 for DSD64) when dsd != None
    std::string decoder;        // "dr_flac", "FFmpeg", "DSD reader"... (signal path)
};

// Header-level description of a file decoded through FFmpeg (tags, format,
// artwork), for the library scanner. No audio is decoded.
struct ProbeResult {
    StreamInfo stream;
    double durationSeconds = 0.0;
    int bitrateKbps = 0;
    std::vector<std::pair<std::string, std::string>> tags; // UTF-8, keys as stored in the file
    bool hasCover = false;
    std::string cover; // image bytes, when asked for
};

// Uniform PCM source for the engine. Every decoder delivers interleaved,
// left-justified signed 32-bit samples, so integer sources (FLAC/WAV/ALAC/
// AIFF/APE/WavPack...) reach the bit-perfect path untouched.
class Decoder {
public:
    virtual ~Decoder() = default;

    const StreamInfo& info() const { return m_info; }

    // Returns frames read; fewer than requested means end of stream.
    virtual uint64_t readS32(int32_t* out, uint64_t frames) = 0;
    // The same as doubles at 1.0 = full scale, for the DSP path. Integer
    // sources give exactly readS32 / 2^31; float sources (MP3, AAC, Opus,
    // Vorbis, float WAV, DSD converted to PCM) keep what lies above full
    // scale, which readS32 has to clip, so the gain stages can still lower it.
    virtual uint64_t readDouble(double* out, uint64_t frames);
    virtual bool seek(uint64_t frame) = 0;

    // A file, or one track of a CD image ("<sheet>.cue#<number>", see CueSheet).
    static std::unique_ptr<Decoder> open(const std::filesystem::path& path, std::string* error = nullptr);
    static std::unique_ptr<Decoder> openFile(const std::filesystem::path& path, std::string* error = nullptr);

    // Formats this build can play: lower-case extensions without the dot.
    static const std::vector<std::string>& supportedExtensions();
    static bool isSupportedExtension(std::string_view lowerExtension);
    static bool isSupportedExtension(const std::filesystem::path& path);
    // Formats read through FFmpeg (tags and format come from probe()).
    static bool usesFfmpeg(std::string_view lowerExtension);
    static bool probe(const std::filesystem::path& path, ProbeResult& out, bool wantCover);

protected:
    StreamInfo m_info;

private:
    std::vector<int32_t> m_s32Scratch; // readDouble()'s default
};

// A double at 1.0 = full scale to s32, rounded to nearest, clamped.
inline int32_t doubleToS32(double v) noexcept
{
    v *= 2147483648.0;
    return v >= 2147483647.0 ? 2147483647 : v <= -2147483648.0 ? INT32_MIN : static_cast<int32_t>(std::lrint(v));
}

} // namespace sonkore
