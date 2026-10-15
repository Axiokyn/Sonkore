#pragma once

#include "Decoder.hpp"
#include "Convolver.hpp"
#include "Crossfeed.hpp"
#include "DsdDecoder.hpp"
#include "FilePreload.hpp"
#include "LinearPhaseEq.hpp"
#include "Limiter.hpp"
#include "Resampler.hpp"
#include "SigmaDelta.hpp"
#include "SampleConvert.hpp"
#include "SpscRingBuffer.hpp"
#include "SuperEQ.hpp"
#include "TimeStretch.hpp"

#include <miniaudio.h>

#include <array>
#include <atomic>
#include <chrono>
#include <cmath>
#include <condition_variable>
#include <cstdint>
#include <filesystem>
#include <map>
#include <memory>
#include <mutex>
#include <string>
#include <thread>
#include <vector>

namespace sonkore {

class AlsaDsdOutput;
class AsioOutput;
class PipeWireOutput;
}
struct hdcd_simple;
namespace sonkore {

// Audio engine: Decoder -> DSP chain -> lock-free ring -> miniaudio.
//
//   ┌──────────────────── worker thread (not real-time) ─────────────────────┐   ┌─ RT callback ─┐
//   decoder s32 ─┬─ bypass (bit-perfect) ───────────────────────────────────┬─▶ ring<int32> ─▶ device (s32)
//                └─ s32→f32 → SoundTouch → f64 → SuperEQ → vol ─────────────┘
//                   → [soxr, only if the device refuses the source rate]
//                   → quantise (grid of device bit depth, optional TPDF)
//
// * The device runs in ma_format_s32 at the source sample rate whenever it
//   accepts it, so in bypass the decoded integers reach the driver untouched
//   (s32→s24 inside miniaudio is a pure truncation of zero bits). When it
//   does not, the device is opened at its own rate and the worker converts
//   with SoX VHQ instead of miniaudio's linear resampler.
// * Everything that may allocate or lock (decoding, time-stretch, EQ,
//   resampling) lives on the worker. The real-time callback only memcpy's
//   from the ring buffer.
// * Memory: stretcher, resampler, processing buffers and the visualiser tap
//   are only allocated while they are in use.
// * Structural operations (load, seek, stop) park the worker and stop the
//   device first, so decoder/DSP state never has two owners at once.
// * Several backends are live at once (e.g. ALSA hw: + PipeWire + JACK on
//   Linux) so the user can pick any physical or virtual output.
//
// Public methods are meant to be called from a single control thread.
class DSPManager {
public:
    // DSD files: converted to PCM (any output), or sent as DSD to a DAC that
    // takes it — native DSD words on an ALSA hw: device that offers
    // DSD_U32_BE/LE, or DoP on an exclusive output. Anything that cannot
    // carry it untouched falls back to PCM.
    enum class DsdOutput { Pcm, Native, Dop };
    // PCM sent at a multiple of its rate, same family (44.1 -> 88.2 / 176.4 /
    // 352.8 kHz), converted with SoX VHQ: Max takes the highest the device
    // accepts (up to 8x / 768 kHz).
    enum class Upsampling { Off, Double, Quadruple, Max };
    // PCM converted to DSD (8th-order sigma-delta) and sent as DSD through
    // the DSD output (native or DoP): DSD64 / 128 / 256, or the highest of
    // them the DAC takes.
    enum class PcmToDsd { Off, Dsd64, Dsd128, Dsd256, Max, Dsd512 }; // Max: the highest the DAC takes, up to DSD256

