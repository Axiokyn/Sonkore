#include "AsioOutput.hpp"

#include "AsioConvert.hpp"

#if defined(_WIN32)
#include "AsioAbi.hpp"

#include <windows.h>
#include <objbase.h>
#endif

#include <algorithm>
#include <chrono>
#include <cstring>
#include <thread>

namespace sonkore {

std::atomic<AsioOutput*> AsioOutput::s_active{nullptr};

#if defined(_WIN32)

namespace {

using namespace asio;

void setError(std::string* error, const std::string& message)
{
    if (error)
        *error = message;
}

std::string narrow(const wchar_t* s)
{
    const int n = WideCharToMultiByte(CP_UTF8, 0, s, -1, nullptr, 0, nullptr, nullptr);
    std::string out(static_cast<std::size_t>(std::max(0, n - 1)), '\0');
    if (n > 1)
        WideCharToMultiByte(CP_UTF8, 0, s, -1, out.data(), n, nullptr, nullptr);
    return out;
}

std::wstring widen(const std::string& s)
{
    const int n = MultiByteToWideChar(CP_UTF8, 0, s.c_str(), -1, nullptr, 0);
    std::wstring out(static_cast<std::size_t>(std::max(0, n - 1)), L'\0');
    if (n > 1)
        MultiByteToWideChar(CP_UTF8, 0, s.c_str(), -1, out.data(), n);
    return out;
}

// The class id a driver registered under HKLM\SOFTWARE\ASIO\<name>\CLSID.
bool driverClsid(const std::string& name, CLSID& clsid)
{
    HKEY key = nullptr;
    const std::wstring path = L"SOFTWARE\\ASIO\\" + widen(name);
    if (RegOpenKeyExW(HKEY_LOCAL_MACHINE, path.c_str(), 0, KEY_READ, &key) != ERROR_SUCCESS)
        return false;
    wchar_t value[128] = {};
    DWORD size = sizeof(value) - sizeof(wchar_t);
    DWORD type = 0;
    const bool ok = RegQueryValueExW(key, L"CLSID", nullptr, &type, reinterpret_cast<BYTE*>(value), &size) == ERROR_SUCCESS
                 && type == REG_SZ && SUCCEEDED(CLSIDFromString(value, &clsid));
    RegCloseKey(key);
    return ok;
}

IASIO* createDriver(const std::string& name, std::string* error)
{
    CLSID clsid{};
    if (!driverClsid(name, clsid)) {
        setError(error, "ASIO driver \"" + name + "\" is not installed");
        return nullptr;
    }
    IASIO* driver = nullptr;
    // ASIO drivers answer to their class id as interface id.
    if (FAILED(CoCreateInstance(clsid, nullptr, CLSCTX_INPROC_SERVER, clsid, reinterpret_cast<void**>(&driver))) || !driver) {
        setError(error, "The ASIO driver \"" + name + "\" could not be loaded");
        return nullptr;
    }
    if (!driver->init(GetDesktopWindow())) {
        char message[124] = {};
        driver->getErrorMessage(message);
        setError(error, "The ASIO driver \"" + name + "\" did not start" + (message[0] ? std::string(": ") + message : std::string()));
        driver->Release();
        return nullptr;
    }
    return driver;
}

// COM for the calling thread, as ASIO drivers expect (apartment threaded).
bool initCom()
{
    return SUCCEEDED(CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED));
}

ASIOTime* onBufferSwitchTimeInfo(ASIOTime* params, long index, ASIOBool directProcess)
{
    AsioOutput::onBufferSwitch(index, directProcess);
    return params;
}

void onSampleRateDidChange(ASIOSampleRate) {}

ASIOCallbacks g_callbacks = {&AsioOutput::onBufferSwitch, &onSampleRateDidChange, &AsioOutput::onMessage, &onBufferSwitchTimeInfo};

constexpr uint32_t kStandardRates[] = {44100, 48000, 88200, 96000, 176400, 192000, 352800, 384000, 705600, 768000};

} // namespace

