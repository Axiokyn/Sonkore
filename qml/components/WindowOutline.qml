pragma ComponentBehavior: Bound
import QtQuick
import Sonkore

// Edge of a borderless window (Settings › Appearance › Window outline), so it
// stays defined on any wallpaper. Also draws the previews in Settings.
//   off · line: a hairline · bold: a 2 px edge · highlight: a dark edge with a
//   bright inner line · accent: a hairline in the accent · glow: the accent
//   edge fading inwards.
Item {
    id: outline

    property string style: Theme.windowOutline
    property real radius: 0

    visible: style !== "off"

    // Outer edge
    Rectangle {
        anchors.fill: parent
        radius: outline.radius
        color: "transparent"
        border.width: outline.style === "bold" ? 2 : 1
        border.color: outline.style === "highlight" ? Qt.rgba(0, 0, 0, Theme.dark ? 0.7 : 0.28)
                    : outline.style === "accent" || outline.style === "glow" ? Theme.accent
                    : outline.style === "bold" ? Theme.textFaint
                    : Theme.border
    }
    // Inner lines: one bright line (highlight), or the accent fading (glow).
    Repeater {
        model: outline.style === "highlight" ? 1 : outline.style === "glow" ? 3 : 0
        Rectangle {
            id: inner
            required property int index
            anchors.fill: parent
            anchors.margins: inner.index + 1
            radius: Math.max(0, outline.radius - inner.index - 1)
            color: "transparent"
            border.width: 1
            border.color: outline.style === "highlight" ? Qt.rgba(1, 1, 1, Theme.dark ? 0.13 : 0.55)
                        : Qt.rgba(Theme.accent.r, Theme.accent.g, Theme.accent.b, [0.45, 0.22, 0.08][inner.index])
        }
    }
}