    struct OutputConfig {
        std::string deviceKey;              // "" = system default (see DeviceInfo::key)
        DsdOutput dsd = DsdOutput::Pcm;
        bool playFromMemory = false;        // each file is loaded into RAM before it plays
        bool hdcd = true;                   // decode HDCD CDs (only where HDCD codes are found)
        Upsampling upsampling = Upsampling::Off;
        PcmToDsd pcmToDsd = PcmToDsd::Off;  // needs dsd = Native or Dop
        Resampler::Filter filter = Resampler::Filter::Linear;
        bool exclusiveMode = true;          // WASAPI exclusive (ALSA hw: is always exclusive)
        bool allowSampleRateSwitch = true;  // the system follows the track's rate (Core Audio nominal rate, PipeWire graph rate)
        uint32_t bufferMs = 150;            // ring buffer depth (DSP change latency)
        uint32_t resyncDelayMs = 0;         // silence after the DAC changes rate (lets it lock)
        uint32_t fixedRate = 0;             // every track converted to this rate (0 = each at its own)
    };

    struct DeviceInfo {
        std::string key;        // stable id: "<backend>|<device id>"
        std::string backend;    // "ALSA", "PipeWire", "JACK", "WASAPI", "Core Audio"...
        std::string name;       // human readable
        std::string detail;     // e.g. "hw:CARD=DX1,DEV=0"
        bool isDefault = false;
        bool direct = false;    // raw hardware access, no mixer in the path
    };

    using TrackInfo = StreamInfo;

    struct OutputStatus {
        std::string backend;
        std::string deviceName;
        bool exclusive = false;
        uint32_t deviceSampleRate = 0;
        uint32_t deviceChannels = 0;
        int deviceBits = 0;
        bool deviceFloat = false;
        bool resampled = false;           // converted to deviceSampleRate by soxr
        uint32_t sourceSampleRate = 0;
        bool pathCanBeBitPerfect = false; // device matches source rate/channels/depth, no mixer
        bool bitPerfectNow = false;       // ...and the DSP chain is currently neutral
        std::string dsd;                  // "DSD64 native", "DSD128 DoP"; empty for PCM
        bool hdcd = false;                // HDCD codes found: decoded to 20 bits (peak extend, gain)
        bool upsampled = false;           // resampled on purpose (Upsampling), not because the device refused the rate
        uint32_t pcmToDsdRate = 0;        // PCM modulated to this DSD rate (Hz), 0 = no
        uint64_t modulatorResets = 0;     // sigma-delta loop overloads (should stay 0)
        // Native PipeWire: the rate the graph runs at now (0: not known yet, or
        // another output) and whether anything there scales the samples (this
        // stream's or the sink's software volume below 100 %, muted).
        uint32_t graphRate = 0;
        bool systemVolumeNeutral = true;
    };

    enum class State { Empty, Stopped, Playing, Paused };

    DSPManager();
    ~DSPManager();
    DSPManager(const DSPManager&) = delete;
    DSPManager& operator=(const DSPManager&) = delete;

    // "miniaudio 0.11.21 · dr_flac / dr_wav / dr_mp3 · FFmpeg 7.1 · ..." for About.
    static std::string componentVersions();

    bool initialize(std::string* error = nullptr);
    void shutdown();

    std::vector<DeviceInfo> playbackDevices(); // re-enumerates
    // Windows: the WASAPI endpoint id of an output ("" for the system default
    // or any other backend), to reach the device's own volume control.
    std::wstring wasapiEndpointId(const std::string& key);
    void setOutputConfig(const OutputConfig& cfg); // applied on next load()
    OutputConfig outputConfig() const;

    // `gain`: the track's own linear gain (ReplayGain), on top of the volume.
    bool load(const std::filesystem::path& path, std::string* error = nullptr, double gain = 1.0);
    void unload();

