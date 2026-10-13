#pragma once

#include "DsdDecoder.hpp"

#include <atomic>
#include <functional>
#include <string>
#include <thread>
#include <vector>

typedef struct _snd_pcm snd_pcm_t;

namespace sonkore {

// Native DSD output on a raw ALSA hw: device (DSD_U32_BE / DSD_U32_LE), which
// miniaudio cannot open. A writer thread pulls packed DSD words from the
// engine through `render` and writes them unchanged, paced by the DAC.
class AlsaDsdOutput {
public:
    using Render = std::function<void(int32_t* out, uint32_t frames)>;

    ~AlsaDsdOutput();

    // The native packing `device` accepts for this stream, if any.
    static bool supported(const std::string& device, uint32_t channels, uint32_t frameRate, DsdPacking& packing);

    bool open(const std::string& device, DsdPacking packing, uint32_t channels, uint32_t frameRate, Render render,
              std::string* error);
    void close();
    bool isOpen() const { return m_pcm != nullptr; }
    bool start();
    void stop(); // returns once `render` is no longer called
    uint32_t bufferFrames() const { return m_bufferFrames; }

private:
    void run();

    snd_pcm_t* m_pcm = nullptr;
    Render m_render;
    uint32_t m_channels = 0;
    uint32_t m_periodFrames = 0;
    uint32_t m_bufferFrames = 0;
    std::vector<int32_t> m_buffer;
    std::thread m_thread;
    std::atomic<bool> m_running{false};
};

} // namespace sonkore
