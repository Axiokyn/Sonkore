#include "MprisService.hpp"
#include "AppPaths.hpp"

#include "DspController.hpp"
#include "Metadata.hpp"

#include <QCoreApplication>
#include <QCryptographicHash>
#include <QDBusConnection>
#include <QDBusConnectionInterface>
#include <QDBusInterface>
#include <QDBusMessage>
#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QStandardPaths>
#include <QUrl>

#include <algorithm>

namespace {

const QString kService = QStringLiteral("org.mpris.MediaPlayer2.sonkore");
const QString kPath = QStringLiteral("/org/mpris/MediaPlayer2");
const QString kRootIface = QStringLiteral("org.mpris.MediaPlayer2");
const QString kPlayerIface = QStringLiteral("org.mpris.MediaPlayer2.Player");

} // namespace

// --- Service ---------------------------------------------------------------------

MprisService::MprisService(DspController* player, QObject* parent)
    : QObject(parent)
    , m_player(player)
{
    new MprisRootAdaptor(this);
    auto* playerAdaptor = new MprisPlayerAdaptor(this);

    updateMetadata();
    connect(player, &DspController::trackChanged, this, [this] {
        updateMetadata();
        notify(kPlayerIface, {{"Metadata", m_metadata},
                              {"CanGoNext", !m_player->queue().isEmpty()},
                              {"CanGoPrevious", !m_player->queue().isEmpty()},
                              {"CanPlay", !m_player->queue().isEmpty()},
                              {"CanSeek", m_player->duration() > 0}});
    });
    connect(player, &DspController::stateChanged, this,
            [this] { notify(kPlayerIface, {{"PlaybackStatus", playbackStatus()}}); });
    connect(player, &DspController::shuffleChanged, this,
            [this] { notify(kPlayerIface, {{"Shuffle", m_player->shuffle()}}); });
    connect(player, &DspController::repeatModeChanged, this, [this, playerAdaptor] {
        notify(kPlayerIface, {{"LoopStatus", playerAdaptor->loopStatus()}});
    });
    connect(player, &DspController::volumeChanged, this,
            [this] { notify(kPlayerIface, {{"Volume", m_player->volume()}}); });
    connect(player, &DspController::seeked, playerAdaptor,
            [playerAdaptor](double seconds) { emit playerAdaptor->Seeked(static_cast<qlonglong>(seconds * 1e6)); });
}

bool MprisService::registerOnBus()
{
    if (m_registered)
        return true;
    QDBusConnection bus = QDBusConnection::sessionBus();
    if (!bus.isConnected())
        return false;
    QString name = kService;
    if (!bus.registerService(name)) {
        name = kService + QStringLiteral(".instance%1").arg(QCoreApplication::applicationPid());
        if (!bus.registerService(name))
            return false;
    }
    m_serviceName = name;
    m_registered = bus.registerObject(kPath, this);
    if (!m_registered) {
        bus.unregisterService(name);
        m_serviceName.clear();
    }
    return m_registered;
}

void MprisService::unregisterFromBus()
{
    if (!m_registered)
        return;
    QDBusConnection bus = QDBusConnection::sessionBus();
    bus.unregisterObject(kPath);
    bus.unregisterService(m_serviceName);
    m_serviceName.clear();
    m_registered = false;
}

void MprisService::setEnabled(bool on)
{
    if (on) {
        updateMetadata(); // the desktop reads everything afresh on appearance
        registerOnBus();
    } else {
        unregisterFromBus();
    }
}

QString MprisService::playbackStatus() const
{
    if (m_player->playing())
        return QStringLiteral("Playing");
    return m_player->loaded() ? QStringLiteral("Paused") : QStringLiteral("Stopped");
}

