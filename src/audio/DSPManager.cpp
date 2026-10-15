#include "DSPManager.hpp"
#include "CueSheet.hpp"

#if defined(SONKORE_HAVE_FFMPEG)
#include "FfmpegDecoder.hpp"
#endif
#if defined(SONKORE_HAVE_ALSA_DSD)
#include "AlsaDsdOutput.hpp"
#else
namespace sonkore {
class AlsaDsdOutput {}; // no ALSA: never created (m_dsdOut stays empty)
}
#endif
#include "AsioOutput.hpp"
#include "PipeWireOutput.hpp"

#include <SoundTouch.h>
#include <soxr.h>
#include <hdcd_simple.h>

#include <algorithm>
#if defined(__SSE__) || defined(_M_X64)
#include <xmmintrin.h>
#endif
#include <cmath>
#include <numbers>
#include <chrono>
#if defined(__linux__)
#include <sys/syscall.h>
#include <unistd.h>
#endif
#include <cstdlib>
#include <cstring>
#include <fstream>
#include <regex>
#include <span>

namespace sonkore {

namespace {

constexpr std::size_t kDecodeBlockFrames = 1024;     // source frames per worker iteration
constexpr std::size_t kStChunkFrames = 4096;         // time-stretch receive chunk
constexpr std::size_t kMaxChannels = SuperEQ::kMaxChannels;
constexpr auto kIdleSleep = std::chrono::milliseconds(20);     // nothing to decode (end of stream, waiting for the next track)
constexpr auto kRingFullSleepMin = std::chrono::milliseconds(2); // ring full: at least this long...
constexpr uint32_t kDsdPrerollMs = 60; // DSD silence first, while the DAC switches mode
constexpr std::size_t kMaxPreloadBytes = std::size_t(2) << 30; // play from memory up to 2 GB per file

// Channel order of a source with this many channels: WAVE / FLAC order, which
// is also how FFmpeg hands its decoded channels over.
std::array<ma_channel, MA_MAX_CHANNELS> sourceChannelMap(uint32_t channels)
{
    std::array<ma_channel, MA_MAX_CHANNELS> m{};
    static const ma_channel layouts[9][8] = {
        {},
        {MA_CHANNEL_MONO},
        {MA_CHANNEL_FRONT_LEFT, MA_CHANNEL_FRONT_RIGHT},
        {MA_CHANNEL_FRONT_LEFT, MA_CHANNEL_FRONT_RIGHT, MA_CHANNEL_FRONT_CENTER},
        {MA_CHANNEL_FRONT_LEFT, MA_CHANNEL_FRONT_RIGHT, MA_CHANNEL_BACK_LEFT, MA_CHANNEL_BACK_RIGHT},
        {MA_CHANNEL_FRONT_LEFT, MA_CHANNEL_FRONT_RIGHT, MA_CHANNEL_FRONT_CENTER, MA_CHANNEL_BACK_LEFT, MA_CHANNEL_BACK_RIGHT},
        {MA_CHANNEL_FRONT_LEFT, MA_CHANNEL_FRONT_RIGHT, MA_CHANNEL_FRONT_CENTER, MA_CHANNEL_LFE, MA_CHANNEL_BACK_LEFT, MA_CHANNEL_BACK_RIGHT},
        {MA_CHANNEL_FRONT_LEFT, MA_CHANNEL_FRONT_RIGHT, MA_CHANNEL_FRONT_CENTER, MA_CHANNEL_LFE, MA_CHANNEL_BACK_CENTER, MA_CHANNEL_SIDE_LEFT, MA_CHANNEL_SIDE_RIGHT},
        {MA_CHANNEL_FRONT_LEFT, MA_CHANNEL_FRONT_RIGHT, MA_CHANNEL_FRONT_CENTER, MA_CHANNEL_LFE, MA_CHANNEL_BACK_LEFT, MA_CHANNEL_BACK_RIGHT, MA_CHANNEL_SIDE_LEFT, MA_CHANNEL_SIDE_RIGHT},
    };
    for (uint32_t c = 0; c < std::min<uint32_t>(channels, 8); ++c)
        m[c] = layouts[channels][c];
    return m;
}

// For every device channel, the source channel at the same position (back and
// side surrounds stand in for each other when the device lacks one of them).
// False when some channel has nowhere to go: the device would have to mix.
bool channelOrder(const ma_channel* source, const ma_channel* device, uint32_t channels, std::array<uint8_t, MA_MAX_CHANNELS>& order)
{
    const auto alias = [](ma_channel c) -> ma_channel {
        switch (c) {
        case MA_CHANNEL_SIDE_LEFT: return MA_CHANNEL_BACK_LEFT;
        case MA_CHANNEL_SIDE_RIGHT: return MA_CHANNEL_BACK_RIGHT;
        case MA_CHANNEL_BACK_LEFT: return MA_CHANNEL_SIDE_LEFT;
        case MA_CHANNEL_BACK_RIGHT: return MA_CHANNEL_SIDE_RIGHT;
        default: return c;
        }
    };
    std::array<bool, MA_MAX_CHANNELS> used{};
    for (int pass = 0; pass < 2; ++pass)
        for (uint32_t d = 0; d < channels; ++d) {
            if (pass == 1 && order[d] != 0xFF)
                continue;
            if (pass == 0)
                order[d] = 0xFF;
            for (uint32_t s = 0; s < channels; ++s)
                if (!used[s] && (pass == 0 ? source[s] == device[d] : alias(source[s]) == device[d])) {
                    order[d] = static_cast<uint8_t>(s);
                    used[s] = true;
                    break;
                }
        }
    for (uint32_t d = 0; d < channels; ++d)
        if (order[d] == 0xFF)
            return false;
    return true;
}

// Rate multiples to try for an upsampling setting, best first.
std::vector<uint32_t> upsamplingFactors(DSPManager::Upsampling mode)
{
    switch (mode) {
    case DSPManager::Upsampling::Double: return {2};
    case DSPManager::Upsampling::Quadruple: return {4, 2};
    case DSPManager::Upsampling::Max: return {16, 8, 4, 2};
    default: return {};
    }
}

void setError(std::string* error, std::string message)
{
    if (error)
        *error = std::move(message);
}

bool pipewireRunning()
{
#if defined(__linux__)
    const char* runtimeDir = std::getenv("XDG_RUNTIME_DIR");
    std::error_code ec;
    return runtimeDir && std::filesystem::exists(std::filesystem::path(runtimeDir) / "pipewire-0", ec);
#else
    return false;
#endif
}

std::string backendTag(ma_backend b)
{
    switch (b) {
    case ma_backend_alsa: return "alsa";
    case ma_backend_pulseaudio: return "pulse";
    case ma_backend_jack: return "jack";
    case ma_backend_wasapi: return "wasapi";
    case ma_backend_coreaudio: return "coreaudio";
    default: return ma_get_backend_name(b);
    }
}

std::string backendLabel(ma_backend b, bool pipewire)
{
    switch (b) {
    case ma_backend_alsa: return "ALSA";
    case ma_backend_pulseaudio: return pipewire ? "PipeWire" : "PulseAudio";
    case ma_backend_jack: return pipewire ? "PipeWire JACK" : "JACK";
    case ma_backend_wasapi: return "WASAPI";
    case ma_backend_coreaudio: return "Core Audio";
    default: return ma_get_backend_name(b);
    }
}

#if defined(__linux__)
struct AlsaPcm {
    std::string cardId;   // e.g. "DX1"
    std::string cardName; // e.g. "Topping DX1"
    int device = 0;
    std::string pcmName;  // e.g. "USB Audio"
};

std::string trim(std::string s)
{
    const auto b = s.find_first_not_of(" \t\r\n");
    const auto e = s.find_last_not_of(" \t\r\n");
    return b == std::string::npos ? std::string() : s.substr(b, e - b + 1);
}

// Every playback PCM of every sound card, straight from procfs. These are the
// raw hw: endpoints (no plug/dmix/PipeWire in between) that miniaudio's own
// enumeration does not list.
std::vector<AlsaPcm> enumerateAlsaHardware()
{
    std::vector<AlsaPcm> result;
    std::ifstream cards("/proc/asound/cards");
    const std::regex header(R"(^\s*(\d+)\s+\[([^\]\s]+)\s*\]:\s*(.*?)\s+-\s+(.*)$)");
    std::string line;
    while (std::getline(cards, line)) {
        std::smatch m;
        if (!std::regex_match(line, m, header))
            continue;
        const std::string cardIndex = m[1];
        const std::string cardId = m[2];
        std::string cardName = trim(m[4]);
        std::string longName;
        if (std::getline(cards, longName)) {
            longName = trim(longName);
            const auto at = longName.find(" at ");
            if (at != std::string::npos)
                longName.resize(at);
            if (!longName.empty())
                cardName = longName;
        }

        std::error_code ec;
        const std::filesystem::path cardDir = "/proc/asound/card" + cardIndex;
        std::vector<AlsaPcm> pcms;
        for (const auto& entry : std::filesystem::directory_iterator(cardDir, ec)) {
            const std::string name = entry.path().filename().string();
            if (name.size() < 5 || name.rfind("pcm", 0) != 0 || name.back() != 'p')
                continue;
            AlsaPcm pcm;
            pcm.cardId = cardId;
            pcm.cardName = cardName;
            pcm.device = std::atoi(name.c_str() + 3);
            std::ifstream info(entry.path() / "info");
            std::string infoLine;
            while (std::getline(info, infoLine))
                if (infoLine.rfind("name:", 0) == 0)
                    pcm.pcmName = trim(infoLine.substr(5));
            pcms.push_back(pcm);
        }
        std::sort(pcms.begin(), pcms.end(), [](const AlsaPcm& a, const AlsaPcm& b) { return a.device < b.device; });
        result.insert(result.end(), pcms.begin(), pcms.end());
    }
    return result;
}
#endif

} // namespace

DSPManager::DSPManager() = default;

std::string DSPManager::componentVersions()
{
    std::string soxr = soxr_version(); // "libsoxr-0.1.3"
    if (soxr.rfind("lib", 0) == 0)
        soxr.erase(0, 3);
    // Non-breaking spaces (U+00A0) inside each name: lines wrap only between
    // libraries, never between a name and its version.
#define NBSP "\u00a0"
    std::string s = "miniaudio" NBSP MA_VERSION_STRING " · dr_flac / dr_wav / dr_mp3";
#if defined(SONKORE_HAVE_FFMPEG)
    s += " · FFmpeg" NBSP + ffmpeg::version();
#endif
    if (const std::string pw = PipeWireOutput::libraryVersion(); !pw.empty())
        s += " · PipeWire" NBSP + pw;
    s += " · SoundTouch" NBSP SOUNDTOUCH_VERSION " · " + soxr + " · Qt" NBSP "6";
#undef NBSP
    return s;
}

DSPManager::~DSPManager()
{
    shutdown();
    hdcd_free(m_hdcd);
}

// -----------------------------------------------------------------------------
// Contexts / devices
// -----------------------------------------------------------------------------

bool DSPManager::initialize(std::string* error)
{
    if (!m_backends.empty())
        return true;

#if defined(_WIN32)
    const ma_backend wanted[] = {ma_backend_wasapi};
#elif defined(__APPLE__)
    const ma_backend wanted[] = {ma_backend_coreaudio};
#else
    // Listed in this order: the desktop sound server (PipeWire, through its
    // PulseAudio protocol), raw ALSA hw: devices, then JACK graphs.
    const ma_backend wanted[] = {ma_backend_pulseaudio, ma_backend_alsa, ma_backend_jack};
#endif
    m_pipewireRunning = pipewireRunning();
    // PipeWire spoken to natively: its PulseAudio and JACK faces lead to the
    // same sinks, and their client libraries (libpulse with libsndfile, FLAC,
    // Vorbis, Opus, LAME and mpg123; libjack) would only take memory. They
    // are made when needed (PipeWire's own library refuses, a JACK device
    // saved in the settings).
    m_lazyServers = !m_nullBackend && m_pipewireRunning && PipeWireOutput::available();
    const ma_backend null[] = {ma_backend_null};

    for (ma_backend type : m_nullBackend ? std::span<const ma_backend>(null) : std::span<const ma_backend>(wanted)) {
        if (m_lazyServers && (type == ma_backend_pulseaudio || type == ma_backend_jack))
            continue;
        addBackend(type);
    }

    if (m_backends.empty() && !m_lazyServers) {
        setError(error, "No audio backend could be initialised");
        return false;
    }
    playbackDevices();
    return true;
}

bool DSPManager::addBackend(ma_backend type)
{
    for (const Backend& b : m_backends)
        if (b.type == type)
            return true;
    ma_context_config cc = ma_context_config_init();
    cc.threadPriority = ma_thread_priority_realtime;
    cc.alsa.useVerboseDeviceEnumeration = MA_TRUE;
    cc.pulse.pApplicationName = "Sonkore";
    cc.jack.pClientName = "Sonkore";
    cc.jack.tryStartServer = MA_FALSE;
    auto ctx = std::make_unique<ma_context>();
    if (ma_context_init(&type, 1, &cc, ctx.get()) != MA_SUCCESS)
        return false;
    // Listed in the usual order whenever it was made: sound server, ALSA, JACK.
    const auto rank = [](ma_backend t) { return t == ma_backend_pulseaudio ? 0 : t == ma_backend_alsa ? 1 : 2; };
    auto at = m_backends.begin();
    while (at != m_backends.end() && rank(at->type) <= rank(type))
        ++at;
    m_backends.insert(at, {type, backendLabel(type, m_pipewireRunning), std::move(ctx)});
    return true;
}

void DSPManager::shutdown()
{
    unload();
    for (auto& b : m_backends)
        ma_context_uninit(b.context.get());
    m_backends.clear();
    m_devices.clear();
}

std::vector<DSPManager::DeviceInfo> DSPManager::playbackDevices()
{
    std::vector<DeviceInfo> result;
    m_devices.clear();

    // Linux: PipeWire's sinks, spoken to natively (rate switching, no
    // PulseAudio layer). They stand in for the same sinks seen through
    // pipewire-pulse, which stay reachable for settings saved before.
    m_pipewireNative = false;
    if (!m_nullBackend)
        for (const PipeWireOutput::Sink& sink : PipeWireOutput::sinks()) {
            DeviceEntry e;
            e.pw = true;
            e.pipewire = sink.name;
            e.name = sink.description;
            const std::string key = "pipewire|" + sink.name;
            m_devices[key] = e;
            result.push_back({key, "PipeWire", sink.description, sink.name, sink.isDefault, false});
            m_pipewireNative = true;
        }

    if (m_lazyServers && !m_pipewireNative) {
        // PipeWire's library is there but lists no sink: its PulseAudio face.
        addBackend(ma_backend_pulseaudio);
        addBackend(ma_backend_jack);
    }

    for (std::size_t bi = 0; bi < m_backends.size(); ++bi) {
        Backend& backend = m_backends[bi];
        const std::string tag = backendTag(backend.type);

#if defined(__linux__)
        if (backend.type == ma_backend_alsa) {
            // Only raw hardware endpoints: ALSA's "default"/plugin devices
            // route into PipeWire anyway and are listed under that backend.
            for (const AlsaPcm& pcm : enumerateAlsaHardware()) {
                const std::string hw = "hw:CARD=" + pcm.cardId + ",DEV=" + std::to_string(pcm.device);
                DeviceEntry e;
                e.backendIndex = bi;
                e.hasId = true;
                e.direct = true;
                e.name = pcm.cardName + (pcm.pcmName.empty() ? "" : " · " + pcm.pcmName);
                std::strncpy(e.id.alsa, hw.c_str(), sizeof(e.id.alsa) - 1);
                const std::string key = tag + "|" + hw;
                m_devices[key] = e;
                result.push_back({key, backend.label, e.name, hw, false, true});
            }
            continue;
        }
#endif

        ma_device_info* infos = nullptr;
        ma_uint32 count = 0;
        if (ma_context_get_devices(backend.context.get(), &infos, &count, nullptr, nullptr) != MA_SUCCESS)
            continue;

        for (ma_uint32 i = 0; i < count; ++i) {
            std::string id;
            switch (backend.type) {
            case ma_backend_pulseaudio: id = infos[i].id.pulse; break;
            case ma_backend_coreaudio: id = infos[i].id.coreaudio; break;
            case ma_backend_alsa: id = infos[i].id.alsa; break;
            default: id = infos[i].name; break;
            }
            DeviceEntry e;
            e.backendIndex = bi;
            e.id = infos[i].id;
            e.hasId = true;
            e.direct = false;
            e.name = infos[i].name;
            const std::string key = tag + "|" + id;
            m_devices[key] = e;
            if (backend.type == ma_backend_pulseaudio && m_pipewireNative)
                continue; // listed above, natively
            const std::string detail = (backend.type == ma_backend_pulseaudio) ? id : std::string();
            result.push_back({key, backend.label, e.name, detail, infos[i].isDefault != 0, false});
        }
    }
    // Windows: ASIO drivers, each a direct output (no Windows mixer).
    if (!m_nullBackend)
        for (const std::string& driver : AsioOutput::drivers()) {
            DeviceEntry e;
            e.direct = true;
            e.name = driver;
            e.asio = driver;
            const std::string key = "asio|" + driver;
            m_devices[key] = e;
            result.push_back({key, "ASIO", driver, std::string(), false, true});
        }
    return result;
}

bool DSPManager::resolveDevice(const std::string& key, DeviceEntry& out)
{
    if (!key.empty()) {
        auto it = m_devices.find(key);
        if (it == m_devices.end() && m_pipewireNative && key.rfind("pulse|", 0) == 0)
            it = m_devices.find("pipewire|" + key.substr(6)); // the same sink, natively
        if (it == m_devices.end() && m_lazyServers && key.rfind("jack|", 0) == 0)
            addBackend(ma_backend_jack); // a JACK device saved in the settings
        if (it == m_devices.end()) {
            playbackDevices(); // hot-plugged since the last enumeration?
            it = m_devices.find(key);
        }
        if (it != m_devices.end()) {
            out = it->second;
            // A sink saved as seen through pipewire-pulse: the same sink, natively.
            if (m_pipewireNative && key.rfind("pulse|", 0) == 0)
                if (const auto native = m_devices.find("pipewire|" + key.substr(6)); native != m_devices.end())
                    out = native->second;
            return true;
        }
    }
    if (m_pipewireNative) { // the desktop's default sink, natively
        out = DeviceEntry{};
        out.pw = true;
        out.name = "System default";
        return true;
    }
    // System default: the desktop mixer (PipeWire/PulseAudio) when present,
    // otherwise the first backend's default device.
    std::size_t index = 0;
    for (std::size_t i = 0; i < m_backends.size(); ++i)
        if (m_backends[i].type == ma_backend_pulseaudio)
            index = i;
    out = DeviceEntry{};
    out.backendIndex = index;
    out.name = "System default";
    return !m_backends.empty();
}

std::wstring DSPManager::wasapiEndpointId(const std::string& key)
{
    DeviceEntry entry;
    if (key.empty() || !resolveDevice(key, entry) || !entry.asio.empty() || !entry.hasId
        || m_backends[entry.backendIndex].type != ma_backend_wasapi)
        return {};
#if defined(_WIN32)
    return std::wstring(reinterpret_cast<const wchar_t*>(entry.id.wasapi));
#else
    return {};
#endif
}

void DSPManager::setOutputConfig(const OutputConfig& cfg)
{
    std::lock_guard lock(m_configMutex);
    m_config = cfg;
}

DSPManager::OutputConfig DSPManager::outputConfig() const
{
    std::lock_guard lock(m_configMutex);
    return m_config;
}

// The rate to open a device at when it refuses the track's: the lowest whole
// multiple of it (44.1 kHz -> 88.2 kHz rather than 48: one exact conversion
// stage, and the family the music was made in), else the lowest rate above
// it (nothing of the signal is lost; 88.2 kHz -> 96 kHz), or, for a track
// above everything the device takes, the highest whole divisor of its rate
// (384 kHz -> 192 kHz), else the highest rate.
static uint32_t pickRate(std::vector<uint32_t> rates, uint32_t wanted)
{
    if (rates.empty() || wanted == 0)
        return 0;
    std::sort(rates.begin(), rates.end());
    rates.erase(std::unique(rates.begin(), rates.end()), rates.end());
    for (uint32_t r : rates)
        if (r > wanted && r % wanted == 0)
            return r;
    if (const auto above = std::lower_bound(rates.begin(), rates.end(), wanted); above != rates.end())
        return *above;
    for (auto r = rates.rbegin(); r != rates.rend(); ++r)
        if (wanted % *r == 0)
            return *r;
    return rates.back();
}

// pickRate() among the rates the device lists; 0 when it does not say which
// rates it takes.
uint32_t DSPManager::bestDeviceRate(Backend& backend, const DeviceEntry& entry, uint32_t wanted)
{
    ma_device_info info{};
    if (ma_context_get_device_info(backend.context.get(), ma_device_type_playback, entry.hasId ? &entry.id : nullptr, &info)
        != MA_SUCCESS)
        return 0;
    std::vector<uint32_t> rates;
    for (ma_uint32 i = 0; i < info.nativeDataFormatCount; ++i) {
        const uint32_t r = info.nativeDataFormats[i].sampleRate;
        if (r == 0)
            return 0; // takes any rate
        rates.push_back(r);
    }
    return pickRate(std::move(rates), wanted);
}

bool DSPManager::openDevice(uint32_t sampleRate, uint32_t channels, std::string* error)
{
    const OutputConfig cfg = outputConfig();
    const bool sameFormat = m_deviceOpen ? sampleRate == m_device.sampleRate && channels == m_device.playback.channels
                                         : pwOpen() && sampleRate == m_pwRequestedRate && channels == m_channels;
    if (m_reuseDevice && (m_deviceOpen || pwOpen()) && m_outputRunning && m_outputDsd == StreamInfo::Dsd::None
        && (!m_monoToStereo || pwOpen()) && cfg.deviceKey == m_openKey && cfg.exclusiveMode == m_openExclusive && sameFormat) {
        // Same device and format as the track before: it keeps running.
        m_status.sourceSampleRate = sampleRate;
        m_status.resampled = m_outputRate != sampleRate;
        m_status.upsampled = false;
        m_status.pcmToDsdRate = 0;
        m_status.dsd.clear();
        updateBitPerfectPath();
        m_quantizer.setFormat(m_outputRate, channels);
        return true;
    }
    closeDevice();

    DeviceEntry entry;
    if (!resolveDevice(cfg.deviceKey, entry)) {
        setError(error, "No output device available");
        return false;
    }
    if (entry.pw) {
        if (openPipeWire(entry, sampleRate, channels, error))
            return true;
        // libpipewire refused (too old, a sandbox...): the same sink through
        // PipeWire's PulseAudio service.
        if (m_lazyServers && addBackend(ma_backend_pulseaudio))
            playbackDevices();
        const auto pulse = m_devices.find("pulse|" + entry.pipewire);
        if (pulse != m_devices.end() && !entry.pipewire.empty()) {
            entry = pulse->second;
        } else {
            entry = DeviceEntry{};
            entry.name = "System default";
            for (std::size_t i = 0; i < m_backends.size(); ++i)
                if (m_backends[i].type == ma_backend_pulseaudio)
                    entry.backendIndex = i;
        }
        if (m_backends.empty())
            return false;
    } else if (m_pwOut) {
        m_pwOut->release(); // another output now: the graph's rate given back
    }
    if (!entry.asio.empty())
        return openAsio(entry, false, sampleRate, channels, error);
    Backend& backend = m_backends[entry.backendIndex];

    bool exclusive = false;
    if (backend.type == ma_backend_alsa && entry.direct)
        exclusive = true;
    else if (backend.type == ma_backend_wasapi)
        exclusive = cfg.exclusiveMode;

    ma_device_config dc = ma_device_config_init(ma_device_type_playback);
    // The engine hands the device left-justified s32 at the *source* rate.
    dc.playback.format = ma_format_s32;
    dc.playback.channels = channels;
    std::array<ma_channel, MA_MAX_CHANNELS> sourceMap = sourceChannelMap(channels);
    std::array<ma_channel, MA_MAX_CHANNELS> deviceMap{};
    dc.playback.pChannelMap = sourceMap.data();
    dc.sampleRate = sampleRate;
    dc.playback.pDeviceID = entry.hasId ? &entry.id : nullptr;
    dc.playback.shareMode = exclusive ? ma_share_mode_exclusive : ma_share_mode_shared;
    dc.dataCallback = &DSPManager::dataCallback;
    dc.pUserData = this;
    dc.performanceProfile = ma_performance_profile_conservative;
    dc.periodSizeInMilliseconds = 40; // 3 x 40 ms: robust, yet DSP edits are heard quickly
    dc.periods = 3;
    dc.noPreSilencedOutputBuffer = MA_TRUE; // the callback writes every sample
    dc.noClip = MA_TRUE;
    dc.noFixedSizedCallback = MA_TRUE;      // no extra intermediary buffer


    // Windows: MMCSS "Pro Audio", no OS sample-rate conversion, no offload.
    dc.wasapi.usage = ma_wasapi_usage_pro_audio;
    dc.wasapi.noAutoConvertSRC = MA_TRUE;
    dc.wasapi.noDefaultQualitySRC = MA_TRUE;
    dc.wasapi.noHardwareOffloading = MA_TRUE;

    // Linux: keep ALSA's plug layer from silently converting format/rate.
    dc.alsa.noAutoFormat = MA_TRUE;
    dc.alsa.noAutoChannels = MA_TRUE;
    dc.alsa.noAutoResample = MA_TRUE;
    dc.pulse.pStreamNamePlayback = "Sonkore";

    // macOS: switch the device's nominal sample rate to the track's rate.
    dc.coreaudio.allowNominalSampleRateChange = cfg.allowSampleRateSwitch ? MA_TRUE : MA_FALSE;

    ma_result r = ma_device_init(backend.context.get(), &dc, &m_device);
    // Busy: usually the sound server, which agreed to hand the card over
    // (device reservation) but closes it a moment later. Wait for it at the
    // track's rate; the fallbacks below would otherwise open the card at
    // its default rate once it is free and resample everything.
    for (int i = 0; r == MA_BUSY && i < 40; ++i) {
        std::this_thread::sleep_for(std::chrono::milliseconds(25));
        r = ma_device_init(backend.context.get(), &dc, &m_device);
    }
    if (r != MA_SUCCESS && exclusive && backend.type == ma_backend_wasapi) {
        // Device held by another application in exclusive mode: share it.
        dc.playback.shareMode = ma_share_mode_shared;
        exclusive = false;
        r = ma_device_init(backend.context.get(), &dc, &m_device);
    }
    if (r != MA_SUCCESS) {
        // The device refuses the rate (88.2 kHz on an onboard chip, 384 kHz on
        // a 192 kHz DAC...): the best rate it does take, resampled here.
        const uint32_t best = bestDeviceRate(backend, entry, sampleRate);
        if (best != 0 && best != sampleRate) {
            dc.sampleRate = best;
            r = ma_device_init(backend.context.get(), &dc, &m_device);
        }
    }
    if (r != MA_SUCCESS) {
        // Nothing known about its rates: open at its own and resample here.
        dc.sampleRate = 0;
        r = ma_device_init(backend.context.get(), &dc, &m_device);
    }
    if (r == MA_SUCCESS && dc.sampleRate != 0 && m_device.playback.internalSampleRate != sampleRate) {
        // The device runs at another rate: miniaudio would convert with its
        // linear resampler. Reopen at the device's rate and convert with soxr.
        dc.sampleRate = m_device.playback.internalSampleRate;
        ma_device_uninit(&m_device);
        r = ma_device_init(backend.context.get(), &dc, &m_device);
    }
    // The device wants another channel order: hand it the samples already in
    // its order (moved, never mixed), so miniaudio has nothing to convert.
    m_reorder = false;
    m_channelsExact = true;
    if (r == MA_SUCCESS && channels > 1
        && !ma_channel_map_is_equal(sourceMap.data(), m_device.playback.internalChannelMap, channels)) {
        std::copy_n(m_device.playback.internalChannelMap, channels, deviceMap.begin());
        if (channelOrder(sourceMap.data(), deviceMap.data(), channels, m_channelOrder)) {
            dc.playback.pChannelMap = deviceMap.data();
            dc.sampleRate = m_device.sampleRate;
            ma_device_uninit(&m_device);
            r = ma_device_init(backend.context.get(), &dc, &m_device);
            m_reorder = true;
        } else {
            m_channelsExact = false; // miniaudio mixes the layouts together
        }
    }
    m_monoToStereo = false;
    if (r != MA_SUCCESS && channels == 1) {
        // Some backends (JACK) take no mono stream: open in stereo and copy
        // the one channel to both, sample for sample.
        static const ma_channel stereo[2] = {MA_CHANNEL_FRONT_LEFT, MA_CHANNEL_FRONT_RIGHT};
        dc.playback.channels = 2;
        dc.playback.pChannelMap = const_cast<ma_channel*>(stereo);
        dc.sampleRate = sampleRate;
        r = ma_device_init(backend.context.get(), &dc, &m_device);
        if (r != MA_SUCCESS) {
            dc.sampleRate = 0;
            r = ma_device_init(backend.context.get(), &dc, &m_device);
        }
        if (r == MA_SUCCESS && dc.sampleRate != 0 && m_device.playback.internalSampleRate != sampleRate) {
            dc.sampleRate = m_device.playback.internalSampleRate;
            ma_device_uninit(&m_device);
            r = ma_device_init(backend.context.get(), &dc, &m_device);
        }
        m_monoToStereo = r == MA_SUCCESS;
    }
    if (r != MA_SUCCESS) {
        std::string msg = "Could not open \"" + entry.name + "\": " + ma_result_description(r);
        if (entry.direct)
            msg += " (the device may be in use by PipeWire or another application)";
        setError(error, msg);
        return false;
    }
    m_deviceOpen = true;
    m_outputRate = m_device.sampleRate;
    m_openKey = cfg.deviceKey;
    m_openExclusive = cfg.exclusiveMode;
    noteOutputRate(m_outputRate);
    m_rampFrames.store(std::max<std::size_t>(64, m_outputRate * 8 / 1000)); // 8 ms
    m_cbRampingIn = false;

    const ma_format fmt = m_device.playback.internalFormat;
    OutputStatus st;
    st.backend = backend.label;
    st.deviceName = entry.name;
    st.exclusive = exclusive;
    st.deviceSampleRate = m_device.playback.internalSampleRate;
    st.deviceChannels = m_device.playback.internalChannels;
    st.deviceBits = static_cast<int>(ma_get_bytes_per_sample(fmt) * 8);
    st.deviceFloat = (fmt == ma_format_f32);
    if (m_nullBackend && m_nullDeviceBits > 0) {
        st.deviceBits = m_nullDeviceBits;
        st.deviceFloat = false;
    }
    st.sourceSampleRate = sampleRate;
    st.resampled = m_outputRate != sampleRate;
    m_status = st;
    m_mixerFree = exclusive || backend.type == ma_backend_coreaudio;
    updateBitPerfectPath();

    // Requantise processed audio onto the grid the DAC actually receives.
    m_quantizer.setTargetBits(st.deviceFloat ? 24 : std::min(st.deviceBits, 24));
    m_quantizer.setFormat(m_outputRate, channels);
    m_deviceDepth = st.deviceFloat ? 24 : st.deviceBits;
    return true;
}

// Whether the current track can reach the DAC untouched on the open device.
void DSPManager::updateBitPerfectPath()
{
    OutputStatus& st = m_status;
    // Mono duplicated onto a stereo device is still every sample untouched.
    const bool channelsMatch = st.deviceChannels == m_track.channels || (m_track.channels == 1 && st.deviceChannels == 2);
    const bool formatMatches = !st.resampled && st.deviceSampleRate == m_track.sampleRate && channelsMatch;
    // miniaudio's s32->s24/s16 is truncation and s32->f32 scales by exactly
    // 2^-31, so integer data survives if the container is wide enough.
    const bool depthOk = st.deviceFloat ? m_track.bitsPerSample <= 24
                                        : st.deviceBits >= static_cast<int>(m_track.bitsPerSample);
    st.pathCanBeBitPerfect = formatMatches && depthOk && m_mixerFree && m_track.lossless && m_channelsExact;
    st.hdcd = m_trackHdcd;
}

bool DSPManager::asioOpen() const
{
    return m_asioOut && m_asioOut->isOpen();
}

bool DSPManager::pwOpen() const
{
    return m_pwOut && m_pwOut->isOpen();
}

bool DSPManager::outputOpen() const
{
    bool open = m_deviceOpen || asioOpen() || pwOpen();
#if defined(SONKORE_HAVE_ALSA_DSD)
    open = open || (m_dsdOut && m_dsdOut->isOpen());
#endif
    return open;
}

// Native PipeWire: the stream at the track's rate (the graph held at it when
// the settings allow), or at the graph's rate with the engine converting.
bool DSPManager::openPipeWire(const DeviceEntry& entry, uint32_t rate, uint32_t channels, std::string* error)
{
    if (!m_pwOut)
        m_pwOut = std::make_unique<PipeWireOutput>();
    const OutputConfig cfg = outputConfig();
    // Mono goes to both speakers, sample for sample (render() copies it):
    // PipeWire would otherwise mix it, or leave one side silent.
    const uint32_t streamChannels = channels == 1 ? 2 : channels;
    if (!m_pwOut->open(entry.pipewire, streamChannels, rate, cfg.allowSampleRateSwitch,
                       [this](int32_t* out, uint32_t n) { render(out, n); }, error))
        return false;
    m_pwRequestedRate = rate;
    m_monoToStereo = streamChannels != channels;
    m_outputRate = m_pwOut->rate();
    m_openKey = cfg.deviceKey;
    m_openExclusive = cfg.exclusiveMode;
    noteOutputRate(m_outputRate);
    m_rampFrames.store(std::max<std::size_t>(64, m_outputRate * 8 / 1000));
    m_cbRampingIn = false;
    m_reorder = false; // channels go out with their positions: PipeWire places them
    m_channelsExact = true;
    // No exclusive access, but nothing alters the samples while the graph
    // runs at their rate and no software volume is below 100 % (checked live
    // in outputStatus()).
    m_mixerFree = true;

    OutputStatus st;
    st.backend = "PipeWire";
    st.deviceName = entry.pipewire.empty() ? m_pwOut->sinkDescription() : entry.name;
    st.exclusive = false;
    st.deviceSampleRate = m_outputRate;
    st.deviceChannels = streamChannels;
    st.deviceBits = 32;
    st.deviceFloat = true; // PipeWire carries float32: exact for up to 24-bit integers
    st.sourceSampleRate = rate;
    st.resampled = m_outputRate != rate;
    m_status = st;
    updateBitPerfectPath();
    m_quantizer.setTargetBits(24);
    m_quantizer.setFormat(m_outputRate, channels);
    m_deviceDepth = 24;
    return true;
}

// An ASIO driver as the output: PCM at `rate` (or the closest rate it takes,
// converted here with SoX), or native DSD at the 1-bit rate `rate`.
bool DSPManager::openAsio(const DeviceEntry& entry, bool dsd, uint32_t rate, uint32_t channels, std::string* error)
{
    if (!m_asioOut)
        m_asioOut = std::make_unique<AsioOutput>();
    const auto feed = [this](int32_t* out, uint32_t n) { render(out, n); };
    // Mono goes to both speakers: the driver is opened in stereo and the one
    // channel copied, sample for sample (render() does it).
    const uint32_t driverChannels = !dsd && channels == 1 ? 2 : channels;
    uint32_t outputRate = rate;
    bool opened = m_asioOut->open(entry.asio, dsd ? AsioOutput::Mode::Dsd : AsioOutput::Mode::Pcm, driverChannels, rate, feed, error);
    if (!opened && !dsd) {
        // The driver refuses the rate: the closest it takes (pickRate()).
        const uint32_t best = pickRate(AsioOutput::sampleRates(entry.asio), rate);
        if (best != 0 && best != rate) {
            outputRate = best;
            opened = m_asioOut->open(entry.asio, AsioOutput::Mode::Pcm, driverChannels, best, feed, error);
        }
    }
    if (!opened)
        return false;
    m_monoToStereo = driverChannels != channels;
    m_outputRate = dsd ? DsdDecoder::frameRate(rate, DsdPacking::NativeU32BE) : outputRate;
    m_openKey = outputConfig().deviceKey;
    m_openExclusive = true;
    noteOutputRate(m_outputRate);
    m_rampFrames.store(std::max<std::size_t>(64, m_outputRate * 8 / 1000));
    m_cbRampingIn = false;
    m_reorder = false; // ASIO channels in the file's order
    m_channelsExact = true;
    m_mixerFree = true;

    OutputStatus st;
    st.backend = "ASIO";
    st.deviceName = entry.name;
    st.exclusive = true;
    st.deviceSampleRate = m_outputRate;
    st.deviceChannels = driverChannels;
    st.deviceBits = dsd ? 32 : m_asioOut->pcmBits();
    st.deviceFloat = !dsd && m_asioOut->pcmFloat();
    st.sourceSampleRate = dsd ? m_outputRate : rate;
    st.resampled = !dsd && outputRate != rate;
    m_status = st;
    updateBitPerfectPath();
    if (!dsd) {
        m_quantizer.setTargetBits(st.deviceFloat ? 24 : std::min(st.deviceBits, 24));
        m_quantizer.setFormat(m_outputRate, channels);
        m_deviceDepth = st.deviceFloat ? 24 : st.deviceBits;
    }
    return true;
}

void DSPManager::closeDevice()
{
    if (m_outputRunning) {
        muteOutput(true); // closing cuts whatever the DAC still holds: let it be silence
        stopOutput();
    }
#if defined(SONKORE_HAVE_ALSA_DSD)
    if (m_dsdOut)
        m_dsdOut->close();
#endif
    if (m_asioOut)
        m_asioOut->close();
    if (m_pwOut)
        m_pwOut->close(); // the graph's rate stays held for the next track
    if (m_deviceOpen) {
        ma_device_uninit(&m_device);
        m_deviceOpen = false;
    }
    m_outputDsd = StreamInfo::Dsd::None;
    m_monoToStereo = false;
    m_status = {};
}

bool DSPManager::startOutput()
{
    if (asioOpen()) {
        m_outputRunning = m_asioOut->start();
        return m_outputRunning;
    }
    if (pwOpen()) {
        m_outputRunning = m_pwOut->start();
        return m_outputRunning;
    }
#if defined(SONKORE_HAVE_ALSA_DSD)
    if (m_dsdOut && m_dsdOut->isOpen()) {
        m_outputRunning = m_dsdOut->start();
        return m_outputRunning;
    }
#endif
    m_outputRunning = m_deviceOpen && ma_device_start(&m_device) == MA_SUCCESS;
    return m_outputRunning;
}

// Roon's "resync delay": a DAC re-locks its clock when the rate changes and
// may click or swallow the first notes while it does; it gets silence first.
void DSPManager::noteOutputRate(uint32_t rate)
{
    const uint64_t ms = outputConfig().resyncDelayMs;
    m_leadSilence.store(rate != m_lastOutputRate ? uint64_t(rate) * ms / 1000 : 0, std::memory_order_relaxed);
    m_lastOutputRate = rate;
}

void DSPManager::muteOutput(bool waitHeard)
{
    using namespace std::chrono;
    if (!m_outputRunning) {
        m_mute.store(Muted, std::memory_order_release);
        return;
    }
    if (m_mute.load(std::memory_order_acquire) != Muted) {
        m_muteAck.store(false, std::memory_order_relaxed);
        m_mutedFrames.store(0, std::memory_order_relaxed);
        m_mute.store(FadingOut, std::memory_order_release);
        const auto deadline = steady_clock::now() + milliseconds(500);
        while (!m_muteAck.load(std::memory_order_acquire)) {
            if (steady_clock::now() > deadline) { // the device stalled: stop it outright
                stopOutput();
                m_mute.store(Muted, std::memory_order_release);
                return;
            }
            std::this_thread::sleep_for(milliseconds(1));
        }
    }
    if (waitHeard) {
        // Everything queued in the DAC ahead of the silence, fade included.
        const uint64_t heard = m_latencyFrames.load(std::memory_order_relaxed) + m_rampFrames.load(std::memory_order_relaxed);
        const auto deadline = steady_clock::now() + milliseconds(1000);
        while (m_mutedFrames.load(std::memory_order_relaxed) < heard && steady_clock::now() < deadline)
            std::this_thread::sleep_for(milliseconds(2));
    }
}

void DSPManager::unmuteOutput(bool rampIn)
{
    m_rampIn.store(rampIn && m_outputDsd == StreamInfo::Dsd::None, std::memory_order_relaxed);
    m_muteAck.store(false, std::memory_order_relaxed);
    m_mute.store(Open, std::memory_order_release);
}

// Returns once the device thread no longer reads the ring.
void DSPManager::stopOutput()
{
    if (asioOpen()) {
        m_asioOut->stop();
        m_outputRunning = false;
        return;
    }
    if (pwOpen()) {
        m_pwOut->stop();
        m_outputRunning = false;
        return;
    }
#if defined(SONKORE_HAVE_ALSA_DSD)
    if (m_dsdOut && m_dsdOut->isOpen()) {
        m_dsdOut->stop();
        m_outputRunning = false;
        return;
    }
#endif
    if (m_deviceOpen)
        ma_device_stop(&m_device);
    m_outputRunning = false;
}

void DSPManager::adoptDecoder(std::unique_ptr<Decoder> decoder, double gain)
{
    // An output kept running (silent) from the previous track renders buffers
    // of its own channel count: it cannot go on once m_channels says another
    // (it would write past the device's buffer). It is reopened for the track.
    if ((m_deviceOpen || pwOpen() || m_outputRunning) && decoder->info().channels != m_channels)
        closeDevice();
    m_decoder = std::move(decoder);
    m_track = m_decoder->info();
    if (m_channels != m_track.channels) // (the device thread reads it: only written while it is closed)
        m_channels = m_track.channels;
    m_trackGain.store(gain);
    m_workerRate = m_track.sampleRate;
    m_workerChannels = m_track.channels;
    m_workerDsd = m_track.dsd;
    m_workerBits = m_track.bitsPerSample;
    m_snapGains = true; // a new track starts at its own gains, no glide from the last one's
}

// A DSD file sent as DSD, when the settings ask for it and the output takes it
// untouched. False leaves nothing open: the caller converts to PCM instead.
bool DSPManager::openDsd(const std::filesystem::path& path, double gain)
{
    const OutputConfig cfg = outputConfig();
    DsdDecoder::Format format;
    if (cfg.dsd == DsdOutput::Pcm || !DsdDecoder::isDsdExtension(path) || !DsdDecoder::probe(path, format)
        || format.channels > kMaxChannels)
        return false;
    DeviceEntry entry;
    if (!resolveDevice(cfg.deviceKey, entry) || entry.pw) // PipeWire mixes: DSD goes as PCM
        return false;
    if (!entry.asio.empty() && cfg.dsd == DsdOutput::Native) {
        // ASIO DSD mode: the driver takes the 1-bit stream at its own rate.
        auto decoder = DsdDecoder::open(path, DsdPacking::NativeU32BE, nullptr);
        if (!decoder)
            return false;
        closeDevice();
        adoptDecoder(std::move(decoder), gain);
        if (!openAsio(entry, true, format.dsdRate, m_channels, nullptr)) {
            m_decoder.reset();
            m_track = {};
            return false; // no DSD mode at this rate: PCM instead
        }
        m_outputDsd = StreamInfo::Dsd::Native;
        m_dsdPacking = DsdPacking::NativeU32BE;
        m_status.dsd = m_track.codec;
        updateBitPerfectPath();
        m_resampler.release();
        m_resamplerDelay.store(0.0, std::memory_order_relaxed);
        return true;
    }
    const Backend& backend = m_backends[entry.backendIndex];

    if (cfg.dsd == DsdOutput::Native) {
#if defined(SONKORE_HAVE_ALSA_DSD)
        if (backend.type != ma_backend_alsa || !entry.direct)
            return false;
        const std::string device = entry.id.alsa;
        const uint32_t rate = DsdDecoder::frameRate(format.dsdRate, DsdPacking::NativeU32BE);
        DsdPacking packing = DsdPacking::NativeU32BE;
        if (!AlsaDsdOutput::supported(device, format.channels, rate, packing))
            return false; // the DAC does not offer native DSD at this rate
        auto decoder = DsdDecoder::open(path, packing, nullptr);
        if (!decoder)
            return false;
        closeDevice();
        adoptDecoder(std::move(decoder), gain);
        if (!m_dsdOut)
            m_dsdOut = std::make_unique<AlsaDsdOutput>();
        if (!m_dsdOut->open(device, packing, m_channels, rate, [this](int32_t* out, uint32_t n) { render(out, n); }, nullptr)) {
            m_decoder.reset();
            m_track = {};
            return false;
        }
        m_outputDsd = StreamInfo::Dsd::Native;
        m_dsdPacking = packing;
        m_outputRate = rate;
        noteOutputRate(rate);
        OutputStatus st;
        st.backend = backend.label;
        st.deviceName = entry.name;
        st.exclusive = true;
        st.deviceSampleRate = rate;
        st.deviceChannels = m_channels;
        st.deviceBits = 32;
        st.sourceSampleRate = rate;
        st.dsd = m_track.codec;
        m_status = st;
        m_mixerFree = true;
        m_reorder = false; // DSD words go out in the file's channel order
        m_channelsExact = true;
        updateBitPerfectPath();
        m_resampler.release();
        m_resamplerDelay.store(0.0, std::memory_order_relaxed);
        return true;
#else
        return false;
#endif
    }

    // DoP rides the PCM path, so only where nothing can alter a sample: an
    // exclusive device, at the exact rate, in an integer format of 24+ bits.
    const bool exclusiveCapable = !entry.asio.empty() || (backend.type == ma_backend_alsa && entry.direct)
                               || (backend.type == ma_backend_wasapi && cfg.exclusiveMode)
                               || backend.type == ma_backend_coreaudio;
    if (!exclusiveCapable)
        return false;
    auto decoder = DsdDecoder::open(path, DsdPacking::Dop, nullptr);
    if (!decoder)
        return false;
    adoptDecoder(std::move(decoder), gain);
    if (!openDevice(m_track.sampleRate, m_channels, nullptr) || m_outputRate != m_track.sampleRate
        || !m_status.pathCanBeBitPerfect || m_status.deviceFloat || m_status.deviceBits < 24) {
        closeDevice();
        m_decoder.reset();
        m_track = {};
        return false;
    }
    m_outputDsd = StreamInfo::Dsd::Dop;
    m_dsdPacking = DsdPacking::Dop;
    m_status.dsd = m_track.codec;
    m_resampler.release();
    m_resamplerDelay.store(0.0, std::memory_order_relaxed);
    return true;
}

// PCM -> DSD: the device opened for DSD (native when it offers it, else DoP
// when chosen) at the highest wanted DSD rate it takes; the PCM is resampled
// to that rate and modulated. False leaves nothing open.
bool DSPManager::openPcmToDsd()
{
    const OutputConfig cfg = outputConfig();
    if (cfg.pcmToDsd == PcmToDsd::Off || cfg.dsd == DsdOutput::Pcm || m_track.dsd != StreamInfo::Dsd::None)
        return false;
    DeviceEntry entry;
    if (!resolveDevice(cfg.deviceKey, entry) || entry.pw)
        return false;
    const Backend& backend = m_backends[entry.backendIndex];
    std::vector<int> multiples;
    switch (cfg.pcmToDsd) {
    case PcmToDsd::Dsd64: multiples = {64}; break;
    case PcmToDsd::Dsd128: multiples = {128, 64}; break;
    case PcmToDsd::Dsd256: multiples = {256, 128, 64}; break;
    case PcmToDsd::Dsd512: multiples = {512, 256, 128, 64}; break;
    default: multiples = {256, 128, 64}; break; // Max: DSD512 only when asked for (a 22.6 MHz loop per channel)
    }
    for (int multiple : multiples) {
        const uint32_t dsdRate = 44100u * multiple;
        if (cfg.dsd == DsdOutput::Native && !entry.asio.empty()) {
            if (!openAsio(entry, true, dsdRate, m_channels, nullptr))
                continue;
            m_outputDsd = StreamInfo::Dsd::Native;
            m_dsdPacking = DsdPacking::NativeU32BE;
        } else if (cfg.dsd == DsdOutput::Native) {
#if defined(SONKORE_HAVE_ALSA_DSD)
            if (backend.type != ma_backend_alsa || !entry.direct)
                return false;
            const std::string device = entry.id.alsa;
            const uint32_t rate = DsdDecoder::frameRate(dsdRate, DsdPacking::NativeU32BE);
            DsdPacking packing = DsdPacking::NativeU32BE;
            if (!AlsaDsdOutput::supported(device, m_channels, rate, packing))
                continue;
            if (!m_dsdOut)
                m_dsdOut = std::make_unique<AlsaDsdOutput>();
            if (!m_dsdOut->open(device, packing, m_channels, rate, [this](int32_t* out, uint32_t n) { render(out, n); }, nullptr))
                continue;
            m_outputDsd = StreamInfo::Dsd::Native;
            m_dsdPacking = packing;
            m_outputRate = rate;
            noteOutputRate(rate);
            OutputStatus st;
            st.backend = backend.label;
            st.deviceName = entry.name;
            st.exclusive = true;
            st.deviceSampleRate = rate;
            st.deviceChannels = m_channels;
            st.deviceBits = 32;
            m_status = st;
            m_mixerFree = true;
            m_reorder = false;
            m_channelsExact = true;
#else
            return false;
#endif
        } else {
            const bool exclusiveCapable = !entry.asio.empty() || (backend.type == ma_backend_alsa && entry.direct)
                                       || (backend.type == ma_backend_wasapi && cfg.exclusiveMode)
                                       || backend.type == ma_backend_coreaudio;
            const uint32_t rate = DsdDecoder::frameRate(dsdRate, DsdPacking::Dop);
            if (!exclusiveCapable)
                return false;
            if (!openDevice(rate, m_channels, nullptr) || m_outputRate != rate || m_status.deviceFloat || m_status.deviceBits < 24
                || !m_mixerFree || !m_channelsExact) {
                closeDevice();
                continue;
            }
            m_outputDsd = StreamInfo::Dsd::Dop;
            m_dsdPacking = DsdPacking::Dop;
        }
        if (!m_resampler.prepare(m_track.sampleRate, dsdRate, m_channels, nullptr, cfg.filter)) {
            closeDevice();
            return false;
        }
        m_toDsd = true;
        m_dsdRate = dsdRate;
        m_modulator.setup(multiple, m_channels, m_dsdPacking);
        m_modulatorResets.store(0);
        m_status.sourceSampleRate = m_track.sampleRate;
        m_status.resampled = true;
        m_status.pcmToDsdRate = dsdRate;
        m_status.dsd = DsdDecoder::rateLabel(dsdRate) + (m_outputDsd == StreamInfo::Dsd::Native ? " native" : " DoP") + " (from PCM)";
        m_status.pathCanBeBitPerfect = false;
        return true;
    }
    return false;
}

// Native DSD / DoP: a short run of DSD silence ahead of the music, so the DAC
// has switched to DSD before the first note.
void DSPManager::pushDsdPreroll()
{
    if (m_outputDsd == StreamInfo::Dsd::None)
        return;
    const std::size_t frames = (static_cast<std::size_t>(m_outputRate) * kDsdPrerollMs / 1000) & ~std::size_t(1); // even: ends on 0xFA
    std::vector<int32_t> silence(frames * m_channels);
    for (std::size_t f = 0; f < frames; ++f)
        for (std::size_t c = 0; c < m_channels; ++c)
            silence[f * m_channels + c] = DsdDecoder::silence(m_dsdPacking, f & 1);
    pushToRing(silence.data(), frames);
}

// -----------------------------------------------------------------------------
// Transport (control thread)
// -----------------------------------------------------------------------------

bool DSPManager::load(const std::filesystem::path& path, std::string* error, double gain)
{
    if (!initialize(error))
        return false;
    // A running PCM output stays open (silent) for the next track: it is kept
    // if that track wants the same format, so the DAC does not re-lock (click).
    const bool keepDevice = (m_deviceOpen || pwOpen()) && m_outputRunning && m_outputDsd == StreamInfo::Dsd::None && !m_toDsd;
    if (keepDevice) {
        muteOutput(false);
        releaseTrack();
    } else {
        closeDevice(); // (a rate PipeWire holds for us stays: the next output likely wants it)
        releaseTrack();
    }
    if (outputConfig().playFromMemory) {
        // Before the decoder, so even its header reads come from RAM. A big
        // file (DSD, a CD image, a network share) loads in the background
        // instead: waiting for it would hold the start back for seconds.
        constexpr std::uintmax_t kWaitBytes = 64u << 20;
        std::error_code ec;
        const std::uintmax_t size = std::filesystem::file_size(path, ec);
        m_preload = startPreload(path, ec || size > kWaitBytes);
    }

    // DSD goes out as DSD when asked for and possible; anything else, or a DSD
    // file the output cannot take untouched, is decoded to PCM.
    m_trackHdcd = m_workerHdcd = false;
    m_toDsd = false;
    // A fixed output rate keeps the DAC at one rate: DSD files become PCM too.
    const uint32_t fixedRate = outputConfig().fixedRate;
    if (fixedRate != 0 || !openDsd(path, gain)) {
        auto decoder = Decoder::open(path, error);
        if (!decoder)
            return false;
        const StreamInfo& info = decoder->info();
        if (info.channels == 0 || info.channels > kMaxChannels || info.sampleRate == 0) {
            setError(error, "Unsupported channel layout or sample rate");
            return false;
        }
        adoptDecoder(std::move(decoder), gain);
        m_trackHdcd = outputConfig().hdcd && m_track.bitsPerSample == 16 && m_track.channels == 2 && detectHdcd(path);
        m_workerHdcd = m_trackHdcd;
        // Upsampling: the highest wanted multiple the device really takes.
        bool opened = false;
        m_reuseDevice = keepDevice;
        if (!m_trackHdcd && openPcmToDsd())
            opened = true;
        else if (fixedRate == 0)
        for (uint32_t factor : upsamplingFactors(outputConfig().upsampling)) {
            if (opened)
                break;
            const uint64_t rate = static_cast<uint64_t>(m_track.sampleRate) * factor;
            if (rate > 768000)
                continue;
            if (openDevice(static_cast<uint32_t>(rate), m_channels, nullptr) && m_outputRate == rate) {
                opened = true;
                break;
            }
            closeDevice();
        }
        // DSD decoded to PCM comes out at 1/8 of its bit rate (DSD128 ->
        // 705.6 kHz): above 384 kHz it is brought down by whole factors.
        uint32_t pcmRate = fixedRate != 0 ? fixedRate : m_track.sampleRate;
        if (fixedRate == 0 && m_track.codec.rfind("DSD", 0) == 0)
            while (pcmRate > 384000 && pcmRate % 2 == 0)
                pcmRate /= 2;
        const bool fine = opened || openDevice(pcmRate, m_channels, error);
        m_reuseDevice = false;
        if (!fine) {
            m_decoder.reset();
            m_track = {};
            return false;
        }
        if (opened && !m_toDsd) {
            m_status.upsampled = true;
            m_status.sourceSampleRate = m_track.sampleRate;
            m_status.resampled = true;
            updateBitPerfectPath();
        } else if (!opened) {
            // The device was opened for a rate other than the track's (fixed
            // output rate, DSD brought down): the status speaks of the track.
            m_status.sourceSampleRate = m_track.sampleRate;
            m_status.resampled = m_outputRate != m_track.sampleRate;
            updateBitPerfectPath();
        }
    }
    if (m_toDsd) {
        // the resampler (track rate -> DSD rate) is already prepared
    } else if (m_outputDsd == StreamInfo::Dsd::None && m_outputRate != m_track.sampleRate) {
        if (!m_resampler.prepare(m_track.sampleRate, m_outputRate, m_channels, error, outputConfig().filter)) {
            closeDevice();
            m_decoder.reset();
            m_track = {};
            return false;
        }
    } else {
        m_resampler.release();
        m_resamplerDelay.store(0.0, std::memory_order_relaxed);
    }

    const std::size_t ch = m_channels;
    // The ring must hold at least twice what the device pulls in one go
    // (it pre-fills its whole buffer on start), plus one decode block.
    std::size_t deviceFrames = 0;
    if (asioOpen())
        deviceFrames = m_asioOut->bufferFrames();
    else if (pwOpen())
        deviceFrames = m_pwOut->bufferFrames();
#if defined(SONKORE_HAVE_ALSA_DSD)
    else if (m_outputDsd == StreamInfo::Dsd::Native)
        deviceFrames = m_dsdOut->bufferFrames();
#endif
    if (deviceFrames == 0)
        deviceFrames = static_cast<std::size_t>(m_device.playback.internalPeriodSizeInFrames)
                     * std::max<ma_uint32>(1, m_device.playback.internalPeriods);
    m_latencyFrames.store(deviceFrames, std::memory_order_relaxed);
    const std::size_t ringFrames =
        std::max(static_cast<std::size_t>(m_outputRate) * outputConfig().bufferMs / 1000, 2 * deviceFrames)
        + kDecodeBlockFrames;
    m_ring.allocate(ringFrames * ch);
    m_decodeBuf.assign(kDecodeBlockFrames * ch, 0);
    releaseBuffers(); // the processing buffers are sized for this track on first use

    m_stretch.prepare(m_track.sampleRate, m_channels, static_cast<uint32_t>(kDecodeBlockFrames));
    m_eq.prepare(static_cast<double>(m_track.sampleRate), static_cast<int>(m_channels));
    m_linearEq.prepare(m_track.sampleRate, m_channels);
    m_linearEqLatency.store(0);
    m_stWasActive = false;
    m_wasBypassed = true;
    m_crossfeedFor = -1; // set up again for this track's rate
    fitConvolution();
    {
        std::lock_guard lock(m_convMutex); // the worker is not running yet: take it now
        m_convolver = std::move(m_pendingConvolver);
        m_convolverPending = false;
        m_convActive.store(m_convolver != nullptr);
    }

    m_decodedFrames.store(0);
    m_startFrame = 0;
    m_decoderEof.store(false);
    m_outputDrained.store(false);
    m_trackEnded = false;
    m_underruns.store(0);
    m_framesPushed.store(0);
    m_pushedPartial = 0;
    m_framesPlayed.store(0);
    m_tailFrames.store(0);
    m_clipped.store(0);
    m_quantizer.reset();
    m_cbDopPhase = false;
    if (m_workerHdcd) {
        if (!m_hdcd)
            m_hdcd = hdcd_new();
        hdcd_reset_ext(m_hdcd, static_cast<int>(m_track.sampleRate), 16);
    }
    m_tidKnown.store(false);
    m_audioTid.store(0);
    pushDsdPreroll();

    startWorker();
    m_state.store(State::Stopped);
    return true;
}

void DSPManager::unload()
{
    closeDevice(); // fades out first when playing
    releaseTrack();
    if (m_pwOut)
        m_pwOut->release(); // nothing plays: PipeWire's rate given back
}

// Everything of the current track but the output device.
void DSPManager::releaseTrack()
{
    stopWorker();
    clearGapless();
    m_decoder.reset();
    {
        std::lock_guard lock(m_nextMutex);
        m_preload.reset();
    }
    m_track = {};
    // Nothing plays: give the DSP memory back.
    m_stretch.release();
    m_resampler.release();
    m_resamplerDelay.store(0.0, std::memory_order_relaxed);
    m_linearEq.release();
    m_linearEqLatency.store(0);
    releaseBuffers();
    m_decodeBuf = {};
    m_ring.allocate(0);
    m_state.store(State::Empty);
}

void DSPManager::play()
{
    const State s = state();
    if (s == State::Empty || !outputOpen())
        return;
    if (s == State::Playing) {
        if (m_outputDrained.load()) // played to the end: once more from the start
            seek(0.0);
        return;
    }
    if (m_outputDrained.load())
        seek(0.0);
    waitForPrefill();
    // Resuming mid-track fades in; a track from its very start does not (its
    // first samples reach the DAC untouched). Decided from the transport, not
    // the position, which can run up to a block ahead while the worker decodes.
    const bool rampIn = s == State::Paused || m_startFrame > 0;
    if (m_outputRunning) { // paused or a new track: the DAC kept running, silent
        unmuteOutput(rampIn);
        m_state.store(State::Playing);
        return;
    }
    unmuteOutput(rampIn);
    if (startOutput())
        m_state.store(State::Playing);
}

// The DAC keeps running, silent, so nothing clicks on pause or resume; the
// controller gives it back after a while (Settings › release when idle).
void DSPManager::pause()
{
    if (state() != State::Playing)
        return;
    muteOutput(false);
    m_state.store(State::Paused);
}

void DSPManager::stop()
{
    if (state() == State::Empty)
        return;
    muteOutput(true);
    stopOutput();
    m_state.store(State::Stopped);
    seek(0.0);
}

void DSPManager::seek(double seconds)
{
    // (The state, not m_decoder: the worker swaps that one at a gapless join.)
    if (state() == State::Empty)
        return;
    const bool wasPlaying = state() == State::Playing;
    const bool running = m_outputRunning;
    muteOutput(false); // fades out; returns once the device thread leaves the ring

    parkWorker();

    {
        // Seeking while a spliced track is still queued behind the current
        // one: the seek is meant for the track being heard. Undo the splice;
        // the next track goes back to waiting (from its start).
        std::lock_guard lock(m_nextMutex);
        if (m_splicePoint.load() >= 0 && m_previous.decoder) {
            m_decoder->seek(0);
            m_next = {std::move(m_decoder), std::move(m_splicedPath), m_trackGain.load(), m_preload};
            m_preload = m_previous.preload;
            m_trackGain.store(m_previous.gain);
            m_decoder = std::move(m_previous.decoder);
            m_previous = {};
            m_splicedPath.clear();
            m_workerRate = m_track.sampleRate;
            m_workerChannels = m_track.channels;
            m_workerDsd = m_track.dsd;
            m_workerBits = m_track.bitsPerSample;
            m_next.hdcd = m_workerHdcd;
            m_workerHdcd = m_trackHdcd;
            m_splicePoint.store(-1);
        }
    }

    uint64_t target = static_cast<uint64_t>(std::max(0.0, seconds) * m_track.sampleRate);
    if (m_track.totalFrames > 0)
        target = std::min(target, m_track.totalFrames);
    if (!m_decoder->seek(target)) {
        target = 0;
        m_decoder->seek(0);
    }

    m_ring.clear();
    m_stretch.clear();
    m_resampler.clear();
    m_resamplerDelay.store(0.0, std::memory_order_relaxed);
    m_eq.reset();
    m_linearEq.reset(); // engages again from the new position, without the old history
    m_linearEqLatency.store(0);
    m_crossfeed.reset();
    if (m_convolver)
        m_convolver->reset();
    if (m_limiterRunning)
        m_limiter.reset();
    if (m_toDsd)
        m_modulator.reset();
    m_decodedFrames.store(target);
    m_startFrame = target;
    m_decoderEof.store(false);
    m_outputDrained.store(false);
    m_trackEnded = false;
    m_framesPushed.store(0);
    m_pushedPartial = 0;
    m_framesPlayed.store(0);
    m_tailFrames.store(0);
    m_clipped.store(0);
    m_quantizer.reset();
    m_cbDopPhase = false;
    if (m_workerHdcd && m_hdcd)
        hdcd_reset_ext(m_hdcd, static_cast<int>(m_track.sampleRate), 16);
    pushDsdPreroll();

    unparkWorker();

    if (wasPlaying) {
        waitForPrefill();
        unmuteOutput(target > 0); // fades in, unless back at the very start
        if (!running)
            startOutput();
    }
}

// Lets the worker queue enough audio before the device starts pulling, so
// playback never begins (or resumes after a seek) with an underrun.
void DSPManager::waitForPrefill() const
{
    const std::size_t target = m_ring.capacity() / 2;
    const auto deadline = std::chrono::steady_clock::now() + std::chrono::milliseconds(400);
    while (m_ring.size() < target && !m_decoderEof.load(std::memory_order_acquire)
           && std::chrono::steady_clock::now() < deadline)
        std::this_thread::sleep_for(std::chrono::milliseconds(1));
}

DSPManager::OutputStatus DSPManager::outputStatus() const
{
    OutputStatus st = m_status;
    // Judged from the current settings rather than the last processed block,
    // so the indicator is correct while paused (the ring catches up within
    // bufferMs once playback resumes).
    st.modulatorResets = m_modulatorResets.load(std::memory_order_relaxed);
    if (pwOpen()) {
        // PipeWire: untouched only while the graph runs at the stream's rate
        // and no software volume scales it.
        st.graphRate = m_pwOut->graphRate();
        st.systemVolumeNeutral = m_pwOut->volumeNeutral();
        if ((st.graphRate != 0 && st.graphRate != st.deviceSampleRate) || !st.systemVolumeNeutral)
            st.pathCanBeBitPerfect = false;
    }
    st.bitPerfectNow = st.pathCanBeBitPerfect && !st.hdcd && (dspIsNeutral() || m_outputDsd != StreamInfo::Dsd::None);
    return st;
}

bool DSPManager::dspIsNeutral() const
{
    return m_stretch.isNeutral() && m_eq.isNeutral() && !m_convActive.load(std::memory_order_relaxed)
        && !m_linearEq.busy.load(std::memory_order_relaxed) // still holding (or handing back) music
        && m_volume.load(std::memory_order_relaxed) == 1.0
        && m_trackGain.load(std::memory_order_relaxed) == 1.0
        && (m_crossfeedPreset.load(std::memory_order_relaxed) == 0 || m_channels != 2);
}

double DSPManager::durationSeconds() const
{
    return m_track.sampleRate ? static_cast<double>(m_track.totalFrames) / m_track.sampleRate : 0.0;
}

double DSPManager::positionSeconds() const
{
    if (state() == State::Empty || m_track.sampleRate == 0)
        return 0.0;
    // Decoder position minus what is still queued, converted back to source time.
    const double rateRatio = m_outputRate ? static_cast<double>(m_track.sampleRate) / m_outputRate : 1.0;
    const double speed = m_bypassActive.load(std::memory_order_relaxed) ? 1.0 : m_stretch.speedFactor();
    const int64_t splice = m_splicePoint.load(std::memory_order_acquire);
    if (splice >= 0) {
        // The worker already decodes the next track: what is still heard of
        // this one is the part of the ring before the splice point.
        const uint64_t played = m_framesPlayed.load(std::memory_order_relaxed);
        const double before = static_cast<double>(static_cast<uint64_t>(splice) - std::min<uint64_t>(splice, played));
        const double pos = static_cast<double>(m_spliceSourceEnd.load(std::memory_order_relaxed)) - before * rateRatio * speed;
        return std::max(0.0, pos) / m_track.sampleRate;
    }
    const double queuedFrames = static_cast<double>(m_ring.size() / m_channels) * rateRatio;
    // What the pitch / tempo engine and the linear-phase EQ still hold has
    // not been heard either.
    const double held = m_bypassActive.load(std::memory_order_relaxed) ? 0.0 : m_stretch.heldSourceFrames();
    const double inFilter = static_cast<double>(m_linearEqLatency.load(std::memory_order_relaxed));
    // ...nor what the sample-rate converter holds (output frames).
    const double converting = m_bypassActive.load(std::memory_order_relaxed) ? 0.0 : m_resamplerDelay.load(std::memory_order_relaxed) * rateRatio;
    const double pos = static_cast<double>(m_decodedFrames.load(std::memory_order_relaxed)) - held - (queuedFrames + inFilter + converting) * speed;
    return std::max(0.0, pos) / m_track.sampleRate;
}

bool DSPManager::reachedEnd() const
{
    return m_outputDrained.load(std::memory_order_relaxed);
}

void DSPManager::setVolume(double linear)
{
    m_volume.store(std::clamp(linear, 0.0, 4.0), std::memory_order_relaxed);
}

// -----------------------------------------------------------------------------
// Real-time callback: lock-free, allocation-free, never blocks.
// -----------------------------------------------------------------------------

void DSPManager::dataCallback(ma_device* device, void* output, const void* /*input*/, ma_uint32 frameCount)
{
    static_cast<DSPManager*>(device->pUserData)->render(static_cast<int32_t*>(output), frameCount);
}

// Silence for the device: zeros for PCM; the DSD idle pattern for DSD, with
// DoP markers that keep alternating (zeros there would be a full-scale click).
void DSPManager::fillSilence(int32_t* out, std::size_t frames)
{
    const std::size_t ch = m_channels;
    if (m_outputDsd == StreamInfo::Dsd::None) {
        std::memset(out, 0, frames * ch * sizeof(int32_t));
        return;
    }
    for (std::size_t f = 0; f < frames; ++f) {
        const int32_t word = DsdDecoder::silence(m_dsdPacking, m_cbDopPhase);
        for (std::size_t c = 0; c < ch; ++c)
            out[f * ch + c] = word;
        m_cbDopPhase = !m_cbDopPhase;
    }
}

void DSPManager::render(int32_t* out, uint32_t frameCount)
{
    renderBlock(out, frameCount);
    if (const OutputTap tap = m_tap.load(std::memory_order_acquire))
        tap(m_tapUser.load(std::memory_order_relaxed), out, frameCount, m_monoToStereo ? 2u : m_channels);
}

void DSPManager::renderBlock(int32_t* out, uint32_t frameCount)
{
#if defined(__linux__)
    if (!m_tidKnown.exchange(true, std::memory_order_relaxed))
        m_audioTid.store(static_cast<int64_t>(::syscall(SYS_gettid)), std::memory_order_relaxed);
#endif
    const std::size_t ch = m_channels;
    const std::size_t wanted = static_cast<std::size_t>(frameCount) * ch;
    const auto expandMono = [&] {
        // Mono on a device that only takes stereo: each sample copied to both
        // channels, last frame first so nothing is overwritten before it is read.
        if (m_monoToStereo)
            for (std::size_t f = frameCount; f-- > 0;)
                out[2 * f] = out[2 * f + 1] = out[f];
    };

    // De-click: muted, the ring is left alone (pause, seek, track change).
    const int mute = m_mute.load(std::memory_order_acquire);
    if (mute != m_cbLastMute) {
        if (mute == FadingOut)
            m_cbRampPos = 0;
        m_cbLastMute = mute;
    }
    if (mute == Muted) {
        fillSilence(out, frameCount);
        m_mutedFrames.fetch_add(frameCount, std::memory_order_relaxed);
        m_muteAck.store(true, std::memory_order_release);
        expandMono();
        return;
    }
    // Resync delay: silence while the DAC locks to a new rate, music after.
    if (const uint64_t lead = m_leadSilence.load(std::memory_order_relaxed); lead > 0 && mute == Open) {
        fillSilence(out, frameCount);
        m_leadSilence.store(lead > frameCount ? lead - frameCount : 0, std::memory_order_relaxed);
        expandMono();
        return;
    }
    const std::size_t rampFrames = m_rampFrames.load(std::memory_order_relaxed);
    std::size_t limit = wanted;
    if (mute == FadingOut) // only what the fade needs: a pause resumes where it faded
        limit = m_outputDsd != StreamInfo::Dsd::None ? 0 : (rampFrames - std::min(m_cbRampPos, rampFrames)) * ch;

    // Whole frames only, so channels can never rotate after an underrun.
    const std::size_t available = (std::min(m_ring.availableToRead(), limit) / ch) * ch;
    const std::size_t got = m_ring.read(out, std::min(wanted, available));
    m_framesPlayed.fetch_add(got / ch, std::memory_order_relaxed);
    if (m_reorder) {
        // Source order -> the device's: whole samples moved, values untouched.
        int32_t frame[MA_MAX_CHANNELS];
        for (std::size_t f = 0; f < got / ch; ++f) {
            int32_t* p = out + f * ch;
            std::copy_n(p, ch, frame);
            for (std::size_t c = 0; c < ch; ++c)
                p[c] = frame[m_channelOrder[c]];
        }
    }
    if (m_outputDsd == StreamInfo::Dsd::Dop) {
        // The DoP markers are stamped here, on what actually leaves for the
        // DAC: they alternate without fail across underrun fills, joins and
        // seeks, whatever the count of frames in between. The 16 DSD bits of
        // each sample are not touched.
        for (std::size_t f = 0; f < got / ch; ++f) {
            const uint32_t marker = DsdDecoder::silence(DsdPacking::Dop, m_cbDopPhase) & 0xFF000000u;
            for (std::size_t c = 0; c < ch; ++c) {
                auto& s = out[f * ch + c];
                s = static_cast<int32_t>((static_cast<uint32_t>(s) & 0x00FFFFFFu) | marker);
            }
            m_cbDopPhase = !m_cbDopPhase;
        }
    }

    if (mute == FadingOut) {
        // A raised-cosine fade over a few ms, then silence. DSD words cannot
        // be scaled: DSD goes straight to its silence pattern.
        if (m_outputDsd == StreamInfo::Dsd::None)
            applyRamp(out, got / ch, m_cbRampPos, true);
        if (got < limit || m_outputDsd != StreamInfo::Dsd::None)
            m_cbRampPos = rampFrames; // nothing more to fade
        fillSilence(out + got, (wanted - got) / ch);
        if (m_cbRampPos >= rampFrames) {
            m_mutedFrames.store((wanted - got) / ch, std::memory_order_relaxed);
            m_mute.store(Muted, std::memory_order_release);
            m_cbLastMute = Muted;
            m_muteAck.store(true, std::memory_order_release);
        }
        expandMono();
        return;
    }
    if (m_rampIn.exchange(false, std::memory_order_acq_rel)) {
        m_cbRampingIn = m_outputDsd == StreamInfo::Dsd::None;
        m_cbRampInPos = 0;
    }
    if (m_cbRampingIn) {
        applyRamp(out, got / ch, m_cbRampInPos, false);
        m_cbRampingIn = m_cbRampInPos < rampFrames;
    }

    if (got < wanted) {
        fillSilence(out + got, (wanted - got) / ch);
        if (m_decoderEof.load(std::memory_order_acquire)) {
            // The last samples are still in the device's buffer: the end is
            // only reached once that much silence has followed them, so a stop
            // right after it never cuts the final note.
            const uint64_t silent = m_tailFrames.fetch_add((wanted - got) / ch, std::memory_order_relaxed)
                                  + (wanted - got) / ch;
            if (silent >= m_latencyFrames.load(std::memory_order_relaxed))
                m_outputDrained.store(true, std::memory_order_relaxed);
        } else {
            m_underruns.fetch_add(1, std::memory_order_relaxed);
        }
    }

    // Visualiser tap: a mono float copy of what was just handed to the device
    // (PCM only: DSD words are not sample values).
    std::atomic<float>* analysis = m_analysis.load(std::memory_order_acquire);
    if (analysis && m_analysisOn.load(std::memory_order_relaxed) && ch > 0 && m_outputDsd == StreamInfo::Dsd::None) {
        constexpr float kScale = 1.0f / 2147483648.0f;
        const float perChannel = kScale / static_cast<float>(ch);
        uint64_t w = m_analysisWritten.load(std::memory_order_relaxed);
        for (uint32_t f = 0; f < frameCount; ++f, ++w) {
            float sum = 0.0f;
            for (std::size_t c = 0; c < ch; ++c)
                sum += static_cast<float>(out[f * ch + c]);
            analysis[w & (kAnalysisSize - 1)].store(sum * perChannel, std::memory_order_relaxed);
        }
        m_analysisWritten.store(w, std::memory_order_release);
    }

    expandMono();
}

// Scales `frames` interleaved frames along a raised-cosine ramp of
// m_rampFrames, from position `pos` (advanced): 1 -> 0 when `down` (and
// silence past its end), 0 -> 1 otherwise (untouched past its end).
void DSPManager::applyRamp(int32_t* out, std::size_t frames, std::size_t& pos, bool down) const
{
    const std::size_t ch = m_channels;
    const std::size_t len = m_rampFrames.load(std::memory_order_relaxed);
    const double lsb = m_deviceDepth >= 32 ? 1.0 : std::ldexp(1.0, 32 - m_deviceDepth);
    for (std::size_t f = 0; f < frames; ++f, ++pos) {
        if (pos >= len) {
            if (!down)
                return;
            std::fill_n(out + f * ch, (frames - f) * ch, 0);
            pos += frames - f;
            return;
        }
        const double rise = 0.5 - 0.5 * std::cos(std::numbers::pi * static_cast<double>(pos) / static_cast<double>(len));
        const double g = down ? 1.0 - rise : rise;
        // Scaled samples go back onto the device's grid with TPDF dither, as
        // everything else that reaches it (the low bits would otherwise be
        // truncated by the format conversion).
        for (std::size_t c = 0; c < ch; ++c) {
            const double v = out[f * ch + c] * g / lsb;
            const double d = rampNoise() - rampNoise();
            const double q = std::nearbyint(lsb > 1.0 ? v + d : v) * lsb;
            out[f * ch + c] = static_cast<int32_t>(std::clamp(q, -2147483648.0, 2147483647.0 - (lsb - 1.0)));
        }
    }
}

// Uniform [0, 1) for the fades' dither (device thread only).
double DSPManager::rampNoise() const
{
    m_rampRng ^= m_rampRng << 13;
    m_rampRng ^= m_rampRng >> 17;
    m_rampRng ^= m_rampRng << 5;
    return m_rampRng * (1.0 / 4294967296.0);
}

void DSPManager::setAnalysisEnabled(bool on)
{
    if (on && !m_analysisStorage) {
        m_analysisStorage = std::make_unique<std::atomic<float>[]>(kAnalysisSize);
        m_analysis.store(m_analysisStorage.get(), std::memory_order_release);
    }
    m_analysisOn.store(on, std::memory_order_relaxed);
}

bool DSPManager::recentOutput(float* dst, std::size_t count, std::size_t delayFrames) const
{
    const std::atomic<float>* analysis = m_analysis.load(std::memory_order_acquire);
    if (!analysis)
        return false;
    const uint64_t written = m_analysisWritten.load(std::memory_order_acquire);
    delayFrames = std::min(delayFrames, kAnalysisSize - count - 1);
    if (count >= kAnalysisSize || written < count + delayFrames)
        return false;
    const uint64_t first = written - delayFrames - count;
    for (std::size_t i = 0; i < count; ++i)
        dst[i] = analysis[(first + i) & (kAnalysisSize - 1)].load(std::memory_order_relaxed);
    return true;
}

// -----------------------------------------------------------------------------
// Worker thread
// -----------------------------------------------------------------------------

void DSPManager::startWorker()
{
    m_quit.store(false);
    {
        std::lock_guard lock(m_parkMutex);
        m_parkRequested = false;
        m_parked = false;
        m_parkFlag.store(false);
    }
    m_worker = std::thread(&DSPManager::workerLoop, this);
}

void DSPManager::stopWorker()
{
    if (!m_worker.joinable())
        return;
    {
        std::lock_guard lock(m_parkMutex);
        m_quit.store(true, std::memory_order_release);
    }
    m_parkCv.notify_all();
    m_worker.join();
}

void DSPManager::parkWorker()
{
    if (!m_worker.joinable())
        return;
    std::unique_lock lock(m_parkMutex);
    m_parkRequested = true;
    m_parkFlag.store(true, std::memory_order_release);
    m_parkCv.notify_all(); // wakes a worker resting between blocks (rest())
    m_parkCv.wait(lock, [this] { return m_parked; });
}

// The worker waits for the device or for work: woken early by a park (seek,
// stop, unload) or the end, so a long rest never delays them.
void DSPManager::rest(std::chrono::nanoseconds duration)
{
    std::unique_lock lock(m_parkMutex);
    m_parkCv.wait_for(lock, duration, [this] { return m_parkRequested || m_quit.load(std::memory_order_acquire); });
}

void DSPManager::unparkWorker()
{
    if (!m_worker.joinable())
        return;
    {
        std::lock_guard lock(m_parkMutex);
        m_parkRequested = false;
        m_parkFlag.store(false, std::memory_order_release);
    }
    m_parkCv.notify_all();
}

bool DSPManager::interrupted() const
{
    return m_parkFlag.load(std::memory_order_acquire) || m_quit.load(std::memory_order_acquire);
}

void DSPManager::workerLoop()
{
#if defined(__SSE__) || defined(_M_X64)
    // Denormals flushed to zero on this thread (FTZ, and DAZ for the inputs):
    // filter states decaying through digital silence otherwise fall into
    // subnormal numbers, where the EQ, crossfeed and convolver run ~50 times
    // slower. Values that small (below 2^-1022) are nothing to the output.
    _mm_setcsr(_mm_getcsr() | 0x8040);
#endif
    while (!m_quit.load(std::memory_order_acquire)) {
        if (m_parkFlag.load(std::memory_order_acquire)) {
            std::unique_lock lock(m_parkMutex);
            m_parked = true;
            m_parkCv.notify_all();
            m_parkCv.wait(lock, [this] { return !m_parkRequested || m_quit.load(); });
            m_parked = false;
            continue;
        }
        if (!produceBlock())
            rest(kIdleSleep);
    }
}

// Decodes and processes one block. Returns false when there was nothing to do
// (ring full or end of stream) so the caller can sleep.
bool DSPManager::produceBlock()
{
    if (m_decoderEof.load(std::memory_order_relaxed))
        return false;
    if (m_trackEnded) {
        switch (spliceNext()) {
        case Splice::Done:
            m_trackEnded = false;
            break; // carry on with the next track in this same block
        case Splice::Wait:
            return false; // the previous splice has not been heard yet
        case Splice::None:
            finishStream();
            return true;
        }
    }

    const std::size_t ch = m_channels;
    if (m_ring.availableToWrite() / ch < kDecodeBlockFrames)
        return false;

    // Pick up parameter changes at a block boundary.
    m_stretch.applyPendingChanges();
    const bool raw = m_workerDsd != StreamInfo::Dsd::None; // DSD words: never touched
    const bool stActive = !raw && !m_stretch.isNeutral();
    const bool resampling = m_resampler.active();
    const bool hdcd = m_workerHdcd && m_hdcd;
    // Integers the device holds whole go out as is (HDCD's 20-bit output
    // too): no float round trip. Wider ones (24-bit on a 16-bit device,
    // 32-bit FLAC, lossy and DSD-converted words) are requantised with
    // dither instead of having their low bits cut off.
    const int sourceBits = hdcd ? 24 : m_workerBits > 0 ? static_cast<int>(m_workerBits) : 32;
    const bool fits = sourceBits <= m_deviceDepth;
    bool bypass = raw || (dspIsNeutral() && !resampling && fits);
    if (m_snapGains) {
        m_appliedGain = bypass ? 1.0 : m_volume.load(std::memory_order_relaxed) * m_trackGain.load(std::memory_order_relaxed);
        m_appliedHeadroom = bypass ? 1.0 : m_headroom.load(std::memory_order_relaxed);
        m_snapGains = false;
    }
    // Back to the untouched path only once the gains have glided to unity:
    // one more processed block takes them there.
    m_gainsToUnity = bypass && !raw && (m_appliedGain != 1.0 || m_appliedHeadroom != 1.0);
    if (m_gainsToUnity)
        bypass = false;
    m_bypassActive.store(bypass, std::memory_order_relaxed);
    m_quantizer.setDither(static_cast<DitherMode>(m_ditherMode.load(std::memory_order_relaxed)));

    if (stActive != m_stWasActive) {
        // Entering time-stretch: start from a clean engine. Leaving it: drop
        // its latency buffer (stale audio must never be replayed later) and
        // its memory.
        if (stActive)
            m_stretch.clear();
        else
            m_stretch.release();
        m_stWasActive = stActive;
    }
    {
        std::lock_guard lock(m_convMutex);
        if (m_convolverPending) {
            m_convolver = std::move(m_pendingConvolver);
            m_convolverPending = false;
            m_convActive.store(m_convolver != nullptr, std::memory_order_relaxed);
        }
    }
    // Crossfeed coefficients follow the preset and the worker's rate.
    if (const int preset = m_crossfeedPreset.load(std::memory_order_relaxed); preset != m_crossfeedFor) {
        m_crossfeed.setup(m_channels == 2 ? static_cast<Crossfeed::Preset>(preset) : Crossfeed::Preset::Off, m_workerRate);
        m_crossfeedFor = preset;
    }
    if (m_wasBypassed && !bypass) {
        m_eq.reset(); // filter memory is stale after bypassed blocks
        m_crossfeed.reset();
    }
    syncLimiter();
    if (bypass && !m_wasBypassed && !flushLimiter())
        return false; // the limiter's held frames go out before the untouched ones
    m_wasBypassed = bypass;
    if (!bypass)
        ensureBuffers(stActive);

    // The processed path reads doubles: a float source (MP3, AAC, Opus, float
    // WAV, DSD as PCM) keeps what lies above full scale for the gain stages
    // to lower, instead of having it clipped at the decoder. HDCD works on
    // the integers.
    const bool readFloat = !bypass && !hdcd;
    const auto frames = static_cast<std::size_t>(readFloat ? m_decoder->readDouble(m_doubleBuf.data(), kDecodeBlockFrames)
                                                           : m_decoder->readS32(m_decodeBuf.data(), kDecodeBlockFrames));
    m_decodedFrames.fetch_add(frames, std::memory_order_relaxed);
    if (hdcd && frames > 0) {
        // 16-bit samples in, 32-bit out at -6 dB, so peak extension has room
        // above the original full scale (libhdcd's convention, as in FFmpeg).
        int* s = reinterpret_cast<int*>(m_decodeBuf.data());
        static_assert(sizeof(int) == sizeof(int32_t));
        for (std::size_t i = 0; i < frames * 2; ++i)
            s[i] >>= 16;
        hdcd_process(m_hdcd, s, static_cast<int>(frames));
    }
    const bool endOfStream = frames < kDecodeBlockFrames;

    bool ok = true;
    if (frames > 0) {
        if (bypass) {
            // Bit-perfect: decoded integers go to the device untouched (the
            // linear-phase EQ keeps them in mind, to engage seamlessly).
            if (!raw && m_eq.linearPhase())
                m_linearEq.remember(m_decodeBuf.data(), frames);
            ok = pushToRing(m_decodeBuf.data(), frames);
        } else if (stActive) {
            if (readFloat)
                for (std::size_t i = 0; i < frames * ch; ++i)
                    m_floatIn[i] = static_cast<float>(m_doubleBuf[i]);
            else
                s32ToFloat(m_decodeBuf.data(), m_floatIn.data(), frames * ch);
            m_stretch.putSamples(m_floatIn.data(), static_cast<uint32_t>(frames));
            ok = drainStretch();
        } else {
            // EQ/volume/resampling only: double from the source on.
            if (!readFloat)
                s32ToDouble(m_decodeBuf.data(), m_doubleBuf.data(), frames * ch);
            ok = processAndPush(m_doubleBuf.data(), frames);
        }
    }

    // Whatever comes next (a spliced track or the end) is decided on the
    // following call, once this block is fully in the ring.
    if (endOfStream && ok)
        m_trackEnded = true;
    return true;
}

// End of the last track: flush the DSP tails, then let the device drain.
bool DSPManager::finishStream()
{
    bool ok = true;
    const bool raw = m_workerDsd != StreamInfo::Dsd::None; // no PCM tails to flush into DSD
    if (!raw && m_stWasActive) {
        m_stretch.flush();
        ok = drainStretch();
    }
    // The filters still hold their delay and their tails: play them out.
    std::size_t left = raw ? 0 : (m_convolver ? m_convolver->tail() : 0) + (m_linearEq.active() ? m_linearEq.tail() : 0);
    if (left > 0)
        ensureBuffers(false);
    while (ok && left > 0) {
        const std::size_t n = std::min(left, kDecodeBlockFrames);
        std::fill_n(m_doubleBuf.data(), n * m_channels, 0.0);
        ok = processAndPush(m_doubleBuf.data(), n);
        left -= n;
    }
    if (ok && !raw && m_resampler.active())
        ok = drainResampler();
    if (ok && !raw)
        ok = flushLimiter();
    if (ok && m_toDsd) { // the last partial DSD word
        int32_t last[kMaxChannels];
        if (m_modulator.flush(last))
            ok = pushToRing(last, 1);
    }
    if (ok)
        m_decoderEof.store(true, std::memory_order_release);
    return ok;
}

// Worker: continues with the queued track when its format matches the device
// as it is open. The DSP chain keeps running across the join, so the tracks
// meet sample-exactly whether the path is bit-perfect or processed.
DSPManager::Splice DSPManager::spliceNext()
{
    std::lock_guard lock(m_nextMutex);
    if (!m_next.decoder)
        return Splice::None;
    const StreamInfo& info = m_next.decoder->info();
    if (info.sampleRate != m_workerRate || info.channels != m_workerChannels || info.dsd != m_workerDsd) {
        m_next = {}; // needs the device reopened: the caller loads it after the end
        return Splice::None;
    }
    if (m_splicePoint.load(std::memory_order_acquire) >= 0)
        return Splice::Wait;

    // DoP: the next track's first marker follows this one's last.
    if (auto* current = dynamic_cast<DsdDecoder*>(m_decoder.get()))
        if (auto* next = dynamic_cast<DsdDecoder*>(m_next.decoder.get()))
            next->setDopPhase(current->dopPhase());
    m_spliceSourceEnd.store(m_decodedFrames.load(std::memory_order_relaxed), std::memory_order_relaxed);
    m_previous = {std::move(m_decoder), {}, m_trackGain.load(std::memory_order_relaxed), m_preload};
    m_preload = m_next.preload;
    m_decoder = std::move(m_next.decoder);
    m_trackGain.store(m_next.gain, std::memory_order_relaxed); // from the first sample of the new track
    m_snapGains = true; // exactly there, not glided into
    m_workerHdcd = m_next.hdcd;
    m_workerBits = info.bitsPerSample;
    m_splicedHdcd = m_next.hdcd;
    if (m_workerHdcd) {
        if (!m_hdcd)
            m_hdcd = hdcd_new();
        hdcd_reset_ext(m_hdcd, static_cast<int>(info.sampleRate), 16);
    }
    m_splicedPath = std::move(m_next.path);
    m_splicedTrack = info;
    m_next = {};
    m_decodedFrames.store(0, std::memory_order_relaxed);
    // The join reaches the ring once the music the filters still hold is out.
    m_splicePoint.store(static_cast<int64_t>(m_framesPushed.load(std::memory_order_acquire) + pendingDspFrames()),
                        std::memory_order_release);
    return Splice::Done;
}

void DSPManager::setNext(const std::filesystem::path& path, double gain)
{
    std::unique_ptr<Decoder> decoder;
    if (!path.empty() && m_outputDsd != StreamInfo::Dsd::None && DsdDecoder::isDsdExtension(path))
        decoder = DsdDecoder::open(path, m_dsdPacking, nullptr); // joins only at the same DSD rate
    else if (!path.empty())
        decoder = Decoder::open(path, nullptr); // opened here, off the worker
    std::shared_ptr<Preload> preload;
    if (!path.empty() && outputConfig().playFromMemory)
        preload = startPreload(path, true);
    const bool hdcd = decoder && outputConfig().hdcd && decoder->info().bitsPerSample == 16 && decoder->info().channels == 2
                   && decoder->info().dsd == StreamInfo::Dsd::None && detectHdcd(path);
    std::lock_guard lock(m_nextMutex);
    m_next = {std::move(decoder), path, gain, std::move(preload), hdcd};
}

bool DSPManager::setConvolution(const std::filesystem::path& impulseResponse, std::string* error)
{
    std::shared_ptr<const ImpulseResponse> ir;
    if (!impulseResponse.empty()) {
        auto loaded = std::make_shared<ImpulseResponse>();
        if (!ImpulseResponse::load(impulseResponse, *loaded, error))
            return false;
        ir = std::move(loaded);
    }
    {
        std::lock_guard lock(m_convMutex);
        m_ir = std::move(ir);
    }
    if (state() != State::Empty)
        fitConvolution();
    return true;
}

// Builds the filter for the loaded track (control thread); the worker takes it
// at its next block. DSD and tracks it does not fit run without it.
void DSPManager::fitConvolution()
{
    std::shared_ptr<const ImpulseResponse> ir;
    {
        std::lock_guard lock(m_convMutex);
        ir = m_ir;
    }
    ConvolutionStatus status;
    std::unique_ptr<Convolver> conv;
    if (ir) {
        status.file = ir->path;
        status.irChannels = ir->channels;
        if (m_track.dsd != StreamInfo::Dsd::None)
            status.problem = "DSD is sent untouched";
        else
            conv = Convolver::create(*ir, m_track.sampleRate, m_track.channels, &status.problem);
        if (conv) {
            status.active = true;
            status.taps = conv->taps();
            status.resampledFrom = conv->resampledFrom();
        }
    }
    std::lock_guard lock(m_convMutex);
    m_pendingConvolver = std::move(conv);
    m_convolverPending = true;
    m_convStatus = status;
}

DSPManager::ConvolutionStatus DSPManager::convolutionStatus() const
{
    std::lock_guard lock(m_convMutex);
    return m_convStatus;
}

bool DSPManager::detectHdcd(const std::filesystem::path& path)
{
    auto decoder = Decoder::open(path, nullptr);
    if (!decoder)
        return false;
    const StreamInfo& info = decoder->info();
    if (info.channels != 2 || info.bitsPerSample != 16 || info.dsd != StreamInfo::Dsd::None)
        return false;
    hdcd_simple* ctx = hdcd_new();
    bool found = false;
    if (ctx && hdcd_reset_ext(ctx, static_cast<int>(info.sampleRate), 16)) {
        constexpr uint64_t kBlock = 4096;
        std::vector<int32_t> in(kBlock * 2);
        std::vector<int> samples(kBlock * 2);
        const uint64_t limit = static_cast<uint64_t>(info.sampleRate) * 5; // the first 5 s
        for (uint64_t done = 0; done < limit && !found;) {
            const uint64_t n = decoder->readS32(in.data(), kBlock);
            if (n == 0)
                break;
            for (std::size_t i = 0; i < n * 2; ++i)
                samples[i] = in[i] >> 16;
            hdcd_process(ctx, samples.data(), static_cast<int>(n));
            found = hdcd_detected(ctx) != HDCD_NONE;
            done += n;
        }
    }
    hdcd_free(ctx);
    return found;
}

std::shared_ptr<DSPManager::Preload> DSPManager::startPreload(const std::filesystem::path& path, bool async)
{
    auto job = std::make_shared<Preload>();
    auto run = [job, path] {
        std::filesystem::path file = path;
        std::filesystem::path sheetPath;
        int number = 0;
        CueSheet sheet;
        if (CueSheet::splitTrackPath(path.string(), sheetPath, number) && CueSheet::parse(sheetPath, sheet))
            if (const auto* track = sheet.track(number))
                file = track->file; // a CD image track: the whole image goes to RAM
        auto preloaded = FilePreload::load(file, kMaxPreloadBytes);
        std::lock_guard lock(job->mutex);
        job->file = std::move(preloaded);
    };
    if (async)
        std::thread(run).detach(); // owns a reference: safe if the track is dropped first
    else
        run();
    return job;
}

int DSPManager::memoryState() const
{
    std::shared_ptr<Preload> job;
    {
        std::lock_guard lock(m_nextMutex);
        job = m_preload;
    }
    if (!job)
        return 0;
    std::lock_guard lock(job->mutex);
    return !job->file ? 0 : job->file->locked() ? 2 : 1;
}

void DSPManager::setGains(double current, double next)
{
    std::lock_guard lock(m_nextMutex);
    if (m_splicePoint.load(std::memory_order_acquire) >= 0) {
        // The worker is already past the join: it decodes the queued track.
        m_trackGain.store(next, std::memory_order_relaxed);
        m_previous.gain = current;
    } else {
        m_trackGain.store(current, std::memory_order_relaxed);
        m_next.gain = next;
    }
}

bool DSPManager::takeTrackChange(std::filesystem::path* path)
{
    const int64_t splice = m_splicePoint.load(std::memory_order_acquire);
    if (splice < 0 || m_framesPlayed.load(std::memory_order_relaxed) < static_cast<uint64_t>(splice))
        return false;
    std::lock_guard lock(m_nextMutex);
    if (path)
        *path = m_splicedPath;
    m_track = m_splicedTrack;
    m_trackHdcd = m_splicedHdcd;
    updateBitPerfectPath();
    m_previous = {};
    m_splicedPath.clear();
    m_splicePoint.store(-1, std::memory_order_release);
    return true;
}

void DSPManager::clearGapless()
{
    std::lock_guard lock(m_nextMutex);
    m_next = {};
    m_previous = {};
    m_splicedPath.clear();
    m_splicePoint.store(-1);
}

bool DSPManager::drainStretch()
{
    const std::size_t ch = m_channels;
    for (;;) {
        const uint32_t got = m_stretch.receiveSamples(m_floatOut.data(), static_cast<uint32_t>(kStChunkFrames));
        if (got == 0)
            return true;
        floatToDouble(m_floatOut.data(), m_doubleBuf.data(), got * ch);
        if (!processAndPush(m_doubleBuf.data(), got))
            return false;
    }
}

bool DSPManager::drainResampler()
{
    for (;;) {
        const std::size_t got = m_resampler.drain();
        if (got == 0)
            return true;
        if (!limitAndPush(m_resampler.output(), got))
            return false;
    }
}

std::size_t DSPManager::pendingDspFrames() const
{
    if (m_wasBypassed)
        return 0;
    const std::size_t held = (m_convolver ? Convolver::kBlock : 0) + m_linearEq.latency();
    const std::size_t converting = m_resampler.active() ? static_cast<std::size_t>(std::lround(m_resampler.delayFrames())) : 0;
    return (m_workerRate ? static_cast<std::size_t>(static_cast<uint64_t>(held) * m_outputRate / m_workerRate) : held) + converting
        + (m_limiterRunning ? m_limiter.latency() : 0);
}

bool DSPManager::processAndPush(double* samples, std::size_t frames)
{
    applyGain(samples, frames, m_appliedHeadroom, m_gainsToUnity ? 1.0 : m_headroom.load(std::memory_order_relaxed));
    const bool linear = m_eq.linearPhase();
    if (linear || m_linearEq.active()) {
        m_linearEq.update(linear && !m_eq.isNeutral(), m_eq); // engaging: primed with what went before this block
        m_linearEq.remember(samples, frames);
    }
    if (!m_linearEq.active())
        return processRest(samples, frames, !linear);
    // Linear-phase EQ: what comes out of the filter (nothing yet while it
    // engages), then what it hands back unfiltered once it is done.
    m_linearEq.process(samples, frames);
    m_linearEqLatency.store(m_linearEq.latency(), std::memory_order_relaxed);
    std::vector<double>& filtered = m_linearEq.filtered();
    std::vector<double>& passed = m_linearEq.passed();
    bool ok = filtered.empty() || processRest(filtered.data(), filtered.size() / m_channels, false);
    if (ok && !passed.empty())
        ok = processRest(passed.data(), passed.size() / m_channels, !linear);
    return ok;
}

bool DSPManager::processRest(double* samples, std::size_t frames, bool biquads)
{
    if (m_convolver)
        m_convolver->process(samples, frames);
    if (biquads)
        m_eq.process(samples, frames);
    if (m_crossfeed.active())
        m_crossfeed.process(samples, frames);

    const double vol = m_volume.load(std::memory_order_relaxed) * m_trackGain.load(std::memory_order_relaxed);
    applyGain(samples, frames, m_appliedGain, m_gainsToUnity ? 1.0 : vol);

    if (m_resampler.active()) {
        const std::size_t got = m_resampler.process(samples, frames);
        m_resamplerDelay.store(m_resampler.delayFrames(), std::memory_order_relaxed);
        return limitAndPush(m_resampler.output(), got);
    }
    return limitAndPush(samples, frames);
}

// The last stage of the processed path: the true-peak limiter when it is
// on, then the DSD modulator or the quantiser.
bool DSPManager::limitAndPush(const double* samples, std::size_t frames)
{
    if (m_limiterRunning) {
        const std::size_t got = m_limiter.process(samples, frames, m_limitBuf);
        m_limiterReduction.store(m_limiter.lastReductionDb(), std::memory_order_relaxed);
        if (got == 0)
            return true;
        return m_toDsd ? modulateAndPush(m_limitBuf.data(), got) : quantizeAndPush(m_limitBuf.data(), got);
    }
    return m_toDsd ? modulateAndPush(samples, frames) : quantizeAndPush(samples, frames);
}

// What the limiter still holds goes out (the end of the track, or the path
// going back to untouched: nothing may stay behind).
bool DSPManager::flushLimiter()
{
    if (!m_limiterRunning || m_limiter.latency() == 0)
        return true;
    const std::size_t got = m_limiter.flush(m_limitBuf);
    m_limiterReduction.store(0.0, std::memory_order_relaxed);
    if (got == 0)
        return true;
    return m_toDsd ? modulateAndPush(m_limitBuf.data(), got) : quantizeAndPush(m_limitBuf.data(), got);
}

void DSPManager::syncLimiter()
{
    const bool on = m_limiterOn.load(std::memory_order_relaxed) && m_workerDsd == StreamInfo::Dsd::None;
    const double ceiling = m_limiterCeilingDb.load(std::memory_order_relaxed);
    const uint32_t rate = m_toDsd ? m_dsdRate : m_outputRate; // the modulator takes PCM at the 1-bit rate
    if (!on) {
        if (m_limiterRunning) {
            flushLimiter();
            m_limiter.release();
            m_limitBuf = {};
            m_limiterRunning = false;
            m_limiterReduction.store(0.0, std::memory_order_relaxed);
        }
        return;
    }
    if (!m_limiterRunning || m_limiter.sampleRate() != rate || m_limiter.channels() != m_channels) {
        if (m_limiterRunning)
            flushLimiter();
        m_limiter.setup(rate, m_channels, ceiling);
        m_limiterRunning = true;
    } else if (m_limiter.ceilingDb() != ceiling) {
        m_limiter.setCeilingDb(ceiling);
    }
}

// Multiplies a block by `target`. A gain that changed since the last block
// (volume, ReplayGain, headroom) glides there sample by sample across the
// block instead of stepping at its start: a step is a click (a 1 dB step on a
// -6 dBFS passage is a -26 dBFS click), and dragging the volume a zipper.
void DSPManager::applyGain(double* samples, std::size_t frames, double& applied, double target) const
{
    const std::size_t ch = m_channels;
    if (applied == target) {
        if (target != 1.0)
            for (std::size_t i = 0, n = frames * ch; i < n; ++i)
                samples[i] *= target;
        return;
    }
    if (frames == 0)
        return;
    const double step = (target - applied) / static_cast<double>(frames);
    double g = applied;
    for (std::size_t f = 0; f < frames; ++f) {
        g = f + 1 == frames ? target : g + step;
        for (std::size_t c = 0; c < ch; ++c)
            samples[f * ch + c] *= g;
    }
    applied = target;
}

// PCM at the DSD rate -> 1-bit stream, packed for the DSD output.
bool DSPManager::modulateAndPush(const double* samples, std::size_t frames)
{
    const std::size_t words = frames / m_modulator.bitsPerWord() + 1;
    if (m_dsdWords.size() < words * m_channels)
        m_dsdWords.resize(words * m_channels);
    const std::size_t got = m_modulator.process(samples, frames, m_dsdWords.data());
    m_modulatorResets.store(m_modulator.resets(), std::memory_order_relaxed);
    return pushToRing(m_dsdWords.data(), got);
}

bool DSPManager::quantizeAndPush(const double* samples, std::size_t frames)
{
    const std::size_t count = frames * m_channels;
    if (m_outBuf.size() < count)
        m_outBuf.resize(count);
    m_quantizer.process(samples, m_outBuf.data(), count);
    if (const uint64_t clipped = m_quantizer.takeClipped())
        m_clipped.fetch_add(clipped, std::memory_order_relaxed);
    return pushToRing(m_outBuf.data(), frames);
}

void DSPManager::ensureBuffers(bool stretch)
{
    const std::size_t ch = m_channels;
    if (m_doubleBuf.empty())
        m_doubleBuf.assign(std::max(kDecodeBlockFrames, kStChunkFrames) * ch, 0.0);
    if (stretch && m_floatIn.empty()) {
        m_floatIn.assign(kDecodeBlockFrames * ch, 0.0f);
        m_floatOut.assign(kStChunkFrames * ch, 0.0f);
    }
}

void DSPManager::releaseBuffers()
{
    if (m_limiterRunning) {
        m_limiter.release();
        m_limiterRunning = false;
    }
    m_limitBuf = {};
    m_floatIn = {};
    m_floatOut = {};
    m_doubleBuf = {};
    m_outBuf = {};
}

bool DSPManager::pushToRing(const int32_t* samples, std::size_t frames)
{
    std::size_t remaining = frames * m_channels;
    while (remaining > 0) {
        const std::size_t written = m_ring.write(samples, remaining);
        // The ring's size is no multiple of 6 or 8 channels: a write may end
        // mid-frame. Frames are counted once complete.
        m_pushedPartial += written;
        const std::size_t frames = m_pushedPartial / m_channels;
        m_pushedPartial -= frames * m_channels;
        m_framesPushed.fetch_add(frames, std::memory_order_release);
        samples += written;
        remaining -= written;
        if (remaining > 0) {
            if (interrupted())
                return false; // seek/stop/unload pending: the data is obsolete
            // ...and as long as the device takes to free the room for the
            // rest, up to a quarter of the ring: a wake-up every 20-40 ms
            // instead of every 2 ms, so the CPU can rest between them (the
            // ring still holds three quarters of its depth when it refills).
            const double rate = std::max<uint32_t>(m_outputRate, 8000);
            const double needed = static_cast<double>(remaining / m_channels) / rate;
            const double quarter = static_cast<double>(m_ring.capacity() / m_channels) / rate / 4.0;
            const auto wait = std::chrono::duration<double>(std::min(needed, quarter));
            rest(std::max<std::chrono::nanoseconds>(kRingFullSleepMin, std::chrono::duration_cast<std::chrono::nanoseconds>(wait)));
        }
    }
    return true;
}

} // namespace sonkore
