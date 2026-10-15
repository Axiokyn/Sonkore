#pragma once

#include "QmlSingleton.hpp"

#include <QObject>
#include <QQmlEngine>
#include <QString>
#include <QVariantList>

#include <functional>
#include <memory>
#include <vector>

class QNetworkAccessManager;
class QNetworkReply;

// The lyrics of the track playing: a .lrc file beside it, the LYRICS tag in
// it, or LRCLIB (lrclib.net, a public domain database of synchronised
// lyrics: the title, artist, album and length are sent). Timed lines carry
// their time, so the view can follow the music and a click can seek.
class Lyrics : public QObject {
    Q_OBJECT
    QML_ELEMENT
    QML_SINGLETON
    Q_PROPERTY(QString path READ path WRITE setPath NOTIFY pathChanged)
    Q_PROPERTY(bool onlineEnabled READ onlineEnabled WRITE setOnlineEnabled NOTIFY onlineEnabledChanged)
    // [{time: seconds (-1 when not timed), text}], in order.
    Q_PROPERTY(QVariantList lines READ lines NOTIFY lyricsChanged)
    Q_PROPERTY(bool available READ available NOTIFY lyricsChanged)
    Q_PROPERTY(bool synced READ synced NOTIFY lyricsChanged)
    Q_PROPERTY(bool loading READ loading NOTIFY lyricsChanged)
    // "file", "embedded", "online" or "".
    Q_PROPERTY(QString source READ source NOTIFY lyricsChanged)
    Q_PROPERTY(QString status READ status NOTIFY lyricsChanged)
    SONKORE_QML_SINGLETON_INSTANCE(Lyrics)

public:
    explicit Lyrics(QObject* parent);
    ~Lyrics() override;

    QString path() const { return m_path; }
    void setPath(const QString& path);
    bool onlineEnabled() const { return m_online; }
    void setOnlineEnabled(bool on);

    QVariantList lines() const;
    bool available() const { return !m_lines.empty(); }
    bool synced() const { return m_synced; }
    bool loading() const { return m_reply != nullptr || m_waiting; }
    QString source() const { return m_source; }
    QString status() const { return m_status; }

    // The line being sung at `seconds` (-1 before the first); the last line
    // whose time has come.
    Q_INVOKABLE int lineAt(double seconds) const;
    // Forgets every downloaded lyric.
    Q_INVOKABLE void clearCache();

    struct Line {
        double time = -1.0; // seconds, -1 = not timed
        QString text;
    };
    // "[mm:ss.xx]text" lines (several stamps a line allowed) or plain text.
    static std::vector<Line> parse(const QString& text, bool* synced);

signals:
    void pathChanged();
    void onlineEnabledChanged();
    void lyricsChanged();

private:
    void resolve();
    void fetch(const QVariantMap& track);
    // GET with retries while LRCLIB says it is busy; `done` is not called
    // once another track is shown.
    void get(const QUrl& url, std::function<void(QNetworkReply*)> done, int attempt = 0);
    void failed(QNetworkReply* reply);
    void setLyrics(const QString& text, const QString& source);
    QNetworkAccessManager& network();
    QString cacheFileFor(const QVariantMap& track) const;

    QString m_path;
    bool m_online = true;
    std::vector<Line> m_lines;
    bool m_synced = false;
    QString m_source, m_status;
    std::unique_ptr<QNetworkAccessManager> m_network;
    QNetworkReply* m_reply = nullptr;
    bool m_waiting = false; // for a retry after a "busy" answer
    int m_generation = 0; // a new path makes replies for the old one moot
};