    // --- Gapless ---------------------------------------------------------------
    // The track to continue with when the current one ends ("" = none). It is
    // opened right away; if it has the same sample rate and channel count, the
    // worker splices it in sample-exactly without reopening the device or
    // resetting the DSP chain. Otherwise playback ends as usual and the caller
    // loads it (the DAC has to switch rate anyway).
    void setNext(const std::filesystem::path& path, double gain = 1.0);
    // True once, when the spliced track starts reaching the device: the engine
    // now reports its info and position. `path` receives that track.
    bool takeTrackChange(std::filesystem::path* path);
    // Play from memory: 0 = off or not loaded (yet), 1 = in RAM (page cache),
    // 2 = in RAM and locked there.
    int memoryState() const;
    // Kernel id of the thread that feeds the device (0 until it has run), so
    // the app can ask for real-time scheduling for it (rtkit on Linux).
    int64_t audioThreadId() const { return m_audioTid.load(std::memory_order_relaxed); }
    // New per-track gains (ReplayGain settings changed): `current` for the
    // track being heard, `next` for the queued one. Applied sample-exactly
    // at the join, whichever side of it the worker is on.
    void setGains(double current, double next);
    void play();
    void pause();
    void stop();
    void seek(double seconds);

    State state() const { return m_state.load(std::memory_order_relaxed); }
    TrackInfo trackInfo() const { return m_track; }
    OutputStatus outputStatus() const;
    // The open output is Sonkore's alone (WASAPI exclusive, ASIO, an ALSA hw:
    // device, native DSD): other programs get no sound from it until it is
    // closed, so an idle player should let go of it.
    bool outputHeld() const { return outputOpen() && m_status.exclusive; }
    double durationSeconds() const;
    double positionSeconds() const;
    bool reachedEnd() const;
    uint64_t underrunCount() const { return m_underruns.load(std::memory_order_relaxed); }

    void setVolume(double linear);
    double volume() const { return m_volume.load(std::memory_order_relaxed); }
    void setDither(DitherMode mode) { m_ditherMode.store(static_cast<int>(mode), std::memory_order_relaxed); }
    // Attenuation applied ahead of the DSP chain while it is active (never on
    // a bit-perfect path), so EQ boosts and inter-sample peaks do not clip.
    void setHeadroomDb(double db) { m_headroom.store(std::pow(10.0, std::min(0.0, db) / 20.0), std::memory_order_relaxed); }
    double headroomDb() const { return 20.0 * std::log10(m_headroom.load(std::memory_order_relaxed)); }
    // Headphone crossfeed (stereo tracks only).
    void setCrossfeed(Crossfeed::Preset preset) { m_crossfeedPreset.store(static_cast<int>(preset), std::memory_order_relaxed); }
    // True-peak limiter at the end of the processed path (never on the
    // untouched one): what a boost pushes above `ceilingDb` (dBTP, <= 0) is
    // brought down to it instead of clipping.
    void setLimiter(bool on, double ceilingDb)
    {
        m_limiterCeilingDb.store(std::min(0.0, ceilingDb), std::memory_order_relaxed);
        m_limiterOn.store(on, std::memory_order_relaxed);
    }
    bool limiterEnabled() const { return m_limiterOn.load(std::memory_order_relaxed); }
    // Most reduction in the last processed block, in dB (0: not reducing).
    double limiterReductionDb() const { return m_limiterReduction.load(std::memory_order_relaxed); }
    Crossfeed::Preset crossfeed() const { return static_cast<Crossfeed::Preset>(m_crossfeedPreset.load(std::memory_order_relaxed)); }
    // FIR convolution with an impulse response file ("" = off). It is fitted
    // to each track's rate and channels; false (with a reason) when the file
    // cannot be read.
    bool setConvolution(const std::filesystem::path& impulseResponse, std::string* error = nullptr);
    struct ConvolutionStatus {
        std::filesystem::path file; // empty: off
        bool active = false;        // running on the current track
        std::size_t taps = 0;
        uint32_t irChannels = 0;
        uint32_t resampledFrom = 0; // the response's own rate, when resampled
        std::string problem;        // why it does not run on this track
    };
    ConvolutionStatus convolutionStatus() const;
    // Samples clamped at full scale since the track (or a seek) started.
    uint64_t clippedSamples() const { return m_clipped.load(std::memory_order_relaxed); }
    // Counting starts again (the headroom changed: what clipped before says
    // nothing about the new setting).
    void resetClippedSamples() { m_clipped.store(0, std::memory_order_relaxed); }

