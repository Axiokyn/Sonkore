#include "AppController.hpp"
#include "AppPaths.hpp"
#include "CoverImageProvider.hpp"
#include "DSPManager.hpp"
#include "DspController.hpp"
#include "EqController.hpp"
#include "LibraryModel.hpp"
#include "Metadata.hpp"
#include "PlaylistManager.hpp"
#include "WaveformController.hpp"
#include "WinTaskbar.hpp"

#if defined(SONKORE_HAVE_MPRIS)
#include "MprisService.hpp"
#endif

#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QProcess>
#include <QRegularExpression>
#include <QSaveFile>
#include <QStandardPaths>
#include <QApplication>
#include <QIcon>
#include <QImage>
#include <QImageReader>
#include <QPixmap>
#include <QQmlApplicationEngine>
#include <QQuickStyle>
#include <QQuickWindow>
#include <QThreadPool>
#include <QTimer>
#include <QUrl>

#include <chrono>
#include <cstdlib>
#include <thread>

#if defined(Q_OS_UNIX)
#include <QSocketNotifier>
#include <csignal>
#include <sys/socket.h>
#include <unistd.h>
#endif

namespace {

QIcon appIcon()
{
    // Read as PNG by name, without format detection: a reader left to detect
    // the format asks every image plugin (TIFF, WebP, ICNS...) first.
    QIcon icon;
    for (int size : {16, 22, 24, 32, 48, 64, 128, 256, 512}) {
        QImageReader reader(QStringLiteral(":/icons/png/%1.png").arg(size), "png");
        reader.setAutoDetectImageFormat(false);
        icon.addPixmap(QPixmap::fromImage(reader.read()));
    }
    return icon;
}

// Audio files passed on the command line ("Open with Sonkore", drag onto the
// launcher, `Sonkore song.flac`).
QStringList audioArguments(const QStringList& args)
{
    QStringList files;
    for (const QString& arg : args) {
        const QUrl url(arg);
        const QString path = url.isLocalFile() ? url.toLocalFile() : arg;
        if (QFileInfo(path).isFile() && sonkore::Metadata::isAudioFile(path))
            files << QFileInfo(path).absoluteFilePath();
        else if (QFileInfo(path).suffix().compare(QLatin1String("cue"), Qt::CaseInsensitive) == 0)
            files << sonkore::Metadata::cueTracks(QFileInfo(path).absoluteFilePath());
    }
    return files;
}

#if defined(Q_OS_LINUX)
// Writes `data` to `path` unless it already holds exactly that.
bool writeIfChanged(const QString& path, const QByteArray& data)
{
    QFile current(path);
    if (current.open(QIODevice::ReadOnly) && current.readAll() == data)
        return false;
    current.close();
    QDir().mkpath(QFileInfo(path).path());
    QSaveFile out(path);
    return out.open(QIODevice::WriteOnly) && out.write(data) == data.size() && out.commit();
}

QByteArray resource(const QString& name)
{
    QFile f(name);
    return f.open(QIODevice::ReadOnly) ? f.readAll() : QByteArray();
}

// Run as an AppImage, Sonkore puts itself in the desktop the way a package
// would: menu entry and icons under ~/.local/share, so GNOME / KDE show its
// icon in the dock and the app menu instead of a generic one. The entry
// follows the AppImage when it moves and disappears from the menu when it is
// deleted (TryExec). An install that is not ours (a distro package,
// install-user.sh) is left alone.
void integrateAppImage()
{
    const QString appImage = qEnvironmentVariable("APPIMAGE");
    const QString appDir = qEnvironmentVariable("APPDIR");
    if (appImage.isEmpty() || appDir.isEmpty() || !QFileInfo(appImage).isFile())
        return;
    const QString data = QStandardPaths::writableLocation(QStandardPaths::GenericDataLocation);
    const QString entryPath = data + QStringLiteral("/applications/sonkore.desktop");
    constexpr char kMarker[] = "X-Sonkore-AppImage=true";

    bool ours = true;
    for (const QString& existing : QStandardPaths::locateAll(QStandardPaths::ApplicationsLocation, QStringLiteral("sonkore.desktop"))) {
        if (QFileInfo(existing).absoluteFilePath() != QFileInfo(entryPath).absoluteFilePath())
            ours = false; // installed by a package
        else if (!resource(existing).contains(kMarker))
            ours = false; // installed by hand (install-user.sh)
    }

    bool changed = false;
    if (ours) {
        QString entry = QString::fromUtf8(resource(appDir + QStringLiteral("/sonkore.desktop")));
        if (!entry.isEmpty()) {
            const QString quoted = QLatin1Char('"') + appImage + QLatin1Char('"');
            entry.replace(QRegularExpression(QStringLiteral("(?m)^Exec=Sonkore")), QStringLiteral("Exec=") + quoted);
            entry.replace(QRegularExpression(QStringLiteral("(?m)^TryExec=.*$")), QStringLiteral("TryExec=") + appImage);
            entry.replace(QStringLiteral("[Desktop Entry]\n"), QStringLiteral("[Desktop Entry]\n") + QLatin1String(kMarker) + QLatin1Char('\n'));
            changed |= writeIfChanged(entryPath, entry.toUtf8());
            for (int size : {16, 22, 24, 32, 48, 64, 128, 256, 512})
                changed |= writeIfChanged(data + QStringLiteral("/icons/hicolor/%1x%1/apps/sonkore.png").arg(size),
                                          resource(QStringLiteral(":/icons/png/%1.png").arg(size)));
            changed |= writeIfChanged(data + QStringLiteral("/icons/hicolor/scalable/apps/sonkore.svg"),
                                      resource(QStringLiteral(":/icons/sonkore.svg")));
        }
    }
    if (changed) {
        const QString db = QStandardPaths::findExecutable(QStringLiteral("update-desktop-database"));
        if (!db.isEmpty())
            QProcess::startDetached(db, {data + QStringLiteral("/applications")});
        const QString cache = QStandardPaths::findExecutable(QStringLiteral("gtk-update-icon-cache"));
        if (!cache.isEmpty())
            QProcess::startDetached(cache, {QStringLiteral("-q"), QStringLiteral("-t"), data + QStringLiteral("/icons/hicolor")});
    }

    // File managers built on GVfs (Nautilus, Nemo, Caja) do not look inside
    // an AppImage for its icon: give the file itself Sonkore's icon. The image
    // is kept with the user's data, so it outlives the AppImage's mount.
    const QString gio = QStandardPaths::findExecutable(QStringLiteral("gio"));
    const QString icon = sonkore::paths::dataFile(QStringLiteral("appimage-icon.png"));
    const QByteArray png = resource(QStringLiteral(":/icons/png/256.png"));
    if (gio.isEmpty() || png.isEmpty())
        return;
    writeIfChanged(icon, png);
    if (QFileInfo::exists(icon))
        QProcess::startDetached(gio, {QStringLiteral("set"), appImage, QStringLiteral("metadata::custom-icon"),
                                      QUrl::fromLocalFile(icon).toString()});
}
#endif

#if defined(Q_OS_UNIX)
// SIGTERM / SIGINT / SIGHUP (logout, `kill`, Ctrl+C) quit through the event
// loop, so the queue, position and settings are saved as on a normal close.
int g_signalPipe[2] = {-1, -1};

void forwardSignal(int)
{
    const char byte = 1;
    [[maybe_unused]] const auto written = ::write(g_signalPipe[0], &byte, 1);
}

void quitOnTerminationSignals(QCoreApplication& app)
{
    if (::socketpair(AF_UNIX, SOCK_STREAM, 0, g_signalPipe) != 0)
        return;
    auto* notifier = new QSocketNotifier(g_signalPipe[1], QSocketNotifier::Read, &app);
    QObject::connect(notifier, &QSocketNotifier::activated, &app, [] {
        char byte = 0;
        [[maybe_unused]] const auto got = ::read(g_signalPipe[1], &byte, 1);
        QCoreApplication::quit();
    });
    struct sigaction action {};
    action.sa_handler = forwardSignal;
    sigemptyset(&action.sa_mask);
    action.sa_flags = SA_RESTART;
    for (int sig : {SIGTERM, SIGINT, SIGHUP})
        ::sigaction(sig, &action, nullptr);
}
#endif

} // namespace

