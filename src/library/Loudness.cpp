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

    std::vector<int32_t> buffer(kBlockFrames * info.channels);
    for (;;) {
        if (cancel.load(std::memory_order_relaxed))
            return nullptr;
        const uint64_t got = decoder->readS32(buffer.data(), kBlockFrames);
        // Left-justified s32: libebur128 scales int input by 2^31, so full
        // scale stays 1.0 whatever the source bit depth.
        if (got > 0 && ebur128_add_frames_int(st.get(), buffer.data(), static_cast<size_t>(got)) != EBUR128_SUCCESS)
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