    // Parameter surfaces (setters are thread-safe).
    TimeStretch& timeStretch() { return m_stretch; }
    const TimeStretch& timeStretch() const { return m_stretch; }
    SuperEQ& equalizer() { return m_eq; }

    // --- Analysis tap for visualisers (read-only copy of the output) -------
    // The RT callback keeps the most recent output as mono floats when
    // enabled; it never alters what the DAC receives.
    static constexpr std::size_t kAnalysisSize = 65536; // power of two
    void setAnalysisEnabled(bool on);
    // Copies `count` mono samples ending `delayFrames` before the newest one.
    // Returns false when not enough audio has been seen yet.
    bool recentOutput(float* dst, std::size_t count, std::size_t delayFrames) const;
    // Frames the device buffers ahead of the speaker (what is "in flight").
    std::size_t outputLatencyFrames() const { return m_latencyFrames.load(std::memory_order_relaxed); }
    uint32_t outputSampleRate() const { return m_outputRate; }

    // --- Tests ------------------------------------------------------------------
    // miniaudio's null device instead of the sound cards (call before
    // initialize()), and a copy of every frame handed to the device, on the
    // device thread, exactly as the DAC would receive it.
    using OutputTap = void (*)(void* user, const int32_t* samples, uint32_t frames, uint32_t channels);
    void useNullBackend() { m_nullBackend = true; }
    // Tests: the null device presents itself as this many integer bits
    // (16, 24...), as a DAC that takes no more would.
    void setNullDeviceBits(int bits) { m_nullDeviceBits = bits; }
    void setOutputTap(OutputTap tap, void* user)
    {
        m_tapUser.store(user, std::memory_order_relaxed);
        m_tap.store(tap, std::memory_order_release);
    }

private:
    struct Backend {
        ma_backend type;
        std::string label;
        std::unique_ptr<ma_context> context; // heap: miniaudio keeps self-pointers
    };
    struct DeviceEntry {
        std::size_t backendIndex = 0;
        ma_device_id id{};
        bool hasId = false;
        bool direct = false;
        std::string name;
        std::string asio; // Windows: an ASIO driver (by name) instead of a miniaudio device
        bool pw = false;  // Linux: native PipeWire...
        std::string pipewire; // ...to this sink (node.name; "" = the desktop's default)
    };

    uint32_t bestDeviceRate(Backend& backend, const DeviceEntry& entry, uint32_t wanted);
    static void dataCallback(ma_device* device, void* output, const void* input, ma_uint32 frameCount);
    void render(int32_t* out, uint32_t frames); // device thread: ring -> device
    void renderBlock(int32_t* out, uint32_t frames);
    void fillSilence(int32_t* out, std::size_t frames);
    bool openDsd(const std::filesystem::path& path, double gain);
    bool openPcmToDsd();
    bool modulateAndPush(const double* samples, std::size_t frames);
    void adoptDecoder(std::unique_ptr<Decoder> decoder, double gain);
    bool startOutput();
    void stopOutput();
    // De-click: the output fades out over a few ms and then plays silence
    // without touching the ring (pause, seek, a track change by hand).
    // Returns once the device thread no longer reads the ring; with
    // `waitHeard`, once the fade itself has left the DAC (before stopping or
    // closing the device, which would cut what it still holds).
    void muteOutput(bool waitHeard);
    void unmuteOutput(bool rampIn); // rampIn: fade in (resuming mid-track)
    void releaseTrack();            // unload() but the output device
    void noteOutputRate(uint32_t rate); // an output opened at `rate`: resync silence if it changed
    void applyRamp(int32_t* out, std::size_t frames, std::size_t& pos, bool down) const;
    double rampNoise() const;
    void pushDsdPreroll();
    bool openDevice(uint32_t sampleRate, uint32_t channels, std::string* error);
    void closeDevice();
    bool resolveDevice(const std::string& key, DeviceEntry& out);
    void startWorker();
    void stopWorker();
    void parkWorker();
    void unparkWorker();

