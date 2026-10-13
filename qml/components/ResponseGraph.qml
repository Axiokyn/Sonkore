import QtQuick
import QtQuick.Controls
import Sonkore

// Frequency response of the equalizer (incl. preamp) on a log axis.
// Band handles can be dragged: horizontally = frequency, vertically = gain;
// the mouse wheel changes Q (parametric mode). A handle never gets closer
// than 1/6 octave to its neighbours, so bands keep their order. In graphic
// mode a double click puts the band back on its standard frequency, and a
// right click resets the band (0 dB, and its standard frequency in graphic mode).
Item {
    id: graph

    readonly property var points: Equalizer.response
    readonly property bool graphic: Equalizer.mode === "graphic"
    readonly property real range: {
        let m = 12
        for (const p of points) m = Math.max(m, Math.abs(p.db))
        for (const b of Equalizer.bands) m = Math.max(m, Math.abs(b.gain))
        return Math.ceil((m + 1) / 6) * 6
    }
    readonly property real logMin: Math.log10(20)
    readonly property real logSpan: 3 // log10(20000 / 20)
    readonly property int leftGutter: 34
    readonly property int bottomGutter: 20
    readonly property int topGutter: 8

    function xOf(f) { return leftGutter + (width - leftGutter) * (Math.log10(f) - logMin) / logSpan }
    function fOf(x) { return Math.pow(10, logMin + (x - leftGutter) / (width - leftGutter) * logSpan) }
    function yOf(db) { return topGutter + (height - bottomGutter - topGutter) * (range - db) / (2 * range) }
    function dbOf(y) { return range - 2 * range * (y - topGutter) / (height - bottomGutter - topGutter) }
    function hasGain(type) { return type === "PK" || type === "LSC" || type === "HSC" }

    // Minimum distance between neighbouring bands.
    readonly property real minGap: Math.pow(2, 1 / 6)
    // Frequency range band `index` may move in: between the nearest bands
    // below and above it (bands on the very same frequency do not block).
    function freqLimits(index) {
        const bands = Equalizer.bands
        const f = bands[index].freq
        let below = 0, above = Infinity
        for (let i = 0; i < bands.length; ++i) {
            if (i === index)
                continue
            const g = bands[i].freq
            if (g < f) below = Math.max(below, g)
            else if (g > f) above = Math.min(above, g)
        }
        // Already closer than the gap: it may not move closer, only away.
        const lo = below > 0 ? Math.min(f, below * minGap) : 20
        const hi = above < Infinity ? Math.max(f, above / minGap) : 20000
        return { lo: Math.max(20, lo), hi: Math.min(20000, hi) }
    }

    onPointsChanged: canvas.requestPaint()
    onWidthChanged: canvas.requestPaint()
    onHeightChanged: canvas.requestPaint()
    Connections {
        target: Theme
        function onSkinChanged() { canvas.requestPaint() }
    }
    Connections {
        target: Equalizer
        function onEnabledChanged() { canvas.requestPaint() }
    }

    Canvas {
        id: canvas
        anchors.fill: parent
        renderStrategy: Canvas.Cooperative
        onPaint: {
            const ctx = getContext("2d")
            ctx.reset()
            const plotBottom = height - graph.bottomGutter
            ctx.font = "10px sans-serif"
            ctx.lineWidth = 1

            // dB grid
            for (let db = -graph.range; db <= graph.range; db += 6) {
                const y = Math.round(graph.yOf(db)) + 0.5
                ctx.strokeStyle = db === 0 ? Theme.textFaint : Theme.border
                ctx.globalAlpha = db === 0 ? 0.6 : 1.0
                ctx.beginPath(); ctx.moveTo(graph.leftGutter, y); ctx.lineTo(width, y); ctx.stroke()
                ctx.globalAlpha = 1.0
                ctx.fillStyle = Theme.textFaint
                ctx.textAlign = "right"
                ctx.fillText((db > 0 ? "+" : "") + db, graph.leftGutter - 6, y + 3)
            }
            // frequency grid
            const marks = [20, 50, 100, 200, 500, 1000, 2000, 5000, 10000, 20000]
            ctx.textAlign = "center"
            for (const f of marks) {
                const x = Math.round(graph.xOf(f)) + 0.5
                ctx.strokeStyle = Theme.border
                ctx.beginPath(); ctx.moveTo(x, 0); ctx.lineTo(x, plotBottom); ctx.stroke()
                ctx.fillStyle = Theme.textFaint
                ctx.fillText(f >= 1000 ? (f / 1000) + "k" : f, Math.min(Math.max(x, graph.leftGutter + 8), width - 12), height - 5)
            }

            if (graph.points.length < 2)
                return
            const active = Equalizer.enabled
            const accent = active ? Theme.accent : Theme.textFaint
            const zeroY = graph.yOf(0)

            // fill between curve and 0 dB
            const grad = ctx.createLinearGradient(0, 0, 0, plotBottom)
            grad.addColorStop(0, Qt.rgba(Theme.accent.r, Theme.accent.g, Theme.accent.b, active ? 0.35 : 0.08))
            grad.addColorStop(0.5, Qt.rgba(Theme.accent.r, Theme.accent.g, Theme.accent.b, active ? 0.06 : 0.02))
            grad.addColorStop(1, Qt.rgba(Theme.accent.r, Theme.accent.g, Theme.accent.b, active ? 0.35 : 0.08))
            ctx.fillStyle = grad
            ctx.beginPath()
            ctx.moveTo(graph.xOf(graph.points[0].f), zeroY)
            for (const p of graph.points) ctx.lineTo(graph.xOf(p.f), graph.yOf(p.db))
            ctx.lineTo(graph.xOf(graph.points[graph.points.length - 1].f), zeroY)
            ctx.closePath()
            ctx.fill()

            // curve
            ctx.strokeStyle = accent
            ctx.lineWidth = 2.2
            ctx.lineJoin = "round"
            ctx.beginPath()
            graph.points.forEach((p, i) => {
                const x = graph.xOf(p.f), y = graph.yOf(p.db)
                if (i === 0) ctx.moveTo(x, y); else ctx.lineTo(x, y)
            })
            ctx.stroke()
        }
    }

    // Handles (integer model: delegates survive while bands are edited)
    Repeater {
        model: Equalizer.bands.length
        delegate: Item {
            id: handle
            required property int index
            readonly property var band: Equalizer.bands[index] || ({ freq: 1000, gain: 0, q: 1, type: "PK", enabled: true })
            x: graph.xOf(band.freq) - width / 2
            // Drawn on the curve, which includes the preamp.
            y: graph.yOf((graph.hasGain(band.type) ? band.gain : 0) + Equalizer.preamp) - height / 2
            width: 22
            height: 22
            z: drag.pressed ? 2 : 1

            Rectangle {
                anchors.centerIn: parent
                width: drag.containsMouse || drag.pressed ? 18 : 14
                height: width
                radius: width / 2
                color: handle.band.enabled ? Theme.accent : Theme.textFaint
                border.width: 2
                border.color: Theme.surface
                opacity: Equalizer.enabled ? 1 : 0.5
                Behavior on width { NumberAnimation { duration: 90 } }
                Label {
                    anchors.centerIn: parent
                    visible: !graph.graphic
                    text: handle.index + 1
                    color: Theme.onAccent
                    font.pixelSize: 8
                    font.weight: Font.Bold
                }
            }

            MouseArea {
                id: drag
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: pressed ? Qt.ClosedHandCursor : Qt.OpenHandCursor
                preventStealing: true
                acceptedButtons: Qt.LeftButton | Qt.RightButton
                // Fixed for the whole drag: the neighbours do not move meanwhile.
                property var limits: ({ lo: 20, hi: 20000 })
                onPressed: (mouse) => {
                    limits = graph.freqLimits(handle.index)
                    if (mouse.button !== Qt.RightButton)
                        return
                    const b = handle.band
                    const standard = graph.graphic ? Equalizer.graphicFrequency(handle.index) : 0
                    Equalizer.setBand(handle.index, b.type, standard > 0 ? standard : b.freq, 0, b.q, b.enabled)
                }
                onPositionChanged: (mouse) => {
                    if (!pressed || !(pressedButtons & Qt.LeftButton))
                        return
                    const p = mapToItem(graph, mouse.x, mouse.y)
                    const b = handle.band
                    const target = graph.dbOf(p.y) - Equalizer.preamp
                    const gain = graph.hasGain(b.type) ? Math.round(Math.max(-24, Math.min(24, target)) * 10) / 10 : b.gain
                    const f = Math.max(limits.lo, Math.min(limits.hi, graph.fOf(p.x)))
                    let freq = f < 1000 ? Math.round(f) : Math.round(f / 10) * 10
                    freq = Math.max(limits.lo, Math.min(limits.hi, freq)) // rounding stays inside too
                    if (freq !== b.freq || gain !== b.gain)
                        Equalizer.setBand(handle.index, b.type, freq, gain, b.q, b.enabled)
                }
                onDoubleClicked: (mouse) => {
                    if (mouse.button !== Qt.LeftButton)
                        return
                    const standard = Equalizer.graphicFrequency(handle.index)
                    if (!graph.graphic || standard <= 0)
                        return
                    const b = handle.band
                    Equalizer.setBand(handle.index, b.type, standard, b.gain, b.q, b.enabled)
                }
                onWheel: (wheel) => {
                    if (graph.graphic)
                        return
                    const b = handle.band
                    const q = Math.max(0.1, Math.min(20, b.q * (wheel.angleDelta.y > 0 ? 1.1 : 1 / 1.1)))
                    Equalizer.setBand(handle.index, b.type, b.freq, b.gain, Math.round(q * 100) / 100, b.enabled)
                }
            }
            ToolTip.visible: drag.containsMouse || drag.pressed
            ToolTip.delay: 300
            ToolTip.text: Equalizer.typeLabel(band.type) + " · " + Equalizer.frequencyLabel(band.freq) + " Hz · "
                          + (band.gain > 0 ? "+" : "") + band.gain.toFixed(1) + " dB · Q " + band.q.toFixed(2)
        }
    }
}
