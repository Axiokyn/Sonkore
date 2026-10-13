#pragma once

#include "DsdDecoder.hpp"

#include <array>
#include <condition_variable>
#include <cstddef>
#include <cstdint>
#include <mutex>
#include <thread>
#include <vector>

namespace sonkore {

// PCM to DSD: an 8th-order single-bit sigma-delta modulator, run at the DSD
// rate on PCM already resampled to it, packing the bits for the DAC the same
// way DsdDecoder does (native DSD_U32 words or DoP).
//
// Noise transfer function after Schreier's synthesizeNTF: optimised zeros
// (Legendre roots across the 0-22.05 kHz band) and poles set for an
// out-of-band gain of 1.5 (Lee's rule for a stable 1-bit loop), designed for
// each oversampling ratio (DSD64 / 128 / 256 / 512). Simulated in-band SNR
// (20 Hz - 20 kHz, 997 Hz at -6.9 dBFS): about 113 dB at DSD64, where the
// 7th-order loop of earlier versions gave 108 dB, and well beyond 24 bits
// from DSD128 up. Implemented in error-feedback form, double precision:
//   w = u + ((N - D) / D) e,  y = sign(w),  e = y - w
// so the output is u + NTF * e.
//
// A 1-bit loop of this order overloads above about -5.5 dBFS of modulation:
// the input is scaled so PCM full scale lands at 45 % (-6.9 dB, a little under
// the SACD 0 dB reference of 50 %), limited to 50 %, and a loop that diverges
// all the same is reset (counted in resets()).
//
// Each channel's loop is independent, so with more than one channel the
// channels after the first run on their own threads (started by setup(),
// idle between blocks): DSD512 stereo at 22.6 MHz would otherwise need more
// than one core's worth of time.
class SigmaDelta {
public:
    static constexpr int kOrder = 8;
    static constexpr double kInputScale = 0.45;
    static constexpr double kInputLimit = 0.5;

    SigmaDelta() = default;
    ~SigmaDelta();
    SigmaDelta(const SigmaDelta&) = delete;
    SigmaDelta& operator=(const SigmaDelta&) = delete;

    // dsdMultiple: 64, 128, 256 or 512 (times 44.1 kHz).
    void setup(int dsdMultiple, uint32_t channels, DsdPacking packing);
    void reset();

    // Interleaved PCM frames at the DSD rate in; packed words out (one per
    // channel and output frame: 32 DSD bits native, 16 for DoP). Returns the
    // output frames written to `out` (room for frames / bitsPerWord + 1).
    std::size_t process(const double* in, std::size_t frames, int32_t* out);
    // Completes the last partial word with DSD silence; returns 0 or 1 frame.
    std::size_t flush(int32_t* out);
    std::size_t bitsPerWord() const { return m_bitsPerWord; }
    uint64_t resets() const;

private:
    struct Loop {
        std::array<double, kOrder> e{}; // past quantisation errors, newest first
        std::array<double, kOrder> r{}; // past feedback values, newest first
        uint32_t bits = 0;              // the word being filled
        uint64_t resets = 0;
        std::vector<int32_t> words;     // this channel's words of the current block
    };
    int32_t pack(uint32_t bits) const;
    // Runs channel `c` over `frames` interleaved frames: its words of the
    // block into m_loops[c].words.
    void modulate(uint32_t c, const double* in, std::size_t frames);
    void stopWorkers();

    std::array<double, kOrder> m_fb{};  // (N - D) coefficients, z^-1 .. z^-8
    std::array<double, kOrder> m_d{};   // D coefficients, z^-1 .. z^-8
    std::vector<Loop> m_loops;
    uint32_t m_channels = 0;
    DsdPacking m_packing = DsdPacking::NativeU32BE;
    std::size_t m_bitsPerWord = 32;
    std::size_t m_filled = 0; // bits in the current word (the same in every channel)

    // One worker per channel after the first; a block is a job generation.
    std::vector<std::thread> m_workers;
    std::mutex m_mutex;
    std::condition_variable m_wake, m_done;
    uint64_t m_generation = 0;
    unsigned m_pending = 0;
    bool m_quit = false;
    const double* m_jobIn = nullptr;
    std::size_t m_jobFrames = 0;
};

} // namespace sonkore
