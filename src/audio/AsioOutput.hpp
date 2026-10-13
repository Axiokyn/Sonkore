#pragma once

#include <atomic>
#include <cstdint>
#include <functional>
#include <string>
#include <vector>

namespace sonkore {

namespace asio {
struct IASIO;
}

// Windows: an ASIO driver as output, the way pro-audio and Hi-Fi DACs are
// meant to be driven: no Windows mixer, the DAC at the track's own rate, and
// native DSD for the drivers that offer it (ASIO 2.2 DSD mode). The engine
// fills buffers through `render` exactly as for its other outputs: PCM as
// left-justified s32 frames, DSD as DsdPacking::NativeU32BE words (32 DSD
// bits per channel and frame, so frames run at the DSD rate / 32). Here they
// become the driver's own sample type, losslessly.
//
// Only one ASIO driver can run at a time (the callbacks carry no context).
class AsioOutput {
public:
    using Render = std::function<void(int32_t* out, uint32_t frames)>;
    enum class Mode { Pcm, Dsd };

    // Installed drivers, by name (HKLM\SOFTWARE\ASIO). Empty off Windows.
    static std::vector<std::string> drivers();
    // The PCM rates `driver` takes (asks the driver; it must not be open).
    static std::vector<uint32_t> sampleRates(const std::string& driver);

    AsioOutput() = default;
    ~AsioOutput();
    AsioOutput(const AsioOutput&) = delete;
    AsioOutput& operator=(const AsioOutput&) = delete;

    // `rate`: PCM sample rate, or the DSD bit rate (2822400 for DSD64). False,
    // with a reason, when the driver cannot play this exactly (another rate,
    // fewer channels, no DSD mode...): nothing is left open.
    bool open(const std::string& driver, Mode mode, uint32_t channels, uint32_t rate, Render render, std::string* error);
    // Tests: a driver object made by the caller (owned from here on).
    bool openDriver(asio::IASIO* driver, Mode mode, uint32_t channels, uint32_t rate, Render render, std::string* error);
    void close();
    bool isOpen() const { return m_driver != nullptr; }
    bool start();
    void stop(); // returns once `render` is no longer called

    // Engine frames held by the driver: both buffer halves and its latency.
    uint32_t bufferFrames() const { return m_bufferFrames; }
    // PCM: the bits the driver's sample type carries (16..32) and whether it is
    // floating point.
    int pcmBits() const { return m_pcmBits; }
    bool pcmFloat() const { return m_pcmFloat; }
    std::string driverName() const { return m_name; }
    // The driver asked to be reset (its settings changed): reopen it.
    bool resetRequested() const { return m_resetRequested.load(std::memory_order_relaxed); }

    // Driver callbacks (any instance: the one running).
    static void onBufferSwitch(long index, long directProcess);
    static long onMessage(long selector, long value, void* message, double* opt);

private:
    void fill(long index);

    asio::IASIO* m_driver = nullptr;
    Mode m_mode = Mode::Pcm;
    Render m_render;
    uint32_t m_channels = 0;
    uint32_t m_bufferSize = 0;   // driver samples per channel and half (DSD: bits)
    uint32_t m_engineFrames = 0; // engine frames per half (DSD: words)
    uint32_t m_bufferFrames = 0;
    int m_pcmBits = 32;
    bool m_pcmFloat = false;
    bool m_outputReady = false;
    bool m_buffersCreated = false;
    bool m_dsdFormatSet = false;
    bool m_comInitialized = false;
    std::string m_name;
    std::vector<long> m_types;    // per channel
    std::vector<void*> m_buffers; // per channel, 2 halves each: [2c], [2c + 1]
    std::vector<int32_t> m_scratch;
    std::atomic<bool> m_running{false};
    std::atomic<int> m_inCallback{0};
    std::atomic<bool> m_resetRequested{false};

    static std::atomic<AsioOutput*> s_active;
};

} // namespace sonkore
