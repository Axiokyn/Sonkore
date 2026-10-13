#include "WaveformController.hpp"

#include "Decoder.hpp"

#include <QFile>
#include <QMetaObject>

#include <algorithm>
#include <cmath>
#include <filesystem>
#include <vector>

namespace {

constexpr int kCacheSize = 8;

std::filesystem::path toFsPath(const QString& path)
{
#if defined(_WIN32)
    return std::filesystem::path(path.toStdWString());
#else
    return std::filesystem::path(QFile::encodeName(path).toStdString());
#endif
}

// Decodes the whole file: the level (RMS, all channels) of every chunk of
// about 0.1 s, and for each bucket the loudest of its chunks. RMS rather than
// peaks: on loud, compressed masters the peaks all touch full scale and the
// wave would be a block.
QList<qreal> measure(const QString& path, const std::atomic<bool>& cancel)
{
    auto decoder = sonkore::Decoder::open(toFsPath(path));
    if (!decoder)
        return {};
    const sonkore::StreamInfo& info = decoder->info();
    if (info.dsd != sonkore::StreamInfo::Dsd::None || info.channels == 0)
        return {};

    constexpr uint64_t kChunk = 4096;
    std::vector<int32_t> buffer(kChunk * info.channels);
    std::vector<double> levels;
    while (!cancel.load(std::memory_order_relaxed)) {
        const uint64_t got = decoder->readS32(buffer.data(), kChunk);
        if (got == 0)
            break;
        const size_t samples = size_t(got) * info.channels;
        double sum = 0.0;
        for (size_t i = 0; i < samples; ++i) {
            const double v = double(buffer[i]);
            sum += v * v;
        }
        levels.push_back(std::sqrt(sum / double(samples)));
        if (got < kChunk)
            break;
    }
    if (cancel.load(std::memory_order_relaxed) || levels.empty())
        return {};

    const int buckets = WaveformController::kBuckets;
    std::vector<double> loudest(buckets, 0.0);
    for (size_t c = 0; c < levels.size(); ++c) {
        const int b = int(std::min<size_t>(buckets - 1, c * buckets / levels.size()));
        loudest[b] = std::max(loudest[b], levels[c]);
    }
    // Fewer chunks than buckets (a very short file): fill the gaps.
    for (int b = 1; b < buckets; ++b)
        if (loudest[b] == 0.0 && levels.size() < size_t(buckets))
            loudest[b] = loudest[b - 1];
    const double top = std::max(1.0, *std::max_element(loudest.begin(), loudest.end()));
    QList<qreal> peaks;
    peaks.reserve(buckets);
    for (double v : loudest)
        peaks.append((v / top) * (v / top)); // energy: the quiet parts stand out
    return peaks;
}

} // namespace

WaveformController::WaveformController(QObject* parent)
    : QObject(parent)
{
}

WaveformController::~WaveformController()
{
    stopWorker();
}

void WaveformController::stopWorker()
{
    if (m_cancel)
        m_cancel->store(true);
    if (m_worker.joinable())
        m_worker.join();
    m_cancel.reset();
}

void WaveformController::setActive(bool active)
{
    if (active == m_active)
        return;
    m_active = active;
    emit activeChanged();
    load();
}

void WaveformController::setPath(const QString& path)
{
    if (path == m_path)
        return;
    m_path = path;
    load();
}

// The current track's waveform: from the cache, or read on the worker; none
// while inactive.
void WaveformController::load()
{
    stopWorker();
    ++m_generation;
    const QString path = m_active ? m_path : QString();

    if (path.isEmpty() || m_cache.contains(path)) {
        m_peaks = m_cache.value(path);
        m_busy = false;
        emit peaksChanged();
        return;
    }
    m_peaks.clear();
    m_busy = true;
    emit peaksChanged();

    auto cancel = std::make_shared<std::atomic<bool>>(false);
    m_cancel = cancel;
    const quint64 generation = m_generation;
    m_worker = std::thread([this, cancel, generation, path] {
        const QList<qreal> peaks = measure(path, *cancel);
        if (cancel->load())
            return;
        QMetaObject::invokeMethod(this, [this, generation, path, peaks] { finished(generation, path, peaks); },
                                  Qt::QueuedConnection);
    });
}

void WaveformController::finished(quint64 generation, const QString& path, const QList<qreal>& peaks)
{
    if (generation != m_generation)
        return;
    if (!peaks.isEmpty()) {
        m_cache.insert(path, peaks);
        m_cacheOrder.removeAll(path);
        m_cacheOrder.append(path);
        while (m_cacheOrder.size() > kCacheSize)
            m_cache.remove(m_cacheOrder.takeFirst());
    }
    m_peaks = peaks;
    m_busy = false;
    emit peaksChanged();
}
