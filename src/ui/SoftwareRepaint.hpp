#pragma once

#include <QObject>
#include <QPointer>

#include <atomic>

class QQuickWindow;

// The software renderer's repaint area, chosen per frame.
//
// Qt's software renderer repaints only what changed, except at a fractional
// device pixel ratio (GNOME at 125 %, Windows at 150 %, Sonkore's own UI
// scale), where it repaints the whole window on every frame. A 4K window
// then costs a quarter of a CPU core just to move the visualizer.
// Sonkore asks for partial repaints anyway (QSG_SOFTWARE_RENDERER_FORCE_
// PARTIAL_UPDATES, set at startup) and this keeps the cases where much of
// the window changes fast: once a frame's repaint spread over more than half
// of the window or came in many pieces (a scrolling list gives hundreds of
// slivers, each a clip and a damage rectangle for the compositor), the next
// frames are repainted whole; every eighth frame is left to the renderer to
// see whether the movement is over.
// Measured on a 4K desktop at a ratio of 1.67, playing with the visualizer:
// 29-37 % of a core down to 9 %; scrolling 88-93 fps became 100-109 (with
// FrameAnimationDriver). Screenshots after scrolling and playing match a
// whole repaint pixel for pixel.
class SoftwareRepaint : public QObject {
    Q_OBJECT
public:
    explicit SoftwareRepaint(QQuickWindow* window);

    // Before the window exists: partial repaints for the software renderer,
    // unless the environment already says otherwise.
    static void prepare();
    // The software renderer draws this window.
    static bool wanted(QQuickWindow* window);

private:
    void beforeFrame(); // render thread (the GUI thread with the basic loop)
    void afterFrame();

    QPointer<QQuickWindow> m_window;
    std::atomic<bool> m_wholeFrames{false};
    int m_sinceProbe = 0;
    bool m_forced = false; // this frame was marked whole by us
};
