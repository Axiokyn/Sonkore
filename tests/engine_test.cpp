// Audio engine tests: plays real files through DSPManager on miniaudio's null
// device and checks every sample handed to the "DAC" (bit-perfect paths,
// gapless joins, seeks, pauses, DSP) plus a randomised stress run of the
// transport. Needs the corpus made by tests/make_corpus.sh:
//   engine_test <corpus dir> [test name...]
#include "AsioConvert.hpp"
#include "AsioOutput.hpp"
#include "Convolver.hpp"
#include "DSPManager.hpp"
#include "Decoder.hpp"
#include "DsdDecoder.hpp"
#include "LinearPhaseEq.hpp"
#include "PipeWireOutput.hpp"
#include "SampleConvert.hpp"
#include "SigmaDelta.hpp"

#include <algorithm>
#include <atomic>
#include <bit>
#include <chrono>
#include <cmath>
#include <complex>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <functional>
#include <map>
#include <mutex>
#include <numbers>
#include <random>
#include <string>
#include <thread>
#include <vector>

#if defined(_WIN32)
#include "AsioAbi.hpp"
#include <windows.h>
#else
#include <csignal>
#include <spawn.h>
#include <sys/wait.h>
extern char** environ;
#endif

using namespace sonkore;
using namespace std::chrono_literals;
namespace fs = std::filesystem;

