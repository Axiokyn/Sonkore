pragma Singleton
import QtQuick
import Sonkore

// One clock for everything that moves while music plays: the visualizer, the
// "playing" bars in the lists and the progress bars. Every change redraws the
// whole window, so they all move on the same tick: separate timers would land
// on different display frames and add up (60 + 30 + ... frames a second).
// Ticks on every display frame while playing and a moment after, so the
// visualizer falls to rest.
Item {
    id: clock

    property real time: 0 // seconds, advanced while running
    signal tick(real dt)

    property bool lingering: false
    // Held for a moment while the window resizes (Main: resizeWhenIdle), so
    // no frame of the old size is still being drawn when the size changes.
    property bool held: false
    readonly property bool running: (DspController.playing || lingering) && !held

    Connections {
        target: DspController
        function onStateChanged() {
            if (!DspController.playing && !clock.lingering) {
                clock.lingering = true
                linger.restart()
            }
        }
    }
    Timer {
        id: linger
        interval: 1500
        onTriggered: clock.lingering = false
    }

    function advance(dt) {
        time += dt
        tick(dt)
    }

    FrameAnimation {
        running: clock.running
        onTriggered: clock.advance(frameTime)
    }
}
