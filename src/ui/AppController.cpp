#include "AppController.hpp"

#include "SoftwareRepaint.hpp"
#include "SettingsLock.hpp"
#include "AppPaths.hpp"
#include <QJsonDocument>
#include <QJsonObject>
#include <QVariantMap>
#include <QTranslator>
#include <QQmlEngine>
#include <QLocale>
#include <QLibraryInfo>

#include <QCoreApplication>
#include <QDesktopServices>
#include <QGuiApplication>
#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QFontDatabase>
#include <QHash>
#include <QMouseEvent>
#include <QProcess>
#include <QQuickItem>
#include <QQuickWindow>
#include <QRegularExpression>
#include <QScreen>
#include <QSettings>
#include <QStandardPaths>
#include <QTimer>
#include <QUrl>

#include <algorithm>
#include <chrono>
#include <cmath>

#if defined(__GLIBC__)
#include <malloc.h>
#endif

namespace {

// Variables this process put into its own environment. A restart must not
// pass them on, or the new process would take them for user overrides.
QList<QByteArray> g_ownEnvironment;

void setOwnEnv(const char* name, const QByteArray& value)
{
    qputenv(name, value);
    g_ownEnvironment << QByteArray(name);
}

QString rendererName(const QString& renderer)
{
    if (renderer == QLatin1String("opengl"))
        return QStringLiteral("OpenGL");
    if (renderer == QLatin1String("vulkan"))
        return QStringLiteral("Vulkan");
    if (renderer == QLatin1String("software"))
        return AppController::tr("Software");
    return AppController::tr("Automatic");
}

#if defined(Q_OS_LINUX)
// PCI vendor ids of the GPUs the kernel drives (/sys/class/drm/cardN).
QStringList gpuVendors()
{
    QStringList vendors;
    static const QRegularExpression cardName(QStringLiteral("^card\\d+$"));
    const QDir drm(QStringLiteral("/sys/class/drm"));
    for (const QString& card : drm.entryList(QDir::Dirs | QDir::System | QDir::NoDotAndDotDot)) {
        if (!cardName.match(card).hasMatch())
            continue;
        QFile f(drm.filePath(card + QStringLiteral("/device/vendor")));
        if (f.open(QIODevice::ReadOnly))
            vendors << QString::fromLatin1(f.readAll()).trimmed().toLower();
    }
    vendors.removeDuplicates();
    return vendors;
}

QString findEglVendorManifest(const QString& nameFragment)
{
    QStringList dirs{QStringLiteral("/etc/glvnd/egl_vendor.d")};
    const QStringList dataDirs = qEnvironmentVariable("XDG_DATA_DIRS", QStringLiteral("/usr/local/share:/usr/share")).split(u':', Qt::SkipEmptyParts);
    for (const QString& dir : dataDirs)
        dirs << dir + QStringLiteral("/glvnd/egl_vendor.d");
    for (const QString& dir : dirs)
        for (const QString& manifest : QDir(dir).entryList({QStringLiteral("*.json")}, QDir::Files))
            if (manifest.contains(nameFragment, Qt::CaseInsensitive))
                return QDir(dir).filePath(manifest);
    return {};
}
#endif

// The graphics loaders open every installed driver to find one that works:
// the Vulkan loader loads all ICDs (Mesa's radv and lavapipe each map LLVM,
// ~25 MB), and with only NVIDIA's driver in use, GLVND still starts Mesa's
// EGL. Limit both to the drivers of the GPUs actually present. Nothing is
// restricted on unknown hardware, when the environment already configures
// the loaders, or after a failed start.
void loadOnlyPresentGpuDrivers(bool vulkan)
{
#if defined(Q_OS_LINUX)
    const QStringList vendors = gpuVendors();
    if (vendors.isEmpty())
        return;
    QStringList vulkanDrivers;
    for (const QString& vendor : vendors) {
        if (vendor == QLatin1String("0x10de"))
            vulkanDrivers << QStringLiteral("*nvidia*") << QStringLiteral("*nouveau*");
        else if (vendor == QLatin1String("0x1002"))
            vulkanDrivers << QStringLiteral("*radeon*") << QStringLiteral("*amd*");
        else if (vendor == QLatin1String("0x8086"))
            vulkanDrivers << QStringLiteral("*intel*");
        else
            return; // a GPU we have no mapping for: leave the loaders alone
    }

    if (vulkan) {
        for (const char* var : {"VK_DRIVER_FILES", "VK_ICD_FILENAMES", "VK_LOADER_DRIVERS_SELECT", "VK_LOADER_DRIVERS_DISABLE"})
            if (!qEnvironmentVariableIsEmpty(var))
                return;
        setOwnEnv("VK_LOADER_DRIVERS_SELECT", vulkanDrivers.join(u',').toLatin1());
        return;
    }

    // OpenGL: only a lone NVIDIA GPU on NVIDIA's own driver has a single
    // obvious EGL vendor (hybrid laptops keep both).
    if (vendors != QStringList{QStringLiteral("0x10de")} || !QFileInfo::exists(QStringLiteral("/proc/driver/nvidia/version")))
        return;
    if (!qEnvironmentVariableIsEmpty("__EGL_VENDOR_LIBRARY_FILENAMES") || !qEnvironmentVariableIsEmpty("__EGL_VENDOR_LIBRARY_DIRS"))
        return;
    const QString manifest = findEglVendorManifest(QStringLiteral("nvidia"));
    if (!manifest.isEmpty())
        setOwnEnv("__EGL_VENDOR_LIBRARY_FILENAMES", QFile::encodeName(manifest));
#else
    Q_UNUSED(vulkan)
#endif
}

// Compiled GPU pipelines are kept between runs, so the first frame need not
// wait for the shader compiler. Qt ignores a cache written by another
// driver/GPU. Not with NVIDIA's own driver alone: it keeps its own shader
// cache (~/.cache/nvidia), and Qt's copy on top was measured to change
// neither memory (libnvidia-gpucomp stays mapped, 62 MB) nor start-up.
void usePipelineCache(const QString& api, bool discard)
{
    if (!qEnvironmentVariableIsEmpty("QSG_RHI_PIPELINE_CACHE_LOAD") || !qEnvironmentVariableIsEmpty("QSG_RHI_PIPELINE_CACHE_SAVE"))
        return;
#if defined(Q_OS_LINUX)
    if (gpuVendors() == QStringList{QStringLiteral("0x10de")} && QFileInfo::exists(QStringLiteral("/proc/driver/nvidia/version")))
        return;
#endif
    const QString dir = sonkore::paths::cacheDir();
    if (dir.isEmpty() || !QDir().mkpath(dir))
        return;
    const QString file = dir + QStringLiteral("/pipelines-%1.bin").arg(api);
    if (discard)
        QFile::remove(file); // the last start failed: do not feed it to the driver again
    else if (QFileInfo::exists(file))
        setOwnEnv("QSG_RHI_PIPELINE_CACHE_LOAD", QFile::encodeName(file));
    setOwnEnv("QSG_RHI_PIPELINE_CACHE_SAVE", QFile::encodeName(file));
}

} // namespace

