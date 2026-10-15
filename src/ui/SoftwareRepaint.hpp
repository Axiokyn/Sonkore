#pragma once

#include <QObject>
#include <QPointer>
#include <QRegion>

#include <atomic>

class QQuickWindow;
class QSGSoftwareRenderer;

// The software renderer's repaint area, chosen per frame.
//
// Qt's software renderer repaints only what changed, except at a fractional
// device pixel ratio (GNOME at 125 %, Windows at 150 %, Sonkore's own UI
// scale), where it repaints the whole window on every frame. A 4K window
// then costs a quarter of a CPU core just to move the visualizer.
// Sonkore asks for partial repaints anyway (QSG_SOFTWARE_RENDERER_FORCE_
// PARTIAL_UPDATES, set at startup) and handles what made Qt give them up:
// - at a fractional scale factor of Qt's own, the edges of a repainted area
//   are rounded differently by the painter, the backing store and the damage
//   sent to the compositor, which left stray lines and see-through pixels;
//   each frame's area is widened to a step whose edges all agree on
//   (snapToGrid);
// - once a frame's repaint spreads over more than half of the window or
//   comes in many pieces (a scrolling list gives hundreds of slivers, each a
//   clip and a damage rectangle for the compositor), the next frames are
//   repainted whole; every eighth frame is left to the renderer to see
//   whether the movement is over.
// Measured on a 4K desktop at a ratio of 1.67, playing with the visualizer:
// 29-37 % of a core down to 9 %; scrolling 88-93 fps became 100-109 (with
// FrameAnimationDriver). In a headless mutter at 100-200 % with interface
// scales of 1-1.75, the buffer and the compositor's copy (rebuilt from the
// damage) match a whole repaint pixel for pixel after playing, scrolling and
// items showing and hiding.
class SoftwareRepaint : public QObject {
    Q_OBJECT
public:
    explicit SoftwareRepaint(QQuickWindow* window);

    // Before the window exists: partial repaints for the software renderer,
    // unless the environment already says otherwise.
    static void prepare();
    // The software renderer draws this window.
    static bool wanted(QQuickWindow* window);
    // Every Qt Quick window, the ones shown later included (Settings, the
    // SoundTouch editor): partial repaints are asked for all of them.
    static void watchWindows();
    static void attach(QQuickWindow* window);

private:
    void beforeFrame(); // render thread (the GUI thread with the basic loop)
    void afterFrame();
    void snapToGrid(QSGSoftwareRenderer& renderer);

    QPointer<QQuickWindow> m_window;
    const bool m_canSnap;  // built against the running Qt's minor version
    std::atomic<bool> m_wholeFrames{false};
    int m_sinceProbe = 0;
    int m_grid = 1;        // see snapToGrid()
    bool m_forced = false; // this frame was marked whole by us
    QRegion m_snapped;     // what this frame was widened to
    QRegion m_mend;        // missed by the last frame: repainted on the next
};
