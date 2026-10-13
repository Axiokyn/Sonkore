#include "Convolver.hpp"

#include "Decoder.hpp"
#include "Resampler.hpp"
#include "SampleConvert.hpp"

#include <pocketfft_hdronly.h>

#include <algorithm>

namespace sonkore {

namespace {

constexpr std::size_t kMaxTaps = std::size_t(1) << 20;
constexpr std::size_t kN = 2 * Convolver::kBlock; // FFT size

// Real spectra in pocketfft's packed (FFTPACK) layout:
// [r0, r1, i1, r2, i2, ..., r(N/2)].
void multiplyAdd(const double* x, const double* h, double* acc)
{
    acc[0] += x[0] * h[0];
    for (std::size_t k = 1; k + 1 < kN; k += 2) {
        const double xr = x[k], xi = x[k + 1], hr = h[k], hi = h[k + 1];
        acc[k] += xr * hr - xi * hi;
        acc[k + 1] += xr * hi + xi * hr;
    }
    acc[kN - 1] += x[kN - 1] * h[kN - 1];
}

} // namespace

bool ImpulseResponse::load(const std::filesystem::path& path, ImpulseResponse& out, std::string* error)
{
    auto decoder = Decoder::open(path, error);
    if (!decoder)
        return false;
    const StreamInfo& info = decoder->info();
    if (info.channels != 1 && info.channels != 2 && info.channels != 4) {
        if (error)
            *error = "An impulse response has 1, 2 or 4 (true stereo) channels";
        return false;
    }
    out = {};
    out.path = path;
    out.sampleRate = info.sampleRate;
    out.channels = info.channels;
    out.taps.assign(info.channels, {});
    std::vector<int32_t> block(4096 * info.channels);
    for (;;) {
        const uint64_t got = decoder->readS32(block.data(), 4096);
        for (uint64_t f = 0; f < got; ++f)
            for (uint32_t c = 0; c < info.channels; ++c)
                out.taps[c].push_back(block[f * info.channels + c] * kS32InvFullScale);
        if (got < 4096 || out.taps[0].size() > kMaxTaps)
            break;
    }
    if (out.taps[0].empty() || out.taps[0].size() > kMaxTaps) {
        if (error)
            *error = out.taps[0].empty() ? "The impulse response is empty" : "The impulse response is longer than 2^20 taps";
        return false;
    }
    return true;
}

// Per output channel: the spectra of each filter partition, applied to one or
// two input channels (two for true stereo: LL+RL, LR+RR).
struct Convolver::Impl {
    pocketfft::detail::pocketfft_r<double> fft{kN};
    std::size_t partitions = 0;
    // filters[out][src] = partitions x kN spectra; src index into inputs
    std::vector<std::vector<std::vector<double>>> filters;
    std::vector<std::vector<uint32_t>> sources;        // input channels feeding each output
    std::vector<std::vector<double>> history;          // per input: FDL of input spectra (partitions x kN)
    std::vector<std::vector<double>> window;           // per input: last 2*kBlock samples
    std::size_t head = 0;                              // newest FDL slot
    std::vector<std::vector<double>> inBlock, outBlock; // per channel, kBlock
    std::size_t fill = 0;                              // frames gathered in inBlock / handed out of outBlock
    std::vector<double> acc;
    std::vector<double> pending;                       // replaceTaps(): the next filter's spectra (one channel)
    std::vector<double> accNew;
};

namespace {
// Partition spectra of a filter (zero-padded to whole partitions).
std::vector<double> partitionSpectra(pocketfft::detail::pocketfft_r<double>& fft, const std::vector<double>& t,
                                     std::size_t partitions)
{
    std::vector<double> s(partitions * kN, 0.0);
    for (std::size_t p = 0; p < partitions; ++p) {
        double* part = s.data() + p * kN;
        const std::size_t n = std::min(Convolver::kBlock, t.size() - p * Convolver::kBlock);
        std::copy_n(t.data() + p * Convolver::kBlock, n, part); // zero-padded to kN
        fft.exec(part, 1.0, true);
    }
    return s;
}
} // namespace

std::unique_ptr<Convolver> Convolver::create(const ImpulseResponse& ir, uint32_t sampleRate, uint32_t channels,
                                             std::string* error)
{
    if (ir.channels == 4 && channels != 2) {
        if (error)
            *error = "A true-stereo impulse response needs a stereo track";
        return nullptr;
    }
    if (ir.channels == 2 && channels != 2) {
        if (error)
            *error = "A stereo impulse response needs a stereo track";
        return nullptr;
    }
    std::unique_ptr<Convolver> c(new Convolver);
    c->m_channels = channels;

    // The response at the track's rate. Resampling changes the number of
    // taps per second, so the taps are scaled back to keep the filter's gain.
    std::vector<std::vector<double>> taps = ir.taps;
    if (ir.sampleRate != sampleRate) {
        for (auto& t : taps) {
            Resampler r;
            if (!r.prepare(ir.sampleRate, sampleRate, 1, error))
                return nullptr;
            std::vector<double> out;
            const std::size_t got = r.process(t.data(), t.size());
            out.insert(out.end(), r.output(), r.output() + got);
            for (std::size_t n; (n = r.drain()) > 0;)
                out.insert(out.end(), r.output(), r.output() + n);
            const double scale = static_cast<double>(ir.sampleRate) / sampleRate;
            for (double& v : out)
                v *= scale;
            t = std::move(out);
        }
        c->m_resampledFrom = ir.sampleRate;
    }
    c->m_taps = taps[0].size();

    auto d = std::make_unique<Impl>();
    d->partitions = (c->m_taps + kBlock - 1) / kBlock;
    const auto spectra = [&](const std::vector<double>& t) { return partitionSpectra(d->fft, t, d->partitions); };
    d->filters.resize(channels);
    d->sources.resize(channels);
    for (uint32_t o = 0; o < channels; ++o) {
        if (ir.channels == 4) { // LL LR RL RR
            d->filters[o] = {spectra(taps[o == 0 ? 0 : 1]), spectra(taps[o == 0 ? 2 : 3])};
            d->sources[o] = {0, 1};
        } else {
            d->filters[o] = {spectra(taps[ir.channels == 2 ? o : 0])};
            d->sources[o] = {o};
        }
    }
    d->history.assign(channels, std::vector<double>(d->partitions * kN, 0.0));
    d->window.assign(channels, std::vector<double>(kN, 0.0));
    d->inBlock.assign(channels, std::vector<double>(kBlock, 0.0));
    d->outBlock.assign(channels, std::vector<double>(kBlock, 0.0));
    d->acc.assign(kN, 0.0);
    c->d = std::move(d);
    return c;
}

Convolver::~Convolver() = default;

bool Convolver::replaceTaps(const std::vector<double>& taps)
{
    if (taps.empty() || (taps.size() + kBlock - 1) / kBlock != d->partitions)
        return false;
    for (const auto& sources : d->sources)
        if (sources.size() != 1)
            return false; // true stereo: not a single-channel response
    d->pending = partitionSpectra(d->fft, taps, d->partitions);
    d->accNew.assign(kN, 0.0);
    m_taps = taps.size();
    return true;
}

void Convolver::reset()
{
    for (auto& h : d->history)
        std::fill(h.begin(), h.end(), 0.0);
    for (auto& w : d->window)
        std::fill(w.begin(), w.end(), 0.0);
    for (auto& b : d->outBlock)
        std::fill(b.begin(), b.end(), 0.0);
    d->head = 0;
    d->fill = 0;
}

// Overlap-save: the FFT of the last two input blocks enters the frequency-
// domain delay line; the output block is the second half of the inverse FFT
// of sum(X[n-p] * H[p]).
void Convolver::step()
{
    const std::size_t parts = d->partitions;
    d->head = (d->head + parts - 1) % parts;
    for (uint32_t c = 0; c < m_channels; ++c) {
        auto& w = d->window[c];
        std::copy(w.begin() + kBlock, w.end(), w.begin());
        std::copy(d->inBlock[c].begin(), d->inBlock[c].end(), w.begin() + kBlock);
        double* slot = d->history[c].data() + d->head * kN;
        std::copy(w.begin(), w.end(), slot);
        d->fft.exec(slot, 1.0, true);
    }
    const bool crossfade = !d->pending.empty();
    for (uint32_t o = 0; o < m_channels; ++o) {
        std::fill(d->acc.begin(), d->acc.end(), 0.0);
        for (std::size_t s = 0; s < d->sources[o].size(); ++s) {
            const std::vector<double>& h = d->filters[o][s];
            const std::vector<double>& x = d->history[d->sources[o][s]];
            for (std::size_t p = 0; p < parts; ++p)
                multiplyAdd(x.data() + ((d->head + p) % parts) * kN, h.data() + p * kN, d->acc.data());
        }
        d->fft.exec(d->acc.data(), 1.0 / kN, false);
        if (crossfade) {
            // The same input through the new filter, faded in across the block.
            std::fill(d->accNew.begin(), d->accNew.end(), 0.0);
            const std::vector<double>& x = d->history[d->sources[o][0]];
            for (std::size_t p = 0; p < parts; ++p)
                multiplyAdd(x.data() + ((d->head + p) % parts) * kN, d->pending.data() + p * kN, d->accNew.data());
            d->fft.exec(d->accNew.data(), 1.0 / kN, false);
            for (std::size_t i = 0; i < kBlock; ++i) {
                const double w = (static_cast<double>(i) + 0.5) / kBlock;
                d->acc[kBlock + i] += (d->accNew[kBlock + i] - d->acc[kBlock + i]) * w;
            }
        }
        std::copy(d->acc.begin() + kBlock, d->acc.end(), d->outBlock[o].begin());
    }
    if (crossfade) {
        for (auto& f : d->filters)
            f[0] = d->pending;
        d->pending = {};
        d->accNew = {};
    }
}

void Convolver::process(double* samples, std::size_t frames)
{
    const std::size_t ch = m_channels;
    for (std::size_t f = 0; f < frames; ++f) {
        double* s = samples + f * ch;
        for (std::size_t c = 0; c < ch; ++c) {
            d->inBlock[c][d->fill] = s[c];
            s[c] = d->outBlock[c][d->fill]; // the block computed one step ago
        }
        if (++d->fill == kBlock) {
            step();
            d->fill = 0;
        }
    }
}

} // namespace sonkore