    void workerLoop();
    bool produceBlock();
    bool drainStretch();
    bool drainResampler();
    bool processAndPush(double* samples, std::size_t frames);
    bool processRest(double* samples, std::size_t frames, bool biquads); // after the linear-phase EQ
    void applyGain(double* samples, std::size_t frames, double& applied, double target) const;
    std::size_t pendingDspFrames() const; // worker: frames of music inside the DSP chain, at the output rate
    bool quantizeAndPush(const double* samples, std::size_t frames);
    void ensureBuffers(bool stretch);
    void releaseBuffers();
    bool pushToRing(const int32_t* samples, std::size_t frames);
    bool interrupted() const;
    bool dspIsNeutral() const;
    void waitForPrefill() const;
    bool finishStream();
    enum class Splice { Done, Wait, None };
    Splice spliceNext();
    void updateBitPerfectPath();
    void clearGapless();

    // miniaudio
    bool m_nullBackend = false;
    int m_nullDeviceBits = 0;
    std::atomic<OutputTap> m_tap{nullptr};
    std::atomic<void*> m_tapUser{nullptr};
    std::vector<Backend> m_backends;
    std::map<std::string, DeviceEntry> m_devices;
    ma_device m_device{};
    bool m_deviceOpen = false;            // miniaudio device
    bool m_outputRunning = false;         // the device (or the DSD writer) is started
    std::string m_openKey;                // device and mode the open device was opened for
    bool m_openExclusive = false;
    bool m_reuseDevice = false;           // load(): openDevice may keep the running PCM device
    // De-click (render() reads these; m_cb* belong to the device thread).
    enum Mute : int { Open = 0, FadingOut = 1, Muted = 2 };
    std::atomic<int> m_mute{Open};
    std::atomic<bool> m_muteAck{false};       // the device thread saw Muted: it leaves the ring alone
    std::atomic<uint64_t> m_mutedFrames{0};   // silence written since the fade ended
    std::atomic<bool> m_rampIn{false};
    std::atomic<std::size_t> m_rampFrames{256};
    std::atomic<uint64_t> m_leadSilence{0}; // frames of silence before the music (resync delay)
    uint32_t m_lastOutputRate = 0;          // the rate the DAC was last opened at
    int m_cbLastMute = Open;
    std::size_t m_cbRampPos = 0;
    std::size_t m_cbRampInPos = 0;
    bool m_cbRampingIn = false;
    std::unique_ptr<AlsaDsdOutput> m_dsdOut; // native DSD instead of miniaudio
    std::unique_ptr<AsioOutput> m_asioOut;   // Windows: ASIO driver (PCM and native DSD) instead of miniaudio
    bool asioOpen() const;
    bool openAsio(const DeviceEntry& entry, bool dsd, uint32_t rate, uint32_t channels, std::string* error);
    std::unique_ptr<PipeWireOutput> m_pwOut; // Linux: native PipeWire instead of miniaudio
    bool m_pipewireNative = false;           // PipeWire answered at the last enumeration
    bool m_pipewireRunning = false;          // a PipeWire daemon in this session
    bool m_lazyServers = false;              // the PulseAudio / JACK contexts are made on demand
    bool addBackend(ma_backend type);        // a miniaudio context for `type` (once)
    void rest(std::chrono::nanoseconds duration); // worker: sleep, woken early by a park or the end
    uint32_t m_pwRequestedRate = 0;          // the rate the open stream was asked for
    bool pwOpen() const;
    bool outputOpen() const;                 // any output (device, DSD writer, ASIO, PipeWire)
    bool openPipeWire(const DeviceEntry& entry, uint32_t rate, uint32_t channels, std::string* error);
    StreamInfo::Dsd m_outputDsd = StreamInfo::Dsd::None; // what the open output carries
    DsdPacking m_dsdPacking = DsdPacking::Dop;
    bool m_toDsd = false;                 // PCM modulated to DSD for this output
    uint32_t m_dsdRate = 0;               // its 1-bit rate
    SigmaDelta m_modulator;               // worker
    std::vector<int32_t> m_dsdWords;      // worker
    std::atomic<uint64_t> m_modulatorResets{0};
    bool m_cbDopPhase = false;            // device thread: marker of the next silence frame
    std::atomic<int64_t> m_audioTid{0};
    std::atomic<bool> m_tidKnown{false};  // reset when an output opens: its thread may be new
    OutputConfig m_config;
    mutable std::mutex m_configMutex;
    OutputStatus m_status;
    bool m_mixerFree = false; // exclusive access or Core Audio: no system mixer
    // Channel order: the source's (WAVE / FLAC / FFmpeg order) against the
    // device's. A different order is fixed by moving whole samples around
    // (m_channelOrder[device channel] = source channel), never by mixing.
    bool m_channelsExact = true;
    bool m_reorder = false;
    bool m_monoToStereo = false; // device opened in stereo for a mono track: render() duplicates
    std::array<uint8_t, MA_MAX_CHANNELS> m_channelOrder{};
    std::atomic<bool> m_bypassActive{false};

