#pragma once

#include "QmlSingleton.hpp"

#include <QElapsedTimer>
#include <QHash>
#include <QObject>
#include <QTranslator>
#include <atomic>
#include <QVariantList>
#include <QString>
#include <QStringList>
#include <QWindow>
#include <QtQml/qqmlregistration.h>

// Application-level settings that need C++: interface scale and renderer
// (both applied before the GUI starts, so they need a restart), restart,
// opening folders in the file manager, window raise requests (MPRIS, second
// instance) and the start-up crash guard for graphics settings.
class AppController : public QObject {
    Q_OBJECT
    QML_NAMED_ELEMENT(App)
    QML_SINGLETON
    SONKORE_QML_SINGLETON_INSTANCE(AppController)

    Q_PROPERTY(double uiScale READ uiScale WRITE setUiScale NOTIFY settingsChanged)
    Q_PROPERTY(QString renderer READ renderer WRITE setRenderer NOTIFY settingsChanged)
    Q_PROPERTY(bool threadedRendering READ threadedRendering WRITE setThreadedRendering NOTIFY settingsChanged)
    Q_PROPERTY(bool restartRequired READ restartRequired NOTIFY settingsChanged)
    // Frames a second at most for every window (0 = the display's rate):
    // scrolling, animations and the visualizer alike. Applied at once.
    Q_PROPERTY(int frameLimit READ frameLimit WRITE setFrameLimit NOTIFY settingsChanged)
    // Set when the previous start crashed and the graphics settings were
    // rolled back (or in --safe-mode); shown once in the UI.
    Q_PROPERTY(QString recoveryMessage READ recoveryMessage CONSTANT)
    Q_PROPERTY(QString version READ version CONSTANT)
    Q_PROPERTY(QString platform READ platform CONSTANT)
    Q_PROPERTY(bool mprisAvailable READ mprisAvailable CONSTANT)
    // MPRIS on / off (Settings › System): the desktop's media controls, media
    // keys and playerctl; applied at once.
    Q_PROPERTY(bool mprisEnabled READ mprisEnabled WRITE setMprisEnabled NOTIFY mprisEnabledChanged)
    // The window was created with an alpha channel (rounded corners at
    // start-up), so its corners can be see-through.
    // Without it the window is opaque, which costs the compositor least.
    Q_PROPERTY(bool alphaWindow READ alphaWindow CONSTANT)
    // A tiling window manager (Sway, Hyprland, i3, niri…) lays the window
    // out: the integrated bar shows no window buttons and square corners.
    Q_PROPERTY(bool tilingWindowManager READ tilingWindowManager CONSTANT)
    // The window buttons the desktop shows ("minimize", "maximize",
    // "close"), from its button layout (GNOME, Cinnamon, KDE, Xfce); all
    // three when it can't tell.
    Q_PROPERTY(QStringList systemWindowButtons READ systemWindowButtons CONSTANT)
    // Interface language: "" follows the system, otherwise a code from
    // `languages` ({code, name}). Applied live.
    Q_PROPERTY(QString language READ language WRITE setLanguage NOTIFY languageChanged)
    Q_PROPERTY(QVariantList languages READ languages CONSTANT)
    // Interface font: "" is the system's, otherwise an id from `fonts`
    // ({id, name}; open fonts shipped in the binary). `fontFamily` is the
    // family to use ("" = default). Applied live.
    Q_PROPERTY(QString font READ font WRITE setFont NOTIFY fontChanged)
    Q_PROPERTY(QString fontFamily READ fontFamily NOTIFY fontChanged)
    Q_PROPERTY(QVariantList fonts READ fonts CONSTANT)

public:
    // No default argument: see SONKORE_QML_SINGLETON_INSTANCE.
    explicit AppController(QObject* parent);

    // Reads the persisted scale / renderer and applies them. Must run before
    // the QGuiApplication is constructed. When the previous start never got
    // as far as confirmStartup(), the renderer is rolled back step by step
    // (custom -> automatic -> software) so a bad choice cannot lock the user
    // out. `safeMode` (--safe-mode) uses the software renderer for this run
    // only.
    static void applyStartupSettings(bool safeMode);
    // This process exits before showing a window (it handed its files to a
    // running instance): undo the start-up check.
    static void abandonStartup();

