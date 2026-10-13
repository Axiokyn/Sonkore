#pragma once

#include <cstdint>
#include <filesystem>
#include <limits>
#include <string>
#include <string_view>
#include <vector>

namespace sonkore {

// A CD image described by a .cue sheet: one (or more) audio files and where
// each track starts in them. A track is addressed as "<sheet>.cue#<number>"
// (no real file has that name), which Decoder::open and the library resolve.
struct CueSheet {
    static constexpr double kNoGain = std::numeric_limits<double>::quiet_NaN();

    struct Track {
        int number = 0;
        std::string title;
        std::string performer;
        std::filesystem::path file;  // the audio file, resolved next to the sheet
        uint64_t start = 0;          // INDEX 01, in CD frames (1/75 s)
        uint64_t end = 0;            // next track's INDEX 01 in the same file; 0 = end of file
        double gain = kNoGain;       // REM REPLAYGAIN_TRACK_GAIN / _PEAK
        double peak = 0.0;
    };

    std::string title;     // album
    std::string performer; // album artist
    std::string genre;
    std::string date;
    double albumGain = kNoGain;
    double albumPeak = 0.0;
    std::vector<Track> tracks;

    // Reads a sheet (UTF-8, or Latin-1 / Windows-1252 as older rippers wrote
    // them). FILE entries are looked up next to it, also under another audio
    // extension when the image was converted after ripping (.wav -> .flac).
    static bool parse(const std::filesystem::path& sheet, CueSheet& out);
    const Track* track(int number) const;

    // "<sheet>#<number>" track addresses.
    static bool isTrackPath(std::string_view path);
    static bool splitTrackPath(std::string_view path, std::filesystem::path& sheet, int& number);
    static std::string trackPath(const std::filesystem::path& sheet, int number);
    static bool isSheet(const std::filesystem::path& path); // ".cue"
};

} // namespace sonkore