    // decoder (worker-owned while the worker runs)
    std::unique_ptr<Decoder> m_decoder;
    TrackInfo m_track;
    std::atomic<uint64_t> m_decodedFrames{0};
    uint64_t m_startFrame = 0; // control thread: where playback (re)started (load: 0, seek: its target)
    std::atomic<bool> m_decoderEof{false};
    std::atomic<bool> m_outputDrained{false};
    std::atomic<uint64_t> m_tailFrames{0}; // silence sent after the end (callback)
    bool m_trackEnded = false; // worker: current decoder exhausted, splice pending

    // gapless: the next track (control thread -> worker) and the splice point
    // (worker -> callback/control thread). One splice in flight at a time.
    // A file being read into RAM on a thread of its own (the queued track
    // must not block the caller); kept alive for as long as its track plays.
    struct Preload {
        std::mutex mutex;
        std::unique_ptr<FilePreload> file; // guarded by mutex
    };
    static std::shared_ptr<Preload> startPreload(const std::filesystem::path& path, bool async);
    struct Queued {
        std::unique_ptr<Decoder> decoder;
        std::filesystem::path path;
        double gain = 1.0;
        std::shared_ptr<Preload> preload;
        bool hdcd = false;
    };
    // A 16-bit stereo track whose first seconds carry HDCD codes.
    static bool detectHdcd(const std::filesystem::path& path);
    bool m_trackHdcd = false;          // the track being heard (control thread)
    bool m_splicedHdcd = false;        // guarded by m_nextMutex
    bool m_workerHdcd = false;         // worker: what it decodes now
    uint32_t m_workerBits = 0;         // worker: the source's bits per sample (0: float / lossy)
    int m_deviceDepth = 32;            // integer bits the output carries exactly (float32: 24)
    double m_appliedGain = 1.0;        // worker: volume x track gain on the last processed sample
    double m_appliedHeadroom = 1.0;    // worker: headroom on the last processed sample
    bool m_gainsToUnity = false;       // worker: gains gliding back to 1 before the untouched path
    bool m_snapGains = true;           // worker: take the gains as they are (new track), no glide
    mutable uint32_t m_rampRng = 0x9E3779B9u; // device thread: the fades' dither
    hdcd_simple* m_hdcd = nullptr;     // worker-owned decoder state, reset per track
    std::shared_ptr<Preload> m_preload; // the current track's (guarded by m_nextMutex once loaded)
    mutable std::mutex m_nextMutex;
    Queued m_next;              // guarded by m_nextMutex
    Queued m_previous;          // the replaced track until the splice is heard (guarded)
    std::filesystem::path m_splicedPath; // guarded
    TrackInfo m_splicedTrack;   // guarded
    uint32_t m_workerRate = 0, m_workerChannels = 0; // format the worker is decoding
    StreamInfo::Dsd m_workerDsd = StreamInfo::Dsd::None;
    // Output frames written to / read from the ring since load or seek.
    std::atomic<uint64_t> m_framesPushed{0};
    std::size_t m_pushedPartial = 0; // worker: samples of a frame not yet complete in the ring
    std::atomic<uint64_t> m_framesPlayed{0};
    std::atomic<int64_t> m_splicePoint{-1};      // m_framesPushed value where the next track starts
    std::atomic<uint64_t> m_spliceSourceEnd{0};  // frames decoded from the previous track

