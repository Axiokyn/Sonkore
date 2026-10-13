#pragma once

#include "Decoder.hpp"

#include <filesystem>
#include <memory>
#include <string>
#include <vector>

// Formats beyond FLAC/WAV/MP3, through the system's FFmpeg (libavformat /
// libavcodec, LGPL, linked dynamically). Integer codecs (ALAC, AIFF/PCM, APE,
// WavPack, TTA...) are handed over as left-justified s32 without any
// conversion, so they keep the bit-perfect path.
namespace sonkore::ffmpeg {

const std::vector<std::string>& extensions(); // lower case, no dot
std::string version();                        // of the libavcodec in use
std::unique_ptr<Decoder> open(const std::filesystem::path& path, std::string* error);
bool probe(const std::filesystem::path& path, ProbeResult& out, bool wantCover);

} // namespace sonkore::ffmpeg
