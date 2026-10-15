#include "Loudness.hpp"

#include "Decoder.hpp"

#include <ebur128.h>

#include <algorithm>
#include <cmath>
#include <memory>

namespace sonkore::Loudness {

namespace {

constexpr uint64_t kBlockFrames = 16384;

struct StateDeleter {
    void operator()(ebur128_state* st) const { ebur128_destroy(&st); }
};
using State = std::unique_ptr<ebur128_state, StateDeleter>;

// Feeds one file into a fresh R128 state. Null when the file cannot be read.
State measureFile(const std::filesystem::path& path, const std::atomic<bool>& cancel, double& peak)
{
    auto decoder = Decoder::open(path, nullptr);
    if (!decoder)
        return nullptr;
    const StreamInfo& info = decoder->info();
    if (info.channels == 0 || info.sampleRate == 0)
        return nullptr;
    // Histogram mode: a few KB per state, so a whole album can be kept for
    // the album measurement.
    State st(ebur128_init(info.channels, info.sampleRate, EBUR128_MODE_I | EBUR128_MODE_TRUE_PEAK | EBUR128_MODE_HISTOGRAM));
    if (!st)
        return nullptr;
    // Channels weighed as they are heard: mono plays on both speakers (+3 dB
    // over one), and the side pair of a 7.1 file (WAVE order FL FR FC LFE BL
    // BR SL SR) counts like the back pair, which libebur128 would ignore.
    if (info.channels == 1)
        ebur128_set_channel(st.get(), 0, EBUR128_DUAL_MONO);
    if (info.channels == 8) {
        ebur128_set_channel(st.get(), 6, EBUR128_Mp090);
        ebur128_set_channel(st.get(), 7, EBUR128_Mm090);
    }

    // Doubles: the true peak of a lossy or float file above full scale is
    // measured, not clipped first (integer sources are exact either way).
    std::vector<double> buffer(kBlockFrames * info.channels);
    for (;;) {
        if (cancel.load(std::memory_order_relaxed))
            return nullptr;
        const uint64_t got = decoder->readDouble(buffer.data(), kBlockFrames);
        if (got > 0 && ebur128_add_frames_double(st.get(), buffer.data(), static_cast<size_t>(got)) != EBUR128_SUCCESS)
            return nullptr;
        if (got < kBlockFrames)
            break;
    }

    peak = 0.0;
    for (unsigned c = 0; c < info.channels; ++c) {
        double p = 0.0;
        if (ebur128_true_peak(st.get(), c, &p) == EBUR128_SUCCESS)
            peak = std::max(peak, p);
    }
    return st;
}

} // namespace

AlbumMeasurement measureAlbum(const std::vector<std::filesystem::path>& paths, const std::atomic<bool>& cancel)
{
    AlbumMeasurement result;
    result.tracks.resize(paths.size());
    std::vector<State> states;
    std::vector<ebur128_state*> measured;

    for (std::size_t i = 0; i < paths.size(); ++i) {
        double peak = 0.0;
        State st = measureFile(paths[i], cancel, peak);
        if (cancel.load(std::memory_order_relaxed))
            return result;
        if (!st)
            continue;
        double lufs = 0.0;
        // Silence (below the absolute gate) has no loudness: no gain for it.
        if (ebur128_loudness_global(st.get(), &lufs) == EBUR128_SUCCESS && std::isfinite(lufs)) {
            result.tracks[i] = {true, lufs, peak};
            result.album.peak = std::max(result.album.peak, peak);
            measured.push_back(st.get());
            states.push_back(std::move(st));
        }
    }

    double albumLufs = 0.0;
    if (!measured.empty()
        && ebur128_loudness_global_multiple(measured.data(), measured.size(), &albumLufs) == EBUR128_SUCCESS
        && std::isfinite(albumLufs)) {
        result.album.ok = true;
        result.album.lufs = albumLufs;
    }
    return result;
}

} // namespace sonkore::Loudness
