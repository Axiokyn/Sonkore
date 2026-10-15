#pragma once

#include <QAnimationDriver>
#include <QElapsedTimer>
#include <QPointer>
#include <QTimer>

class QQuickWindow;

// Animations stepped at the compositor's pace, for the software renderer on
// Wayland.
//
// With the GPU, Qt advances animations once per frame, in step with the
// display. The software renderer has no such driver: its animations tick on
// a timer of their own while frames go out when the compositor allows one
// (frame callbacks), and the two rhythms beat against each other. Scrolling
// a list then judders. This driver advances every animation when the
// compositor hands the window its turn (the UpdateRequest a frame callback
// delivers), so each frame shows the state for that moment.
// Measured on a 4K 144 Hz GNOME desktop scrolling the track list: about 92
// fps became 104, and frames slower than 11 ms went from 41 % to 25 %.
// (GNOME hands software-drawn windows their turns irregularly, 7 to 14 ms
// apart, so 144 steady frames are out of reach without the GPU.)
//
// When no turn comes (the window hidden or minimised), a slow timer keeps
// animation time moving so nothing waits on them forever.
// SONKORE_FRAME_DRIVER=0 in the environment turns it off.
class FrameAnimationDriver : public QAnimationDriver {
    Q_OBJECT
public:
    explicit FrameAnimationDriver(QQuickWindow* window);

    // Software renderer on Wayland: the case it is for (elsewhere the update
    // requests are not tied to the display and it would only spin).
    static bool wanted(QQuickWindow* window);

protected:
    bool eventFilter(QObject* watched, QEvent* event) override;
    void start() override;
    void stop() override;

private:
    void step();

    QPointer<QQuickWindow> m_window;
    QTimer m_watchdog;
    QElapsedTimer m_clock;
    qint64 m_lastStep = 0;
};