QString MprisService::artUrlFor(const QString& path)
{
    const QByteArray image = sonkore::Metadata::embeddedCover(path);
    if (image.isEmpty()) {
        const QString folderImage = sonkore::Metadata::folderCover(QFileInfo(path).absolutePath());
        return folderImage.isEmpty() ? QString() : QUrl::fromLocalFile(folderImage).toString();
    }
    const QString dir = sonkore::paths::cacheDir() + QStringLiteral("/mpris");
    QDir().mkpath(dir);
    const QString ext = image.startsWith("\x89PNG") ? QStringLiteral("png") : QStringLiteral("jpg");
    const QString file = dir + '/' + QCryptographicHash::hash(image, QCryptographicHash::Sha1).toHex().left(16) + '.' + ext;
    if (!QFile::exists(file)) {
        QFile f(file);
        if (f.open(QIODevice::WriteOnly))
            f.write(image);
    }
    return QUrl::fromLocalFile(file).toString();
}

void MprisService::updateMetadata()
{
    const QVariantMap t = m_player->currentTrack();
    const QString path = m_player->currentPath();
    m_metadata.clear();
    if (path.isEmpty()) {
        m_metadata.insert("mpris:trackid", QVariant::fromValue(QDBusObjectPath("/org/mpris/MediaPlayer2/TrackList/NoTrack")));
        return;
    }
    ++m_trackCounter;
    m_metadata.insert("mpris:trackid", QVariant::fromValue(QDBusObjectPath(QStringLiteral("/org/sonkore/track/%1").arg(m_trackCounter))));
    m_metadata.insert("mpris:length", static_cast<qlonglong>(t.value("duration").toDouble() * 1e6));
    m_metadata.insert("xesam:title", t.value("title").toString());
    m_metadata.insert("xesam:album", t.value("album").toString());
    m_metadata.insert("xesam:artist", QStringList{t.value("artist").toString()});
    if (!t.value("albumArtist").toString().isEmpty())
        m_metadata.insert("xesam:albumArtist", QStringList{t.value("albumArtist").toString()});
    if (t.value("trackNo").toInt() > 0)
        m_metadata.insert("xesam:trackNumber", t.value("trackNo").toInt());
    if (!t.value("genre").toString().isEmpty())
        m_metadata.insert("xesam:genre", QStringList{t.value("genre").toString()});
    m_metadata.insert("xesam:url", QUrl::fromLocalFile(path).toString());
    const QString art = artUrlFor(path);
    if (!art.isEmpty())
        m_metadata.insert("mpris:artUrl", art);
}

void MprisService::notify(const QString& interface, const QVariantMap& changed)
{
    if (!m_registered)
        return;
    QDBusMessage signal = QDBusMessage::createSignal(kPath, QStringLiteral("org.freedesktop.DBus.Properties"),
                                                     QStringLiteral("PropertiesChanged"));
    signal << interface << changed << QStringList();
    QDBusConnection::sessionBus().send(signal);
}

bool MprisService::forwardToRunningInstance(const QStringList& paths)
{
    QDBusConnection bus = QDBusConnection::sessionBus();
    if (!bus.isConnected() || !bus.interface()->isServiceRegistered(kService))
        return false;
    QDBusInterface player(kService, kPath, kPlayerIface, bus);
    QDBusInterface root(kService, kPath, kRootIface, bus);
    if (!player.isValid())
        return false;
    for (const QString& p : paths)
        player.call(QStringLiteral("OpenUri"), QUrl::fromLocalFile(QFileInfo(p).absoluteFilePath()).toString());
    root.call(QStringLiteral("Raise"));
    return true;
}

bool MprisService::sendToRunningInstance(const QString& method)
{
    QDBusConnection bus = QDBusConnection::sessionBus();
    if (!bus.isConnected() || !bus.interface()->isServiceRegistered(kService))
        return false;
    QDBusInterface player(kService, kPath, kPlayerIface, bus);
    if (!player.isValid())
        return false;
    player.call(method);
    return true;
}

// --- org.mpris.MediaPlayer2 ---------------------------------------------------------