    double uiScale() const { return m_uiScale; }
    void setUiScale(double scale);
    QString renderer() const { return m_renderer; }
    void setRenderer(const QString& renderer);
    bool threadedRendering() const { return m_threaded; }
    void setThreadedRendering(bool on);
    int frameLimit() const { return m_frameLimit; }
    void setFrameLimit(int fps);
    bool restartRequired() const;
    QString recoveryMessage() const;
    QString version() const;
    QString platform() const;
    bool mprisAvailable() const;
    bool mprisEnabled() const { return m_mprisEnabled; }
    void setMprisEnabled(bool on);
    bool alphaWindow() const { return s_appliedAlpha; }
    static bool tilingWindowManager();
    QStringList systemWindowButtons() const;
    static QStringList readSystemWindowButtons();
    QString language() const { return m_language; }
    void setLanguage(const QString& code);
    QVariantList languages() const;
    QString font() const { return m_font; }
    void setFont(const QString& id);
    QString fontFamily() const { return m_fontFamily; }
    QVariantList fonts() const;
    // The engine whose bindings are re-evaluated when the language changes.
    void setQmlEngine(class QQmlEngine* engine) { m_qmlEngine = engine; }

    Q_INVOKABLE void restart();
    // Erases every stored preference and restarts; library data, playlists,
    // the play queue and EQ profiles live in their own files and stay.
    Q_INVOKABLE void restoreDefaults();
    Q_INVOKABLE void openContainingFolder(const QString& path) const;
    Q_INVOKABLE void requestRaise() { emit raiseRequested(); }
    Q_INVOKABLE double refreshRate(QWindow* window) const;
    // FPS counter: counts the frames `window` really presents (frameSwapped,
    // on the render thread) without asking for any; takeFrameCount() returns
    // the count since the last call. A QML FrameAnimation would itself
    // redraw the window at the display rate and count its own ticks.
    Q_INVOKABLE void countFrames(QWindow* window, bool on);
    Q_INVOKABLE int takeFrameCount() { return m_frames.exchange(0, std::memory_order_relaxed); }
    // Hides every window before they are destroyed at exit, so the render
    // threads stop cleanly first (destroying a Wayland window while its
    // Vulkan swapchain is still presenting a resize can deadlock).
    static void hideWindowsForExit();
    // Returns freed heap pages to the system (glibc keeps them otherwise).
    Q_INVOKABLE static void releaseMemory();

public slots:
    // The window has been rendering for a while: the graphics setup works.
    void confirmStartup();
    // The scene graph could not start with the chosen API: fall back and restart.
    void handleSceneGraphError(const QString& message);

protected:
    // The frame limit: holds back each window's redraw requests.
    bool eventFilter(QObject* watched, QEvent* event) override;

signals:
    void settingsChanged();
    void raiseRequested();
    void languageChanged();
    void mprisEnabledChanged();
    void fontChanged();

private:
    double m_uiScale = 1.0;
    QString m_renderer;
    bool m_threaded = true;
    bool m_confirmed = false;
    std::atomic<int> m_frames{0};
    int m_frameLimit = 60;
    struct FramePacing {
        qint64 last = -1; // when its last frame was let through (ns)
        bool pending = false; // a held-back request is scheduled
    };
    QHash<QObject*, FramePacing> m_pacing;
    QElapsedTimer m_frameClock;
    QMetaObject::Connection m_frameConnection;
    QString m_language;
    bool m_mprisEnabled = true;
    QString m_font;
    QString m_fontFamily;
    QString loadFont(const QString& id) const; // family name, "" if unknown
    QTranslator m_appTranslator;
    QTranslator m_qtTranslator;
    class QQmlEngine* m_qmlEngine = nullptr;
    void installTranslators();

    static inline double s_appliedScale = 1.0;
    static inline QString s_appliedRenderer;
    static inline bool s_appliedThreaded = true;
    static inline bool s_safeMode = false;
    static inline bool s_appliedAlpha = false;
    static inline int s_failedStarts = 0;
    static inline int s_recoveryKind = 0; // 0 none · 1 safe mode · 2 rolled back
    static inline QString s_recoveryRenderer;
    static inline bool s_recoveryThreaded = true;
};
