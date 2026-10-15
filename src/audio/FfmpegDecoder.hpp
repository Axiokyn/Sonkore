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

// DTS (Coherent Acoustics, 1411.2 kbit/s, the CD rate) from interleaved s32
// frames, for the bit-perfect test signal; false when FFmpeg or its encoder
// is missing.
// Decoders Sonkore needs that this FFmpeg does not have (empty: all there);
// one entry when FFmpeg itself cannot be loaded.
std::vector<std::string> missingDecoders();
bool dtsEncoderAvailable();
bool encodeDts(const int32_t* interleaved, std::size_t frames, uint32_t channels, uint32_t rate, std::vector<uint8_t>& out,
               std::string* error);

} // namespace sonkore::ffmpeg
