#include "Decoder.hpp"
#include "CueSheet.hpp"

#if defined(SONKORE_HAVE_FFMPEG)
#include "FfmpegDecoder.hpp"
#endif

#include <dr_flac.h>
#include <dr_mp3.h>
#include <dr_wav.h>

#include <algorithm>
#include <cctype>
#include <vector>

namespace sonkore {

namespace {

std::string lowerExtension(const std::filesystem::path& path)
{
    std::string ext = path.extension().string();
    std::transform(ext.begin(), ext.end(), ext.begin(), [](unsigned char c) { return std::tolower(c); });
    return ext;
}

class FlacDecoder final : public Decoder {
public:
    explicit FlacDecoder(drflac* flac)
        : m_flac(flac)
    {
        m_info = {flac->sampleRate, flac->channels, flac->bitsPerSample, flac->totalPCMFrameCount, "FLAC", true,
                  StreamInfo::Dsd::None, 0, "dr_flac"};
    }
    ~FlacDecoder() override { drflac_close(m_flac); }

    uint64_t readS32(int32_t* out, uint64_t frames) override
    {
        return drflac_read_pcm_frames_s32(m_flac, frames, out);
    }
    bool seek(uint64_t frame) override { return drflac_seek_to_pcm_frame(m_flac, frame); }

private:
    drflac* m_flac;
};

class WavDecoder final : public Decoder {
public:
    WavDecoder() = default;
    ~WavDecoder() override
    {
        if (m_open)
            drwav_uninit(&m_wav);
    }

    bool open(const std::filesystem::path& path)
    {
#if defined(_WIN32)
        m_open = drwav_init_file_w(&m_wav, path.c_str(), nullptr);
#else
        m_open = drwav_init_file(&m_wav, path.c_str(), nullptr);
#endif
        if (!m_open)
            return false;
        const bool isFloat = m_wav.translatedFormatTag == DR_WAVE_FORMAT_IEEE_FLOAT;
        m_info = {m_wav.sampleRate, m_wav.channels, isFloat ? 0u : m_wav.bitsPerSample,
                  m_wav.totalPCMFrameCount, "WAV", !isFloat || m_wav.bitsPerSample <= 32, StreamInfo::Dsd::None, 0, "dr_wav"};
        return true;
    }

    uint64_t readS32(int32_t* out, uint64_t frames) override
    {
        return drwav_read_pcm_frames_s32(&m_wav, frames, out);
    }
    bool seek(uint64_t frame) override { return drwav_seek_to_pcm_frame(&m_wav, frame); }

private:
    drwav m_wav{};
    bool m_open = false;
};

class Mp3Decoder final : public Decoder {
public:
    Mp3Decoder() = default;
    ~Mp3Decoder() override
    {
        if (m_open)
            drmp3_uninit(&m_mp3);
    }

    bool open(const std::filesystem::path& path)
    {
#if defined(_WIN32)
        m_open = drmp3_init_file_w(&m_mp3, path.c_str(), nullptr);
#else
        m_open = drmp3_init_file(&m_mp3, path.c_str(), nullptr);
#endif
        if (!m_open)
            return false;
        const uint64_t total = drmp3_get_pcm_frame_count(&m_mp3);
        drmp3_seek_to_pcm_frame(&m_mp3, 0);
        m_info = {m_mp3.sampleRate, m_mp3.channels, 0, total, "MP3", false, StreamInfo::Dsd::None, 0, "dr_mp3"};
        return true;
    }