std::vector<std::string> AsioOutput::drivers()
{
    std::vector<std::string> names;
    HKEY key = nullptr;
    if (RegOpenKeyExW(HKEY_LOCAL_MACHINE, L"SOFTWARE\\ASIO", 0, KEY_READ, &key) != ERROR_SUCCESS)
        return names;
    wchar_t name[256];
    for (DWORD i = 0;; ++i) {
        DWORD length = 255;
        if (RegEnumKeyExW(key, i, name, &length, nullptr, nullptr, nullptr, nullptr) != ERROR_SUCCESS)
            break;
        name[length] = L'\0';
        CLSID clsid{};
        const std::string n = narrow(name);
        if (driverClsid(n, clsid))
            names.push_back(n);
    }
    RegCloseKey(key);
    std::sort(names.begin(), names.end());
    return names;
}

std::vector<uint32_t> AsioOutput::sampleRates(const std::string& driverName)
{
    std::vector<uint32_t> rates;
    const bool com = initCom();
    if (IASIO* driver = createDriver(driverName, nullptr)) {
        for (uint32_t r : kStandardRates)
            if (driver->canSampleRate(static_cast<ASIOSampleRate>(r)) == ASE_OK)
                rates.push_back(r);
        driver->Release();
    }
    if (com)
        CoUninitialize();
    return rates;
}

AsioOutput::~AsioOutput()
{
    close();
}

bool AsioOutput::open(const std::string& driverName, Mode mode, uint32_t channels, uint32_t rate, Render render, std::string* error)
{
    close();
    m_comInitialized = initCom();
    IASIO* driver = createDriver(driverName, error);
    if (!driver) {
        if (m_comInitialized)
            CoUninitialize();
        m_comInitialized = false;
        return false;
    }
    m_name = driverName;
    return openDriver(driver, mode, channels, rate, std::move(render), error);
}

