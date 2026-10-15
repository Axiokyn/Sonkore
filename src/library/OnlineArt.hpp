#pragma once

#include "QmlSingleton.hpp"

#include <QHash>
#include <QMutex>
#include <QObject>
#include <QQmlEngine>
#include <QSet>
#include <QStringList>
#include <QTimer>
#include <QUrl>

#include <memory>
#include <vector>

class QNetworkAccessManager;
class QNetworkReply;

// Album art for albums that have none on disk, from the Cover Art Archive:
// the album is looked up on MusicBrainz (artist and album names are sent,
// nothing else) and the front cover kept in the cache folder, so it is
// fetched once. Asked lazily, by the cover provider, for what is shown.
//
// One album at a time, one MusicBrainz request a second (its rate limit),
// a "none" marker for albums without art that is tried again a month later.
class OnlineArt : public QObject {
    Q_OBJECT
    QML_ELEMENT
    QML_SINGLETON
    Q_PROPERTY(bool enabled READ enabled WRITE setEnabled NOTIFY enabledChanged)
    Q_PROPERTY(int pending READ pending NOTIFY progressChanged)
    Q_PROPERTY(int fetched READ fetched NOTIFY progressChanged)
    Q_PROPERTY(QString lastError READ lastError NOTIFY progressChanged)
    SONKORE_QML_SINGLETON_INSTANCE(OnlineArt)

public:
    explicit OnlineArt(QObject* parent);
    ~OnlineArt() override;

    bool enabled() const { return m_enabled; }
    void setEnabled(bool on);
    int pending() const { return m_queue.size() + m_ready.size() + static_cast<int>(m_fetching.size()) + (m_searching ? 1 : 0); }
    int fetched() const { return m_fetched; }
    QString lastError() const { return m_lastError; }

    // Forgets every downloaded cover and "none" marker.
    Q_INVOKABLE void clearCache();
    // Size of the cache folder on disk, in bytes.
    Q_INVOKABLE double cacheBytes() const;

    // From any thread (the cover provider's): the downloaded cover for this
    // track's album, or "" when there is none yet. The first ask for an
    // album queues its download when enabled.
    static QString cachedCover(const QString& trackPath);

signals:
    void enabledChanged();
    void progressChanged();
    // A cover arrived for these tracks: what shows them reloads.
    void coverArrived(const QStringList& paths);

private:
    struct Job {
        QString key;    // albumKey() of the album
        QString artist; // what is looked up
        QString album;
        QStringList releases; // MusicBrainz release ids still to try
        QString releaseGroup;
        QNetworkReply* reply = nullptr;
    };

    void request(const QString& path); // main thread
    void startSearch();
    void lookup();                     // MusicBrainz search for m_searching
    void startFetches();
    void fetchFront(Job* job, const QUrl& url); // Cover Art Archive
    void finishJob(const Job& job, const QByteArray& image);
    void indexAlbum(const QString& key, const QString& file);
    void dropAll();
    QNetworkAccessManager& network();
    QString cacheDir() const;
    QString fileFor(const QString& key) const;
    QString noneFor(const QString& key) const;

    bool m_enabled = true;
    int m_fetched = 0;
    QString m_lastError;
    std::unique_ptr<QNetworkAccessManager> m_network;
    QTimer m_pace; // spaces MusicBrainz requests
    QList<Job> m_queue;                    // to be looked up
    std::unique_ptr<Job> m_searching;      // at MusicBrainz now
    QList<Job> m_ready;                    // known to MusicBrainz, waiting for a download slot
    std::vector<std::unique_ptr<Job>> m_fetching; // at the archive now
    QSet<QString> m_queuedKeys;

    // Shared with the provider's thread.
    static inline QMutex s_mutex;
    static inline QHash<QString, QString> s_byPath; // track path -> cover file ("" = asked, nothing yet)
};