    uint64_t readS32(int32_t* out, uint64_t frames) override
    {
        const std::size_t samples = static_cast<std::size_t>(frames) * m_info.channels;
        if (m_scratch.size() < samples)
            m_scratch.resize(samples);
        const uint64_t got = drmp3_read_pcm_frames_f32(&m_mp3, frames, m_scratch.data());
        const std::size_t n = static_cast<std::size_t>(got) * m_info.channels;
        for (std::size_t i = 0; i < n; ++i) {
            const double v = static_cast<double>(m_scratch[i]) * 2147483648.0;
            out[i] = static_cast<int32_t>(std::clamp(v, -2147483648.0, 2147483647.0));
        }
        return got;
    }
    bool seek(uint64_t frame) override { return drmp3_seek_to_pcm_frame(&m_mp3, frame); }

private:
    drmp3 m_mp3{};
    bool m_open = false;
    std::vector<float> m_scratch;
};

} // namespace

const std::vector<std::string>& Decoder::supportedExtensions()
{
    static const std::vector<std::string> list = [] {
        std::vector<std::string> v = {"flac", "wav", "wave", "mp3"};
#if defined(SONKORE_HAVE_FFMPEG)
        const auto& extra = ffmpeg::extensions();
        v.insert(v.end(), extra.begin(), extra.end());
#endif
        return v;
    }();
    return list;
}

bool Decoder::isSupportedExtension(std::string_view lowerExtension)
{
    const auto& list = supportedExtensions();
    return std::find(list.begin(), list.end(), lowerExtension) != list.end();
}

bool Decoder::isSupportedExtension(const std::filesystem::path& path)
{
    const std::string ext = lowerExtension(path);
    return !ext.empty() && isSupportedExtension(std::string_view(ext).substr(1));
}

bool Decoder::usesFfmpeg(std::string_view lowerExtension)
{
#if defined(SONKORE_HAVE_FFMPEG)
    const auto& list = ffmpeg::extensions();
    return std::find(list.begin(), list.end(), lowerExtension) != list.end();
#else
    (void)lowerExtension;
    return false;
#endif
}

bool Decoder::probe(const std::filesystem::path& path, ProbeResult& out, bool wantCover)
{
#if defined(SONKORE_HAVE_FFMPEG)
    return ffmpeg::probe(path, out, wantCover);
#else
    (void)path, (void)out, (void)wantCover;
    return false;
#endif
}

namespace {

// One track of a CD image: the stretch [start, end) of the image file, seen
// as a file of its own. Consecutive tracks share their boundary, so playing
// them in turn (gapless) is the image without a missing or doubled sample.
class RangeDecoder : public Decoder {
public:
    RangeDecoder(std::unique_ptr<Decoder> inner, uint64_t start, uint64_t end)
        : m_inner(std::move(inner))
        , m_start(start)
    {
        m_info = m_inner->info();
        const uint64_t total = m_info.totalFrames;
        const uint64_t last = end > 0 && (total == 0 || end < total) ? end : total;
        m_info.totalFrames = last > start ? last - start : 0;
        m_inner->seek(start);
    }
    uint64_t readS32(int32_t* out, uint64_t frames) override
    {
        frames = std::min(frames, m_info.totalFrames - std::min(m_info.totalFrames, m_position));
        const uint64_t got = frames ? m_inner->readS32(out, frames) : 0;
        m_position += got;
        return got;
    }
    bool seek(uint64_t frame) override
    {
        m_position = std::min(frame, m_info.totalFrames);
        return m_inner->seek(m_start + m_position);
    }

private:
    std::unique_ptr<Decoder> m_inner;
    uint64_t m_start = 0;
    uint64_t m_position = 0;
};

// CD frames (1/75 s) to sample frames; exact at 44.1 kHz (588 per CD frame).
uint64_t cdToFrames(uint64_t cd, uint32_t rate)
{
    return (cd * rate + 37) / 75;
}

} // namespace

std::unique_ptr<Decoder> Decoder::open(const std::filesystem::path& path, std::string* error)
{
    std::filesystem::path sheetPath;
    int number = 0;
    if (CueSheet::splitTrackPath(path.string(), sheetPath, number)) {
        CueSheet sheet;
        const CueSheet::Track* track = CueSheet::parse(sheetPath, sheet) ? sheet.track(number) : nullptr;
        if (!track) {
            if (error)
                *error = "Track " + std::to_string(number) + " not found in " + sheetPath.filename().string();
            return nullptr;
        }
        auto inner = openFile(track->file, error);
        if (!inner)
            return nullptr;
        const uint32_t rate = inner->info().sampleRate;
        return std::make_unique<RangeDecoder>(std::move(inner), cdToFrames(track->start, rate),
                                              track->end ? cdToFrames(track->end, rate) : 0);
    }
    return openFile(path, error);
}

std::unique_ptr<Decoder> Decoder::openFile(const std::filesystem::path& path, std::string* error)
{
    const std::string ext = lowerExtension(path);
    auto fail = [&](const char* what) -> std::unique_ptr<Decoder> {
        if (error)
            *error = std::string(what) + ": " + path.filename().string();
        return nullptr;
    };
    // dr_libs first for their formats; FFmpeg (when built in) also takes the
    // odd FLAC/WAV/MP3 they cannot read (WAV with ADPCM, FLAC in odd
    // containers...).
    auto fallback = [&](const char* what) -> std::unique_ptr<Decoder> {
#if defined(SONKORE_HAVE_FFMPEG)
        if (auto dec = ffmpeg::open(path, nullptr))
            return dec;
#endif
        return fail(what);
    };

    if (ext == ".flac") {
#if defined(_WIN32)
        drflac* flac = drflac_open_file_w(path.c_str(), nullptr);
#else
        drflac* flac = drflac_open_file(path.c_str(), nullptr);
#endif
        if (!flac)
            return fallback("Unreadable FLAC file");
        return std::make_unique<FlacDecoder>(flac);
    }
    if (ext == ".wav" || ext == ".wave") {
        auto dec = std::make_unique<WavDecoder>();
        if (!dec->open(path))
            return fallback("Unreadable WAV file");
        return dec;
    }
    if (ext == ".mp3") {
        auto dec = std::make_unique<Mp3Decoder>();
        if (!dec->open(path))
            return fallback("Unreadable MP3 file");
        return dec;
    }
#if defined(SONKORE_HAVE_FFMPEG)
    if (usesFfmpeg(std::string_view(ext).substr(ext.empty() ? 0 : 1)))
        return ffmpeg::open(path, error);
#endif
    return fail("Unsupported format");
}

} // namespace sonkore
