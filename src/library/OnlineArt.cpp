#include "OnlineArt.hpp"

#include "AppPaths.hpp"
#include "CoverImageProvider.hpp"
#include "LibraryModel.hpp"
#include "NetRetry.hpp"

#include <QCoreApplication>
#include <QCryptographicHash>
#include <QDateTime>
#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QNetworkAccessManager>
#include <QNetworkReply>
#include <QNetworkRequest>
#include <QSettings>
#include <QUrlQuery>

#include <algorithm>

namespace {

constexpr int kPaceMs = 1100;                 // MusicBrainz asks for at most one request a second
constexpr qint64 kRetryNoneDays = 30;         // an album without art is looked up again after this
constexpr int kMinScore = 70;                 // MusicBrainz search score (0..100) worth trusting
constexpr int kParallelFetches = 3;           // downloads from the archive at a time

QString albumKeyOf(const QVariantMap& track, QString* artist, QString* album)
{
    const QString albumArtist = track.value("albumArtist").toString().trimmed();
    const QString trackArtist = track.value("artist").toString().trimmed();
    *artist = albumArtist.isEmpty() ? trackArtist : albumArtist;
    *album = track.value("album").toString().trimmed();
    if (artist->isEmpty() || album->isEmpty())
        return {};
    return artist->toLower() + QChar(0x1f) + album->toLower();
}

QString hashOf(const QString& key)
{
    return QString::fromLatin1(QCryptographicHash::hash(key.toUtf8(), QCryptographicHash::Sha1).toHex().left(24));
}

QNetworkRequest requestFor(const QUrl& url)
{
    QNetworkRequest r(url);
    // MusicBrainz and the archive want to know who asks and how to reach them.
    r.setHeader(QNetworkRequest::UserAgentHeader,
                QStringLiteral("Sonkore/%1 ( https://github.com/Axiokyn/Sonkore )").arg(QCoreApplication::applicationVersion()));
    r.setRawHeader("Accept", "application/json, image/*");
    r.setAttribute(QNetworkRequest::RedirectPolicyAttribute, QNetworkRequest::NoLessSafeRedirectPolicy);
    r.setTransferTimeout(20000);
    return r;
}

// Lucene query text: quotes and backslashes would change its meaning.
QString quoted(QString value)
{
    value.remove(QLatin1Char('"')).remove(QLatin1Char('\\'));
    return QLatin1Char('"') + value + QLatin1Char('"');
}

} // namespace

OnlineArt::OnlineArt(QObject* parent)
    : QObject(parent)
{
    m_enabled = QSettings().value("library/onlineCovers", true).toBool();
    m_pace.setSingleShot(true);
    m_pace.setInterval(kPaceMs);
    connect(&m_pace, &QTimer::timeout, this, [this] {
        if (m_searching && !m_searching->reply)
            lookup();
    });
    // The library's albums against the cache, so a cover already fetched
    // shows at once (one stat per album).
    const auto reindex = [this] {
        QHash<QString, QString> files;
        QHash<QString, QString> byPath;
        if (LibraryModel* lib = LibraryModel::instance())
            for (const sonkore::TrackMeta& t : lib->tracks()) {
                const QString key = t.albumKey();
                auto it = files.find(key);
                if (it == files.end()) {
                    const QString file = fileFor(key);
                    it = files.insert(key, QFileInfo::exists(file) ? file : QString());
                }
                if (!it->isEmpty())
                    byPath.insert(t.path, *it);
            }
        QMutexLocker lock(&s_mutex);
        s_byPath = std::move(byPath);
    };
    reindex();
    if (LibraryModel* lib = LibraryModel::instance())
        connect(lib, &LibraryModel::countChanged, this, reindex);
}

OnlineArt::~OnlineArt()
{
    dropAll();
}

void OnlineArt::dropAll()
{
    m_queue.clear();
    m_ready.clear();
    m_queuedKeys.clear();
    // Taken out first: an abort finishes the reply at once, and its handler
    // then finds no job to act on.
    const auto searching = std::move(m_searching);
    const auto fetching = std::move(m_fetching);
    if (searching && searching->reply)
        searching->reply->abort();
    for (const auto& job : fetching)
        if (job->reply)
            job->reply->abort();
}

void OnlineArt::setEnabled(bool on)
{
    if (on == m_enabled)
        return;
    m_enabled = on;
    QSettings().setValue("library/onlineCovers", on);
    if (!on)
        dropAll();
    // Tracks asked for without an answer yet are asked for again when shown.
    QMutexLocker lock(&s_mutex);
    for (auto it = s_byPath.begin(); it != s_byPath.end();) {
        if (it->isEmpty())
            it = s_byPath.erase(it);
        else
            ++it;
    }
    lock.unlock();
    emit enabledChanged();
    emit progressChanged();
}

