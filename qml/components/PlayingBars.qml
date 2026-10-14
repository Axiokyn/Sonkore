import QtQuick
import Sonkore

// Three animated bars shown next to the track that is playing, drawn like the
// visualizer (SpectrumView: anti-aliased, smooth with every renderer).
// Stepped by the shared Clock, on the frames the visualizer already draws: an
// Animation or a timer of its own would add frames of its own.
SpectrumView {
    id: root
    property bool running: true
    property real time: 0 // seconds, advanced while running

    color: Theme.accent
    shaded: false
    barFraction: 3 / (13 / 3) // 3 px bars, 2 px apart
    minimumHeight: 3
    implicitWidth: 13
    implicitHeight: 12
    // One rise and fall per period, a little slower for each bar.
    levels: [0, 1, 2].map(i => (3 + 9 * (0.5 - 0.5 * Math.cos(2 * Math.PI * root.time / (0.56 + i * 0.16)))) / 12)

    Connections {
        target: Clock
        enabled: root.running
        function onTick(dt) { root.time += dt }
    }
}
