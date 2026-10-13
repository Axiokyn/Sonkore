#pragma once

#include <atomic>
#include <filesystem>
#include <vector>

namespace sonkore::Loudness {

// ReplayGain 2.0 reference level.
inline constexpr double kReferenceLufs = -18.0;

struct Measurement {
    bool ok = false;     // decoded and not silent
    double lufs = 0.0;   // EBU R128 integrated loudness
    double peak = 0.0;   // true peak, linear (1.0 = full scale)
    double gainDb() const { return kReferenceLufs - lufs; }
};

struct AlbumMeasurement {
    std::vector<Measurement> tracks; // same order as the paths
    Measurement album;               // all tracks gated together, highest peak
};

// Decodes every file once (EBU R128 integrated loudness with true peak). The
// album value is measured over all its tracks together, as ReplayGain asks,
// not averaged. Returns early, with ok = false, once `cancel` is set.
AlbumMeasurement measureAlbum(const std::vector<std::filesystem::path>& paths, const std::atomic<bool>& cancel);

} // namespace sonkore::Loudness