QString OnlineArt::cacheDir() const
{
    return sonkore::paths::cacheDir() + QStringLiteral("/covers");
}

QString OnlineArt::fileFor(const QString& key) const
{
    return cacheDir() + QLatin1Char('/') + hashOf(key) + QStringLiteral(".img");
}

QString OnlineArt::noneFor(const QString& key) const
{
    return cacheDir() + QLatin1Char('/') + hashOf(key) + QStringLiteral(".none");
}

double OnlineArt::cacheBytes() const
{
    double bytes = 0;
    for (const QFileInfo& f : QDir(cacheDir()).entryInfoList(QDir::Files))
        bytes += static_cast<double>(f.size());
    return bytes;
}

void OnlineArt::clearCache()
{
    QDir dir(cacheDir());
    for (const QString& f : dir.entryList(QDir::Files))
        dir.remove(f);
    dropAll();
    {
        QMutexLocker lock(&s_mutex);
        s_byPath.clear();
    }
    m_fetched = 0;
    m_lastError.clear();
    emit progressChanged();
}

QString OnlineArt::cachedCover(const QString& trackPath)
{
    {
        QMutexLocker lock(&s_mutex);
        const auto it = s_byPath.constFind(trackPath);
        if (it != s_byPath.cend())
            return *it;
        s_byPath.insert(trackPath, QString()); // asked once
    }
    if (OnlineArt* self = instance())
        QMetaObject::invokeMethod(self, [self, trackPath] { self->request(trackPath); }, Qt::QueuedConnection);
    return {};
}

void OnlineArt::request(const QString& path)
{
    if (!m_enabled)
        return;
    LibraryModel* lib = LibraryModel::instance();
    if (!lib)
        return;
    QString artist, album;
    const QString key = albumKeyOf(lib->track(path), &artist, &album);
    if (key.isEmpty())
        return; // without an artist and an album there is nothing safe to look up
    const QString file = fileFor(key);
    if (QFileInfo::exists(file)) { // fetched before (the library changed since the index was made)
        indexAlbum(key, file);
        return;
    }
    if (const QFileInfo none(noneFor(key)); none.exists() && none.lastModified().daysTo(QDateTime::currentDateTime()) < kRetryNoneDays)
        return;
    if (m_queuedKeys.contains(key))
        return;
    m_queuedKeys.insert(key);
    m_queue.append({key, artist, album, {}, {}, nullptr});
    emit progressChanged();
    startSearch();
}

void OnlineArt::startSearch()
{
    if (m_searching || m_queue.isEmpty() || !m_enabled)
        return;
    m_searching = std::make_unique<Job>(m_queue.takeFirst());
    if (!m_pace.isActive())
        lookup();
    // Otherwise the pace timer calls lookup() when the second is up.
}

QNetworkAccessManager& OnlineArt::network()
{
    if (!m_network) {
        m_network = std::make_unique<QNetworkAccessManager>();
        m_network->setAutoDeleteReplies(false);
    }
    return *m_network;
}

void OnlineArt::lookup()
{
    m_pace.start(kPaceMs);
    QUrl url(QStringLiteral("https://musicbrainz.org/ws/2/release/"));
    QUrlQuery q;
    q.addQueryItem("query", QStringLiteral("release:%1 AND artist:%2").arg(quoted(m_searching->album), quoted(m_searching->artist)));
    q.addQueryItem("fmt", "json");
    q.addQueryItem("limit", "8");
    url.setQuery(q);
    QNetworkReply* reply = network().get(requestFor(url));
    m_searching->reply = reply;
    connect(reply, &QNetworkReply::finished, this, [this, reply] {
        reply->deleteLater();
        if (!m_searching || m_searching->reply != reply)
            return; // dropped meanwhile
        if (const int wait = sonkore::retryDelayMs(reply, m_searching->attempts); wait >= 0) {
            m_searching->reply = nullptr; // the pace timer asks again
            ++m_searching->attempts;
            m_pace.start(wait);
            return;
        }
        Job job = *m_searching;
        m_searching.reset();
        job.reply = nullptr;
        if (reply->error() != QNetworkReply::NoError) {
            // Nothing written: the album is asked for again another time.
            m_queuedKeys.remove(job.key);
            m_lastError = tr("MusicBrainz did not answer: %1").arg(reason(reply));
            emit progressChanged();
            startSearch();
            return;
        }
        const QJsonArray releases = QJsonDocument::fromJson(reply->readAll()).object().value("releases").toArray();
        // Official releases first, then the rest, among the credible matches.
        QStringList official, other;
        for (const QJsonValue& v : releases) {
            const QJsonObject r = v.toObject();
            if (r.value("score").toInt() < kMinScore)
                continue;
            if (job.releaseGroup.isEmpty())
                job.releaseGroup = r.value("release-group").toObject().value("id").toString();
            (r.value("status").toString() == QLatin1String("Official") ? official : other) << r.value("id").toString();
        }
        job.releases = (official + other).mid(0, 4);
        if (job.releaseGroup.isEmpty() && job.releases.isEmpty())
            finishJob(job, {}); // not known to MusicBrainz
        else
            m_ready.append(job);
        startSearch();
        startFetches();
    });
}

