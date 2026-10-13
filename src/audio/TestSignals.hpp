#pragma once

#include <cstdint>
#include <filesystem>
#include <string>
#include <vector>

namespace sonkore::testsignals {

// Signals made on the spot to check an output, as files the engine plays
// like any other (so they go through the same path as the music):
//
// * tone: a sine on every channel, at a chosen PCM rate, 24 bits. On a DAC
//   with a display, the rate shown is the rate it receives.
// * channels: the same tone on each channel in turn, in the file's channel
//   order (front left, front right, centre, LFE, back left, back right, side
//   left, side right), to check which speaker each one reaches.
// * dsd: the tone as DSD (a DSF file through the engine's own modulator), for
//   a DAC that shows DSD64 / 128 / 256.
// * dts: a DTS stream inside a CD-quality WAV (FFmpeg's encoder): a receiver
//   that decodes DTS plays the tone only when every bit arrives untouched,
//   anything that scales or resamples the samples turns it into noise. The
//   stream is written as the WAV's 16-bit samples, so Sonkore treats it as
//   PCM and the test is of the whole chain after the file.
//
// Each returns false with `error` when the file could not be written (the
// DTS one also when FFmpeg or its encoder is missing).

bool writeTone(const std::filesystem::path& path, uint32_t rate, uint32_t channels, double seconds, double freqHz,
               double dbfs, std::string* error);
bool writeChannelSequence(const std::filesystem::path& path, uint32_t rate, uint32_t channels, double freqHz, double dbfs,
                          std::string* error);
bool writeDsdTone(const std::filesystem::path& path, int dsdMultiple, uint32_t channels, double seconds, double freqHz,
                  double dbfs, std::string* error);
bool writeDtsTest(const std::filesystem::path& path, std::string* error);
bool dtsAvailable(); // FFmpeg with the DTS encoder is installed

// The name of channel `index` in an `channels`-channel file ("Front left"...),
// in English; the UI translates.
const char* channelName(uint32_t channels, uint32_t index);

// A 24-bit WAV (WAVE_FORMAT_EXTENSIBLE beyond stereo) from interleaved
// samples in [-1, 1]; `bits` 16 or 24.
bool writeWav(const std::filesystem::path& path, uint32_t rate, uint32_t channels, int bits, const std::vector<double>& samples,
              std::string* error);
// A WAV whose 16-bit stereo payload is `bytes` as they are (a DTS CD).
bool writeWavBytes(const std::filesystem::path& path, uint32_t rate, const std::vector<uint8_t>& bytes, std::string* error);

} // namespace sonkore::testsignals
