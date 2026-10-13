#pragma once

#include <cstdint>
#include <cstring>

// Writes the engine's samples into an ASIO driver's buffers, in the driver's
// own sample type. PCM: left-justified s32, the bits a narrower format cannot
// hold being zeros already (the engine quantised to the device's depth), so
// every conversion is exact. DSD: packed words of 32 DSD bits, oldest bit in
// the most significant position (DsdPacking::NativeU32BE).
// Platform-independent so it is tested everywhere (tests/engine_test.cpp).
namespace sonkore::asio {

// ASIOSampleType values (AsioAbi.hpp) handled here.
enum class SampleType : long {
    Int16MSB = 0, Int24MSB = 1, Int32MSB = 2, Float32MSB = 3, Float64MSB = 4,
    Int32MSB16 = 8, Int32MSB18 = 9, Int32MSB20 = 10, Int32MSB24 = 11,
    Int16LSB = 16, Int24LSB = 17, Int32LSB = 18, Float32LSB = 19, Float64LSB = 20,
    Int32LSB16 = 24, Int32LSB18 = 25, Int32LSB20 = 26, Int32LSB24 = 27,
    DsdInt8LSB1 = 32, DsdInt8MSB1 = 33,
};

inline bool isDsd(long type) { return type == 32 || type == 33; }

inline bool isSupportedPcm(long type)
{
    switch (type) {
    case 0: case 1: case 2: case 3: case 4: case 8: case 9: case 10: case 11:
    case 16: case 17: case 18: case 19: case 20: case 24: case 25: case 26: case 27:
        return true;
    default:
        return false;
    }
}

// Bits of real resolution the type carries (24 for float: the engine never
// produces more than a float's mantissa holds exactly for its quantised grid).
inline int pcmBits(long type)
{
    switch (type) {
    case 0: case 16: case 8: case 24: return 16;
    case 9: case 25: return 18;
    case 10: case 26: return 20;
    case 1: case 17: case 11: case 27: return 24;
    case 3: case 19: return 24;
    default: return 32;
    }
}

inline int bytesPerSample(long type)
{
    switch (type) {
    case 0: case 16: return 2;
    case 1: case 17: return 3;
    case 4: case 20: return 8;
    default: return 4;
    }
}

namespace detail {
inline void putBE(uint8_t* p, uint64_t v, int bytes)
{
    for (int i = bytes - 1; i >= 0; --i, v >>= 8)
        p[i] = static_cast<uint8_t>(v);
}
inline void putLE(uint8_t* p, uint64_t v, int bytes)
{
    for (int i = 0; i < bytes; ++i, v >>= 8)
        p[i] = static_cast<uint8_t>(v);
}
inline uint8_t reverseBits(uint8_t b)
{
    b = static_cast<uint8_t>((b & 0xF0) >> 4 | (b & 0x0F) << 4);
    b = static_cast<uint8_t>((b & 0xCC) >> 2 | (b & 0x33) << 2);
    return static_cast<uint8_t>((b & 0xAA) >> 1 | (b & 0x55) << 1);
}
} // namespace detail

// One channel of `frames` PCM frames from interleaved s32 (`stride` samples
// per frame) into `dst`, in `type`.
inline void writePcm(long type, void* dst, const int32_t* src, std::size_t stride, std::size_t frames)
{
    auto* out = static_cast<uint8_t*>(dst);
    const int bytes = bytesPerSample(type);
    for (std::size_t f = 0; f < frames; ++f, out += bytes) {
        const int32_t s = src[f * stride];
        const auto u = static_cast<uint32_t>(s);
        switch (type) {
        case 0: detail::putBE(out, u >> 16, 2); break;
        case 1: detail::putBE(out, u >> 8, 3); break;
        case 2: detail::putBE(out, u, 4); break;
        case 8: detail::putBE(out, static_cast<uint32_t>(s >> 16), 4); break;
        case 9: detail::putBE(out, static_cast<uint32_t>(s >> 14), 4); break;
        case 10: detail::putBE(out, static_cast<uint32_t>(s >> 12), 4); break;
        case 11: detail::putBE(out, static_cast<uint32_t>(s >> 8), 4); break;
        case 16: detail::putLE(out, u >> 16, 2); break;
        case 17: detail::putLE(out, u >> 8, 3); break;
        case 18: detail::putLE(out, u, 4); break;
        case 24: detail::putLE(out, static_cast<uint32_t>(s >> 16), 4); break;
        case 25: detail::putLE(out, static_cast<uint32_t>(s >> 14), 4); break;
        case 26: detail::putLE(out, static_cast<uint32_t>(s >> 12), 4); break;
        case 27: detail::putLE(out, static_cast<uint32_t>(s >> 8), 4); break;
        case 3: case 19: { // exact: s / 2^31 of a value on a 24-bit grid
            const float v = static_cast<float>(s) * (1.0f / 2147483648.0f);
            uint32_t bits;
            std::memcpy(&bits, &v, 4);
            type == 3 ? detail::putBE(out, bits, 4) : detail::putLE(out, bits, 4);
            break;
        }
        case 4: case 20: {
            const double v = static_cast<double>(s) * (1.0 / 2147483648.0);
            uint64_t bits;
            std::memcpy(&bits, &v, 8);
            type == 4 ? detail::putBE(out, bits, 8) : detail::putLE(out, bits, 8);
            break;
        }
        default: std::memset(out, 0, static_cast<std::size_t>(bytes)); break;
        }
    }
}

// One channel of `words` DSD words (interleaved by `stride`) into `dst`, 4
// bytes per word. The words are DsdPacking::NativeU32BE: their 4 bytes in
// memory order are the stream's, oldest first, oldest bit in each MSB.
inline void writeDsd(long type, void* dst, const int32_t* src, std::size_t stride, std::size_t words)
{
    auto* out = static_cast<uint8_t*>(dst);
    const bool lsbFirst = type == static_cast<long>(SampleType::DsdInt8LSB1);
    for (std::size_t w = 0; w < words; ++w, out += 4) {
        std::memcpy(out, src + w * stride, 4);
        if (lsbFirst)
            for (int i = 0; i < 4; ++i)
                out[i] = detail::reverseBits(out[i]);
    }
}

// DSD silence (the 0x69 idle pattern) for a whole buffer.
inline void dsdSilence(void* dst, std::size_t bytes, long type)
{
    std::memset(dst, type == static_cast<long>(SampleType::DsdInt8LSB1) ? detail::reverseBits(0x69) : 0x69, bytes);
}

} // namespace sonkore::asio
