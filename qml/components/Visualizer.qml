pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Shapes
import Sonkore

// Live spectrum of what is playing (24 bands from DspController.spectrum),
// smoothed on every Clock tick: fast attack, gentle fall. Styles: bars,
// mirror, line, dots. Costs nothing while hidden or silent.
Item {
    id: viz

    property string style: "bars"
    property color color: Theme.accent
    readonly property int bands: 24
    property var levels: new Array(bands).fill(0)
    property real loudest: 0

    implicitWidth: 96
    implicitHeight: 36

    // Asks the engine for the spectrum only while something can be seen.
    Binding {
        target: DspController
        property: "visualizerActive"
        value: viz.visible && viz.width > 0
    }

    // Moved by the shared Clock, on the same frames as everything else that
    // moves.
    readonly property bool animating: viz.visible && (DspController.playing || viz.loudest > 0.005)
    Connections {
        target: Clock
        enabled: viz.animating
        function onTick(dt) { viz.advance(dt) }
    }

    // Moves the bars `frameTime` seconds towards the spectrum.
    function advance(frameTime) {
        const target = DspController.spectrum
        const out = viz.levels.slice()
        const rise = Math.min(1, frameTime * 30)
        const fall = Math.min(1, frameTime * 6)
        let loudest = 0
        for (let i = 0; i < viz.bands; ++i) {
            const t = target[i] || 0
            out[i] += (t - out[i]) * (t > out[i] ? rise : fall)
            loudest = Math.max(loudest, out[i])
        }
        viz.levels = out
        viz.loudest = loudest
    }

    readonly property real step: width / bands
    readonly property real barWidth: Math.max(1, step * 0.62)

    // --- bars / mirror ----------------------------------------------------------
    Repeater {
        model: viz.style === "bars" || viz.style === "mirror" ? viz.bands : 0
        Rectangle {
            required property int index
            readonly property real level: viz.levels[index] || 0
            x: index * viz.step + (viz.step - viz.barWidth) / 2
            width: viz.barWidth
            height: Math.max(2, level * viz.height)
            y: viz.style === "mirror" ? (viz.height - height) / 2 : viz.height - height
            radius: Math.min(width / 2, 2)
            color: viz.color
            opacity: 0.45 + 0.55 * level
        }
    }

    // --- dots: LED columns ---------------------------------------------------
    Repeater {
        model: viz.style === "dots" ? viz.bands : 0
        Column {
            id: dotColumn
            required property int index
            readonly property int lit: Math.round((viz.levels[index] || 0) * 6)
            x: index * viz.step + (viz.step - viz.barWidth) / 2
            anchors.bottom: parent.bottom
            spacing: Math.max(1, (viz.height - 6 * viz.barWidth) / 5)
            Repeater {
                model: 6
                Rectangle {
                    required property int index
                    width: viz.barWidth
                    height: viz.barWidth
                    radius: width / 2
                    color: viz.color
                    opacity: 5 - index < dotColumn.lit ? 1 : 0.14
                }
            }
        }
    }

    // --- line: filled curve ---------------------------------------------------
    Shape {
        id: lineShape
        visible: viz.style === "line"
        anchors.fill: parent
        preferredRendererType: Shape.CurveRenderer
        readonly property var points: {
            const pts = [Qt.point(0, viz.height)]
            for (let i = 0; i < viz.bands; ++i)
                pts.push(Qt.point((i + 0.5) * viz.step, viz.height - 1 - (viz.levels[i] || 0) * (viz.height - 2)))
            pts.push(Qt.point(viz.width, viz.height))
            return pts
        }
        ShapePath {
            strokeWidth: 0
            strokeColor: "transparent"
            fillColor: Qt.rgba(viz.color.r, viz.color.g, viz.color.b, 0.22)
            PathPolyline { path: lineShape.points }
        }
        ShapePath {
            strokeWidth: 1.6
            strokeColor: viz.color
            fillColor: "transparent"
            joinStyle: ShapePath.RoundJoin
            PathPolyline { path: lineShape.points.slice(1, -1) }
        }
    }
}