int main(int argc, char* argv[])
{
    // Settings: ~/.config/sonkore/sonkore.conf (see AppPaths.hpp).
    QCoreApplication::setOrganizationName(QString::fromLatin1(sonkore::paths::kAppId));
    QCoreApplication::setApplicationName(QString::fromLatin1(sonkore::paths::kAppId));
    QCoreApplication::setApplicationVersion(QStringLiteral(PROJECT_VERSION_STRING));
    bool safeMode = false;
    for (int i = 1; i < argc; ++i)
        safeMode = safeMode || qstrcmp(argv[i], "--safe-mode") == 0;
    AppController::applyStartupSettings(safeMode); // UI scale + renderer: before the GUI exists

    // QApplication (not QGuiApplication): the tray icon and its menu fall back
    // to Qt Widgets on desktops without a native StatusNotifier host.
    QApplication app(argc, argv);
    // Qt's pooled threads (image loading, scans) go away soon after their
    // work instead of idling for half a minute.
    QThreadPool::globalInstance()->setExpiryTimeout(3000);
    QGuiApplication::setApplicationDisplayName(QStringLiteral("Sonkore"));
    QGuiApplication::setDesktopFileName(QStringLiteral("sonkore")); // Wayland app_id -> sonkore.desktop
    QGuiApplication::setWindowIcon(appIcon());
    QQuickStyle::setStyle(QStringLiteral("Fusion"));
#if defined(Q_OS_UNIX)
    quitOnTerminationSignals(app);
#endif
#if defined(Q_OS_LINUX)
    QTimer::singleShot(3000, &app, integrateAppImage); // after the window is up
#endif

    const QStringList files = audioArguments(app.arguments().mid(1));
#if defined(SONKORE_HAVE_MPRIS)
    // Already running: hand the files over and bring that window forward.
    if (!files.isEmpty() && MprisService::forwardToRunningInstance(files)) {
        AppController::abandonStartup();
        return 0;
    }
#endif

    // Destroyed in reverse order: QML first, then the controllers (which save
    // their state), and the audio engine last.
    sonkore::DSPManager engine;
    engine.initialize(); // failures surface when a track is opened

    AppController appController(nullptr);
    AppController::setInstance(&appController);
    LibraryModel library(nullptr);
    LibraryModel::setInstance(&library);
    PlaylistManager playlists(nullptr);
    PlaylistManager::setInstance(&playlists);
    EqController equalizer(engine.equalizer());
    EqController::setInstance(&equalizer);
    DspController controller(engine);
    DspController::setInstance(&controller);
    WaveformController waveform(nullptr);
    WaveformController::setInstance(&waveform);
    QObject::connect(&controller, &DspController::trackChanged, &waveform,
                     [&] { waveform.setPath(controller.currentPath()); });
    waveform.setPath(controller.currentPath());

#if defined(SONKORE_HAVE_MPRIS)
    MprisService mpris(&controller);
    QObject::connect(&mpris, &MprisService::raiseRequested, &appController, &AppController::requestRaise);
    mpris.setEnabled(appController.mprisEnabled());
    QObject::connect(&appController, &AppController::mprisEnabledChanged, &mpris,
                     [&] { mpris.setEnabled(appController.mprisEnabled()); });
#endif
    WinTaskbar taskbar(&controller);

    QQmlApplicationEngine qml;
    appController.setQmlEngine(&qml);
    QObject::connect(&appController, &AppController::languageChanged, &controller, &DspController::refreshDevices);
    qml.addImageProvider(QStringLiteral("cover"), new CoverImageProvider);
    QObject::connect(&qml, &QQmlApplicationEngine::objectCreationFailed, &app,
                     [] { QCoreApplication::exit(-1); }, Qt::QueuedConnection);
    qml.loadFromModule("Sonkore", "Main");

    if (!qml.rootObjects().isEmpty()) {
        if (auto* window = qobject_cast<QQuickWindow*>(qml.rootObjects().constFirst())) {
            taskbar.attach(window);
            // Without a handler Qt aborts when the graphics API cannot start;
            // fall back to a safer renderer and restart instead.
            QObject::connect(window, &QQuickWindow::sceneGraphError, &appController,
                             [&appController](QQuickWindow::SceneGraphError, const QString& message) {
                                 appController.handleSceneGraphError(message);
                             });
            // A few seconds of frames: the graphics settings work.
            QObject::connect(window, &QQuickWindow::frameSwapped, &appController, [&appController] {
                QTimer::singleShot(3000, &appController, &AppController::confirmStartup);
                QTimer::singleShot(4000, &appController, &AppController::releaseMemory); // start-up garbage
            }, Qt::SingleShotConnection);
        }
    }
    if (!files.isEmpty())
        QTimer::singleShot(0, &controller, [&controller, files] { controller.playPaths(files, 0); });

    const int status = app.exec();
    // Everything the user expects back next time is on disk before teardown.
    controller.flush();
    controller.closeOutput(); // DAC closed before it goes back to the system
    equalizer.flush();
    AppController::hideWindowsForExit();
    // Qt 6.11 can deadlock destroying a Vulkan window on Wayland right after a
    // resize; quitting must never hang, so give the teardown a few seconds.
    std::thread([status] {
        std::this_thread::sleep_for(std::chrono::seconds(3));
        std::_Exit(status);
    }).detach();
    return status;
}