bool AsioOutput::openDriver(IASIO* driver, Mode mode, uint32_t channels, uint32_t rate, Render render, std::string* error)
{
    if (!driver || channels == 0 || rate == 0) {
        if (driver)
            driver->Release();
        setError(error, "Invalid ASIO stream");
        return false;
    }
    if (s_active.load() && s_active.load() != this) {
        driver->Release();
        setError(error, "Another ASIO stream is running");
        return false;
    }
    m_driver = driver;
    m_mode = mode;
    m_channels = channels;
    m_render = std::move(render);
    if (m_name.empty()) {
        char name[32] = {};
        driver->getDriverName(name);
        m_name = name;
    }
    const auto fail = [&](const std::string& why) {
        setError(error, "ASIO \"" + m_name + "\": " + why);
        close();
        return false;
    };

    // PCM or DSD: set before the rate (a DSD rate means nothing in PCM mode).
    ASIOIoFormat format{};
    format.FormatType = mode == Mode::Dsd ? kASIODSDFormat : kASIOPCMFormat;
    if (mode == Mode::Dsd) {
        if (driver->future(kAsioCanDoIoFormat, &format) != ASE_SUCCESS || driver->future(kAsioSetIoFormat, &format) != ASE_SUCCESS)
            return fail("no native DSD mode");
        m_dsdFormatSet = true;
    } else {
        driver->future(kAsioSetIoFormat, &format); // drivers without DSD do not know the call: fine
    }

    long inputs = 0, outputs = 0;
    if (driver->getChannels(&inputs, &outputs) != ASE_OK || outputs < static_cast<long>(channels))
        return fail(std::to_string(channels) + " output channels needed, the driver has " + std::to_string(outputs));

    const auto wanted = static_cast<ASIOSampleRate>(rate);
    if (driver->canSampleRate(wanted) != ASE_OK)
        return fail("does not take " + std::to_string(rate) + " Hz");
    ASIOSampleRate current = 0;
    if ((driver->getSampleRate(&current) != ASE_OK || current != wanted) && driver->setSampleRate(wanted) != ASE_OK)
        return fail("could not switch to " + std::to_string(rate) + " Hz");

    m_types.assign(channels, 0);
    m_pcmBits = 32;
    m_pcmFloat = false;
    for (uint32_t c = 0; c < channels; ++c) {
        ASIOChannelInfo info{};
        info.channel = static_cast<long>(c);
        info.isInput = 0;
        if (driver->getChannelInfo(&info) != ASE_OK)
            return fail("channel information unavailable");
        const bool usable = mode == Mode::Dsd ? asio::isDsd(info.type) : asio::isSupportedPcm(info.type);
        if (!usable)
            return fail("sample type " + std::to_string(info.type) + " is not supported");
        m_types[c] = info.type;
        if (mode == Mode::Pcm) {
            m_pcmBits = std::min(m_pcmBits, asio::pcmBits(info.type));
            m_pcmFloat = m_pcmFloat || info.type == ASIOSTFloat32LSB || info.type == ASIOSTFloat32MSB
                      || info.type == ASIOSTFloat64LSB || info.type == ASIOSTFloat64MSB;
        }
    }

    long minSize = 0, maxSize = 0, preferred = 0, granularity = 0;
    if (driver->getBufferSize(&minSize, &maxSize, &preferred, &granularity) != ASE_OK || preferred <= 0)
        return fail("no buffer size");
    m_bufferSize = static_cast<uint32_t>(preferred);
    if (mode == Mode::Dsd && m_bufferSize % 32 != 0)
        return fail("DSD buffer of " + std::to_string(m_bufferSize) + " bits is not a whole number of 32-bit words");
    m_engineFrames = mode == Mode::Dsd ? m_bufferSize / 32 : m_bufferSize;

    std::vector<ASIOBufferInfo> infos(channels);
    for (uint32_t c = 0; c < channels; ++c) {
        infos[c].isInput = 0;
        infos[c].channelNum = static_cast<long>(c);
    }
    s_active.store(this);
    if (driver->createBuffers(infos.data(), static_cast<long>(channels), static_cast<long>(m_bufferSize), &g_callbacks) != ASE_OK) {
        s_active.store(nullptr);
        return fail("could not create its buffers");
    }
    m_buffersCreated = true;
    m_buffers.assign(2 * channels, nullptr);
    for (uint32_t c = 0; c < channels; ++c) {
        m_buffers[2 * c] = infos[c].buffers[0];
        m_buffers[2 * c + 1] = infos[c].buffers[1];
    }
    m_scratch.assign(static_cast<std::size_t>(m_engineFrames) * channels, 0);
    m_outputReady = driver->outputReady() == ASE_OK;

    long inputLatency = 0, outputLatency = 0;
    driver->getLatencies(&inputLatency, &outputLatency);
    const uint32_t latency = static_cast<uint32_t>(std::max(0L, outputLatency));
    m_bufferFrames = 2 * m_engineFrames + (mode == Mode::Dsd ? latency / 32 : latency);

    // Both halves silent until the engine fills them.
    for (uint32_t c = 0; c < channels; ++c)
        for (int half = 0; half < 2; ++half) {
            void* b = m_buffers[2 * c + half];
            if (mode == Mode::Dsd)
                asio::dsdSilence(b, m_bufferSize / 8, m_types[c]);
            else
                std::memset(b, 0, static_cast<std::size_t>(m_bufferSize) * asio::bytesPerSample(m_types[c]));
        }
    m_resetRequested.store(false);
    return true;
}