AppController::AppController(QObject* parent)
    : QObject(parent)
{
    QSettings s;
    m_uiScale = s.value("appearance/uiScale", 1.0).toDouble();
    m_renderer = s.value("advanced/renderer", "software").toString();
    m_threaded = s.value("advanced/threadedRendering", true).toBool();
    m_frameLimit = std::max(0, s.value("advanced/frameLimit", 60).toInt());
    m_frameClock.start();
    qApp->installEventFilter(this);
    m_language = s.value("appearance/language").toString();
    m_mprisEnabled = s.value("system/mpris", true).toBool();
    installTranslators();
    // Sonkore Sans unless a font (or the system's, "") was picked.
    m_font = s.value("appearance/font", QStringLiteral("sonkore-sans")).toString();
    if (m_font == QLatin1String("inter")) // no longer bundled: Sonkore Sans is derived from it
        m_font = QStringLiteral("sonkore-sans");
    m_fontFamily = loadFont(m_font);
    if (m_fontFamily.isEmpty())
        m_font.clear();
    applyFont();
    connect(qApp, &QCoreApplication::aboutToQuit, this, &AppController::confirmStartup);
}

void AppController::applyStartupSettings(bool safeMode)
{
    QSettings s;
    s_appliedScale = std::clamp(s.value("appearance/uiScale", 1.0).toDouble(), 0.5, 3.0);
    QString renderer = s.value("advanced/renderer", "software").toString();
    bool threaded = s.value("advanced/threadedRendering", true).toBool();

    // Crash guard: the last start never confirmed that rendering works.
    const int failedStarts = s.value("advanced/startupPending", 0).toInt();
    s_failedStarts = failedStarts;
    if (safeMode) {
        s_safeMode = true;
        renderer = QStringLiteral("software");
        s_recoveryKind = 1;
    } else if (failedStarts > 0) {
        // One step down the ladder per failed start:
        // custom renderer -> automatic -> automatic without the render thread -> software.
        if (renderer != QLatin1String("auto") && renderer != QLatin1String("software"))
            renderer = QStringLiteral("auto");
        else if (threaded && renderer != QLatin1String("software"))
            threaded = false;
        else
            renderer = QStringLiteral("software");
        s.setValue("advanced/renderer", renderer);
        s.setValue("advanced/threadedRendering", threaded);
        s_recoveryKind = 2;
        s_recoveryRenderer = renderer;
        s_recoveryThreaded = threaded;
    }
    s.setValue("advanced/startupPending", failedStarts + 1);
    s.sync();

    s_appliedRenderer = s.value("advanced/renderer", "software").toString();
    s_appliedThreaded = threaded;

    // Earlier versions let the desktop show through the window; gone now.
    s.remove("appearance/translucency");
    s.remove("appearance/transparency");
    s.remove("appearance/coverTint");
    s.remove("appearance/vibrancy");
    // Frame-rate settings of a test build, gone.
    s.remove("advanced/maxFps");
    s.remove("ui/visualizerFps");
    // The title bar is always the integrated one; the window-button order is fixed.
    s.remove("ui/titleBar");
    s.remove("appearance/buttonStyle");
    s.remove("appearance/windowButtonOrder");
    // "List" moved from the interface density to its own track-list setting.
    if (s.value("appearance/density").toString() == QLatin1String("list")) {
        s.setValue("appearance/density", QStringLiteral("auto"));
        s.setValue("appearance/listDensity", QStringLiteral("list"));
    }
    // The animated aurora is gone too: keep its look, static.
    if (s.value("appearance/backdrop").toString() == QLatin1String("aurora-live"))
        s.setValue("appearance/backdrop", QStringLiteral("aurora"));

    // The rounded corners of the integrated title bar need a window with an
    // alpha channel, which must be chosen before the window exists. Only then:
    // opaque windows are cheaper to composite.
    // The mini player may round its corners on its own (Theme.compactStore).
    const QJsonObject compactLook = QJsonDocument::fromJson(s.value("appearanceCompact/values").toString().toUtf8()).object();
    const bool rounded = s.value("appearance/windowRadius", 8).toInt() > 0
                         || compactLook.value(QStringLiteral("windowRadius")).toInt() > 0;
    s_appliedAlpha = !safeMode && rounded && !tilingWindowManager();
    if (s_appliedAlpha)
        QQuickWindow::setDefaultAlphaBuffer(true);

    // An explicit environment always wins over the stored preference.
    if (qEnvironmentVariableIsEmpty("QT_SCALE_FACTOR") && std::abs(s_appliedScale - 1.0) > 1e-3)
        setOwnEnv("QT_SCALE_FACTOR", QByteArray::number(s_appliedScale, 'f', 2));

    // Qt picks the single-threaded "basic" loop on some Linux drivers (e.g.
    // NVIDIA + Wayland), which caps animations at roughly half of a 144 Hz
    // display. The threaded loop renders at the full refresh rate.
    // The software renderer always uses the basic loop: its threaded variant
    // paints into the Wayland shared-memory buffer while the GUI thread
    // replaces it on resize, which crashes.
    const bool software = renderer == QLatin1String("software");
    if (qEnvironmentVariableIsEmpty("QSG_RENDER_LOOP"))
        setOwnEnv("QSG_RENDER_LOOP", threaded && !software ? "threaded" : "basic");

    if (qEnvironmentVariableIsEmpty("QSG_RHI_BACKEND") && qEnvironmentVariableIsEmpty("QT_QUICK_BACKEND")) {
        if (renderer == QLatin1String("opengl"))
            QQuickWindow::setGraphicsApi(QSGRendererInterface::OpenGL);
        else if (renderer == QLatin1String("vulkan"))
            QQuickWindow::setGraphicsApi(QSGRendererInterface::Vulkan);
        else if (software)
            QQuickWindow::setGraphicsApi(QSGRendererInterface::Software);
        if (software)
            SoftwareRepaint::prepare(); // partial repaints also at fractional scales
        else // glyphs rasterized and hinted to the pixel grid, as the software renderer does
            QQuickWindow::setTextRenderType(QQuickWindow::NativeTextRendering);
        if (!software && failedStarts == 0 && !safeMode)
            loadOnlyPresentGpuDrivers(renderer == QLatin1String("vulkan"));
        if (!software && !safeMode)
            usePipelineCache(renderer == QLatin1String("vulkan") ? QStringLiteral("vulkan") : QStringLiteral("opengl"), failedStarts > 0);
    }
}

