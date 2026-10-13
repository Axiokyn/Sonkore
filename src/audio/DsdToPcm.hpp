#pragma once

#include "Decoder.hpp"
#include "DsdDecoder.hpp"
#include "Resampler.hpp"

#include <memory>
#include <vector>

namespace sonkore {

// DSD (DSF, DSDIFF and DST) to PCM, in double precision, for outputs that
// take no DSD:
//
// 1. the 1-bit stream to 352.8 kHz: a linear-phase FIR (Kaiser window,
//    160 dB stop band) flat up to the output's Nyquist frequency and closed
//    from where content would fold onto it, evaluated a byte of bits at a time
//    from tables (the filter's sum for each of the 256 patterns of 8 bits):
//    16 to 156 additions per sample from DSD64 to DSD512;
// 2. 352.8 kHz to the output rate with SoX VHQ (linear phase, 175 dB), its
//    low-pass set to the DSD rate: DSD's noise shaping leaves the noise above
//    20-30 kHz (rising to near full scale at a few hundred kHz), and a
//    converter that keeps it sends that noise to the DAC and the amplifier.
//    DSD64 to 88.2 kHz, flat to 30 kHz; DSD128 to 176.4 kHz, flat to 50 and
//    closed from 70 kHz; DSD256 and DSD512 to 176.4 kHz, flat to 70 kHz.
//
// The level is kept: a stream of all ones is PCM full scale (the SACD 0 dB
// reference, 50 % modulation, is -6 dBFS), so nothing clips. The output is
// time-aligned with the DSD (both stages are linear phase and their delay is
// taken out), its length is the file's, and a seek lands on the same samples
// as playing from the start.
class DsdToPcm : public Decoder {
public:
    static constexpr double kStopbandDb = 160.0;

    // Null (with `error`) when the file is not DSD.
    static std::unique_ptr<DsdToPcm> open(const std::filesystem::path& path, std::string* error);

    // The first stage's taps for a DSD rate (DC gain 1, a multiple of 16).
    static std::vector<double> firstStage(uint32_t dsdRate);
    static std::size_t firstStageTaps(uint32_t dsdRate);
    // The rate it converts a DSD rate to: 88.2 kHz for DSD64, 176.4 kHz from
    // DSD128 (96 / 192 kHz on the 48 kHz family).
    static uint32_t outputRate(uint32_t dsdRate);
    // The second stage's band edges in Hz (flat to `passHz`, closed from `stopHz`).
    struct Band {
        double passHz, stopHz;
    };
    static Band secondStage(uint32_t dsdRate);

    uint64_t readS32(int32_t* out, uint64_t frames) override;
    bool seek(uint64_t frame) override;

private:
    DsdToPcm() = default;
    // Positions the reader so the next sample out is intermediate sample `start`.
    void restart(uint64_t start);
    // Converts the next block; false at the end of the stream.
    bool produce();

    std::unique_ptr<DsdDecoder> m_dsd;
    uint32_t m_channels = 0;
    uint32_t m_bytesPerSample = 1;  // DSD bytes per intermediate sample (1 at DSD64, 8 at DSD512)
    uint32_t m_ratio = 2;           // intermediate samples per output frame
    uint32_t m_tableBytes = 0;      // filter length in bytes (taps / 8)
    uint64_t m_bytesPerChannel = 0;
    uint64_t m_intermediateTotal = 0;
    std::vector<double> m_table;    // [m_tableBytes][256]

    // DSD bytes of each channel, absolute byte m_bufferStart first.
    std::vector<std::vector<uint8_t>> m_bytes;
    int64_t m_bufferStart = 0;
    uint64_t m_nextIntermediate = 0; // the next intermediate sample to compute
    bool m_inputEnded = false, m_drained = false;
    std::vector<int32_t> m_words;
    std::vector<double> m_mid;

    Resampler m_stage2;
    std::vector<int32_t> m_pending; // converted output not handed out yet
    std::size_t m_pendingPos = 0;
    uint64_t m_discard = 0;         // output frames to drop (the pre-roll of a seek)
    uint64_t m_position = 0;        // output frames handed out
};

} // namespace sonkore
