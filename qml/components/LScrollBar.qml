import QtQuick
import QtQuick.Controls
import Sonkore

// Scroll bar drawn with the active skin, the same everywhere: a rounded thumb
// near the outer edge that shows up while the content scrolls or the
// pointer is over the bar, then fades away. Brighter and wider under the
// pointer, in the accent colour while dragged.
ScrollBar {
    id: control

    readonly property bool scrollable: size < 1.0
    readonly property bool lit: hovered || pressed
    // Shown while scrolling / hovered, and a moment after.
    readonly property bool shown: scrollable && (active || lit || hideDelay.running)

    onActiveChanged: if (!active) hideDelay.restart()
    onLitChanged: if (!lit) hideDelay.restart()
    Timer { id: hideDelay; interval: 900 }

    hoverEnabled: true
    minimumSize: 0.05
    // Air at both ends along the bar (track and thumb), so it never meets
    // the line or rounded corner at the end of what it scrolls.
    property real endInset: 4
    // The thumb centred across its track, with a little air along it.
    leftPadding: horizontal ? 2 + endInset : 2
    rightPadding: horizontal ? 2 + endInset : 2
    topPadding: horizontal ? 2 : 2 + endInset
    bottomPadding: horizontal ? 2 : 2 + endInset
    leftInset: horizontal ? endInset : 0
    rightInset: horizontal ? endInset : 0
    topInset: horizontal ? 0 : endInset
    bottomInset: horizontal ? 0 : endInset

    contentItem: Rectangle {
        implicitWidth: 6
        implicitHeight: 6
        radius: Math.min(width, height) / 2
        color: control.pressed ? Theme.accent : (control.lit ? Theme.textDim : Theme.textFaint)
        opacity: control.shown ? 1 : 0
        Behavior on color { ColorAnimation { duration: 120 } }
        Behavior on opacity { NumberAnimation { duration: 180 } }
    }
    background: Rectangle {
        // 6 px thumb at rest, 8 px under the pointer
        implicitWidth: control.lit ? 10 : 8
        implicitHeight: control.lit ? 10 : 8
        radius: 5
        color: Qt.rgba(Theme.text.r, Theme.text.g, Theme.text.b, control.lit && control.scrollable ? 0.07 : 0)
        Behavior on implicitWidth { NumberAnimation { duration: 120 } }
        Behavior on implicitHeight { NumberAnimation { duration: 120 } }
        Behavior on color { ColorAnimation { duration: 120 } }
    }
}
