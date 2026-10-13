pragma ComponentBehavior: Bound
import QtQuick
import Sonkore

// Three animated bars shown next to the track that is playing.
// Stepped by the shared Clock, on the frames the visualizer already draws: an
// Animation or a timer of its own would add frames of its own.
Row {
    id: root
    property bool running: true
    property color color: Theme.accent
    property real time: 0 // seconds, advanced while running
    spacing: 2
    height: 12

    Connections {
        target: Clock
        enabled: root.running
        function onTick(dt) { root.time += dt }
    }

    Repeater {
        model: 3
        Rectangle {
            id: bar
            required property int index
            // one rise and fall per period, a little slower for each bar
            readonly property real period: 0.56 + bar.index * 0.16
            width: 3
            radius: 1
            anchors.bottom: parent ? parent.bottom : undefined
            color: root.color
            height: 3 + 9 * (0.5 - 0.5 * Math.cos(2 * Math.PI * root.time / period))
        }
    }
}
