#include "Lyrics.hpp"

#include "AppPaths.hpp"
#include "LibraryModel.hpp"
#include "Metadata.hpp"
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
#include <QTimer>
#include <QRegularExpression>
#include <QSettings>
#include <QUrlQuery>

#include <algorithm>
#include <cmath>

namespace {

constexpr qint64 kRetryNoneDays = 30;

QNetworkRequest requestFor(const QUrl& url)
{
    QNetworkRequest r(url);
    r.setHeader(QNetworkRequest::UserAgentHeader,
                QStringLiteral("Sonkore/%1 ( https://github.com/Axiokyn/Sonkore )").arg(QCoreApplication::applicationVersion()));
    r.setRawHeader("Lrclib-Client", "Sonkore");
    r.setAttribute(QNetworkRequest::RedirectPolicyAttribute, QNetworkRequest::NoLessSafeRedirectPolicy);
    r.setTransferTimeout(20000);
    return r;
}

// The .lrc beside the track (same name), or "".
QString lrcBeside(const QString& path)
{
    const QFileInfo info(path);
    if (!info.exists())
        return {};
    for (const QString& candidate : {info.path() + QLatin1Char('/') + info.completeBaseName() + QStringLiteral(".lrc"),
                                     info.path() + QLatin1Char('/') + info.completeBaseName() + QStringLiteral(".LRC")})
        if (QFileInfo::exists(candidate))
            return candidate;
    return {};
}

QString readText(const QString& file)
{
    QFile f(file);
    if (!f.open(QIODevice::ReadOnly))
        return {};
    return QString::fromUtf8(f.readAll());
}

} // namespace

Lyrics::Lyrics(QObject* parent)
    : QObject(parent)
{
    m_online = QSettings().value("library/onlineLyrics", true).toBool();
}

Lyrics::~Lyrics()
{
    if (m_reply) {
        m_reply->abort();
        m_reply->deleteLater();
    }
}

void Lyrics::setPath(const QString& path)
{
    if (path == m_path)
        return;
    m_path = path;
    ++m_generation;
    m_waiting = false;
    if (m_reply) {
        m_reply->abort(); // its finished() sees the old generation and does nothing
        m_reply = nullptr;
    }
    emit pathChanged();
    resolve();
}

void Lyrics::setOnlineEnabled(bool on)
{
    if (on == m_online)
        return;
    m_online = on;
    QSettings().setValue("library/onlineLyrics", on);
    emit onlineEnabledChanged();
    if (on && m_lines.empty())
        resolve();
}

QVariantList Lyrics::lines() const
{
    QVariantList out;
    out.reserve(static_cast<qsizetype>(m_lines.size()));
    for (const Line& l : m_lines)
        out << QVariantMap{{"time", l.time}, {"text", l.text}};
    return out;
}

int Lyrics::lineAt(double seconds) const
{
    if (!m_synced)
        return -1;
    int at = -1;
    for (std::size_t i = 0; i < m_lines.size(); ++i)
        if (m_lines[i].time >= 0.0 && m_lines[i].time <= seconds)
            at = static_cast<int>(i);
        else if (m_lines[i].time > seconds)
            break;
    return at;
}

std::vector<Lyrics::Line> Lyrics::parse(const QString& text, bool* synced)
{
    static const QRegularExpression stamp(QStringLiteral("\\[(\\d+):(\\d{1,2})(?:[.:](\\d{1,3}))?\\]"));
    static const QRegularExpression tag(QStringLiteral("^\\[[a-zA-Z]+:[^\\]]*\\]\\s*$"));
    std::vector<Line> lines;
    bool timed = false;
    for (QString line : text.split(QRegularExpression(QStringLiteral("\\r?\\n")))) {
        if (tag.match(line).hasMatch()) // [ar:], [ti:], [offset:] ...
            continue;
        std::vector<double> times;
        int end = 0;
        auto it = stamp.globalMatch(line);
        while (it.hasNext()) {
            const QRegularExpressionMatch m = it.next();
            if (m.capturedStart() != end)
                break; // stamps are only read at the start of the line
            const QString frac = m.captured(3);
            double fraction = 0.0;
            if (!frac.isEmpty())
                fraction = frac.toDouble() / std::pow(10.0, frac.size());
            times.push_back(m.captured(1).toInt() * 60.0 + m.captured(2).toInt() + fraction);
            end = m.capturedEnd();
        }
        const QString words = line.mid(end).trimmed();
        if (times.empty()) {
            if (!words.isEmpty() || !lines.empty())
                lines.push_back({-1.0, words});
            continue;
        }
        timed = true;
        for (double t : times)
            lines.push_back({t, words});
    }
    // Trailing blank lines go; timed lines in time order (a line may be
    // written once with several stamps).
    while (!lines.empty() && lines.back().text.isEmpty() && lines.back().time < 0.0)
        lines.pop_back();
    if (timed) {
        std::erase_if(lines, [](const Line& l) { return l.time < 0.0 && l.text.isEmpty(); });
        std::stable_sort(lines.begin(), lines.end(), [](const Line& a, const Line& b) { return a.time < b.time; });
    }
    if (synced)
        *synced = timed;
    return lines;
}