void AppController::abandonStartup()
{
    QSettings s;
    if (s_failedStarts > 0)
        s.setValue("advanced/startupPending", s_failedStarts);
    else
        s.remove("advanced/startupPending");
}

void AppController::confirmStartup()
{
    if (m_confirmed)
        return;
    m_confirmed = true;
    QSettings s;
    s.remove("advanced/startupPending");
    s.sync();
}

void AppController::handleSceneGraphError(const QString& message)
{
    qWarning("Sonkore: the renderer failed to start: %s", qPrintable(message));
    if (s_safeMode || s_appliedRenderer == QLatin1String("software")) {
        QCoreApplication::exit(1); // nothing safer to fall back to
        return;
    }
    QSettings s;
    s.setValue("advanced/renderer", s_appliedRenderer == QLatin1String("auto") ? "software" : "auto");
    s.sync();
    restart();
}

void AppController::setUiScale(double scale)
{
    scale = std::clamp(std::round(scale * 100.0) / 100.0, 0.5, 3.0);
    if (scale == m_uiScale)
        return;
    m_uiScale = scale;
    QSettings().setValue("appearance/uiScale", scale);
    emit settingsChanged();
}

void AppController::setRenderer(const QString& renderer)
{
    if (renderer == m_renderer)
        return;
    m_renderer = renderer;
    QSettings().setValue("advanced/renderer", renderer);
    emit settingsChanged();
}

