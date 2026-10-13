#include "DsdToPcm.hpp"

#include <algorithm>
#include <cmath>
#include <cstring>
#include <numbers>

namespace sonkore {

namespace {

constexpr uint8_t kSilenceByte = 0x69; // DSD idle pattern: 0 V average
constexpr std::size_t kBlockWords = 4096; // DSD read per step: 16 KB per channel
constexpr uint64_t kSeekPreroll = 8192;   // intermediate samples run before a seek's target (23 ms)

// DSD bytes per intermediate sample: the intermediate rate is the DSD rate /
// 8k, in 352.8 or 384 kHz (DSD on the 44.1 or the 48 kHz family).
uint32_t bytesPerSample(uint32_t dsdRate)
{
    return std::max<uint32_t>(1, (dsdRate / 8 + 176400) / 352800);
}

double besselI0(double x)
{
    double sum = 1.0, term = 1.0;
    for (int k = 1; k < 200; ++k) {
        term *= (x / (2.0 * k)) * (x / (2.0 * k));
        sum += term;
        if (term < sum * 1e-17)
            break;
    }
    return sum;
}

} // namespace

std::size_t DsdToPcm::firstStageTaps(uint32_t dsdRate)
{
    const double fs = dsdRate;
    const double mid = fs / (8.0 * bytesPerSample(dsdRate)); // 352.8 kHz
    const double pass = outputRate(dsdRate) / 2.0;           // the output's Nyquist frequency
    const double stop = mid - pass;                          // what folds onto it after decimation
    // Kaiser's estimate of the length.
    const auto taps = static_cast<std::size_t>(std::ceil((kStopbandDb - 7.95) / (14.36 * (stop - pass) / fs))) + 1;
    return (taps + 15) / 16 * 16; // whole bytes, and an even number of them
}

uint32_t DsdToPcm::outputRate(uint32_t dsdRate)
{
    const uint32_t k = bytesPerSample(dsdRate);
    return dsdRate / (8 * k) / (k <= 1 ? 4 : 2); // DSD64: 88.2 kHz; DSD128 and up: 176.4 kHz
}

std::vector<double> DsdToPcm::firstStage(uint32_t dsdRate)
{
    const double fs = dsdRate;
    const double mid = fs / (8.0 * bytesPerSample(dsdRate));
    const std::size_t taps = firstStageTaps(dsdRate);
    const double beta = 0.1102 * (kStopbandDb - 8.7);
    const double cutoff = mid / 2.0 / fs; // cycles per bit, in the middle of the transition
    std::vector<double> h(taps);
    const double centre = (static_cast<double>(taps) - 1.0) / 2.0;
    const double norm = besselI0(beta);
    double sum = 0.0;
    for (std::size_t n = 0; n < taps; ++n) {
        const double t = static_cast<double>(n) - centre;
        const double x = 2.0 * std::numbers::pi * cutoff * t;
        const double sinc = 2.0 * cutoff * (x == 0.0 ? 1.0 : std::sin(x) / x);
        const double r = t / centre;
        h[n] = sinc * besselI0(beta * std::sqrt(std::max(0.0, 1.0 - r * r))) / norm;
        sum += h[n];
    }
    for (double& v : h)
        v /= sum; // DC gain 1: all ones is full scale
    return h;
}

DsdToPcm::Band DsdToPcm::secondStage(uint32_t dsdRate)
{
    const uint32_t k = bytesPerSample(dsdRate);
    const double nyquist = outputRate(dsdRate) / 2.0;
    if (k <= 1)
        return {30000.0, nyquist}; // DSD64: the noise rises from ~25 kHz
    if (k == 2)
        return {50000.0, 70000.0}; // DSD128
    return {70000.0, nyquist};     // DSD256 and DSD512
}

std::unique_ptr<DsdToPcm> DsdToPcm::open(const std::filesystem::path& path, std::string* error)
{
    std::unique_ptr<DsdToPcm> d(new DsdToPcm);
    d->m_dsd = DsdDecoder::open(path, DsdPacking::NativeU32BE, error);
    if (!d->m_dsd)
        return nullptr;
    const StreamInfo& in = d->m_dsd->info();
    const uint32_t dsdRate = in.dsdRate;
    d->m_channels = in.channels;
    d->m_bytesPerSample = bytesPerSample(dsdRate);
    // The DSD reader pads its last word with silence: the file's own length.
    DsdDecoder::Format format;
    d->m_bytesPerChannel = DsdDecoder::probe(path, format) ? format.bytesPerChannel : in.totalFrames * 4;
    d->m_intermediateTotal = d->m_bytesPerChannel / d->m_bytesPerSample;

    const std::vector<double> h = firstStage(dsdRate);
    d->m_tableBytes = static_cast<uint32_t>(h.size() / 8);
    d->m_table.resize(static_cast<std::size_t>(d->m_tableBytes) * 256);
    for (uint32_t b = 0; b < d->m_tableBytes; ++b)
        for (unsigned v = 0; v < 256; ++v) {
            double s = 0.0;
            for (int j = 0; j < 8; ++j) // MSB first: the oldest bit meets the first tap
                s += ((v >> (7 - j)) & 1u ? 1.0 : -1.0) * h[b * 8 + j];
            d->m_table[static_cast<std::size_t>(b) * 256 + v] = s;
        }

    const uint32_t mid = dsdRate / (8 * d->m_bytesPerSample);
    const uint32_t outRate = outputRate(dsdRate);
    d->m_ratio = mid / outRate;
    const Band band = secondStage(dsdRate);
    const double nyquist = outRate / 2.0;
    if (!d->m_stage2.prepareLowpass(mid, outRate, d->m_channels, band.passHz / nyquist, std::min(1.0, band.stopHz / nyquist), error))
        return nullptr;

    StreamInfo& info = d->m_info;
    info.sampleRate = outRate;
    info.channels = d->m_channels;
    info.bitsPerSample = 0;  // a conversion: no source depth to keep
    info.lossless = false;   // ... and nothing bit-perfect about it
    info.totalFrames = d->m_intermediateTotal / d->m_ratio;
    info.dsd = StreamInfo::Dsd::None;
    info.dsdRate = dsdRate;  // PCM made from DSD (the signal path says so)
    const bool dst = in.decoder.find("DST") != std::string::npos;
    info.codec = DsdDecoder::rateLabel(dsdRate) + (dst ? " DST" : "");
    info.decoder = dst ? "DST decoder" : "DSD reader"; // the conversion is a stage of its own in the signal path

    d->m_bytes.resize(d->m_channels);
    d->restart(0);
    return d;
}

void DsdToPcm::restart(uint64_t start)
{
    const int64_t half = m_tableBytes / 2;
    const int64_t window = static_cast<int64_t>(start * m_bytesPerSample) - half; // first byte the sample reads
    int64_t read = 0;
    for (auto& b : m_bytes)
        b.clear();
    if (window < 0) {
        m_bufferStart = window;
        for (auto& b : m_bytes)
            b.assign(static_cast<std::size_t>(-window), kSilenceByte);
    } else {
        read = window & ~int64_t(3); // whole 32-bit words
        m_bufferStart = read;
    }
    m_dsd->seek(static_cast<uint64_t>(read / 4));
    m_nextIntermediate = start;
    m_inputEnded = m_drained = false;
    m_stage2.clear();
    m_pending.clear();
    m_pendingPos = 0;
}

bool DsdToPcm::produce()
{
    const std::size_t ch = m_channels;
    if (!m_inputEnded) {
        m_words.resize(kBlockWords * ch);
        const uint64_t got = m_dsd->readS32(m_words.data(), kBlockWords);
        for (std::size_t c = 0; c < ch; ++c) {
            std::vector<uint8_t>& b = m_bytes[c];
            const std::size_t at = b.size();
            b.resize(at + got * 4);
            for (uint64_t f = 0; f < got; ++f)
                std::memcpy(b.data() + at + f * 4, &m_words[f * ch + c], 4); // the word's bytes are in time order
        }
        if (got < kBlockWords) {
            // The end: silence after it, enough for the last windows.
            m_inputEnded = true;
            for (auto& b : m_bytes)
                b.insert(b.end(), m_tableBytes + 8, kSilenceByte);
        }
    }

    // Every intermediate sample whose window is in.
    const int64_t half = m_tableBytes / 2;
    const int64_t end = m_bufferStart + static_cast<int64_t>(m_bytes[0].size());
    uint64_t last = m_nextIntermediate;
    while (last < m_intermediateTotal && static_cast<int64_t>(last * m_bytesPerSample) - half + m_tableBytes <= end)
        ++last;
    const std::size_t count = static_cast<std::size_t>(last - m_nextIntermediate);
    m_mid.resize(count * ch);
    const std::size_t tableBytes = m_tableBytes;
    for (std::size_t c = 0; c < ch; ++c) {
        const uint8_t* bytes = m_bytes[c].data();
        for (std::size_t i = 0; i < count; ++i) {
            const int64_t first = static_cast<int64_t>((m_nextIntermediate + i) * m_bytesPerSample) - half - m_bufferStart;
            const uint8_t* p = bytes + first;
            const double* t = m_table.data();
            // Four running sums: independent additions the CPU overlaps.
            double s0 = 0.0, s1 = 0.0, s2 = 0.0, s3 = 0.0;
            std::size_t b = 0;
            for (; b + 4 <= tableBytes; b += 4, t += 4 * 256) {
                s0 += t[p[b]];
                s1 += t[256 + p[b + 1]];
                s2 += t[512 + p[b + 2]];
                s3 += t[768 + p[b + 3]];
            }
            for (; b < tableBytes; ++b, t += 256)
                s0 += t[p[b]];
            m_mid[i * ch + c] = (s0 + s1) + (s2 + s3);
        }
    }
    m_nextIntermediate = last;
    // Bytes no window reads any more.
    const int64_t keep = static_cast<int64_t>(m_nextIntermediate * m_bytesPerSample) - half;
    if (keep > m_bufferStart) {
        const auto drop = static_cast<std::size_t>(std::min<int64_t>(keep - m_bufferStart, static_cast<int64_t>(m_bytes[0].size())));
        for (auto& b : m_bytes)
            b.erase(b.begin(), b.begin() + static_cast<std::ptrdiff_t>(drop));
        m_bufferStart += static_cast<int64_t>(drop);
    }

    const auto convert = [this](const double* x, std::size_t frames) {
        const std::size_t n = frames * m_channels;
        const std::size_t at = m_pending.size();
        m_pending.resize(at + n);
        for (std::size_t i = 0; i < n; ++i) {
            const double v = std::clamp(x[i] * 2147483648.0, -2147483648.0, 2147483647.0);
            m_pending[at + i] = static_cast<int32_t>(std::lrint(v));
        }
    };
    if (count > 0)
        convert(m_stage2.output(), m_stage2.process(m_mid.data(), count));
    if (m_nextIntermediate >= m_intermediateTotal) {
        for (std::size_t n; (n = m_stage2.drain()) > 0;)
            convert(m_stage2.output(), n);
        m_drained = true;
    }
    return !m_drained || m_pendingPos < m_pending.size();
}

uint64_t DsdToPcm::readS32(int32_t* out, uint64_t frames)
{
    const std::size_t ch = m_channels;
    frames = std::min(frames, m_info.totalFrames - std::min(m_info.totalFrames, m_position));
    uint64_t done = 0;
    while (done < frames) {
        if (m_pendingPos >= m_pending.size()) {
            m_pending.clear();
            m_pendingPos = 0;
            if (m_drained) {
                // The filters gave a frame or two fewer than the length: silence.
                std::fill_n(out + done * ch, (frames - done) * ch, 0);
                done = frames;
                break;
            }
            produce();
            continue;
        }
        std::size_t available = (m_pending.size() - m_pendingPos) / ch;
        if (m_discard > 0) {
            const auto skip = static_cast<std::size_t>(std::min<uint64_t>(m_discard, available));
            m_pendingPos += skip * ch;
            m_discard -= skip;
            continue;
        }
        const auto n = static_cast<std::size_t>(std::min<uint64_t>(available, frames - done));
        std::copy_n(m_pending.data() + m_pendingPos, n * ch, out + done * ch);
        m_pendingPos += n * ch;
        done += n;
    }
    m_position += done;
    return done;
}

// From a little before the target, so the filters hold the music that leads
// to it, then the pre-roll's output is dropped.
bool DsdToPcm::seek(uint64_t frame)
{
    frame = std::min(frame, m_info.totalFrames);
    const uint64_t target = frame * m_ratio;
    const uint64_t start = target > kSeekPreroll ? target - kSeekPreroll : 0; // a whole number of output frames before
    restart(start);
    m_discard = (target - start) / m_ratio;
    m_position = frame;
    return true;
}

} // namespace sonkore