void Lyrics::setLyrics(const QString& text, const QString& source)
{
    bool synced = false;
    m_lines = text.trimmed().isEmpty() ? std::vector<Line>{} : parse(text, &synced);
    m_synced = synced && !m_lines.empty();
    m_source = m_lines.empty() ? QString() : source;
    emit lyricsChanged();
}

QString Lyrics::cacheFileFor(const QVariantMap& track) const
{
    const QString key = track.value("artist").toString().trimmed().toLower() + QChar(0x1f)
        + track.value("title").toString().trimmed().toLower() + QChar(0x1f)
        + track.value("album").toString().trimmed().toLower() + QChar(0x1f)
        + QString::number(static_cast<int>(std::lround(track.value("duration").toDouble())));
    const QString hash = QString::fromLatin1(QCryptographicHash::hash(key.toUtf8(), QCryptographicHash::Sha1).toHex().left(24));
    return sonkore::paths::cacheDir() + QStringLiteral("/lyrics/") + hash;
}

void Lyrics::clearCache()
{
    QDir dir(sonkore::paths::cacheDir() + QStringLiteral("/lyrics"));
    for (const QString& f : dir.entryList(QDir::Files))
        dir.remove(f);
    if (m_source == QLatin1String("online"))
        setLyrics({}, {});
}

void Lyrics::resolve()
{
    m_status.clear();
    if (m_path.isEmpty()) {
        setLyrics({}, {});
        return;
    }
    // 1. A .lrc beside the file.
    if (const QString lrc = lrcBeside(m_path); !lrc.isEmpty()) {
        setLyrics(readText(lrc), QStringLiteral("file"));
        if (!m_lines.empty())
            return;
    }
    // 2. The file's own tag (some taggers leave a link there, not words).
    if (const QString embedded = sonkore::Metadata::embeddedLyrics(m_path).trimmed(); !embedded.isEmpty()
        && !(embedded.count(QLatin1Char('\n')) == 0 && (embedded.startsWith(QLatin1String("http")) || !embedded.contains(QLatin1Char(' '))))) {
        setLyrics(embedded, QStringLiteral("embedded"));
        if (!m_lines.empty())
            return;
    }
    // 3. Fetched before, or LRCLIB now.
    LibraryModel* lib = LibraryModel::instance();
    const QVariantMap track = lib ? lib->track(m_path) : QVariantMap();
    const QString base = cacheFileFor(track);
    if (QFileInfo::exists(base + QStringLiteral(".lrc"))) {
        setLyrics(readText(base + QStringLiteral(".lrc")), QStringLiteral("online"));
        return;
    }
    setLyrics({}, {});
    if (!m_online)
        return;
    if (const QFileInfo none(base + QStringLiteral(".none")); none.exists() && none.lastModified().daysTo(QDateTime::currentDateTime()) < kRetryNoneDays) {
        m_status = tr("No lyrics found for this track");
        emit lyricsChanged();
        return;
    }
    fetch(track);
}

QNetworkAccessManager& Lyrics::network()
{
    if (!m_network) {
        m_network = std::make_unique<QNetworkAccessManager>();
        m_network->setAutoDeleteReplies(false);
    }
    return *m_network;
}