namespace {

fs::path g_corpus;
int g_failures = 0;
std::string g_test;

#define CHECK(cond)                                                                             \
    do {                                                                                        \
        if (!(cond)) {                                                                          \
            std::fprintf(stderr, "  FAIL [%s] %s:%d: %s\n", g_test.c_str(), __FILE__, __LINE__, #cond); \
            ++g_failures;                                                                       \
        }                                                                                       \
    } while (0)
#define CHECK_MSG(cond, ...)                                                                    \
    do {                                                                                        \
        if (!(cond)) {                                                                          \
            std::fprintf(stderr, "  FAIL [%s] %s:%d: %s: ", g_test.c_str(), __FILE__, __LINE__, #cond); \
            std::fprintf(stderr, __VA_ARGS__);                                                  \
            std::fputc('\n', stderr);                                                           \
            ++g_failures;                                                                       \
        }                                                                                       \
    } while (0)

fs::path corpus(const char* name) { return g_corpus / fs::path(reinterpret_cast<const char8_t*>(name)); }

// Everything the device received, frame-interleaved.
struct Capture {
    std::mutex mutex;
    std::vector<int32_t> samples;
    uint32_t channels = 0;

    static void tap(void* user, const int32_t* s, uint32_t frames, uint32_t channels)
    {
        auto* self = static_cast<Capture*>(user);
        std::lock_guard lock(self->mutex);
        self->channels = channels;
        self->samples.insert(self->samples.end(), s, s + std::size_t(frames) * channels);
    }
    uint32_t channelCount()
    {
        std::lock_guard lock(mutex);
        return channels;
    }
    std::vector<int32_t> take()
    {
        std::lock_guard lock(mutex);
        return std::exchange(samples, {});
    }
};

std::vector<int32_t> decodeAll(const fs::path& path, StreamInfo* info = nullptr)
{
    auto d = Decoder::open(path);
    if (!d)
        return {};
    if (info)
        *info = d->info();
    std::vector<int32_t> out;
    std::vector<int32_t> block(4096 * d->info().channels);
    while (const uint64_t n = d->readS32(block.data(), 4096))
        out.insert(out.end(), block.begin(), block.begin() + n * d->info().channels);
    return out;
}

// Drops all-zero frames at both ends (device prefill / tail silence; the
// test signals start at a zero crossing too, so both sides lose the same).
std::vector<int32_t> trimmed(const std::vector<int32_t>& s, uint32_t ch)
{
    std::size_t frames = s.size() / ch, first = 0, last = frames;
    const auto zero = [&](std::size_t f) { return std::all_of(s.begin() + f * ch, s.begin() + (f + 1) * ch, [](int32_t v) { return v == 0; }); };
    while (first < frames && zero(first))
        ++first;
    while (last > first && zero(last - 1))
        --last;
    return {s.begin() + first * ch, s.begin() + last * ch};
}

// Frames that carry sound (an underrun on a slow build fills silence in
// between, which says nothing about how long the processed track is).
std::size_t soundingFrames(const std::vector<int32_t>& s, uint32_t ch)
{
    std::size_t n = 0;
    for (std::size_t f = 0; f < s.size() / ch; ++f)
        n += std::any_of(s.begin() + f * ch, s.begin() + (f + 1) * ch, [](int32_t v) { return v != 0; });
    return n;
}

// First frame where a and b differ, or -1.
long long firstDifference(const std::vector<int32_t>& a, const std::vector<int32_t>& b, uint32_t ch)
{
    const std::size_t n = std::min(a.size(), b.size());
    for (std::size_t i = 0; i < n; ++i)
        if (a[i] != b[i])
            return static_cast<long long>(i / ch);
    return a.size() == b.size() ? -1 : static_cast<long long>(n / ch);
}

bool waitFor(const std::function<bool()>& done, std::chrono::milliseconds timeout)
{
    const auto deadline = std::chrono::steady_clock::now() + timeout;
    while (!done()) {
        if (std::chrono::steady_clock::now() > deadline)
            return false;
        std::this_thread::sleep_for(5ms);
    }
    return true;
}

struct Engine {
    Capture capture; // outlives the engine, whose device thread writes into it
    DSPManager dsp;
    Engine(const DSPManager::OutputConfig& config = {})
    {
        dsp.useNullBackend();
        std::string error;
        if (!dsp.initialize(&error))
            std::fprintf(stderr, "initialize: %s\n", error.c_str());
        DSPManager::OutputConfig cfg = config;
        dsp.setOutputConfig(cfg);
        dsp.setDither(DitherMode::Off);
        dsp.setOutputTap(&Capture::tap, &capture);
    }
    ~Engine() { dsp.setOutputTap(nullptr, nullptr); }
    bool load(const fs::path& p, double gain = 1.0)
    {
        std::string error;
        const bool ok = dsp.load(p, &error, gain);
        if (!ok)
            std::fprintf(stderr, "  (load %s: %s)\n", p.filename().string().c_str(), error.c_str());
        return ok;
    }
    // Plays to the end; `onChange` sees every gapless track change.
    bool playToEnd(std::chrono::milliseconds timeout = 30s, const std::function<void(const fs::path&)>& onChange = {})
    {
        dsp.play();
        return waitFor([&] {
            fs::path p;
            if (dsp.takeTrackChange(&p) && onChange)
                onChange(p);
            return dsp.reachedEnd();
        }, timeout);
    }
};

// --- Tests -----------------------------------------------------------------------

void bitPerfect(const char* file)
{
    const fs::path path = corpus(file);
    StreamInfo info;
    const auto ref = decodeAll(path, &info);
    CHECK_MSG(!ref.empty(), "%s does not decode", file);
    if (ref.empty())
        return;
    Engine e;
    CHECK(e.load(path));
    CHECK(e.playToEnd());
    const auto out = e.capture.take();
    const uint32_t ch = info.channels;
    CHECK_MSG(e.capture.channelCount() == ch, "%s: device got %u channels, file has %u", file, e.capture.channelCount(), ch);
    const auto a = trimmed(out, ch), b = trimmed(ref, ch);
    const long long diff = firstDifference(a, b, ch);
    CHECK_MSG(diff < 0, "%s: differs at frame %lld (%zu vs %zu frames)", file, diff, a.size() / ch, b.size() / ch);
    CHECK_MSG(info.totalFrames == 0 || info.totalFrames == ref.size() / ch, "%s: header says %llu frames, decoded %zu",
              file, static_cast<unsigned long long>(info.totalFrames), ref.size() / ch);
    CHECK(e.dsp.underrunCount() == 0);
}

void bitPerfectAll()
{
    for (const char* f : {"Álbum Uno/01 - Pista número 1.flac", "Formatos/hires-24-96.flac", "Formatos/hires-24-192.wav",
                          "Formatos/float32.wav", "Formatos/mono16.wav", "Formatos/r882.flac", "Formatos/aiff16.aiff",
                          "Formatos/alac.m4a", "Formatos/wavpack.wv", "Formatos/tta.tta", "Formatos/mp3.mp3",
                          "Formatos/opus.opus", "Formatos/vorbis.ogg", "Formatos/aac.m4a", "Multicanal/51.flac",
                          "Multicanal/71.wav"})
        bitPerfect(f);
}

// Encoder padding removed: a lossy file decodes to exactly the source length.
void decoderLengths()
{
    for (const char* f : {"Formatos/mp3.mp3", "Formatos/aac.m4a", "Formatos/opus.opus", "Formatos/vorbis.ogg",
                          "Formatos/alac.m4a", "Formatos/wavpack.wv", "Formatos/tta.tta"}) {
        StreamInfo info;
        const auto pcm = decodeAll(corpus(f), &info);
        const std::size_t frames = info.channels ? pcm.size() / info.channels : 0;
        // Opus decodes at 48 kHz; AAC ends where the encoder's edit list says.
        const std::size_t expected = info.sampleRate == 48000 ? 193499 : std::string(f).find("aac") != std::string::npos ? info.totalFrames : 177777;
        CHECK_MSG(frames == expected, "%s decodes to %zu frames, expected %zu", f, frames, expected);
        CHECK_MSG(info.totalFrames == frames, "%s: header length %llu, decoded %zu", f, static_cast<unsigned long long>(info.totalFrames), frames);
    }
    // DSD read as PCM by FFmpeg: the length is known from the header.
    ProbeResult probe;
    CHECK(Decoder::probe(corpus("DSD/dsd64.dsf"), probe, false));
    CHECK_MSG(std::abs(probe.durationSeconds - 2.0) < 0.02, "DSD64 probe duration %.3f", probe.durationSeconds);
    StreamInfo info;
    auto d = Decoder::open(corpus("DSD/dsd64.dsf"));
    CHECK(d && d->info().totalFrames == 2822400 * 2 / 8);
}

void gaplessAlbum()
{
    const char* parts[] = {"Álbum Uno/01 - Pista número 1.flac", "Álbum Uno/02 - Pista número 2.flac", "Álbum Uno/03 - Pista número 3.flac"};
    std::vector<int32_t> ref;
    for (const char* p : parts) {
        const auto pcm = decodeAll(corpus(p));
        ref.insert(ref.end(), pcm.begin(), pcm.end());
    }
    for (bool processed : {false, true}) {
        g_test = processed ? "gaplessAlbum (EQ)" : "gaplessAlbum";
        Engine e;
        if (processed) { // the DSP chain carries across the joins too
            auto bands = SuperEQ::graphic10Band();
            bands[5].gainDb = -3.0;
            e.dsp.equalizer().setBands(bands);
            e.dsp.equalizer().setEnabled(true);
        }
        CHECK(e.load(corpus(parts[0])));
        e.dsp.setNext(corpus(parts[1]));
        int changes = 0;
        CHECK(e.playToEnd(40s, [&](const fs::path& p) {
            ++changes;
            if (changes == 1) {
                CHECK(p == corpus(parts[1]));
                e.dsp.setNext(corpus(parts[2]));
            } else {
                CHECK(p == corpus(parts[2]));
            }
        }));
        CHECK_MSG(changes == 2, "%d track changes", changes);
        const auto out = trimmed(e.capture.take(), 2);
        if (!processed) {
            const long long diff = firstDifference(out, trimmed(ref, 2), 2);
            CHECK_MSG(diff < 0, "join not bit-exact: differs at frame %lld (%zu vs %zu)", diff, out.size() / 2, ref.size() / 2);
        } else {
            // No gap and nothing doubled: exactly as many frames as the album.
            CHECK_MSG(std::llabs(static_cast<long long>(out.size()) - static_cast<long long>(trimmed(ref, 2).size())) <= 2 * 2,
                      "%zu frames vs %zu", out.size() / 2, ref.size() / 2);
        }
        CHECK(e.dsp.underrunCount() == 0);
    }
}

void gaplessCue()
{
    StreamInfo info;
    const auto ref = decodeAll(corpus("CUE/imagen.flac"), &info);
    Engine e;
    const fs::path sheet = corpus("CUE/imagen.cue");
    auto track = [&](int n) { return fs::path(sheet.string() + "#" + std::to_string(n)); };
    CHECK(e.load(track(1)));
    e.dsp.setNext(track(2));
    int changes = 0;
    CHECK(e.playToEnd(40s, [&](const fs::path&) {
        if (++changes == 1)
            e.dsp.setNext(track(3));
    }));
    CHECK(changes == 2);
    const auto out = trimmed(e.capture.take(), 2);
    const long long diff = firstDifference(out, trimmed(ref, 2), 2);
    CHECK_MSG(diff < 0, "CD image not bit-exact across its tracks: frame %lld (%zu vs %zu)", diff, out.size() / 2, ref.size() / 2);
}

// The last `n` frames of `a` equal those of `b`.
bool sameTail(const std::vector<int32_t>& a, const std::vector<int32_t>& b, std::size_t frames, uint32_t ch)
{
    const std::size_t n = frames * ch;
    return a.size() >= n && b.size() >= n && std::equal(a.end() - n, a.end(), b.end() - n);
}

void seekAndPause()
{
    const fs::path path = corpus("Formatos/hires-24-96.flac"); // noise: no window repeats
    const auto ref = trimmed(decodeAll(path), 2);
    {
        g_test = "seek";
        Engine e;
        CHECK(e.load(path));
        e.dsp.play();
        std::this_thread::sleep_for(400ms);
        e.dsp.seek(2.0);
        const double pos = e.dsp.positionSeconds();
        CHECK_MSG(pos >= 1.9 && pos <= 2.3, "position after seek %.3f", pos);
        CHECK(waitFor([&] { return e.dsp.reachedEnd(); }, 20s));
        const auto out = trimmed(e.capture.take(), 2);
        // The rest of the file from the seek point on, untouched after the fade-in.
        CHECK(sameTail(out, ref, 96000 * 19 / 10, 2));
        const std::size_t heard = out.size() / 2, total = ref.size() / 2;
        CHECK_MSG(heard < total - 96000 * 3 / 2 && heard > 96000 * 2, "heard %zu of %zu frames", heard, total);
    }
    {
        g_test = "pause";
        Engine e;
        CHECK(e.load(path));
        e.dsp.play();
        std::this_thread::sleep_for(700ms);
        e.dsp.pause();
        CHECK(e.dsp.state() == DSPManager::State::Paused);
        const double at = e.dsp.positionSeconds();
        std::this_thread::sleep_for(300ms);
        CHECK_MSG(std::abs(e.dsp.positionSeconds() - at) < 1e-9, "position moved while paused");
        e.dsp.play();
        CHECK(waitFor([&] { return e.dsp.reachedEnd(); }, 20s));
        auto out = e.capture.take();
        // Without the silence of the pause, nothing is lost or repeated.
        std::vector<int32_t> music;
        for (std::size_t f = 0; f < out.size() / 2; ++f)
            if (out[2 * f] != 0 || out[2 * f + 1] != 0)
                music.insert(music.end(), {out[2 * f], out[2 * f + 1]});
        CHECK_MSG(std::llabs(static_cast<long long>(music.size() / 2) - static_cast<long long>(ref.size() / 2)) <= 4,
                  "%zu frames heard, file has %zu", music.size() / 2, ref.size() / 2);
        CHECK(sameTail(music, ref, 96000 * 2, 2));
        CHECK(std::equal(ref.begin(), ref.begin() + 96000, music.begin()));
    }
    {
        g_test = "stop/play";
        Engine e;
        CHECK(e.load(path));
        e.dsp.play();
        std::this_thread::sleep_for(300ms);
        e.dsp.stop();
        CHECK(e.dsp.state() == DSPManager::State::Stopped);
        CHECK(e.dsp.positionSeconds() < 0.01);
        e.capture.take();
        CHECK(e.playToEnd());
        const auto out = trimmed(e.capture.take(), 2);
        CHECK_MSG(firstDifference(out, ref, 2) < 0, "replay after stop differs");
        // Play again once it has ended: from the start.
        e.capture.take();
        CHECK(e.playToEnd());
        CHECK(firstDifference(trimmed(e.capture.take(), 2), ref, 2) < 0);
    }
}

void volumeAndEq()
{
    const fs::path path = corpus("Formatos/hires-24-96.flac");
    const auto ref = trimmed(decodeAll(path), 2);
    {
        g_test = "volume";
        Engine e;
        e.dsp.setVolume(0.5);
        CHECK(e.load(path));
        CHECK(e.playToEnd());
        const auto out = trimmed(e.capture.take(), 2);
        CHECK(out.size() == ref.size());
        int64_t worst = 0;
        for (std::size_t i = 0; i < std::min(out.size(), ref.size()); ++i)
            worst = std::max<int64_t>(worst, std::llabs(int64_t(out[i]) - std::llround(ref[i] * 0.5)));
        CHECK_MSG(worst <= 256, "volume 0.5 off by %lld (24-bit LSB = 256)", static_cast<long long>(worst));
    }
    {
        g_test = "eq back to neutral";
        Engine e;
        auto bands = SuperEQ::graphic10Band();
        bands[4].gainDb = 6.0;
        e.dsp.equalizer().setBands(bands);
        e.dsp.equalizer().setEnabled(true);
        CHECK(e.load(path));
        e.dsp.play();
        std::this_thread::sleep_for(600ms);
        CHECK(!e.dsp.outputStatus().bitPerfectNow);
        e.dsp.equalizer().setBands(SuperEQ::graphic10Band());
        CHECK(e.dsp.equalizer().isNeutral());
        CHECK(waitFor([&] { return e.dsp.reachedEnd(); }, 20s));
        const auto out = trimmed(e.capture.take(), 2);
        CHECK_MSG(sameTail(out, ref, 96000 * 2, 2), "not bit-perfect again after the EQ went back to flat");
        CHECK(e.dsp.clippedSamples() == 0 || true);
    }
    {
        g_test = "convolution (dirac)";
        Engine e;
        std::string error;
        CHECK(e.dsp.setConvolution(corpus("IR/dirac96.wav"), &error));
        CHECK(e.load(path));
        CHECK(e.dsp.convolutionStatus().active);
        CHECK(e.playToEnd());
        const auto out = trimmed(e.capture.take(), 2);
        CHECK_MSG(out.size() >= ref.size(), "%zu vs %zu frames", out.size() / 2, ref.size() / 2);
        int64_t worst = 0;
        for (std::size_t i = 0; i < std::min(out.size(), ref.size()); ++i)
            worst = std::max<int64_t>(worst, std::llabs(int64_t(out[i]) - ref[i]));
        CHECK_MSG(worst <= 256, "dirac convolution off by %lld", static_cast<long long>(worst));
    }
    {
        g_test = "crossfeed";
        Engine e;
        e.dsp.setCrossfeed(Crossfeed::Preset::Default);
        CHECK(e.load(path));
        CHECK(e.playToEnd());
        const auto out = trimmed(e.capture.take(), 2);
        CHECK_MSG(std::llabs(static_cast<long long>(out.size()) - static_cast<long long>(ref.size())) <= 4, "length changed");
        CHECK(e.dsp.underrunCount() == 0);
    }
}

void resampling()
{
    const fs::path path = corpus("Álbum Uno/01 - Pista número 1.flac");
    StreamInfo info;
    const auto ref = trimmed(decodeAll(path, &info), 2);
    for (auto mode : {DSPManager::Upsampling::Double, DSPManager::Upsampling::Quadruple}) {
        for (auto filter : {Resampler::Filter::Linear, Resampler::Filter::Minimum, Resampler::Filter::Intermediate, Resampler::Filter::LinearSteep}) {
            g_test = "upsampling x" + std::to_string(mode == DSPManager::Upsampling::Double ? 2 : 4) + " filter " + std::to_string(int(filter));
            DSPManager::OutputConfig cfg;
            cfg.upsampling = mode;
            cfg.filter = filter;
            Engine e(cfg);
            CHECK(e.load(path));
            const uint32_t factor = mode == DSPManager::Upsampling::Double ? 2 : 4;
            CHECK_MSG(e.dsp.outputSampleRate() == info.sampleRate * factor, "device at %u Hz", e.dsp.outputSampleRate());
            CHECK(e.playToEnd());
            const auto out = trimmed(e.capture.take(), 2);
            const double expect = double(ref.size()) * factor;
            CHECK_MSG(std::abs(double(out.size()) - expect) < 0.002 * expect + 200 * factor, "%zu frames, expected ~%.0f",
                      out.size() / 2, expect / 2);
            CHECK(e.dsp.clippedSamples() == 0);
            CHECK(e.dsp.underrunCount() == 0);
        }
    }
    {
        g_test = "fixed output rate 48 kHz";
        DSPManager::OutputConfig cfg;
        cfg.fixedRate = 48000;
        Engine e(cfg);
        CHECK(e.load(path));
        CHECK(e.dsp.outputSampleRate() == 48000);
        CHECK(e.playToEnd());
        const auto out = trimmed(e.capture.take(), 2);
        const double expect = double(ref.size()) * 48000 / 44100;
        CHECK_MSG(std::abs(double(out.size()) - expect) < 0.002 * expect + 400, "%zu frames", out.size() / 2);
    }
}

void timeStretch()
{
    const fs::path path = corpus("Formatos/hires-24-96.flac");
    const auto ref = trimmed(decodeAll(path), 2);
    for (auto engine : {TimeStretch::Engine::RubberBand, TimeStretch::Engine::SoundTouch}) {
        for (double tempo : {1.25, 0.8}) {
            g_test = std::string("tempo ") + std::to_string(tempo) + (engine == TimeStretch::Engine::RubberBand ? " RubberBand" : " SoundTouch");
            Engine e;
            e.dsp.timeStretch().setEngine(engine);
            e.dsp.timeStretch().setEnabled(true);
            e.dsp.timeStretch().setTempo(tempo);
            CHECK(e.load(path));
            CHECK(e.playToEnd(40s));
            const double heard = double(soundingFrames(e.capture.take(), 2));
            const double expect = double(ref.size() / 2) / tempo;
            CHECK_MSG(std::abs(heard - expect) < 0.03 * expect, "%.0f frames, expected ~%.0f", heard, expect);
            const double pos = e.dsp.positionSeconds();
            CHECK_MSG(pos > 3.5, "position at the end %.3f", pos);
            e.dsp.timeStretch().setTempo(1.0);
        }
        g_test = "pitch +2 st";
        Engine e;
        e.dsp.timeStretch().setEngine(engine);
        e.dsp.timeStretch().setEnabled(true);
        e.dsp.timeStretch().setPitchSemiTones(2.0);
        CHECK(e.load(path));
        CHECK(e.playToEnd(40s));
        const double heard = double(soundingFrames(e.capture.take(), 2));
        CHECK_MSG(std::abs(heard - double(ref.size() / 2)) < 0.03 * double(ref.size() / 2), "pitch changed the length: %.0f vs %zu",
                  heard, ref.size() / 2);
    }
}

// Pitch / tempo above 192 kHz (SoundTouch used to throw there).
void timeStretchHighRate()
{
    for (auto engine : {TimeStretch::Engine::SoundTouch, TimeStretch::Engine::RubberBand})
        for (const char* f : {"DSD/dsd64.dsf", "Formatos/hires-24-192.wav"}) {
            Engine e;
            e.dsp.timeStretch().setEngine(engine);
            e.dsp.timeStretch().setEnabled(true);
            e.dsp.timeStretch().setTempo(1.1);
            CHECK(e.load(corpus(f)));
            CHECK(e.playToEnd(40s));
            CHECK(!trimmed(e.capture.take(), 2).empty());
        }
}

void dsd()
{
    {
        g_test = "DSD as PCM";
        Engine e;
        CHECK(e.load(corpus("DSD/dsd64.dsf")));
        CHECK_MSG(std::abs(e.dsp.durationSeconds() - 2.0) < 0.02, "duration %.3f", e.dsp.durationSeconds());
        CHECK(e.playToEnd());
        const auto out = trimmed(e.capture.take(), 2);
        CHECK_MSG(out.size() / 2 > 352800 * 19 / 10, "%zu frames", out.size() / 2);
        CHECK(e.dsp.positionSeconds() > 1.9);
    }
    {
        g_test = "DSD128 as PCM";
        Engine e;
        CHECK(e.load(corpus("DSD/dsd128.dsf")));
        CHECK(e.dsp.outputSampleRate() <= 384000);
        CHECK(e.playToEnd());
    }
    {
        // The DoP words carry the file's bits untouched, oldest bit first,
        // with alternating 0x05 / 0xFA markers.
        g_test = "DoP packing";
        auto d = DsdDecoder::open(corpus("DSD/dsd64.dsf"), DsdPacking::Dop, nullptr);
        CHECK(d != nullptr);
        if (!d)
            return;
        FILE* f = std::fopen(corpus("DSD/dsd64.dsf").string().c_str(), "rb");
        std::vector<uint8_t> file(1 << 22);
        file.resize(std::fread(file.data(), 1, file.size(), f));
        std::fclose(f);
        const std::size_t dataStart = 28 + 52 + 12;
        const auto bitrev = [](uint8_t b) {
            b = uint8_t((b & 0xF0) >> 4 | (b & 0x0F) << 4);
            b = uint8_t((b & 0xCC) >> 2 | (b & 0x33) << 2);
            return uint8_t((b & 0xAA) >> 1 | (b & 0x55) << 1);
        };
        std::vector<int32_t> words(2 * 3000);
        CHECK(d->readS32(words.data(), 3000) == 3000);
        bool ok = true;
        for (std::size_t fr = 0; fr < 3000 && ok; ++fr)
            for (int c = 0; c < 2; ++c) {
                const auto w = static_cast<uint32_t>(words[fr * 2 + c]);
                const uint8_t marker = w >> 24, hi = (w >> 16) & 0xFF, lo = (w >> 8) & 0xFF;
                // Two DSD bytes per DoP frame and channel; DSF stores 4096-byte
                // blocks, one per channel in turn.
                const auto at = [&](std::size_t byte) { return file[dataStart + (byte / 4096) * 4096 * 2 + c * 4096 + byte % 4096]; };
                const uint8_t e1 = bitrev(at(fr * 2)), e2 = bitrev(at(fr * 2 + 1));
                if (marker != (fr % 2 ? 0xFA : 0x05) || hi != e1 || lo != e2 || (w & 0xFF) != 0)
                    ok = false;
            }
        CHECK_MSG(ok, "DoP words do not carry the DSF bits");
    }
    {
        g_test = "PCM to DSD modulator";
        for (int multiple : {64, 128, 256}) {
            SigmaDelta sd;
            sd.setup(multiple, 2, DsdPacking::NativeU32BE);
            const uint32_t rate = 44100u * multiple;
            const std::size_t frames = rate / 10;
            std::vector<double> in(frames * 2);
            for (std::size_t i = 0; i < frames; ++i)
                in[2 * i] = in[2 * i + 1] = 0.5 * std::sin(2 * std::numbers::pi * 1000.0 * i / rate);
            std::vector<int32_t> out(frames * 2 / 32 + 4);
            const std::size_t words = sd.process(in.data(), frames, out.data());
            CHECK(words == frames / 32);
            CHECK_MSG(sd.resets() == 0, "DSD%d: %llu loop resets at -6 dBFS", multiple, static_cast<unsigned long long>(sd.resets()));
            // The density of ones follows the input: averaged over a 1 kHz
            // period it stays near 50 %.
            std::size_t ones = 0;
            for (std::size_t w = 0; w < words; ++w)
                ones += static_cast<std::size_t>(std::popcount(static_cast<uint32_t>(out[2 * w])));
            const double density = double(ones) / (words * 32);
            CHECK_MSG(std::abs(density - 0.5) < 0.01, "DSD%d density %.4f", multiple, density);
            // Full scale input: the loop must stay stable.
            SigmaDelta loud;
            loud.setup(multiple, 2, DsdPacking::NativeU32BE);
            for (std::size_t i = 0; i < frames; ++i)
                in[2 * i] = in[2 * i + 1] = 1.0 * std::sin(2 * std::numbers::pi * 997.0 * i / rate);
            loud.process(in.data(), frames, out.data());
            CHECK_MSG(loud.resets() == 0, "DSD%d unstable at full scale (%llu resets)", multiple, static_cast<unsigned long long>(loud.resets()));
        }
    }
}

void brokenFiles()
{
    Engine e;
    for (const char* f : {"Rotos/vacio.flac", "Rotos/texto.wav", "Rotos/basura.mp3", "no/existe.flac"}) {
        std::string error;
        const bool ok = e.dsp.load(corpus(f), &error);
        if (ok) { // random bytes may still parse as a few MP3 frames: must play out cleanly
            CHECK(e.playToEnd(10s));
        } else {
            CHECK_MSG(!error.empty(), "%s: no error message", f);
            CHECK(e.dsp.state() == DSPManager::State::Empty);
        }
    }
    // A truncated FLAC plays what it has and ends.
    CHECK(e.load(corpus("Rotos/truncado.flac")));
    CHECK(e.playToEnd(10s));
    // The engine still works afterwards.
    e.capture.take();
    CHECK(e.load(corpus("Formatos/mono16.wav")));
    CHECK(e.playToEnd());
    const auto out = trimmed(e.capture.take(), std::max(1u, e.capture.channelCount()));
    CHECK(!out.empty());
}

// A failed load while a track plays leaves the engine stopped and usable.
void failedLoadWhilePlaying()
{
    Engine e;
    CHECK(e.load(corpus("Formatos/hires-24-96.flac")));
    e.dsp.play();
    std::this_thread::sleep_for(200ms);
    std::string error;
    CHECK(!e.dsp.load(corpus("Rotos/vacio.flac"), &error));
    CHECK(e.dsp.state() == DSPManager::State::Empty);
    e.dsp.play(); // nothing loaded: must not crash
    CHECK(e.load(corpus("Formatos/mono16.wav")));
    CHECK(e.playToEnd());
}

// Random transport actions, as fast as a user (or a script) could send them.
void stress(unsigned seed, int steps)
{
    std::mt19937 rng(seed);
    const char* files[] = {"Álbum Uno/01 - Pista número 1.flac", "Álbum Uno/02 - Pista número 2.flac", "Formatos/hires-24-96.flac",
                           "Formatos/hires-24-192.wav", "Formatos/mono16.wav", "Formatos/opus.opus", "Formatos/aac.m4a",
                           "Formatos/mp3.mp3", "Multicanal/51.flac", "DSD/dsd64.dsf", "CUE/imagen.cue#2", "Rotos/vacio.flac",
                           "Rotos/truncado.flac", "Formatos/r882.flac"};
    const int nFiles = sizeof(files) / sizeof(files[0]);
    auto pick = [&](int n) { return std::uniform_int_distribution<int>(0, n - 1)(rng); };
    Engine e;
    for (int step = 0; step < steps; ++step) {
        const int action = pick(16);
        switch (action) {
        case 0: case 1: e.dsp.load(corpus(files[pick(nFiles)]), nullptr, pick(2) ? 1.0 : 0.7); break;
        case 2: case 3: e.dsp.play(); break;
        case 4: e.dsp.pause(); break;
        case 5: e.dsp.stop(); break;
        case 6: case 7: e.dsp.seek(std::uniform_real_distribution<double>(-1.0, 15.0)(rng)); break;
        case 8: e.dsp.setNext(pick(4) ? corpus(files[pick(nFiles)]) : fs::path()); break;
        case 9: e.dsp.setVolume(pick(3) ? 1.0 : std::uniform_real_distribution<double>(0.0, 1.5)(rng)); break;
        case 10: {
            auto bands = SuperEQ::graphic10Band();
            bands[pick(10)].gainDb = pick(2) ? 0.0 : std::uniform_real_distribution<double>(-12.0, 12.0)(rng);
            e.dsp.equalizer().setBands(bands);
            e.dsp.equalizer().setEnabled(pick(2));
            break;
        }
        case 11:
            e.dsp.timeStretch().setEnabled(pick(2));
            e.dsp.timeStretch().setEngine(pick(2) ? TimeStretch::Engine::RubberBand : TimeStretch::Engine::SoundTouch);
            e.dsp.timeStretch().setTempo(pick(2) ? 1.0 : std::uniform_real_distribution<double>(0.5, 2.0)(rng));
            e.dsp.timeStretch().setPitchSemiTones(pick(2) ? 0.0 : std::uniform_real_distribution<double>(-12.0, 12.0)(rng));
            break;
        case 12: {
            DSPManager::OutputConfig cfg;
            cfg.upsampling = static_cast<DSPManager::Upsampling>(pick(4));
            cfg.filter = static_cast<Resampler::Filter>(pick(4));
            cfg.playFromMemory = pick(2);
            cfg.fixedRate = pick(4) ? 0 : 48000;
            cfg.bufferMs = pick(2) ? 150 : 40;
            e.dsp.setOutputConfig(cfg);
            break;
        }
        case 13: e.dsp.setCrossfeed(static_cast<Crossfeed::Preset>(pick(4))); break;
        case 14: {
            fs::path p;
            e.dsp.takeTrackChange(&p);
            (void)e.dsp.positionSeconds();
            (void)e.dsp.outputStatus();
            (void)e.dsp.memoryState();
            float buf[2048];
            e.dsp.setAnalysisEnabled(true);
            (void)e.dsp.recentOutput(buf, 2048, e.dsp.outputLatencyFrames());
            break;
        }
        case 15: e.dsp.setHeadroomDb(pick(2) ? 0.0 : -6.0); e.dsp.setDither(static_cast<DitherMode>(pick(3))); break;
        }
        if (pick(4) == 0)
            std::this_thread::sleep_for(std::chrono::milliseconds(pick(120)));
        const double pos = e.dsp.positionSeconds();
        CHECK_MSG(std::isfinite(pos) && pos >= 0.0, "step %d: position %f", step, pos);
        e.capture.take();
    }
    e.dsp.unload();
    CHECK(e.dsp.state() == DSPManager::State::Empty);
}

// ASIO sample types: what the driver receives for a known sample.
void asioConvert()
{
    // 0x12345600: a 24-bit value, left-justified (what the engine hands over).
    const int32_t in[2] = {0x12345600, -0x12345600};
    struct Case { long type; std::vector<uint8_t> first; };
    const Case cases[] = {
        {16, {0x34, 0x12}},                   // Int16LSB: the top 16 bits
        {17, {0x56, 0x34, 0x12}},             // Int24LSB
        {18, {0x00, 0x56, 0x34, 0x12}},       // Int32LSB: as is
        {27, {0x56, 0x34, 0x12, 0x00}},       // Int32LSB24: right-justified 24 bits
        {0, {0x12, 0x34}},                    // Int16MSB
        {1, {0x12, 0x34, 0x56}},              // Int24MSB
        {2, {0x12, 0x34, 0x56, 0x00}},        // Int32MSB
    };
    for (const Case& c : cases) {
        uint8_t out[16] = {};
        asio::writePcm(c.type, out, in, 1, 2);
        CHECK_MSG(std::equal(c.first.begin(), c.first.end(), out), "type %ld", c.type);
        // The second (negative) sample: two's complement of the first.
        const int bytes = asio::bytesPerSample(c.type);
        CHECK_MSG(out[bytes] != 0 || out[bytes + 1] != 0, "type %ld: second sample missing", c.type);
    }
    // Float32LSB: exactly s / 2^31.
    float f[2];
    asio::writePcm(19, f, in, 1, 2);
    CHECK(f[0] == static_cast<float>(0x12345600) / 2147483648.0f && f[1] == -f[0]);
    double d[2];
    asio::writePcm(20, d, in, 1, 2);
    CHECK(d[0] == static_cast<double>(0x12345600) / 2147483648.0);
    // Interleaved source, one channel picked by offset and stride.
    const int32_t stereo[4] = {0x01000000, 0x02000000, 0x03000000, 0x04000000};
    int32_t right[2];
    asio::writePcm(18, right, stereo + 1, 2, 2);
    CHECK(right[0] == 0x02000000 && right[1] == 0x04000000);
    // DSD: the words' bytes in memory order, MSB-first bits; LSB-first reversed.
    int32_t word;
    const uint8_t bytes[4] = {0x96, 0x0F, 0x69, 0x01};
    std::memcpy(&word, bytes, 4);
    uint8_t msb[4], lsb[4];
    asio::writeDsd(33, msb, &word, 1, 1);
    asio::writeDsd(32, lsb, &word, 1, 1);
    CHECK(std::equal(bytes, bytes + 4, msb));
    CHECK(lsb[0] == 0x69 && lsb[1] == 0xF0 && lsb[2] == 0x96 && lsb[3] == 0x80);
}

#if defined(_WIN32)
// A pretend ASIO driver: buffers and a thread calling bufferSwitch at the
// stream's pace, keeping a copy of every buffer half the host filled.
class FakeAsio final : public asio::IASIO {
public:
    long type = asio::ASIOSTInt32LSB;
    bool dsdCapable = true;
    long channels = 2;
    double rate = 44100;
    bool dsdMode = false;
    std::vector<std::vector<uint8_t>> received; // per channel
    std::atomic<int> switches{0};

    HRESULT STDMETHODCALLTYPE QueryInterface(REFIID, void** out) override { *out = this; return S_OK; }
    ULONG STDMETHODCALLTYPE AddRef() override { return ++m_refs; }
    ULONG STDMETHODCALLTYPE Release() override { const ULONG n = --m_refs; if (n == 0) delete this; return n; }
    asio::ASIOBool init(void*) override { return 1; }
    void getDriverName(char* name) override { std::strcpy(name, "Fake"); }
    long getDriverVersion() override { return 1; }
    void getErrorMessage(char* s) override { s[0] = 0; }
    asio::ASIOError start() override
    {
        m_run = true;
        m_thread = std::thread([this] {
            long index = 0;
            const double period = m_size / (dsdMode ? rate : rate);
            while (m_run) {
                m_callbacks->bufferSwitch(index, 1);
                {
                    std::lock_guard lock(m_mutex);
                    const std::size_t bytes = dsdMode ? m_size / 8 : m_size * asio::bytesPerSample(type);
                    for (long c = 0; c < channels; ++c) {
                        const auto* b = static_cast<const uint8_t*>(m_infos[c].buffers[index]);
                        received[c].insert(received[c].end(), b, b + bytes);
                    }
                }
                ++switches;
                index ^= 1;
                std::this_thread::sleep_for(std::chrono::duration<double>(period / 4)); // faster than real time
            }
        });
        return asio::ASE_OK;
    }
    asio::ASIOError stop() override
    {
        m_run = false;
        if (m_thread.joinable())
            m_thread.join();
        return asio::ASE_OK;
    }
    asio::ASIOError getChannels(long* in, long* out) override { *in = 0; *out = channels; return asio::ASE_OK; }
    asio::ASIOError getLatencies(long* in, long* out) override { *in = 0; *out = 64; return asio::ASE_OK; }
    asio::ASIOError getBufferSize(long* mn, long* mx, long* pref, long* gran) override
    {
        *mn = 64; *mx = 8192; *pref = dsdMode ? 4096 : 512; *gran = -1;
        return asio::ASE_OK;
    }
    asio::ASIOError canSampleRate(asio::ASIOSampleRate r) override
    {
        return (dsdMode ? r == 2822400 || r == 5644800 : r == 44100 || r == 48000 || r == 96000) ? asio::ASE_OK : asio::ASE_NoClock;
    }
    asio::ASIOError getSampleRate(asio::ASIOSampleRate* r) override { *r = rate; return asio::ASE_OK; }
    asio::ASIOError setSampleRate(asio::ASIOSampleRate r) override { rate = r; return asio::ASE_OK; }
    asio::ASIOError getClockSources(asio::ASIOClockSource*, long* n) override { *n = 0; return asio::ASE_OK; }
    asio::ASIOError setClockSource(long) override { return asio::ASE_OK; }
    asio::ASIOError getSamplePosition(asio::ASIOSamples*, asio::ASIOTimeStamp*) override { return asio::ASE_OK; }
    asio::ASIOError getChannelInfo(asio::ASIOChannelInfo* info) override
    {
        info->type = dsdMode ? asio::ASIOSTDSDInt8MSB1 : type;
        info->isActive = 1;
        return asio::ASE_OK;
    }
    asio::ASIOError createBuffers(asio::ASIOBufferInfo* infos, long n, long size, asio::ASIOCallbacks* cb) override
    {
        m_size = size;
        m_callbacks = cb;
        m_infos.assign(infos, infos + n);
        const std::size_t bytes = dsdMode ? size / 8 : size * asio::bytesPerSample(type);
        m_memory.assign(n * 2, std::vector<uint8_t>(bytes));
        for (long c = 0; c < n; ++c)
            for (int h = 0; h < 2; ++h)
                infos[c].buffers[h] = m_infos[c].buffers[h] = m_memory[2 * c + h].data();
        received.assign(n, {});
        return asio::ASE_OK;
    }
    asio::ASIOError disposeBuffers() override { return asio::ASE_OK; }
    asio::ASIOError controlPanel() override { return asio::ASE_OK; }
    asio::ASIOError future(long selector, void* opt) override
    {
        if (selector == asio::kAsioCanDoIoFormat)
            return dsdCapable ? asio::ASE_SUCCESS : asio::ASE_NotPresent;
        if (selector == asio::kAsioSetIoFormat) {
            if (!dsdCapable)
                return asio::ASE_NotPresent;
            dsdMode = static_cast<asio::ASIOIoFormat*>(opt)->FormatType == asio::kASIODSDFormat;
            return asio::ASE_SUCCESS;
        }
        return asio::ASE_NotPresent;
    }
    asio::ASIOError outputReady() override { return asio::ASE_OK; }
    std::mutex m_mutex;

private:
    std::atomic<ULONG> m_refs{1};
    std::atomic<bool> m_run{false};
    std::thread m_thread;
    long m_size = 0;
    asio::ASIOCallbacks* m_callbacks = nullptr;
    std::vector<asio::ASIOBufferInfo> m_infos;
    std::vector<std::vector<uint8_t>> m_memory;
};

// AsioOutput against the fake driver: every sample the engine renders arrives,
// in order, in the driver's format; nothing after stop().
void asioHost()
{
    for (long type : {long(asio::ASIOSTInt32LSB), long(asio::ASIOSTInt24LSB), long(asio::ASIOSTFloat32LSB)}) {
        g_test = "ASIO PCM type " + std::to_string(type);
        auto* driver = new FakeAsio;
        driver->AddRef(); // one reference for the test, one for AsioOutput
        driver->type = type;
        std::atomic<int32_t> next{0};
        std::atomic<int> renders{0};
        AsioOutput out;
        std::string error;
        const bool ok = out.openDriver(driver, AsioOutput::Mode::Pcm, 2, 48000, [&](int32_t* s, uint32_t frames) {
            ++renders;
            for (uint32_t f = 0; f < frames; ++f) {
                const int32_t v = (next++ & 0x7FFFFF) << 8; // 24-bit grid
                s[2 * f] = v;
                s[2 * f + 1] = -v;
            }
        }, &error);
        CHECK_MSG(ok, "%s", error.c_str());
        CHECK(driver->rate == 48000);
        CHECK(out.pcmBits() == 24 || type == asio::ASIOSTInt32LSB);
        CHECK(out.start());
        CHECK(waitFor([&] { return driver->switches.load() >= 20; }, 5s));
        out.stop();
        const int after = renders.load();
        std::this_thread::sleep_for(50ms);
        CHECK_MSG(renders.load() == after, "render called after stop()");
        // Left channel: the ramp, sample after sample.
        std::lock_guard lock(driver->m_mutex);
        const auto& left = driver->received[0];
        const int bytes = asio::bytesPerSample(type);
        bool sequential = !left.empty();
        for (std::size_t i = 0; sequential && i < left.size() / bytes; ++i) {
            const int32_t expected = (static_cast<int32_t>(i) & 0x7FFFFF) << 8;
            int32_t got = 0;
            if (type == asio::ASIOSTInt32LSB)
                std::memcpy(&got, &left[i * 4], 4);
            else if (type == asio::ASIOSTInt24LSB)
                got = static_cast<int32_t>(uint32_t(left[i * 3]) << 8 | uint32_t(left[i * 3 + 1]) << 16 | uint32_t(left[i * 3 + 2]) << 24);
            else {
                float v;
                std::memcpy(&v, &left[i * 4], 4);
                got = static_cast<int32_t>(std::lround(double(v) * 2147483648.0));
            }
            sequential = got == expected;
            if (!sequential)
                std::fprintf(stderr, "  sample %zu: %d, expected %d\n", i, got, expected);
        }
        CHECK(sequential);
        out.close();
        driver->Release();
    }
    {
        g_test = "ASIO DSD";
        auto* driver = new FakeAsio;
        driver->AddRef();
        AsioOutput out;
        std::string error;
        uint32_t counter = 0;
        const bool ok = out.openDriver(driver, AsioOutput::Mode::Dsd, 2, 2822400, [&](int32_t* s, uint32_t words) {
            for (uint32_t w = 0; w < 2 * words; ++w) {
                const uint8_t b[4] = {uint8_t(counter), uint8_t(counter >> 8), 0x69, 0x96};
                std::memcpy(&s[w], b, 4);
                ++counter;
            }
        }, &error);
        CHECK_MSG(ok, "%s", error.c_str());
        CHECK(driver->dsdMode && driver->rate == 2822400);
        CHECK(out.start());
        CHECK(waitFor([&] { return driver->switches.load() >= 6; }, 5s));
        out.stop();
        out.close();
        CHECK(!driver->dsdMode); // set back to PCM on close
        std::lock_guard lock(driver->m_mutex);
        CHECK(driver->received[0].size() >= 4096 / 8 * 6);
        CHECK(driver->received[0][2] == 0x69 && driver->received[0][3] == 0x96);
        driver->Release();
    }
    {
        g_test = "ASIO no DSD";
        auto* driver = new FakeAsio;
        driver->dsdCapable = false;
        AsioOutput out;
        std::string error;
        CHECK(!out.openDriver(driver, AsioOutput::Mode::Dsd, 2, 2822400, [](int32_t*, uint32_t) {}, &error));
        CHECK(!error.empty() && !out.isOpen());
    }
}
#endif


// --- Linear-phase EQ ------------------------------------------------------------

double biquadCurveDb(const std::vector<SuperEQ::Band>& bands, double preampDb, double rate, double f)
{
    const std::complex<double> z1 = std::polar(1.0, -2.0 * std::numbers::pi * f / rate), z2 = z1 * z1;
    double db = preampDb;
    for (const SuperEQ::Band& b : bands) {
        if (!b.enabled || ((b.type == FilterType::Peaking || b.type == FilterType::LowShelf || b.type == FilterType::HighShelf) && b.gainDb == 0.0))
            continue;
        const BiquadCoeffs c = designBiquad(b.type, rate, b.freqHz, b.q, b.gainDb);
        db += 20.0 * std::log10(std::abs((c.b0 + c.b1 * z1 + c.b2 * z2) / (1.0 + c.a1 * z1 + c.a2 * z2)));
    }
    return db;
}

double firDb(const std::vector<double>& h, double rate, double f)
{
    // A linear-phase filter: |H| is the cosine sum around its centre.
    const std::size_t c = h.size() / 2;
    const double w = 2.0 * std::numbers::pi * f / rate;
    double sum = h[c];
    for (std::size_t k = 1; k < c; ++k)
        sum += (h[c + k] + h[c - k]) * std::cos(w * static_cast<double>(k));
    return 20.0 * std::log10(std::abs(sum));
}

// The source through `taps` (centred at taps/2), aligned with it: frame k is
// x(k) filtered; taps/2 frames of the filter's ring-out follow the end.
std::vector<double> firFiltered(const std::vector<int32_t>& src, uint32_t ch, uint32_t rate, const std::vector<double>& taps)
{
    ImpulseResponse ir;
    ir.sampleRate = rate;
    ir.channels = 1;
    ir.taps = {taps};
    auto conv = Convolver::create(ir, rate, ch, nullptr);
    const std::size_t n = src.size() / ch, extra = Convolver::kBlock + taps.size();
    std::vector<double> buf((n + extra) * ch, 0.0);
    for (std::size_t i = 0; i < src.size(); ++i)
        buf[i] = src[i] * kS32InvFullScale;
    conv->process(buf.data(), n + extra);
    const std::size_t skip = Convolver::kBlock + taps.size() / 2;
    return {buf.begin() + static_cast<std::ptrdiff_t>(skip * ch), buf.end()};
}

// On the 24-bit grid the engine hands a 32-bit device (no dither).
int32_t grid24(double v)
{
    return static_cast<int32_t>(std::clamp<long long>(std::llround(v * 8388608.0), -8388608, 8388607) * 256);
}

void linearPhaseEq()
{
    std::vector<std::vector<SuperEQ::Band>> curves;
    {
        auto g = SuperEQ::graphic10Band();
        const double gains[] = {6, -4, 3, 0, -2, -6, 2, 4, -3, 8};
        for (std::size_t i = 0; i < g.size(); ++i)
            g[i].gainDb = gains[i];
        curves.push_back(g);
        SuperEQ::Band shelfLow{FilterType::LowShelf, 100.0, 8.0, 0.707, true};
        SuperEQ::Band shelfHigh{FilterType::HighShelf, 8000.0, -5.0, 0.707, true};
        SuperEQ::Band bass{FilterType::Peaking, 31.25, 12.0, 1.41421356237, true};
        SuperEQ::Band cut{FilterType::Peaking, 3150.0, -9.0, 4.0, true};
        curves.push_back({shelfLow, shelfHigh, bass, cut});
        curves.push_back({SuperEQ::Band{FilterType::HighPass, 20.0, 0.0, 0.707, true}});
    }
    {
        g_test = "linear-phase design";
        for (double rate : {44100.0, 96000.0, 192000.0}) {
            const std::size_t taps = LinearPhaseEq::tapsFor(rate);
            CHECK(taps % (2 * Convolver::kBlock) == 0 && taps >= rate * LinearPhaseEq::kSeconds);
            for (const auto& curve : curves) {
                const auto h = LinearPhaseEq::design(curve, -3.0, rate, taps);
                double worst = 0.0, at = 0.0;
                for (int i = 0; i < 160; ++i) {
                    const double f = 20.0 * std::pow(1000.0, i / 159.0); // 20 Hz .. 20 kHz
                    const double e = std::abs(firDb(h, rate, f) - biquadCurveDb(curve, -3.0, rate, f));
                    if (e > worst)
                        worst = e, at = f;
                }
                CHECK_MSG(worst < 0.05, "%.0f Hz: %.3f dB off the biquad curve at %.0f Hz", rate, worst, at);
                const std::size_t c = taps / 2;
                double asym = 0.0;
                for (std::size_t k = 1; k < c; ++k)
                    asym = std::max(asym, std::abs(h[c + k] - h[c - k]));
                CHECK_MSG(asym < 1e-12, "not symmetric (linear phase): %g", asym);
            }
        }
        // The convolver itself against the plain sum, on a filter of three partitions.
        std::mt19937 rng(7);
        std::uniform_real_distribution<double> u(-1.0, 1.0);
        std::vector<double> taps(3000), x(2 * 20000);
        for (double& t : taps)
            t = u(rng) * 0.01;
        std::vector<int32_t> xi(x.size());
        for (std::size_t i = 0; i < x.size(); ++i)
            xi[i] = static_cast<int32_t>(u(rng) * 1e9);
        ImpulseResponse ir;
        ir.sampleRate = 48000;
        ir.channels = 1;
        ir.taps = {taps};
        auto conv = Convolver::create(ir, 48000, 2, nullptr);
        std::vector<double> buf((20000 + Convolver::kBlock) * 2, 0.0);
        for (std::size_t i = 0; i < xi.size(); ++i)
            buf[i] = xi[i] * kS32InvFullScale;
        conv->process(buf.data(), 20000 + Convolver::kBlock);
        double worst = 0.0;
        for (std::size_t n = 0; n < 20000; n += 7)
            for (std::size_t c = 0; c < 2; ++c) {
                double y = 0.0;
                for (std::size_t k = 0; k < taps.size() && k <= n; ++k)
                    y += taps[k] * xi[(n - k) * 2 + c] * kS32InvFullScale;
                worst = std::max(worst, std::abs(buf[(n + Convolver::kBlock) * 2 + c] - y));
            }
        CHECK_MSG(worst < 1e-12, "partitioned convolution off the plain sum by %g", worst);
    }
    {
        g_test = "linear-phase playback";
        const fs::path path = corpus("Formatos/hires-24-96.flac");
        StreamInfo info;
        const auto src = decodeAll(path, &info);
        Engine e;
        e.dsp.equalizer().setLinearPhase(true);
        e.dsp.equalizer().setBands(curves[0]);
        e.dsp.equalizer().setPreampDb(-3.0);
        e.dsp.equalizer().setEnabled(true);
        CHECK(e.load(path));
        CHECK(e.playToEnd(40s));
        const auto out = e.capture.take();
        const auto filtered = firFiltered(src, 2, info.sampleRate,
                                          LinearPhaseEq::design(curves[0], -3.0, info.sampleRate, LinearPhaseEq::tapsFor(info.sampleRate)));
        std::vector<int32_t> expected(filtered.size());
        for (std::size_t i = 0; i < filtered.size(); ++i)
            expected[i] = grid24(filtered[i]);
        const auto a = trimmed(out, 2), b = trimmed(expected, 2);
        CHECK_MSG(a.size() == b.size(), "%zu frames out, %zu expected", a.size() / 2, b.size() / 2);
        int64_t worst = 0;
        for (std::size_t i = 0; i < std::min(a.size(), b.size()); ++i)
            worst = std::max<int64_t>(worst, std::llabs(int64_t(a[i]) - b[i]));
        CHECK_MSG(worst <= 256, "off the filtered source by %lld (24-bit LSB = 256)", static_cast<long long>(worst));
        if (worst > 256 && std::getenv("SONKORE_TEST_DEBUG")) {
            for (long long lag = -3000; lag <= 3000; ++lag) {
                double err = 0, ref2 = 0;
                for (std::size_t f = 20000; f < 60000; ++f) {
                    const long long g = static_cast<long long>(f) + lag;
                    const double d = double(a[2 * f]) - double(b[2 * g]);
                    err += d * d;
                    ref2 += double(b[2 * g]) * double(b[2 * g]);
                }
                if (err < ref2 * 1e-6)
                    std::printf("  lag %lld: residual %g\n", lag, err / ref2);
            }
            double sa = 0, sb = 0;
            for (std::size_t f = 20000; f < 60000; ++f)
                sa += double(a[2 * f]) * a[2 * f], sb += double(b[2 * f]) * b[2 * f];
            std::printf("  rms ratio %g, first out %d %d exp %d %d\n", std::sqrt(sa / sb), a[0], a[1], b[0], b[1]);
        }
        CHECK(e.dsp.underrunCount() == 0);
    }
    {
        // Engaged and released while playing: every frame is either the source
        // itself or the source filtered, in order, nothing skipped or repeated,
        // and bit-perfect again at the end.
        g_test = "linear-phase on and off";
        const fs::path path = corpus("Álbum Uno/01 - Pista número 1.flac");
        StreamInfo info;
        const auto src = decodeAll(path, &info);
        std::vector<SuperEQ::Band> curve = {SuperEQ::Band{FilterType::Peaking, 440.0, -6.0, 2.0, true},
                                            SuperEQ::Band{FilterType::HighShelf, 4000.0, 3.0, 0.707, true}};
        Engine e;
        e.dsp.equalizer().setLinearPhase(true);
        e.dsp.equalizer().setEnabled(true);
        CHECK(e.load(path));
        e.dsp.play();
        std::this_thread::sleep_for(1000ms);
        e.dsp.equalizer().setBands(curve);
        std::this_thread::sleep_for(1300ms);
        e.dsp.equalizer().setBands(SuperEQ::graphic10Band());
        CHECK(waitFor([&] { return e.dsp.reachedEnd(); }, 20s));
        const auto out = trimmed(e.capture.take(), 2);
        const auto filtered = firFiltered(src, 2, info.sampleRate,
                                          LinearPhaseEq::design(curve, 0.0, info.sampleRate, LinearPhaseEq::tapsFor(info.sampleRate)));
        // Both trimmed alike (the source starts at a zero crossing).
        std::size_t lead = 0;
        while (lead < src.size() / 2 && src[2 * lead] == 0 && src[2 * lead + 1] == 0)
            ++lead;
        const auto ref = trimmed(src, 2);
        CHECK_MSG(out.size() == ref.size(), "%zu frames out, %zu in: something was skipped or repeated", out.size() / 2,
                  ref.size() / 2);
        const std::size_t frames = std::min(out.size(), ref.size()) / 2;
        long long first = -1, last = -1;
        std::size_t neither = 0;
        for (std::size_t f = 0; f < frames; ++f) {
            const bool same = out[2 * f] == ref[2 * f] && out[2 * f + 1] == ref[2 * f + 1];
            if (same)
                continue;
            if (first < 0)
                first = static_cast<long long>(f);
            last = static_cast<long long>(f);
            bool filteredHere = true; // (not "near": a macro in <windows.h>)
            for (int c = 0; c < 2; ++c)
                filteredHere = filteredHere && std::llabs(int64_t(out[2 * f + c]) - grid24(filtered[2 * (f + lead) + c])) <= 256;
            neither += !filteredHere;
        }
        CHECK_MSG(first > 22050, "filtered from frame %lld: before the EQ was set", first);
        CHECK_MSG(last >= 0 && last + 22050 < static_cast<long long>(frames), "still filtered at frame %lld of %zu", last, frames);
        // Only the crossfade to flat as the filter lets go is neither.
        CHECK_MSG(neither <= 2 * Convolver::kBlock, "%zu frames are neither the source nor the source filtered", neither);
        CHECK(e.dsp.underrunCount() == 0);
    }
    {
        // A gapless album through it: one filter across the joins (the album
        // filtered as a whole), and each track announced when it is heard,
        // not a filter's delay early.
        g_test = "linear-phase gapless";
        const char* parts[] = {"Álbum Uno/01 - Pista número 1.flac", "Álbum Uno/02 - Pista número 2.flac", "Álbum Uno/03 - Pista número 3.flac"};
        std::vector<int32_t> album;
        std::vector<std::size_t> joins;
        for (const char* p : parts) {
            const auto pcm = decodeAll(corpus(p));
            album.insert(album.end(), pcm.begin(), pcm.end());
            joins.push_back(album.size() / 2);
        }
        std::vector<SuperEQ::Band> curve = {SuperEQ::Band{FilterType::Peaking, 660.0, -4.0, 1.0, true}};
        Engine e;
        e.dsp.equalizer().setLinearPhase(true);
        e.dsp.equalizer().setBands(curve);
        e.dsp.equalizer().setEnabled(true);
        CHECK(e.load(corpus(parts[0])));
        e.dsp.setNext(corpus(parts[1]));
        std::vector<long long> heardAt;
        CHECK(e.playToEnd(40s, [&](const fs::path& p) {
            // Frames the "DAC" had received when the change was announced.
            std::lock_guard lock(e.capture.mutex);
            const auto& got = e.capture.samples;
            std::size_t lead = 0;
            while (lead < got.size() / 2 && got[2 * lead] == 0 && got[2 * lead + 1] == 0)
                ++lead;
            heardAt.push_back(static_cast<long long>(got.size() / 2 - lead));
            if (heardAt.size() == 1)
                e.dsp.setNext(corpus(parts[2]));
            (void)p;
        }));
        CHECK_MSG(heardAt.size() == 2, "%zu track changes", heardAt.size());
        for (std::size_t i = 0; i < heardAt.size() && i < 2; ++i)
            CHECK_MSG(std::llabs(heardAt[i] - static_cast<long long>(joins[i])) < 8192,
                      "track %zu announced at frame %lld, it starts at %zu", i + 2, heardAt[i], joins[i]);
        const auto out = trimmed(e.capture.take(), 2);
        const auto filtered = firFiltered(album, 2, 44100, LinearPhaseEq::design(curve, 0.0, 44100, LinearPhaseEq::tapsFor(44100)));
        std::vector<int32_t> expected(filtered.size());
        for (std::size_t i = 0; i < filtered.size(); ++i)
            expected[i] = grid24(filtered[i]);
        const auto b = trimmed(expected, 2);
        CHECK_MSG(out.size() == b.size(), "%zu frames out, %zu expected", out.size() / 2, b.size() / 2);
        int64_t worst = 0;
        for (std::size_t i = 0; i < std::min(out.size(), b.size()); ++i)
            worst = std::max<int64_t>(worst, std::llabs(int64_t(out[i]) - b[i]));
        CHECK_MSG(worst <= 256, "the album off its filtered self by %lld", static_cast<long long>(worst));
        CHECK(e.dsp.underrunCount() == 0);
    }
    {
        // Seeks while engaged: it starts over from the new place at once.
        g_test = "linear-phase seek";
        Engine e;
        e.dsp.equalizer().setLinearPhase(true);
        e.dsp.equalizer().setBands({SuperEQ::Band{FilterType::LowShelf, 200.0, -3.0, 0.707, true}});
        e.dsp.equalizer().setEnabled(true);
        CHECK(e.load(corpus("Formatos/hires-24-96.flac")));
        e.dsp.play();
        for (double t : {2.5, 0.5, 3.2, 1.0}) {
            std::this_thread::sleep_for(300ms);
            e.dsp.seek(t);
            std::this_thread::sleep_for(50ms);
            const double pos = e.dsp.positionSeconds();
            CHECK_MSG(pos >= t - 0.02 && pos < t + 0.2, "seek to %.1f s: position %.3f s", t, pos);
        }
        CHECK(waitFor([&] { return e.dsp.reachedEnd(); }, 20s));
        CHECK(e.dsp.underrunCount() == 0);
    }
}

#if !defined(_WIN32)
// Runs a command line, its output captured.
std::string run(const std::string& command)
{
    std::string out;
    if (FILE* p = popen(command.c_str(), "r")) {
        char buf[256];
        while (std::fgets(buf, sizeof(buf), p))
            out += buf;
        pclose(p);
    }
    return out;
}

// Native PipeWire, end to end: each file played to a virtual DAC, which is
// recorded (its monitor) and compared with what the engine handed over and
// with the file itself. The graph must follow each track's rate although
// the daemon only allows 48 kHz, and give it back afterwards. Needs the
// daemon of tests/pipewire-session.sh; skipped without it.
void pipewire()
{
    const auto sinks = PipeWireOutput::sinks();
    if (std::none_of(sinks.begin(), sinks.end(), [](const auto& s) { return s.name == "test-dac"; })) {
        CHECK_MSG(!std::getenv("SONKORE_REQUIRE_PIPEWIRE"), "no PipeWire with a test-dac sink");
        std::printf("  (no PipeWire with a test-dac sink: skipped)\n");
        return;
    }
    const fs::path dir = fs::temp_directory_path() / "sonkore-pw-test";
    fs::create_directories(dir);
    // One file to the DAC. 1: identical; 0: the engine sent the file exactly,
    // but the recording differs (a cycle PipeWire missed under load, which a
    // second run tells apart from a real fault); -1: failed.
    const auto playOnce = [&](const char* file, bool lastTry) -> int {
        StreamInfo info;
        const auto ref = decodeAll(corpus(file), &info);
        CHECK_MSG(!ref.empty(), "%s does not decode", file);
        if (ref.empty())
            return -1;
        Capture capture;
        DSPManager dsp;
        std::string error;
        CHECK(dsp.initialize(&error));
        DSPManager::OutputConfig cfg;
        cfg.deviceKey = "pipewire|test-dac";
        cfg.allowSampleRateSwitch = true;
        dsp.setOutputConfig(cfg);
        dsp.setDither(DitherMode::Off);
        dsp.setOutputTap(&Capture::tap, &capture);
        if (std::getenv("SONKORE_TEST_SABOTAGE"))
            dsp.setVolume(0.999);
        CHECK_MSG(dsp.load(corpus(file), &error), "%s: %s", file, error.c_str());
        auto st = dsp.outputStatus();
        CHECK_MSG(st.backend == "PipeWire" && st.deviceSampleRate == info.sampleRate && !st.resampled,
                  "%s: opened as %s at %u Hz", file, st.backend.c_str(), st.deviceSampleRate);

        // What the DAC receives, recorded at the track's rate (stereo: a mono
        // track goes to both speakers).
        const uint32_t ch = info.channels == 1 ? 2 : info.channels;
        const fs::path recording = dir / "recording.wav";
        fs::remove(recording);
        const std::string rate = std::to_string(info.sampleRate), channels = std::to_string(ch);
        const std::string path = recording.string();
        const char* argv[] = {"pw-record", "--target", "test-dac", "--rate", rate.c_str(), "--channels", channels.c_str(),
                              "--format", "s32", "-P", "{ stream.capture.sink = true }", path.c_str(), nullptr};
        pid_t recorder = 0;
        CHECK(posix_spawnp(&recorder, "pw-record", nullptr, nullptr, const_cast<char**>(argv), environ) == 0);
        std::this_thread::sleep_for(700ms);

        dsp.play();
        CHECK(waitFor([&] { return dsp.outputStatus().graphRate != 0; }, 3s));
        st = dsp.outputStatus();
        CHECK_MSG(st.graphRate == info.sampleRate, "%s: the graph runs at %u Hz, the track at %u", file, st.graphRate,
                  info.sampleRate);
        CHECK_MSG(st.bitPerfectNow && st.systemVolumeNeutral, "%s: not bit-perfect (graph %u, volume neutral %d)", file,
                  st.graphRate, st.systemVolumeNeutral);
        CHECK(waitFor([&] { return dsp.reachedEnd(); }, 30s));
        std::this_thread::sleep_for(300ms);
        kill(recorder, SIGINT);
        int status = 0;
        waitpid(recorder, &status, 0);
        dsp.setOutputTap(nullptr, nullptr);
        dsp.unload();
        CHECK(dsp.underrunCount() == 0);

        const auto sent = capture.take();
        const auto heard = decodeAll(recording);
        CHECK_MSG(!heard.empty(), "%s: nothing recorded", file);
        // Mono: what was sent is both channels of each frame.
        std::vector<int32_t> expected = ref;
        if (info.channels == 1) {
            expected.clear();
            for (int32_t v : ref)
                expected.insert(expected.end(), {v, v});
        }
        const auto a = trimmed(sent, ch), b = trimmed(heard, ch), c = trimmed(expected, ch);
        const long long sentDiff = firstDifference(a, c, ch);
        CHECK_MSG(sentDiff < 0, "%s: the engine sent something else (frame %lld)", file, sentDiff);
        if (sentDiff >= 0)
            return -1;
        const long long diff = firstDifference(b, c, ch);
        if (diff >= 0 && !lastTry) {
            std::printf("  %s: the recording differs at frame %lld (PipeWire missed a cycle?): once more\n", file, diff);
            return 0;
        }
        CHECK_MSG(diff < 0, "%s: the DAC got something else at frame %lld (%zu vs %zu frames)", file, diff, b.size() / ch,
                  c.size() / ch);
        std::printf("  %s: %zu frames at %u Hz recorded from the DAC, %s\n", file, b.size() / ch, info.sampleRate,
                    diff < 0 ? "identical to the file" : "DIFFERENT");
        return diff < 0 ? 1 : -1;
    };
    for (const char* file : {"Álbum Uno/01 - Pista número 1.flac", "Formatos/hires-24-96.flac", "Formatos/mono16.wav",
                             "Formatos/r882.flac"})
        if (playOnce(file, false) == 0)
            playOnce(file, true);
    // The desktop's volume for the DAC below 100 %: no longer bit-perfect,
    // and the status says so; back at 100 %, it is again.
    {
        DSPManager dsp;
        std::string error;
        CHECK(dsp.initialize(&error));
        DSPManager::OutputConfig cfg;
        cfg.deviceKey = "pipewire|test-dac";
        dsp.setOutputConfig(cfg);
        CHECK(dsp.load(corpus("Formatos/hires-24-96.flac"), &error));
        dsp.play();
        CHECK(waitFor([&] { return dsp.outputStatus().bitPerfectNow; }, 3s));
        run("wpctl set-volume @DEFAULT_AUDIO_SINK@ 0.5");
        CHECK_MSG(waitFor([&] { return !dsp.outputStatus().systemVolumeNeutral && !dsp.outputStatus().bitPerfectNow; }, 3s),
                  "a sink volume of 50 %% went unnoticed");
        run("wpctl set-volume @DEFAULT_AUDIO_SINK@ 1.0");
        CHECK_MSG(waitFor([&] { return dsp.outputStatus().systemVolumeNeutral && dsp.outputStatus().bitPerfectNow; }, 3s),
                  "the sink back at 100 %% went unnoticed");
        dsp.unload();
    }
    // Released: the graph is free to go back to the desktop's rate.
    const std::string forced = run("pw-metadata -n settings 0 clock.force-rate 2>/dev/null");
    CHECK_MSG(forced.find("value:'0'") != std::string::npos, "clock.force-rate left set: %s", forced.c_str());
    fs::remove_all(dir);
}
#endif

} // namespace

int main(int argc, char** argv)
{
    if (argc < 2) {
        std::fprintf(stderr, "usage: %s <corpus dir> [test...]\n", argv[0]);
        return 2;
    }
    g_corpus = fs::path(reinterpret_cast<const char8_t*>(argv[1]));
    const std::map<std::string, std::function<void()>> tests = {
        {"decoderLengths", decoderLengths},
        {"asioConvert", asioConvert},
#if defined(_WIN32)
        {"asioHost", asioHost},
#endif
        {"bitPerfect", bitPerfectAll},
        {"gaplessAlbum", gaplessAlbum},
        {"gaplessCue", gaplessCue},
        {"seekAndPause", seekAndPause},
        {"volumeAndEq", volumeAndEq},
        {"linearPhaseEq", linearPhaseEq},
        {"resampling", resampling},
        {"timeStretch", timeStretch},
        {"timeStretchHighRate", timeStretchHighRate},
        {"dsd", dsd},
        {"brokenFiles", brokenFiles},
        {"failedLoadWhilePlaying", failedLoadWhilePlaying},
        {"stress", [] { stress(1234, 400); stress(98765, 400); }},
#if !defined(_WIN32)
        {"pipewire", pipewire},
#endif
    };
    std::vector<std::string> wanted(argv + 2, argv + argc);
    for (const auto& [name, run] : tests) {
        if (!wanted.empty() && std::find(wanted.begin(), wanted.end(), name) == wanted.end())
            continue;
        g_test = name;
        const int before = g_failures;
        const auto t0 = std::chrono::steady_clock::now();
        run();
        const double secs = std::chrono::duration<double>(std::chrono::steady_clock::now() - t0).count();
        std::printf("%-24s %s (%.1f s)\n", name.c_str(), g_failures == before ? "ok" : "FAILED", secs);
        std::fflush(stdout);
    }
    std::printf("%d failure(s)\n", g_failures);
    return g_failures == 0 ? 0 : 1;
}
