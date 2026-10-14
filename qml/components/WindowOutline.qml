pragma ComponentBehavior: Bound
import QtQuick
import Sonkore

// Edge of a borderless window (Settings › Appearance › Window outline), so it
// stays defined on any wallpaper. Also draws the previews in Settings.
//   off · line: a hairline · bold: a 2 px edge · highlight: a dark edge with a
//   bright inner line · accent: a hairline in the accent · glow: the accent
//   edge fading inwards.
// The GPU renderers draw it with Rectangles; the software renderer with
// WindowFrame, on the window's device pixels (it also cuts the rounded
// corners out cleanly there).
Item {
    id: outline

    property string style: Theme.windowOutline
    property real radius: 0
    // The window's edge (not a preview): with the software renderer the
    // rounded corners are cut out here too, with or without an outline.
    property bool windowEdge: false

    readonly property bool software: GraphicsInfo.api === GraphicsInfo.Software
    // The rings, from the edge inwards.
    readonly property var rings: style === "highlight" ? [Qt.rgba(0, 0, 0, Theme.dark ? 0.7 : 0.28), Qt.rgba(1, 1, 1, Theme.dark ? 0.13 : 0.55)]
                               : style === "glow" ? [Theme.accent].concat([0.45, 0.22, 0.08].map(a => Qt.rgba(Theme.accent.r, Theme.accent.g, Theme.accent.b, a)))
                               : style === "accent" ? [Theme.accent]
                               : style === "bold" ? [Theme.textFaint]
                               : style === "line" ? [Theme.border]
                               : []

    WindowFrame {
        anchors.fill: parent
        visible: outline.software && outline.windowEdge
        radius: outline.radius
        lineWidth: outline.style === "bold" ? 2 : 1
        colors: outline.rings
        clipCorners: App.alphaWindow && outline.radius > 0
    }

    Repeater {
        model: outline.software && outline.windowEdge ? 0 : outline.rings.length
        Rectangle {
            id: ring
            required property int index
            anchors.fill: parent
            anchors.margins: outline.style === "bold" ? 0 : ring.index
            radius: Math.max(0, outline.radius - ring.index)
            color: "transparent"
            border.width: outline.style === "bold" ? 2 : 1
            border.color: outline.rings[ring.index]
        }
    }
}