void AsioOutput::close()
{
    stop();
    if (m_driver) {
        if (m_buffersCreated)
            m_driver->disposeBuffers();
        if (m_dsdFormatSet) { // back to PCM for whoever opens it next
            ASIOIoFormat format{};
            format.FormatType = kASIOPCMFormat;
            m_driver->future(kAsioSetIoFormat, &format);
        }
        m_driver->Release();
        m_driver = nullptr;
    }
    AsioOutput* self = this;
    s_active.compare_exchange_strong(self, nullptr);
    m_buffersCreated = false;
    m_dsdFormatSet = false;
    m_buffers.clear();
    m_scratch = {};
    m_render = nullptr;
    if (m_comInitialized)
        CoUninitialize();
    m_comInitialized = false;
    m_name.clear();
}

bool AsioOutput::start()
{
    if (!m_driver)
        return false;
    if (m_running.load())
        return true;
    m_running.store(true);
    if (m_driver->start() != ASE_OK) {
        m_running.store(false);
        return false;
    }
    return true;
}

void AsioOutput::stop()
{
    if (!m_driver || !m_running.exchange(false))
        return;
    m_driver->stop();
    // A callback that began before stop() returned may still be filling.
    while (m_inCallback.load() > 0)
        std::this_thread::sleep_for(std::chrono::microseconds(200));
}

void AsioOutput::onBufferSwitch(long index, long /*directProcess*/)
{
    if (AsioOutput* self = s_active.load(std::memory_order_acquire))
        self->fill(index);
}

long AsioOutput::onMessage(long selector, long value, void* /*message*/, double* /*opt*/)
{
    switch (selector) {
    case kAsioSelectorSupported:
        return value == kAsioEngineVersion || value == kAsioResetRequest || value == kAsioResyncRequest
            || value == kAsioLatenciesChanged || value == kAsioSupportsTimeInfo;
    case kAsioEngineVersion:
        return 2;
    case kAsioResetRequest:
        if (AsioOutput* self = s_active.load())
            self->m_resetRequested.store(true);
        return 1;
    case kAsioResyncRequest:
    case kAsioLatenciesChanged:
        return 1;
    case kAsioSupportsTimeInfo:
        return 0; // plain bufferSwitch is enough
    default:
        return 0;
    }
}

void AsioOutput::fill(long index)
{
    m_inCallback.fetch_add(1, std::memory_order_acq_rel);
    if (m_running.load(std::memory_order_acquire) && m_render && (index == 0 || index == 1)) {
        m_render(m_scratch.data(), m_engineFrames);
        for (uint32_t c = 0; c < m_channels; ++c) {
            void* dst = m_buffers[2 * c + static_cast<std::size_t>(index)];
            if (m_mode == Mode::Dsd)
                asio::writeDsd(m_types[c], dst, m_scratch.data() + c, m_channels, m_engineFrames);
            else
                asio::writePcm(m_types[c], dst, m_scratch.data() + c, m_channels, m_engineFrames);
        }
        if (m_outputReady)
            m_driver->outputReady();
    }
    m_inCallback.fetch_sub(1, std::memory_order_acq_rel);
}

#else // Not Windows: no ASIO.

std::vector<std::string> AsioOutput::drivers() { return {}; }
std::vector<uint32_t> AsioOutput::sampleRates(const std::string&) { return {}; }
AsioOutput::~AsioOutput() = default;
bool AsioOutput::open(const std::string&, Mode, uint32_t, uint32_t, Render, std::string* error)
{
    if (error)
        *error = "ASIO is only available on Windows";
    return false;
}
bool AsioOutput::openDriver(asio::IASIO*, Mode, uint32_t, uint32_t, Render, std::string* error)
{
    return open({}, Mode::Pcm, 0, 0, {}, error);
}
void AsioOutput::close() {}
bool AsioOutput::start() { return false; }
void AsioOutput::stop() {}
void AsioOutput::onBufferSwitch(long, long) {}
long AsioOutput::onMessage(long, long, void*, double*) { return 0; }
void AsioOutput::fill(long) {}

#endif

} // namespace sonkore
