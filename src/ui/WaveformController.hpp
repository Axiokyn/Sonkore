#pragma once

#include "QmlSingleton.hpp"

#include <QHash>
#include <QList>
#include <QObject>
#include <QString>
#include <QStringList>
#include <QtQml/qqmlregistration.h>

#include <atomic>
#include <memory>
#include <thread>

// The playing track's waveform for the "waveform" seek bar (LSlider): the file is decoded once on a worker thread into `peaks`, the
// level of each of kBuckets slices (0..1, scaled to the track's own loudest,
// so a quiet track still fills the bar). Empty while it is being
// read, and for DSD sent as is. The last few tracks are kept.
class WaveformController : public QObject {
    Q_OBJECT
    QML_NAMED_ELEMENT(Waveform)
    QML_SINGLETON
    SONKORE_QML_SINGLETON_INSTANCE(WaveformController)

    Q_PROPERTY(QList<qreal> peaks READ peaks NOTIFY peaksChanged)
    Q_PROPERTY(bool busy READ busy NOTIFY peaksChanged)
    // Off while no bar shows the waveform (Main binds it to the style):
    // tracks are then not decoded at all.
    Q_PROPERTY(bool active READ active WRITE setActive NOTIFY activeChanged)

public:
    static constexpr int kBuckets = 600;

    explicit WaveformController(QObject* parent);
    ~WaveformController() override;

    QList<qreal> peaks() const { return m_peaks; }
    bool busy() const { return m_busy; }
    bool active() const { return m_active; }
    void setActive(bool active);

    // The track to show (DspController's current path); "" clears it.
    void setPath(const QString& path);

signals:
    void peaksChanged();
    void activeChanged();

private:
    void stopWorker();
    void load();
    void finished(quint64 generation, const QString& path, const QList<qreal>& peaks);

    QString m_path;
    QList<qreal> m_peaks;
    bool m_busy = false;
    bool m_active = false;
    QHash<QString, QList<qreal>> m_cache;
    QStringList m_cacheOrder; // oldest first
    std::thread m_worker;
    std::shared_ptr<std::atomic<bool>> m_cancel;
    quint64 m_generation = 0;
};
