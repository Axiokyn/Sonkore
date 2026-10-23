#pragma once

#include <QDBusAbstractAdaptor>
#include <QDBusObjectPath>
#include <QObject>
#include <QStringList>
#include <QVariantMap>

class DspController;

// MPRIS 2 (org.mpris.MediaPlayer2) on the session bus: GNOME's media widget
// and lock screen, KDE's task-manager tooltips, Dash-to-Panel/Dock, media
// keys and `playerctl` all control Sonkore through it.
class MprisService : public QObject {
    Q_OBJECT
public:
    MprisService(DspController* player, QObject* parent = nullptr);
    bool isRegistered() const { return m_registered; }
    // On the session bus or not (Settings › System). Off: the desktop no
    // longer sees the player, and a second launch cannot hand files over.
    void setEnabled(bool on);
    DspController* player() const { return m_player; }

    QVariantMap metadata() const { return m_metadata; }
    QString playbackStatus() const;

    // Sends file paths to an already running instance; true if one answered.
    static bool forwardToRunningInstance(const QStringList& paths);
    // Calls a Player method (PlayPause, Next...) on a running instance; true if one answered.
    static bool sendToRunningInstance(const QString& method);

signals:
    void raiseRequested();

private:
    void updateMetadata();
    void notify(const QString& interface, const QVariantMap& changed);
    QString artUrlFor(const QString& path);
    bool registerOnBus();
    void unregisterFromBus();

    DspController* m_player;
    QVariantMap m_metadata;
    bool m_registered = false;
    QString m_serviceName;
    int m_trackCounter = 0;
};

class MprisRootAdaptor : public QDBusAbstractAdaptor {
    Q_OBJECT
    Q_CLASSINFO("D-Bus Interface", "org.mpris.MediaPlayer2")
    Q_PROPERTY(bool CanQuit READ canQuit)
    Q_PROPERTY(bool CanRaise READ canRaise)
    Q_PROPERTY(bool CanSetFullscreen READ canSetFullscreen)
    Q_PROPERTY(bool Fullscreen READ fullscreen)
    Q_PROPERTY(bool HasTrackList READ hasTrackList)
    Q_PROPERTY(QString Identity READ identity)
    Q_PROPERTY(QString DesktopEntry READ desktopEntry)
    Q_PROPERTY(QStringList SupportedUriSchemes READ supportedUriSchemes)
    Q_PROPERTY(QStringList SupportedMimeTypes READ supportedMimeTypes)

public:
    explicit MprisRootAdaptor(MprisService* service);
    bool canQuit() const { return true; }
    bool canRaise() const { return true; }
    bool canSetFullscreen() const { return false; }
    bool fullscreen() const { return false; }
    bool hasTrackList() const { return false; }
    QString identity() const { return QStringLiteral("Sonkore"); }
    QString desktopEntry() const { return QStringLiteral("com.axiokyn.Sonkore"); }
    QStringList supportedUriSchemes() const { return {QStringLiteral("file")}; }
    QStringList supportedMimeTypes() const;

public slots:
    void Raise();
    void Quit();

private:
    MprisService* m_service;
};

class MprisPlayerAdaptor : public QDBusAbstractAdaptor {
    Q_OBJECT
    Q_CLASSINFO("D-Bus Interface", "org.mpris.MediaPlayer2.Player")
    Q_PROPERTY(QString PlaybackStatus READ playbackStatus)
    Q_PROPERTY(QString LoopStatus READ loopStatus WRITE setLoopStatus)
    Q_PROPERTY(double Rate READ rate WRITE setRate)
    Q_PROPERTY(bool Shuffle READ shuffle WRITE setShuffle)
    Q_PROPERTY(QVariantMap Metadata READ metadata)
    Q_PROPERTY(double Volume READ volume WRITE setVolume)
    Q_PROPERTY(qlonglong Position READ position)
    Q_PROPERTY(double MinimumRate READ minimumRate)
    Q_PROPERTY(double MaximumRate READ maximumRate)
    Q_PROPERTY(bool CanGoNext READ canGoNext)
    Q_PROPERTY(bool CanGoPrevious READ canGoPrevious)
    Q_PROPERTY(bool CanPlay READ canPlay)
    Q_PROPERTY(bool CanPause READ canPause)
    Q_PROPERTY(bool CanSeek READ canSeek)
    Q_PROPERTY(bool CanControl READ canControl)

public:
    explicit MprisPlayerAdaptor(MprisService* service);

    QString playbackStatus() const;
    QString loopStatus() const;
    void setLoopStatus(const QString& status);
    double rate() const { return 1.0; }
    void setRate(double) {}
    bool shuffle() const;
    void setShuffle(bool on);
    QVariantMap metadata() const;
    double volume() const;
    void setVolume(double v);
    qlonglong position() const;
    double minimumRate() const { return 1.0; }
    double maximumRate() const { return 1.0; }
    bool canGoNext() const;
    bool canGoPrevious() const;
    bool canPlay() const;
    bool canPause() const { return true; }
    bool canSeek() const;
    bool canControl() const { return true; }

public slots:
    void Next();
    void Previous();
    void Pause();
    void PlayPause();
    void Stop();
    void Play();
    void Seek(qlonglong offsetUs);
    void SetPosition(const QDBusObjectPath& trackId, qlonglong positionUs);
    void OpenUri(const QString& uri);

signals:
    void Seeked(qlonglong Position);

private:
    MprisService* m_service;
};