    // DSP (worker-owned)
    TimeStretch m_stretch;
    SuperEQ m_eq;
    LinearPhaseEq m_linearEq;                    // worker
    std::atomic<std::size_t> m_linearEqLatency{0}; // its frames held, at the track's rate
    std::atomic<double> m_resamplerDelay{0.0}; // output frames inside the converter (worker -> control thread)
    Resampler m_resampler;
    uint32_t m_outputRate = 0; // rate handed to the device
    Quantizer m_quantizer;
    std::atomic<double> m_volume{1.0};
    std::atomic<double> m_trackGain{1.0}; // per-track gain of what the worker decodes
    std::atomic<int> m_ditherMode{static_cast<int>(DitherMode::Tpdf)};
    std::atomic<double> m_headroom{1.0};
    std::atomic<int> m_crossfeedPreset{0};
    Crossfeed m_crossfeed;           // worker
    int m_crossfeedFor = -1;         // worker: preset the filter is set up for (-1 = redo)
    std::atomic<bool> m_limiterOn{false};
    std::atomic<double> m_limiterCeilingDb{-1.0};
    std::atomic<double> m_limiterReduction{0.0};
    Limiter m_limiter;               // worker
    std::vector<double> m_limitBuf;  // worker: its output
    bool m_limiterRunning = false;   // worker: set up for the current output
    bool limitAndPush(const double* samples, std::size_t frames); // limiter (if on) -> DSD modulator / quantiser
    bool flushLimiter();
    void syncLimiter();              // worker: follows m_limiterOn and the output format
    // Convolution: the response as loaded, and the filter built from it for
    // the current track (handed to the worker through m_pendingConvolver).
    void fitConvolution(); // control thread: builds for m_track
    mutable std::mutex m_convMutex;
    std::shared_ptr<const ImpulseResponse> m_ir;     // guarded by m_convMutex
    std::unique_ptr<Convolver> m_pendingConvolver;   // guarded
    bool m_convolverPending = false;                 // guarded
    ConvolutionStatus m_convStatus;                  // guarded
    std::unique_ptr<Convolver> m_convolver;          // worker
    std::atomic<bool> m_convActive{false};
    std::atomic<uint64_t> m_clipped{0};
    bool m_stWasActive = false;
    bool m_wasBypassed = true;
    std::vector<int32_t> m_decodeBuf;
    std::vector<float> m_floatIn;
    std::vector<float> m_floatOut;
    std::vector<double> m_doubleBuf;
    std::vector<int32_t> m_outBuf;

    // RT handoff
    SpscRingBuffer<int32_t> m_ring;
    uint32_t m_channels = 2;
    std::atomic<uint64_t> m_underruns{0};

    // analysis tap (written by the RT callback; allocated on first use, then
    // kept for the engine's lifetime so the callback never sees it go away)
    std::unique_ptr<std::atomic<float>[]> m_analysisStorage;
    std::atomic<std::atomic<float>*> m_analysis{nullptr};
    std::atomic<uint64_t> m_analysisWritten{0};
    std::atomic<bool> m_analysisOn{false};
    std::atomic<std::size_t> m_latencyFrames{0};

    // worker control
    std::thread m_worker;
    std::mutex m_parkMutex;
    std::condition_variable m_parkCv;
    bool m_parkRequested = false; // guarded by m_parkMutex
    bool m_parked = false;        // guarded by m_parkMutex
    std::atomic<bool> m_parkFlag{false};
    std::atomic<bool> m_quit{false};

    std::atomic<State> m_state{State::Empty};
};

} // namespace sonkore
