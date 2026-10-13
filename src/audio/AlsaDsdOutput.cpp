#include "AlsaDsdOutput.hpp"

#include <alsa/asoundlib.h>

#include <pthread.h>

namespace sonkore {

namespace {

constexpr unsigned kPeriodUs = 40000; // 3 x 40 ms, like the miniaudio path
constexpr unsigned kPeriods = 3;

snd_pcm_format_t alsaFormat(DsdPacking packing)
{
    return packing == DsdPacking::NativeU32LE ? SND_PCM_FORMAT_DSD_U32_LE : SND_PCM_FORMAT_DSD_U32_BE;
}

// Interleaved access, the exact format, channel count and rate; no plugin
// conversion is possible on a hw: device.
bool configure(snd_pcm_t* pcm, DsdPacking packing, uint32_t channels, uint32_t rate, snd_pcm_uframes_t* period,
               snd_pcm_uframes_t* buffer, bool testOnly)
{
    snd_pcm_hw_params_t* hw = nullptr;
    snd_pcm_hw_params_alloca(&hw);
    if (snd_pcm_hw_params_any(pcm, hw) < 0 || snd_pcm_hw_params_set_access(pcm, hw, SND_PCM_ACCESS_RW_INTERLEAVED) < 0
        || snd_pcm_hw_params_set_format(pcm, hw, alsaFormat(packing)) < 0
        || snd_pcm_hw_params_set_channels(pcm, hw, channels) < 0
        || snd_pcm_hw_params_set_rate(pcm, hw, rate, 0) < 0)
        return false;
    if (testOnly)
        return true;
    unsigned periodUs = kPeriodUs;
    unsigned periods = kPeriods;
    int dir = 0;
    snd_pcm_hw_params_set_period_time_near(pcm, hw, &periodUs, &dir);
    snd_pcm_hw_params_set_periods_near(pcm, hw, &periods, &dir);
    if (snd_pcm_hw_params(pcm, hw) < 0)
        return false;
    snd_pcm_hw_params_get_period_size(hw, period, &dir);
    snd_pcm_hw_params_get_buffer_size(hw, buffer);

    snd_pcm_sw_params_t* sw = nullptr;
    snd_pcm_sw_params_alloca(&sw);
    snd_pcm_sw_params_current(pcm, sw);
    snd_pcm_sw_params_set_start_threshold(pcm, sw, *buffer); // start once the buffer is full
    snd_pcm_sw_params_set_avail_min(pcm, sw, *period);
    return snd_pcm_sw_params(pcm, sw) >= 0;
}

} // namespace

AlsaDsdOutput::~AlsaDsdOutput()
{
    close();
}

bool AlsaDsdOutput::supported(const std::string& device, uint32_t channels, uint32_t frameRate, DsdPacking& packing)
{
    snd_pcm_t* pcm = nullptr;
    if (snd_pcm_open(&pcm, device.c_str(), SND_PCM_STREAM_PLAYBACK, SND_PCM_NONBLOCK) < 0)
        return false;
    bool ok = false;
    for (DsdPacking p : {DsdPacking::NativeU32BE, DsdPacking::NativeU32LE}) {
        snd_pcm_uframes_t period = 0, buffer = 0;
        if (configure(pcm, p, channels, frameRate, &period, &buffer, true)) {
            packing = p;
            ok = true;
            break;
        }
    }
    snd_pcm_close(pcm);
    return ok;
}

bool AlsaDsdOutput::open(const std::string& device, DsdPacking packing, uint32_t channels, uint32_t frameRate,
                         Render render, std::string* error)
{
    close();
    int r = snd_pcm_open(&m_pcm, device.c_str(), SND_PCM_STREAM_PLAYBACK, SND_PCM_NONBLOCK);
    snd_pcm_uframes_t period = 0, buffer = 0;
    if (r < 0 || !configure(m_pcm, packing, channels, frameRate, &period, &buffer, false)) {
        if (error)
            *error = "Could not open \"" + device + "\" for native DSD" + (r < 0 ? std::string(": ") + snd_strerror(r) : std::string());
        close();
        return false;
    }
    m_render = std::move(render);
    m_channels = channels;
    m_periodFrames = static_cast<uint32_t>(period);
    m_bufferFrames = static_cast<uint32_t>(buffer);
    m_buffer.assign(static_cast<std::size_t>(m_periodFrames) * channels, 0);
    return true;
}

void AlsaDsdOutput::close()
{
    stop();
    if (m_pcm)
        snd_pcm_close(m_pcm);
    m_pcm = nullptr;
}

bool AlsaDsdOutput::start()
{
    if (!m_pcm || m_running.load())
        return m_running.load();
    if (snd_pcm_prepare(m_pcm) < 0)
        return false;
    m_running.store(true);
    m_thread = std::thread(&AlsaDsdOutput::run, this);
    return true;
}

void AlsaDsdOutput::stop()
{
    if (!m_thread.joinable())
        return;
    m_running.store(false);
    m_thread.join();
    if (m_pcm)
        snd_pcm_drop(m_pcm);
}

void AlsaDsdOutput::run()
{
    // Real-time when allowed (rtkit / limits.conf); otherwise just a thread.
    sched_param param{};
    param.sched_priority = sched_get_priority_min(SCHED_FIFO) + 10;
    pthread_setschedparam(pthread_self(), SCHED_FIFO, &param);

    std::size_t offset = m_periodFrames; // frames of m_buffer already written
    while (m_running.load(std::memory_order_relaxed)) {
        if (offset >= m_periodFrames) {
            m_render(m_buffer.data(), m_periodFrames);
            offset = 0;
        }
        const snd_pcm_sframes_t n = snd_pcm_writei(m_pcm, m_buffer.data() + offset * m_channels, m_periodFrames - offset);
        if (n >= 0) {
            offset += static_cast<std::size_t>(n);
        } else if (n == -EAGAIN) {
            snd_pcm_wait(m_pcm, 20); // wakes up regularly to notice stop()
        } else if (snd_pcm_recover(m_pcm, static_cast<int>(n), 1) < 0) {
            break;
        }
    }
}

} // namespace sonkore