MprisRootAdaptor::MprisRootAdaptor(MprisService* service)
    : QDBusAbstractAdaptor(service)
    , m_service(service)
{
}

QStringList MprisRootAdaptor::supportedMimeTypes() const
{
    return {"audio/flac", "audio/x-flac", "audio/mpeg", "audio/wav", "audio/x-wav"};
}

void MprisRootAdaptor::Raise()
{
    emit m_service->raiseRequested();
}

void MprisRootAdaptor::Quit()
{
    QCoreApplication::quit();
}

// --- org.mpris.MediaPlayer2.Player --------------------------------------------------

MprisPlayerAdaptor::MprisPlayerAdaptor(MprisService* service)
    : QDBusAbstractAdaptor(service)
    , m_service(service)
{
}

QString MprisPlayerAdaptor::playbackStatus() const { return m_service->playbackStatus(); }

QString MprisPlayerAdaptor::loopStatus() const
{
    switch (m_service->player()->repeatMode()) {
    case DspController::RepeatAll: return QStringLiteral("Playlist");
    case DspController::RepeatOne: return QStringLiteral("Track");
    default: return QStringLiteral("None");
    }
}

void MprisPlayerAdaptor::setLoopStatus(const QString& status)
{
    m_service->player()->setRepeatMode(status == "Playlist" ? DspController::RepeatAll
                                       : status == "Track" ? DspController::RepeatOne
                                                           : DspController::RepeatOff);
}

bool MprisPlayerAdaptor::shuffle() const { return m_service->player()->shuffle(); }
void MprisPlayerAdaptor::setShuffle(bool on) { m_service->player()->setShuffle(on); }
QVariantMap MprisPlayerAdaptor::metadata() const { return m_service->metadata(); }
double MprisPlayerAdaptor::volume() const { return m_service->player()->volume(); }
void MprisPlayerAdaptor::setVolume(double v) { m_service->player()->setVolume(std::clamp(v, 0.0, 1.0)); }
qlonglong MprisPlayerAdaptor::position() const { return static_cast<qlonglong>(m_service->player()->position() * 1e6); }
bool MprisPlayerAdaptor::canGoNext() const { return !m_service->player()->queue().isEmpty(); }
bool MprisPlayerAdaptor::canGoPrevious() const { return !m_service->player()->queue().isEmpty(); }
bool MprisPlayerAdaptor::canPlay() const { return !m_service->player()->queue().isEmpty(); }
bool MprisPlayerAdaptor::canSeek() const { return m_service->player()->duration() > 0; }

void MprisPlayerAdaptor::Next() { m_service->player()->next(); }
void MprisPlayerAdaptor::Previous() { m_service->player()->previous(); }
void MprisPlayerAdaptor::Pause() { m_service->player()->pauseFromDesktop(); }
void MprisPlayerAdaptor::PlayPause() { m_service->player()->togglePlayPause(); }
void MprisPlayerAdaptor::Stop() { m_service->player()->stop(); }
void MprisPlayerAdaptor::Play() { m_service->player()->play(); }

void MprisPlayerAdaptor::Seek(qlonglong offsetUs)
{
    DspController* p = m_service->player();
    const double target = p->position() + offsetUs / 1e6;
    if (target >= p->duration())
        p->next();
    else
        p->seek(std::max(0.0, target));
}

void MprisPlayerAdaptor::SetPosition(const QDBusObjectPath& trackId, qlonglong positionUs)
{
    if (trackId.path() != m_service->metadata().value("mpris:trackid").value<QDBusObjectPath>().path())
        return; // stale request for a previous track
    m_service->player()->seek(positionUs / 1e6);
}

void MprisPlayerAdaptor::OpenUri(const QString& uri)
{
    const QUrl url(uri);
    const QString path = url.isLocalFile() ? url.toLocalFile() : uri;
    QStringList queue = m_service->player()->queue();
    queue << path;
    m_service->player()->playPaths(queue, static_cast<int>(queue.size()) - 1);
}