void AppController::setFrameLimit(int fps)
{
    fps = std::max(0, fps);
    if (fps == m_frameLimit)
        return;
    m_frameLimit = fps;
    QSettings().setValue("advanced/frameLimit", fps);
    emit settingsChanged();
}

// A click outside the text field being typed in ends the typing, as in
// desktop apps: the field lets go of the keyboard (its highlighted edge goes,
// its blinking caret stops redrawing the window, Space plays again). Qt Quick
// leaves the focus where it was when the click lands on something that does
// not take it, such as a list or an empty spot.
static void releaseTextFocus(QQuickWindow* window, const QPointF& scenePos)
{
    QQuickItem* item = window->activeFocusItem();
    if (!item || !(item->inherits("QQuickTextInput") || item->inherits("QQuickTextEdit")))
        return;
    if (!item->mapRectToScene(item->boundingRect()).contains(scenePos))
        item->setFocus(false);
}

// Every redraw of a window starts as an UpdateRequest event, whatever asked
// for it (scrolling, an animation, the visualizer) and whichever render loop
// draws it. One that comes sooner than the limit allows is held back and
// sent again when its turn comes; requests in between fold into it.
bool AppController::eventFilter(QObject* watched, QEvent* event)
{
    if (event->type() == QEvent::MouseButtonPress) {
        if (auto* window = qobject_cast<QQuickWindow*>(watched))
            releaseTextFocus(window, static_cast<QMouseEvent*>(event)->scenePosition());
        return false;
    }
    if (event->type() != QEvent::UpdateRequest || m_frameLimit <= 0 || !watched->isWindowType())
        return false;
    if (!m_pacing.contains(watched))
        connect(watched, &QObject::destroyed, this, [this, watched] { m_pacing.remove(watched); });
    FramePacing& pacing = m_pacing[watched];
    const qint64 interval = 1'000'000'000LL / m_frameLimit;
    const qint64 now = m_frameClock.nsecsElapsed();
    const qint64 due = pacing.last + interval;
    if (pacing.last >= 0 && due - now > 500'000) { // early by more than half a millisecond
        if (!pacing.pending) {
            pacing.pending = true;
            const auto wait = std::chrono::milliseconds((due - now + 999'999) / 1'000'000);
            QTimer::singleShot(wait, Qt::PreciseTimer, this, [this, watched] {
                if (!m_pacing.contains(watched))
                    return; // the window is gone
                m_pacing[watched].pending = false;
                QCoreApplication::postEvent(watched, new QEvent(QEvent::UpdateRequest));
            });
        }
        return true;
    }
    // Steady frames keep their cadence (a late timer does not push the next
    // one back); after a pause the count starts again from now.
    pacing.last = pacing.last >= 0 && now - due < interval ? std::max(due, now - interval / 2) : now;
    return false;
}

void AppController::setThreadedRendering(bool on)
{
    if (on == m_threaded)
        return;
    m_threaded = on;
    QSettings().setValue("advanced/threadedRendering", on);
    emit settingsChanged();
}

bool AppController::restartRequired() const
{
    if (s_safeMode)
        return false;
    const bool threadedApplies = m_renderer != QLatin1String("software");
    return std::abs(m_uiScale - s_appliedScale) > 1e-3 || m_renderer != s_appliedRenderer
        || (threadedApplies && m_threaded != s_appliedThreaded);
}

QString AppController::version() const
{
    return QCoreApplication::applicationVersion();
}

QString AppController::platform() const
{
#if defined(Q_OS_WIN)
    return QStringLiteral("windows");
#elif defined(Q_OS_MACOS)
    return QStringLiteral("macos");
#else
    return QStringLiteral("linux");
#endif
}

bool AppController::tilingWindowManager()
{
    // SONKORE_TILING=1 / 0 forces it either way.
    const QByteArray forced = qgetenv("SONKORE_TILING");
    if (!forced.isEmpty())
        return forced != "0";
    for (const char* socket : {"SWAYSOCK", "HYPRLAND_INSTANCE_SIGNATURE", "I3SOCK", "NIRI_SOCKET"}) {
        if (!qEnvironmentVariableIsEmpty(socket))
            return true;
    }
    static const QStringList tilers = {
        QStringLiteral("sway"), QStringLiteral("hyprland"), QStringLiteral("i3"), QStringLiteral("niri"),
        QStringLiteral("river"), QStringLiteral("qtile"), QStringLiteral("bspwm"), QStringLiteral("dwm"),
        QStringLiteral("dwl"), QStringLiteral("leftwm"), QStringLiteral("herbstluftwm"), QStringLiteral("xmonad"),
    };
    const QStringList desktops = qEnvironmentVariable("XDG_CURRENT_DESKTOP").toLower().split(u':', Qt::SkipEmptyParts);
    return std::any_of(desktops.cbegin(), desktops.cend(), [](const QString& d) { return tilers.contains(d); });
}

QStringList AppController::systemWindowButtons() const
{
    // Read once: the desktop's layout rarely changes while the app runs.
    static const QStringList cached = readSystemWindowButtons();
    return cached;
}

QStringList AppController::readSystemWindowButtons()
{
    static const QStringList all = {QStringLiteral("minimize"), QStringLiteral("maximize"), QStringLiteral("close")};
    const auto run = [](const QString& program, const QStringList& args) {
        QProcess p;
        p.start(program, args, QIODevice::ReadOnly);
        if (!p.waitForFinished(500) || p.exitStatus() != QProcess::NormalExit || p.exitCode() != 0)
            return QString();
        return QString::fromUtf8(p.readAllStandardOutput()).trimmed();
    };
    // Each desktop names the buttons its own way; `names` maps them to ours.
    const auto pick = [](const QString& layout, const QHash<QString, QString>& names, bool letters) {
        QStringList shown;
        const QStringList tokens = letters ? layout.split(QString(), Qt::SkipEmptyParts)
                                           : layout.split(QRegularExpression(QStringLiteral("[:,]")), Qt::SkipEmptyParts);
        for (const QString& t : tokens) {
            const QString ours = names.value(t.trimmed());
            if (!ours.isEmpty() && !shown.contains(ours))
                shown << ours;
        }
        QStringList ordered;
        for (const QString& b : all) {
            if (shown.contains(b))
                ordered << b;
        }
        return ordered;
    };

    const QString desktop = qEnvironmentVariable("XDG_CURRENT_DESKTOP").toLower();
    QStringList buttons;
    bool known = false;
    if (desktop.contains(QLatin1String("kde"))) {
        // KWin: letters, I minimize, A maximize, X close (defaults "M" and "IAX").
        const QSettings kwin(QStandardPaths::writableLocation(QStandardPaths::GenericConfigLocation)
                                 + QStringLiteral("/kwinrc"), QSettings::IniFormat);
        const QString layout = kwin.value(QStringLiteral("org.kde.kdecoration2/ButtonsOnLeft"), QStringLiteral("M")).toString()
                               + kwin.value(QStringLiteral("org.kde.kdecoration2/ButtonsOnRight"), QStringLiteral("IAX")).toString();
        buttons = pick(layout, {{"I", "minimize"}, {"A", "maximize"}, {"X", "close"}}, true);
        known = true;
    } else if (desktop.contains(QLatin1String("xfce"))) {
        // xfwm4: letters, H hide, M maximize, C close ("O|HMC" by default).
        const QString layout = run(QStringLiteral("xfconf-query"), {QStringLiteral("-c"), QStringLiteral("xfwm4"),
                                                                    QStringLiteral("-p"), QStringLiteral("/general/button_layout")});
        if (!layout.isEmpty()) {
            buttons = pick(layout, {{"H", "minimize"}, {"M", "maximize"}, {"C", "close"}}, true);
            known = true;
        }
    } else {
        // GNOME and the desktops built on its settings: "appmenu:minimize,maximize,close".
        const QString schema = desktop.contains(QLatin1String("cinnamon")) ? QStringLiteral("org.cinnamon.desktop.wm.preferences")
                                                                           : QStringLiteral("org.gnome.desktop.wm.preferences");
        QString layout = run(QStringLiteral("gsettings"), {QStringLiteral("get"), schema, QStringLiteral("button-layout")});
        layout.remove(u'\'');
        if (layout.contains(u':')) {
            buttons = pick(layout, {{"minimize", "minimize"}, {"maximize", "maximize"}, {"close", "close"}}, false);
            known = true;
        }
    }
    if (!known)
        return all;
    // A layout without any button still needs a way to close the window.
    return buttons.isEmpty() ? QStringList{QStringLiteral("close")} : buttons;
}

bool AppController::mprisAvailable() const
{
#if defined(SONKORE_HAVE_MPRIS)
    return true;
#else
    return false;
#endif
}

void AppController::setMprisEnabled(bool on)
{
    if (on == m_mprisEnabled)
        return;
    m_mprisEnabled = on;
    QSettings().setValue("system/mpris", on);
    emit mprisEnabledChanged();
}

void AppController::restart()
{
    confirmStartup(); // this run worked; the new one starts its own check
    QSettings().sync();
    QProcessEnvironment env = QProcessEnvironment::systemEnvironment();
    for (const QByteArray& name : std::as_const(g_ownEnvironment))
        env.remove(QString::fromLatin1(name));
    QProcess process;
    process.setProgram(QCoreApplication::applicationFilePath());
    process.setProcessEnvironment(env);
    process.startDetached();
    QCoreApplication::quit();
}

void AppController::restoreDefaults()
{
    sonkore::g_settingsFrozen.store(true); // nothing writes the old values back on the way out
    QSettings s;
    s.clear();
    s.sync();
    restart();
}

double AppController::refreshRate(QWindow* window) const
{
    return window && window->screen() ? window->screen()->refreshRate() : 0.0;
}

void AppController::countFrames(QWindow* window, bool on)
{
    disconnect(m_frameConnection);
    m_frames.store(0, std::memory_order_relaxed);
    auto* quick = qobject_cast<QQuickWindow*>(window);
    if (on && quick)
        m_frameConnection = connect(quick, &QQuickWindow::frameSwapped, this,
                                    [this] { m_frames.fetch_add(1, std::memory_order_relaxed); }, Qt::DirectConnection);
}

void AppController::openContainingFolder(const QString& path) const
{
    const QFileInfo fi(path);
    QDesktopServices::openUrl(QUrl::fromLocalFile(fi.isDir() ? fi.absoluteFilePath() : fi.absolutePath()));
}

void AppController::hideWindowsForExit()
{
    for (QWindow* window : QGuiApplication::topLevelWindows())
        if (window->isVisible())
            window->hide();
    QCoreApplication::processEvents();
}

void AppController::releaseMemory()
{
#if defined(__GLIBC__)
    malloc_trim(0);
#endif
}

// --- Font -------------------------------------------------------------------------

// Open fonts (SIL OFL) shipped in the binary: ":/fonts/<id>/<weight>.ttf",
// regular to bold. Loaded only when picked.
static const struct { const char* id; const char* name; } kFonts[] = {
    {"sonkore-sans", "Sonkore Sans"}, {"manrope", "Manrope"},
    {"plus-jakarta-sans", "Plus Jakarta Sans"},
    {"figtree", "Figtree"}, {"outfit", "Outfit"}, {"dm-sans", "DM Sans"},
    {"nunito-sans", "Nunito Sans"}, {"rubik", "Rubik"},
};

QVariantList AppController::fonts() const
{
    QVariantList out;
    for (const auto& f : kFonts)
        out << QVariantMap{{"id", QString::fromLatin1(f.id)}, {"name", QString::fromUtf8(f.name)}};
    return out;
}

QString AppController::loadFont(const QString& id) const
{
    if (id.isEmpty())
        return {};
    const bool known = std::any_of(std::begin(kFonts), std::end(kFonts),
                                   [&id](const auto& f) { return id == QLatin1String(f.id); });
    if (!known)
        return {};
    // Each weight registers once per process; the family name comes from
    // the files themselves.
    static QHash<QString, QString> loaded;
    if (const auto it = loaded.constFind(id); it != loaded.cend())
        return *it;
    QString family;
    for (int weight : {400, 500, 600, 700}) {
        const int handle = QFontDatabase::addApplicationFont(QStringLiteral(":/fonts/%1/%2.ttf").arg(id).arg(weight));
        const QStringList families = QFontDatabase::applicationFontFamilies(handle);
        if (family.isEmpty() && !families.isEmpty())
            family = families.constFirst();
    }
    loaded.insert(id, family);
    return family;
}

// The application font (what Text items and the windows start from): the
// chosen family, hinted to the pixel grid. Small text then has whole-pixel
// stems instead of the soft two-pixel ones of GNOME's light hinting; on
// Windows DirectWrite renders it like GDI. Sonkore Sans carries its own
// hinting instructions (ttfautohint); other fonts get FreeType's.
void AppController::applyFont()
{
    static const QFont system = QGuiApplication::font(); // before any change of ours
    QFont font = system;
    if (!m_fontFamily.isEmpty())
        font.setFamilies({m_fontFamily});
    font.setHintingPreference(QFont::PreferFullHinting);
    QGuiApplication::setFont(font);
}

void AppController::setFont(const QString& id)
{
    if (id == m_font)
        return;
    const QString family = loadFont(id);
    m_font = family.isEmpty() ? QString() : id;
    m_fontFamily = family;
    QSettings().setValue("appearance/font", m_font);
    applyFont();
    emit fontChanged();
}

// --- Language ---------------------------------------------------------------------

// Translations shipped in the binary (":/i18n/sonkore_<code>.qm").
static const struct { const char* code; const char* name; } kLanguages[] = {
    {"en", "English"}, {"es", "Español"}, {"pt_BR", "Português (Brasil)"},
    {"fr", "Français"}, {"de", "Deutsch"}, {"it", "Italiano"},
};

QVariantList AppController::languages() const
{
    QVariantList out;
    for (const auto& l : kLanguages)
        out << QVariantMap{{"code", QString::fromLatin1(l.code)}, {"name", QString::fromUtf8(l.name)}};
    return out;
}

void AppController::installTranslators()
{
    qApp->removeTranslator(&m_appTranslator);
    qApp->removeTranslator(&m_qtTranslator);
    const QLocale locale = m_language.isEmpty() ? QLocale::system() : QLocale(m_language);
    QLocale::setDefault(locale);
    // QTranslator::load(QLocale) tries "es_AR", then "es": a regional system
    // locale finds the language's translation. English is the source text.
    if (m_appTranslator.load(locale, QStringLiteral("sonkore"), QStringLiteral("_"), QStringLiteral(":/i18n")))
        qApp->installTranslator(&m_appTranslator);
    // Qt's own strings (standard dialogs, context menus of text fields).
    if (m_qtTranslator.load(locale, QStringLiteral("qtbase"), QStringLiteral("_"),
                            QLibraryInfo::path(QLibraryInfo::TranslationsPath)))
        qApp->installTranslator(&m_qtTranslator);
}

void AppController::setLanguage(const QString& code)
{
    if (code == m_language)
        return;
    m_language = code;
    QSettings().setValue("appearance/language", code);
    installTranslators();
    if (m_qmlEngine)
        m_qmlEngine->retranslate();
    emit languageChanged();
}

QString AppController::recoveryMessage() const
{
    switch (s_recoveryKind) {
    case 1:
        return tr("Safe mode: Sonkore is using the software renderer for this session. Your graphics settings were not changed.");
    case 2:
        return tr("Sonkore closed unexpectedly while starting, so the graphics settings were reset (renderer: %1%2). You can change them again in Settings › Performance.")
            .arg(rendererName(s_recoveryRenderer), s_recoveryThreaded ? QString() : tr(", without the render thread"));
    default:
        return {};
    }
}