void Lyrics::fetch(const QVariantMap& track)
{
    const QString title = track.value("title").toString().trimmed();
    const QString artist = track.value("artist").toString().trimmed();
    if (title.isEmpty() || artist.isEmpty())
        return; // nothing to look up by
    const QString album = track.value("album").toString().trimmed();
    const int duration = static_cast<int>(std::lround(track.value("duration").toDouble()));

    QUrl url(QStringLiteral("https://lrclib.net/api/get"));
    QUrlQuery q;
    q.addQueryItem("track_name", title);
    q.addQueryItem("artist_name", artist);
    if (!album.isEmpty())
        q.addQueryItem("album_name", album);
    if (duration > 0)
        q.addQueryItem("duration", QString::number(duration));
    url.setQuery(q);

    const QString base = cacheFileFor(track);
    m_status = tr("Looking for the lyrics…");
    const auto store = [this, base](const QString& text) {
        QDir().mkpath(QFileInfo(base).path());
        QFile f(base + QStringLiteral(".lrc"));
        if (f.open(QIODevice::WriteOnly | QIODevice::Truncate)) {
            f.write(text.toUtf8());
            f.close();
        }
        QFile::remove(base + QStringLiteral(".none"));
        m_status.clear();
        setLyrics(text, QStringLiteral("online"));
    };
    const auto none = [this, base] { // looked up again after kRetryNoneDays
        QDir().mkpath(QFileInfo(base).path());
        QFile f(base + QStringLiteral(".none"));
        if (f.open(QIODevice::WriteOnly | QIODevice::Truncate))
            f.close();
        m_status = tr("No lyrics found for this track");
        emit lyricsChanged();
    };
    get(url, [this, store, none, title, artist](QNetworkReply* reply) {
        const int status = sonkore::httpStatus(reply);
        if (reply->error() == QNetworkReply::NoError && status == 200) {
            const QJsonObject o = QJsonDocument::fromJson(reply->readAll()).object();
            const QString synced = o.value("syncedLyrics").toString();
            const QString plain = o.value("plainLyrics").toString();
            if (!synced.trimmed().isEmpty())
                store(synced);
            else if (!plain.trimmed().isEmpty())
                store(plain);
            else
                none(); // instrumental
            return;
        }
        if (status != 404 && reply->error() != QNetworkReply::ContentNotFoundError) {
            failed(reply);
            return;
        }
        // Not under that exact album and length: the closest match by title and artist.
        QUrl url(QStringLiteral("https://lrclib.net/api/search"));
        QUrlQuery q;
        q.addQueryItem("track_name", title);
        q.addQueryItem("artist_name", artist);
        url.setQuery(q);
        get(url, [this, store, none](QNetworkReply* reply) {
            if (reply->error() != QNetworkReply::NoError) {
                failed(reply); // nothing remembered: asked again next time
                return;
            }
            const QJsonArray hits = QJsonDocument::fromJson(reply->readAll()).array();
            QString best;
            for (const QJsonValue& v : hits) { // a timed one first
                const QString synced = v.toObject().value("syncedLyrics").toString();
                if (!synced.trimmed().isEmpty()) {
                    best = synced;
                    break;
                }
                if (best.isEmpty())
                    best = v.toObject().value("plainLyrics").toString();
            }
            if (best.trimmed().isEmpty())
                none();
            else
                store(best);
        });
    });
}

void Lyrics::get(const QUrl& url, std::function<void(QNetworkReply*)> done, int attempt)
{
    const int generation = m_generation;
    QNetworkReply* reply = network().get(requestFor(url));
    m_reply = reply;
    emit lyricsChanged();
    connect(reply, &QNetworkReply::finished, this, [this, reply, generation, url, done = std::move(done), attempt] {
        reply->deleteLater();
        if (generation != m_generation)
            return; // another track by now
        m_reply = nullptr;
        if (const int wait = sonkore::retryDelayMs(reply, attempt); wait >= 0) {
            m_waiting = true;
            m_status = tr("LRCLIB is busy; trying again…");
            emit lyricsChanged();
            QTimer::singleShot(wait, this, [this, generation, url, done, attempt] {
                if (generation != m_generation)
                    return;
                m_waiting = false;
                m_status = tr("Looking for the lyrics…");
                get(url, done, attempt + 1);
            });
            return;
        }
        done(reply);
    });
}

void Lyrics::failed(QNetworkReply* reply)
{
    const int status = sonkore::httpStatus(reply);
    if (status == 429 || status == 503)
        m_status = tr("LRCLIB is busy right now. The lyrics are looked up again the next time this track plays.");
    else if (status > 0)
        m_status = tr("LRCLIB answered with error %1.").arg(status);
    else
        m_status = tr("Could not reach LRCLIB. Check the internet connection.");
    emit lyricsChanged();
}
