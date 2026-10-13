#include "FfmpegDecoder.hpp"
#include "DsdDecoder.hpp"

extern "C" {
#include <libavcodec/avcodec.h>
#include <libavformat/avformat.h>
#include <libavutil/avutil.h>
#include <libavutil/channel_layout.h>
#include <libavutil/mathematics.h>
}

#include <algorithm>
#include <cctype>
#include <cmath>
#include <cstring>

#if defined(_WIN32)
#include <windows.h>
#else
#include <dlfcn.h>
#endif

namespace sonkore::ffmpeg {

namespace {

// --- Run-time loading ------------------------------------------------------------
// FFmpeg is opened on first use instead of being linked: a desktop build of
// libavcodec pulls in a couple of hundred libraries (video codecs, TLS, SVG...)
// that would otherwise sit in memory while FLAC/WAV/MP3 are all that plays.

#define SONKORE_AVUTIL(X) X(av_log_set_level) X(av_dict_set) X(av_dict_free) X(av_dict_get) \
    X(av_get_packed_sample_fmt) X(av_sample_fmt_is_planar) X(av_get_bytes_per_sample) \
    X(av_rescale_q) X(av_rescale) X(av_frame_alloc) X(av_frame_free) X(av_frame_unref) \
    X(av_frame_get_buffer) X(av_channel_layout_default)
#define SONKORE_AVCODEC(X) X(avcodec_descriptor_get) X(avcodec_get_name) X(avcodec_alloc_context3) \
    X(avcodec_parameters_to_context) X(avcodec_open2) X(avcodec_free_context) X(avcodec_receive_frame) \
    X(avcodec_send_packet) X(avcodec_flush_buffers) X(av_packet_alloc) X(av_packet_free) X(av_packet_unref) \
    X(avcodec_find_encoder_by_name) X(avcodec_send_frame) X(avcodec_receive_packet)
#define SONKORE_AVFORMAT(X) X(avformat_open_input) X(avformat_find_stream_info) X(av_find_best_stream) \
    X(av_read_frame) X(av_seek_frame) X(avformat_close_input)

struct Api {
#define SONKORE_DECLARE(name) decltype(&::name) name = nullptr;
    SONKORE_AVUTIL(SONKORE_DECLARE)
    SONKORE_AVCODEC(SONKORE_DECLARE)
    SONKORE_AVFORMAT(SONKORE_DECLARE)
#undef SONKORE_DECLARE
};

void* openLibrary(const char* name, int major)
{
#if defined(_WIN32)
    const std::string file = std::string(name) + "-" + std::to_string(major) + ".dll";
    return reinterpret_cast<void*>(LoadLibraryA(file.c_str()));
#elif defined(__APPLE__)
    const std::string file = "lib" + std::string(name) + "." + std::to_string(major) + ".dylib";
    return dlopen(file.c_str(), RTLD_NOW | RTLD_LOCAL);
#else
    const std::string file = "lib" + std::string(name) + ".so." + std::to_string(major);
    return dlopen(file.c_str(), RTLD_NOW | RTLD_LOCAL);
#endif
}

void* symbol(void* library, const char* name)
{
#if defined(_WIN32)
    return reinterpret_cast<void*>(GetProcAddress(static_cast<HMODULE>(library), name));
#else
    return dlsym(library, name);
#endif
}

// The FFmpeg functions, or nullptr when the libraries are not installed.
// Loaded once and kept for the life of the process.
const Api* api()
{
    static const Api* loaded = []() -> const Api* {
        static Api a;
        void* util = openLibrary("avutil", LIBAVUTIL_VERSION_MAJOR);
        void* codec = util ? openLibrary("avcodec", LIBAVCODEC_VERSION_MAJOR) : nullptr;
        void* format = codec ? openLibrary("avformat", LIBAVFORMAT_VERSION_MAJOR) : nullptr;
        if (!format)
            return nullptr;
        bool ok = true;
#define SONKORE_RESOLVE(library, name) \
        a.name = reinterpret_cast<decltype(a.name)>(symbol(library, #name)); \
        ok = ok && a.name;
#define SONKORE_FROM_UTIL(name) SONKORE_RESOLVE(util, name)
#define SONKORE_FROM_CODEC(name) SONKORE_RESOLVE(codec, name)
#define SONKORE_FROM_FORMAT(name) SONKORE_RESOLVE(format, name)
        SONKORE_AVUTIL(SONKORE_FROM_UTIL)
        SONKORE_AVCODEC(SONKORE_FROM_CODEC)
        SONKORE_AVFORMAT(SONKORE_FROM_FORMAT)
#undef SONKORE_FROM_FORMAT
#undef SONKORE_FROM_CODEC
#undef SONKORE_FROM_UTIL
#undef SONKORE_RESOLVE
        if (!ok)
            return nullptr;
        a.av_log_set_level(AV_LOG_ERROR);
        return &a;
    }();
    return loaded;
}

// Only valid once api() returned non-null (checked by open() and probe()).
const Api& ff()
{
    return *api();
}

// "file:" + UTF-8 path: never mistaken for another protocol, and FFmpeg
// takes UTF-8 file names on every platform (Windows included).
std::string urlFor(const std::filesystem::path& path)
{
    const std::u8string u8 = path.u8string();
    return "file:" + std::string(u8.begin(), u8.end());
}

// Opens the container, local files only (a playlist-like file cannot make
// FFmpeg reach the network).
AVFormatContext* openInput(const std::filesystem::path& path)
{
    AVFormatContext* format = nullptr;
    AVDictionary* options = nullptr;
    ff().av_dict_set(&options, "protocol_whitelist", "file", 0);
    const int r = ff().avformat_open_input(&format, urlFor(path).c_str(), nullptr, &options);
    ff().av_dict_free(&options);
    return r < 0 ? nullptr : format;
}

bool isIntegerFormat(AVSampleFormat f)
{
    switch (ff().av_get_packed_sample_fmt(f)) {
    case AV_SAMPLE_FMT_U8:
    case AV_SAMPLE_FMT_S16:
    case AV_SAMPLE_FMT_S32:
    case AV_SAMPLE_FMT_S64:
        return true;
    default:
        return false;
    }
}

// Codecs that can be lossless (WavPack and DTS are flagged lossy too, for
// their hybrid / core modes); with integer output they reach the device as is.
bool isLosslessCodec(AVCodecID id)
{
    const AVCodecDescriptor* d = ff().avcodec_descriptor_get(id);
    return d && (d->props & AV_CODEC_PROP_LOSSLESS);
}

bool isPcm(AVCodecID id)
{
    return id >= AV_CODEC_ID_PCM_S16LE && id < AV_CODEC_ID_ADPCM_IMA_QT;
}

bool isDsd(AVCodecID id)
{
    return id == AV_CODEC_ID_DSD_LSBF || id == AV_CODEC_ID_DSD_MSBF || id == AV_CODEC_ID_DSD_LSBF_PLANAR
        || id == AV_CODEC_ID_DSD_MSBF_PLANAR;
}

std::string upper(std::string s)
{
    std::transform(s.begin(), s.end(), s.begin(), [](unsigned char c) { return std::toupper(c); });
    return s;
}

// Short label shown in the library ("ALAC", "Opus", "DSD128"...).
std::string codecLabel(AVCodecID id, const AVFormatContext* format, int sampleRate)
{
    switch (id) {
    case AV_CODEC_ID_ALAC: return "ALAC";
    case AV_CODEC_ID_AAC:
    case AV_CODEC_ID_AAC_LATM: return "AAC";
    case AV_CODEC_ID_OPUS: return "Opus";
    case AV_CODEC_ID_VORBIS: return "Vorbis";
    case AV_CODEC_ID_FLAC: return "FLAC";
    case AV_CODEC_ID_MP3: return "MP3";
    case AV_CODEC_ID_MP2: return "MP2";
    case AV_CODEC_ID_APE: return "APE";
    case AV_CODEC_ID_WAVPACK: return "WavPack";
    case AV_CODEC_ID_TTA: return "TTA";
    case AV_CODEC_ID_TAK: return "TAK";
    case AV_CODEC_ID_MUSEPACK7:
    case AV_CODEC_ID_MUSEPACK8: return "MPC";
    case AV_CODEC_ID_WMALOSSLESS: return "WMA Lossless";
    case AV_CODEC_ID_WMAPRO: return "WMA Pro";
    case AV_CODEC_ID_WMAV1:
    case AV_CODEC_ID_WMAV2: return "WMA";
    case AV_CODEC_ID_AC3: return "AC-3";
    case AV_CODEC_ID_EAC3: return "E-AC-3";
    case AV_CODEC_ID_DTS: return "DTS";
    case AV_CODEC_ID_SPEEX: return "Speex";
    default: break;
    }
    if (isDsd(id)) {
        // FFmpeg decodes DSD to PCM at 1/8 of the bit rate (DSD64 -> 352.8 kHz).
        const long multiple = std::lround(sampleRate * 8.0 / 44100.0);
        return multiple > 0 ? "DSD" + std::to_string(multiple) : "DSD";
    }
    if (isPcm(id)) {
        const std::string container = format && format->iformat ? format->iformat->name : "";
        if (container.find("aiff") != std::string::npos)
            return "AIFF";
        if (container == "w64")
            return "W64";
        if (container == "caf")
            return "CAF";
        return "PCM";
    }
    return upper(ff().avcodec_get_name(id));
}

uint32_t bitsOf(const AVCodecParameters* par, AVSampleFormat decoded)
{
    if (decoded != AV_SAMPLE_FMT_NONE && !isIntegerFormat(decoded))
        return 0;
    if (!isLosslessCodec(par->codec_id))
        return 0;
    int bits = par->bits_per_raw_sample > 0 ? par->bits_per_raw_sample : par->bits_per_coded_sample;
    if (decoded != AV_SAMPLE_FMT_NONE)
        bits = std::min(bits > 0 ? bits : 32, ff().av_get_bytes_per_sample(decoded) * 8);
    return static_cast<uint32_t>(std::clamp(bits, 0, 32));
}

uint64_t streamFrames(const std::filesystem::path& path, const AVFormatContext* format, const AVStream* st, int sampleRate)
{
    if (sampleRate <= 0)
        return 0;
    // DSF / DSDIFF headers give the exact length, which FFmpeg's DSF demuxer
    // does not pass on (8.x only estimates it from the bit rate). Its DSD
    // decoders turn each byte of a channel into one sample.
    DsdDecoder::Format dsd;
    if (DsdDecoder::isDsdExtension(path) && DsdDecoder::probe(path, dsd) && dsd.dsdRate / 8 == static_cast<uint32_t>(sampleRate))
        return dsd.bytesPerChannel;
    if (st->duration != AV_NOPTS_VALUE && st->duration > 0) {
        auto frames = static_cast<uint64_t>(ff().av_rescale_q(st->duration, st->time_base, AVRational{1, sampleRate}));
        // Ogg counts the decoder's pre-skip (Opus) in its length; the decoder
        // drops those samples, so they are never played.
        const auto padding = static_cast<uint64_t>(std::max(0, st->codecpar->initial_padding));
        if (std::strcmp(format->iformat->name, "ogg") == 0 && padding < frames)
            frames -= padding;
        return frames;
    }
    if (format->duration != AV_NOPTS_VALUE && format->duration > 0)
        return static_cast<uint64_t>(ff().av_rescale(format->duration, sampleRate, AV_TIME_BASE));
    return 0;
}

// --- Decoder --------------------------------------------------------------------

class FfmpegDecoder final : public Decoder {
public:
    ~FfmpegDecoder() override
    {
        ff().av_frame_free(&m_frame);
        ff().av_packet_free(&m_packet);
        ff().avcodec_free_context(&m_codec);
        ff().avformat_close_input(&m_format);
    }

    bool open(const std::filesystem::path& path, std::string* error)
    {
        auto fail = [&](const std::string& what) {
            if (error)
                *error = what + ": " + path.filename().string();
            return false;
        };
        m_format = openInput(path);
        if (!m_format)
            return fail("Unreadable file");
        if (ff().avformat_find_stream_info(m_format, nullptr) < 0)
            return fail("Unreadable file");

        const AVCodec* codec = nullptr;
        m_stream = ff().av_find_best_stream(m_format, AVMEDIA_TYPE_AUDIO, -1, -1, &codec, 0);
        if (m_stream < 0)
            return fail("No audio in file");
        if (!codec)
            return fail("No decoder for this format in FFmpeg");
        // Only the audio stream is demuxed (no cover art / video packets).
        for (unsigned i = 0; i < m_format->nb_streams; ++i)
            if (static_cast<int>(i) != m_stream)
                m_format->streams[i]->discard = AVDISCARD_ALL;

        const AVStream* st = m_format->streams[m_stream];
        m_codec = ff().avcodec_alloc_context3(codec);
        if (!m_codec || ff().avcodec_parameters_to_context(m_codec, st->codecpar) < 0)
            return fail("Unreadable file");
        m_codec->thread_count = 1;
        if (ff().avcodec_open2(m_codec, codec, nullptr) < 0)
            return fail("Could not open the decoder");
        m_packet = ff().av_packet_alloc();
        m_frame = ff().av_frame_alloc();
        if (!m_packet || !m_frame)
            return fail("Out of memory");

        const int rate = m_codec->sample_rate;
        const int channels = m_codec->ch_layout.nb_channels;
        if (rate <= 0 || channels <= 0)
            return fail("Unsupported channel layout or sample rate");
        m_timeBase = st->time_base;
        m_startPts = st->start_time != AV_NOPTS_VALUE ? st->start_time : 0;

        m_info.sampleRate = static_cast<uint32_t>(rate);
        m_info.channels = static_cast<uint32_t>(channels);
        m_info.bitsPerSample = bitsOf(st->codecpar, m_codec->sample_fmt);
        m_info.totalFrames = streamFrames(path, m_format, st, rate);
        m_info.codec = codecLabel(st->codecpar->codec_id, m_format, rate);
        m_info.decoder = "FFmpeg";
        m_info.lossless = m_info.bitsPerSample > 0;
        // MP4 states the exact length (edit list): stop there so the AAC
        // encoder's padding is not played (gapless albums).
        m_trimToLength = m_info.totalFrames > 0 && std::strstr(m_format->iformat->name, "mp4") != nullptr;
        return true;
    }

    uint64_t readS32(int32_t* out, uint64_t frames) override
    {
        const std::size_t ch = m_info.channels;
        if (m_trimToLength)
            frames = std::min(frames, m_info.totalFrames - std::min(m_info.totalFrames, m_position));
        uint64_t done = 0;
        while (done < frames) {
            if (m_pendingPos >= m_pending.size()) {
                if (!decodeNext())
                    break;
                continue;
            }
            const std::size_t availableFrames = (m_pending.size() - m_pendingPos) / ch;
            const std::size_t n = static_cast<std::size_t>(std::min<uint64_t>(frames - done, availableFrames));
            std::memcpy(out + done * ch, m_pending.data() + m_pendingPos, n * ch * sizeof(int32_t));
            m_pendingPos += n * ch;
            done += n;
        }
        m_position += done;
        return done;
    }

    bool seek(uint64_t frame) override
    {
        const int64_t ts = ff().av_rescale_q(static_cast<int64_t>(frame), AVRational{1, static_cast<int>(m_info.sampleRate)},
                                        m_timeBase) + m_startPts;
        if (ff().av_seek_frame(m_format, m_stream, ts, AVSEEK_FLAG_BACKWARD) < 0)
            return false;
        ff().avcodec_flush_buffers(m_codec);
        m_pending.clear();
        m_pendingPos = 0;
        m_inputDone = false;
        // The demuxer lands on a packet at or before the target: decode from
        // there and drop what precedes it, so seeks are sample-accurate.
        m_skipTo = frame;
        m_skipping = true;
        m_position = frame;
        return true;
    }

private:
    // Decodes until at least one frame of audio is pending. False at the end.
    bool decodeNext()
    {
        m_pending.clear();
        m_pendingPos = 0;
        for (;;) {
            const int r = ff().avcodec_receive_frame(m_codec, m_frame);
            if (r == 0) {
                append(m_frame);
                ff().av_frame_unref(m_frame);
                if (!m_pending.empty())
                    return true;
                continue;
            }
            if (r != AVERROR(EAGAIN) || m_inputDone)
                return false; // end of stream (or an unrecoverable error)

            if (ff().av_read_frame(m_format, m_packet) < 0) {
                ff().avcodec_send_packet(m_codec, nullptr); // drain the decoder
                m_inputDone = true;
                continue;
            }
            if (m_packet->stream_index == m_stream)
                ff().avcodec_send_packet(m_codec, m_packet); // a corrupt packet is skipped
            ff().av_packet_unref(m_packet);
        }
    }

    // Appends one decoded frame as interleaved left-justified s32.
    void append(const AVFrame* f)
    {
        const std::size_t ch = m_info.channels;
        const int frameChannels = f->ch_layout.nb_channels;
        std::size_t first = 0;
        std::size_t count = static_cast<std::size_t>(std::max(0, f->nb_samples));

        if (m_skipping) {
            const int64_t pts = f->pts != AV_NOPTS_VALUE ? f->pts : f->best_effort_timestamp;
            if (pts != AV_NOPTS_VALUE) {
                const int64_t at = ff().av_rescale_q(pts - m_startPts, m_timeBase,
                                                AVRational{1, static_cast<int>(m_info.sampleRate)});
                if (at + static_cast<int64_t>(count) <= static_cast<int64_t>(m_skipTo))
                    return; // entirely before the seek target
                if (at < static_cast<int64_t>(m_skipTo))
                    first = static_cast<std::size_t>(static_cast<int64_t>(m_skipTo) - at);
            }
            m_skipping = false;
        }
        if (first >= count || frameChannels <= 0)
            return;

        const std::size_t base = m_pending.size();
        m_pending.resize(base + (count - first) * ch, 0);
        int32_t* out = m_pending.data() + base;
        const auto fmt = static_cast<AVSampleFormat>(f->format);
        const bool planar = ff().av_sample_fmt_is_planar(fmt);
        const std::size_t used = std::min<std::size_t>(ch, static_cast<std::size_t>(frameChannels));

        auto convert = [&](auto sampleType, auto toS32) {
            using T = decltype(sampleType);
            for (std::size_t i = first; i < count; ++i) {
                int32_t* frameOut = out + (i - first) * ch;
                for (std::size_t c = 0; c < used; ++c) {
                    const T v = planar ? reinterpret_cast<const T*>(f->extended_data[c])[i]
                                       : reinterpret_cast<const T*>(f->extended_data[0])[i * frameChannels + c];
                    frameOut[c] = toS32(v);
                }
            }
        };
        auto fromFloat = [](double v) {
            return static_cast<int32_t>(std::clamp(v * 2147483648.0, -2147483648.0, 2147483647.0));
        };

        switch (ff().av_get_packed_sample_fmt(fmt)) {
        case AV_SAMPLE_FMT_U8:
            convert(uint8_t{}, [](uint8_t v) { return static_cast<int32_t>((static_cast<uint32_t>(v) - 128u) << 24); });
            break;
        case AV_SAMPLE_FMT_S16:
            convert(int16_t{}, [](int16_t v) { return static_cast<int32_t>(static_cast<uint32_t>(v) << 16); });
            break;
        case AV_SAMPLE_FMT_S32:
            convert(int32_t{}, [](int32_t v) { return v; });
            break;
        case AV_SAMPLE_FMT_S64:
            convert(int64_t{}, [](int64_t v) { return static_cast<int32_t>(v >> 32); });
            break;
        case AV_SAMPLE_FMT_FLT:
            convert(float{}, [&](float v) { return fromFloat(v); });
            break;
        case AV_SAMPLE_FMT_DBL:
            convert(double{}, [&](double v) { return fromFloat(v); });
            break;
        default:
            m_pending.resize(base); // unknown layout: drop the frame
            break;
        }
    }

    AVFormatContext* m_format = nullptr;
    AVCodecContext* m_codec = nullptr;
    AVPacket* m_packet = nullptr;
    AVFrame* m_frame = nullptr;
    int m_stream = -1;
    AVRational m_timeBase{1, 1};
    int64_t m_startPts = 0;
    bool m_inputDone = false;
    bool m_trimToLength = false;
    uint64_t m_position = 0; // frames delivered
    bool m_skipping = false;
    uint64_t m_skipTo = 0;
    std::vector<int32_t> m_pending; // one decoded frame, interleaved s32
    std::size_t m_pendingPos = 0;
};

} // namespace

const std::vector<std::string>& extensions()
{
    static const std::vector<std::string> list = {
        "m4a", "m4b", "mp4", "aac", "caf",           // ALAC / AAC
        "ogg", "oga", "opus",                        // Vorbis / Opus / FLAC in Ogg
        "aif", "aiff", "aifc", "w64",                // PCM
        "ape", "wv", "tta", "tak", "mpc",            // other lossless / Musepack
        "dsf", "dff",                                // DSD
        "wma", "mka", "webm", "ac3", "dts", "mp2",
    };
    return list;
}

std::string version()
{
    // Built against this ABI; reading the loaded library's version would load
    // FFmpeg just to show it.
    return "(libavcodec " + std::to_string(LIBAVCODEC_VERSION_MAJOR) + ")";
}

bool dtsEncoderAvailable()
{
    return api() && ff().avcodec_find_encoder_by_name("dca") != nullptr;
}

bool encodeDts(const int32_t* interleaved, std::size_t frames, uint32_t channels, uint32_t rate, std::vector<uint8_t>& out,
               std::string* error)
{
    const auto fail = [&](const std::string& why) {
        if (error)
            *error = why;
        return false;
    };
    if (!api())
        return fail("FFmpeg is not installed");
    const AVCodec* codec = ff().avcodec_find_encoder_by_name("dca");
    if (!codec)
        return fail("This FFmpeg has no DTS encoder");
    AVCodecContext* ctx = ff().avcodec_alloc_context3(codec);
    if (!ctx)
        return fail("FFmpeg: out of memory");
    ctx->sample_rate = static_cast<int>(rate);
    ctx->sample_fmt = AV_SAMPLE_FMT_S32;
    ff().av_channel_layout_default(&ctx->ch_layout, static_cast<int>(channels));
    ctx->bit_rate = 1411200; // the DTS CD rate: the stream fits a 44.1 kHz 16-bit stereo WAV exactly
    ctx->strict_std_compliance = FF_COMPLIANCE_EXPERIMENTAL; // the encoder is marked experimental
    if (ff().avcodec_open2(ctx, codec, nullptr) < 0) {
        ff().avcodec_free_context(&ctx);
        return fail("FFmpeg: the DTS encoder refused the format");
    }
    AVFrame* frame = ff().av_frame_alloc();
    AVPacket* packet = ff().av_packet_alloc();
    bool ok = frame && packet;
    const std::size_t block = static_cast<std::size_t>(std::max(ctx->frame_size, 512));
    const auto drain = [&] {
        while (ff().avcodec_receive_packet(ctx, packet) == 0) {
            out.insert(out.end(), packet->data, packet->data + packet->size);
            ff().av_packet_unref(packet);
        }
    };
    for (std::size_t done = 0; ok && done < frames; done += block) {
        const std::size_t n = std::min(block, frames - done);
        frame->nb_samples = static_cast<int>(block); // the encoder wants whole frames: the last is padded with silence
        frame->format = AV_SAMPLE_FMT_S32;
        frame->sample_rate = ctx->sample_rate;
        ff().av_channel_layout_default(&frame->ch_layout, static_cast<int>(channels));
        if (ff().av_frame_get_buffer(frame, 0) < 0) {
            ok = false;
            break;
        }
        std::memset(frame->data[0], 0, block * channels * sizeof(int32_t));
        std::memcpy(frame->data[0], interleaved + done * channels, n * channels * sizeof(int32_t));
        ok = ff().avcodec_send_frame(ctx, frame) >= 0;
        drain();
        ff().av_frame_unref(frame);
    }
    if (ok) {
        ff().avcodec_send_frame(ctx, nullptr);
        drain();
    }
    if (packet)
        ff().av_packet_free(&packet);
    if (frame)
        ff().av_frame_free(&frame);
    ff().avcodec_free_context(&ctx);
    if (!ok)
        return fail("FFmpeg: DTS encoding failed");
    return !out.empty();
}

std::unique_ptr<Decoder> open(const std::filesystem::path& path, std::string* error)
{
    if (!api()) {
        if (error)
            *error = "FFmpeg is not installed: " + path.filename().string();
        return nullptr;
    }
    auto dec = std::make_unique<FfmpegDecoder>();
    if (!dec->open(path, error))
        return nullptr;
    return dec;
}

bool probe(const std::filesystem::path& path, ProbeResult& out, bool wantCover)
{
    if (!api())
        return false;
    AVFormatContext* format = openInput(path);
    if (!format)
        return false;
    struct Closer {
        AVFormatContext*& f;
        ~Closer() { ff().avformat_close_input(&f); }
    } closer{format};

    int index = ff().av_find_best_stream(format, AVMEDIA_TYPE_AUDIO, -1, -1, nullptr, 0);
    // Most containers describe the stream in their header; a few (raw AAC,
    // Musepack...) need packets to be read first.
    if (index < 0 || format->streams[index]->codecpar->sample_rate <= 0
        || format->streams[index]->codecpar->ch_layout.nb_channels <= 0) {
        if (ff().avformat_find_stream_info(format, nullptr) < 0)
            return false;
        index = ff().av_find_best_stream(format, AVMEDIA_TYPE_AUDIO, -1, -1, nullptr, 0);
    }
    if (index < 0)
        return false;

    const AVStream* st = format->streams[index];
    const AVCodecParameters* par = st->codecpar;
    const int rate = par->sample_rate;
    out.stream.sampleRate = static_cast<uint32_t>(std::max(0, rate));
    out.stream.channels = static_cast<uint32_t>(std::max(0, par->ch_layout.nb_channels));
    out.stream.bitsPerSample = bitsOf(par, AV_SAMPLE_FMT_NONE);
    out.stream.totalFrames = streamFrames(path, format, st, rate);
    if (out.stream.totalFrames == 0 && ff().avformat_find_stream_info(format, nullptr) >= 0)
        out.stream.totalFrames = streamFrames(path, format, st, rate); // no length in the header: estimated
    out.stream.codec = codecLabel(par->codec_id, format, rate);
    out.stream.lossless = out.stream.bitsPerSample > 0;
    out.durationSeconds = rate > 0 ? static_cast<double>(out.stream.totalFrames) / rate : 0.0;
    const int64_t bitRate = par->bit_rate > 0 ? par->bit_rate : format->bit_rate;
    out.bitrateKbps = out.stream.lossless || bitRate <= 0 ? 0 : static_cast<int>((bitRate + 500) / 1000);

    // Container tags first, then the stream's (Ogg keeps them there).
    for (const AVDictionary* dict : {static_cast<const AVDictionary*>(format->metadata),
                                     static_cast<const AVDictionary*>(st->metadata)}) {
        const AVDictionaryEntry* e = nullptr;
        while ((e = ff().av_dict_get(dict, "", e, AV_DICT_IGNORE_SUFFIX)))
            out.tags.emplace_back(e->key, e->value);
    }

    // Embedded artwork (MP4 covr, ID3 APIC, Vorbis METADATA_BLOCK_PICTURE...):
    // the front cover, or the first picture.
    bool haveFront = false;
    for (unsigned i = 0; i < format->nb_streams; ++i) {
        const AVStream* pic = format->streams[i];
        if (!(pic->disposition & AV_DISPOSITION_ATTACHED_PIC) || pic->attached_pic.size <= 0)
            continue;
        out.hasCover = true;
        if (!wantCover)
            break;
        const AVDictionaryEntry* comment = ff().av_dict_get(pic->metadata, "comment", nullptr, 0);
        const bool front = comment && std::strcmp(comment->value, "Cover (front)") == 0;
        if (out.cover.empty() || (front && !haveFront)) {
            out.cover.assign(reinterpret_cast<const char*>(pic->attached_pic.data),
                             static_cast<std::size_t>(pic->attached_pic.size));
            haveFront = front;
        }
    }
    return out.stream.sampleRate > 0;
}

} // namespace sonkore::ffmpeg
