#pragma once

#include "DsdDecoder.hpp"

#include <array>
#include <cstddef>
#include <cstdint>
#include <vector>

namespace sonkore {

// PCM to DSD: a 7th-order single-bit sigma-delta modulator, run at the DSD
// rate on PCM already resampled to it, packing the bits for the DAC the same
// way DsdDecoder does (native DSD_U32 words or DoP).
//
// Noise transfer function after Schreier's synthesizeNTF: optimised zeros
// (Legendre roots across the 0-22.05 kHz band) and poles set for an
// out-of-band gain of 1.5 (Lee's rule for a stable 1-bit loop), designed for
// each oversampling ratio (DSD64 / 128 / 256). Simulated in-band SNR: about
// 132 dB at DSD64, 135 dB at DSD128 and DSD256, with the input at the SACD
// reference level. Implemented in error-feedback form, double precision:
//   w = u + ((N - D) / D) e,  y = sign(w),  e = y - w
// so the output is u + NTF * e.
//
// A 1-bit loop of this order overloads above about -5.5 dBFS of modulation:
// the input is scaled so PCM full scale lands at 45 % (-6.9 dB, a little under
// the SACD 0 dB reference of 50 %), limited to 50 %, and a loop that diverges
// all the same is reset (counted in resets()).
class SigmaDelta {
public:
    static constexpr int kOrder = 7;
    static constexpr double kInputScale = 0.45;
    static constexpr double kInputLimit = 0.5;

    // dsdMultiple: 64, 128 or 256 (times 44.1 kHz).
    void setup(int dsdMultiple, uint32_t channels, DsdPacking packing);
    void reset();

    // Interleaved PCM frames at the DSD rate in; packed words out (one per
    // channel and output frame: 32 DSD bits native, 16 for DoP). Returns the
    // output frames written to `out` (room for frames / bitsPerWord + 1).
    std::size_t process(const double* in, std::size_t frames, int32_t* out);
    // Completes the last partial word with DSD silence; returns 0 or 1 frame.
    std::size_t flush(int32_t* out);
    std::size_t bitsPerWord() const { return m_bitsPerWord; }
    uint64_t resets() const { return m_resets; }

private:
    int32_t pack(uint32_t bits) const;

    std::array<double, kOrder> m_fb{};  // (N - D) coefficients, z^-1 .. z^-7
    std::array<double, kOrder> m_d{};   // D coefficients, z^-1 .. z^-7
    struct Loop {
        std::array<double, kOrder> e{}; // past quantisation errors, newest first
        std::array<double, kOrder> r{}; // past feedback values, newest first
        uint32_t bits = 0;
    };
    std::vector<Loop> m_loops;
    uint32_t m_channels = 0;
    DsdPacking m_packing = DsdPacking::NativeU32BE;
    std::size_t m_bitsPerWord = 32;
    std::size_t m_filled = 0; // bits in the current word
    uint64_t m_resets = 0;
};

} // namespace sonkore