void OnlineArt::startFetches()
{
    while (m_fetching.size() < static_cast<std::size_t>(kParallelFetches) && !m_ready.isEmpty() && m_enabled) {
        m_fetching.push_back(std::make_unique<Job>(m_ready.takeFirst()));
        Job* job = m_fetching.back().get();
        if (!job->releaseGroup.isEmpty()) {
            const QString group = job->releaseGroup;
            job->releaseGroup.clear();
            fetchFront(job, QUrl(QStringLiteral("https://coverartarchive.org/release-group/%1/front-500").arg(group)));
        } else {
            fetchFront(job, QUrl(QStringLiteral("https://coverartarchive.org/release/%1/front-500").arg(job->releases.takeFirst())));
        }
    }
}

void OnlineArt::fetchFront(Job* job, const QUrl& url)
{
    QNetworkReply* reply = network().get(requestFor(url));
    job->reply = reply;
    connect(reply, &QNetworkReply::finished, this, [this, job, reply] {
        reply->deleteLater();
        const auto it = std::find_if(m_fetching.begin(), m_fetching.end(), [&](const auto& j) { return j.get() == job; });
        if (it == m_fetching.end() || job->reply != reply)
            return; // dropped meanwhile
        job->reply = nullptr;
        if (const int wait = sonkore::retryDelayMs(reply, job->attempts); wait >= 0) {
            ++job->attempts;
            QTimer::singleShot(wait, this, [this, job, url = reply->request().url()] {
                if (std::any_of(m_fetching.begin(), m_fetching.end(), [&](const auto& j) { return j.get() == job; }))
                    fetchFront(job, url);
            });
            return;
        }
        const int status = reply->attribute(QNetworkRequest::HttpStatusCodeAttribute).toInt();
        if (reply->error() == QNetworkReply::NoError && status == 200) {
            const QByteArray image = reply->readAll();
            if (image.size() > 256) {
                const Job done = *job;
                m_fetching.erase(it);
                finishJob(done, image);
                startFetches();
                return;
            }
        }
        const bool missing = status == 404 || reply->error() == QNetworkReply::ContentNotFoundError;
        if (missing && !job->releases.isEmpty()) { // nothing under the group's name: its releases
            fetchFront(job, QUrl(QStringLiteral("https://coverartarchive.org/release/%1/front-500").arg(job->releases.takeFirst())));
            return;
        }
        const Job done = *job;
        m_fetching.erase(it);
        if (missing) {
            finishJob(done, {}); // no art for this album anywhere
        } else {
            m_queuedKeys.remove(done.key); // asked again another time
            m_lastError = tr("Cover Art Archive did not answer: %1").arg(reason(reply));
            emit progressChanged();
        }
        startFetches();
    });
}

QString OnlineArt::reason(QNetworkReply* reply)
{
    const int status = sonkore::httpStatus(reply);
    return status > 0 ? QStringLiteral("HTTP %1").arg(status) : tr("no connection");
}

void OnlineArt::finishJob(const Job& job, const QByteArray& image)
{
    QDir().mkpath(cacheDir());
    if (image.isEmpty()) {
        QFile none(noneFor(job.key));
        if (none.open(QIODevice::WriteOnly | QIODevice::Truncate))
            none.close();
    } else {
        QFile f(fileFor(job.key));
        if (f.open(QIODevice::WriteOnly | QIODevice::Truncate)) {
            f.write(image);
            f.close();
            QFile::remove(noneFor(job.key));
            ++m_fetched;
            indexAlbum(job.key, f.fileName());
        }
    }
    m_lastError.clear();
    emit progressChanged();
}

void OnlineArt::indexAlbum(const QString& key, const QString& file)
{
    QStringList paths;
    if (LibraryModel* lib = LibraryModel::instance())
        paths = lib->albumPaths(key);
    {
        QMutexLocker lock(&s_mutex);
        for (const QString& p : paths)
            s_byPath.insert(p, file);
    }
    if (paths.isEmpty())
        return;
    if (CoverImageProvider* provider = CoverImageProvider::instance())
        provider->forget(paths);
    emit coverArrived(paths);
}
