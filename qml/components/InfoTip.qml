import QtQuick
import QtQuick.Controls
import Sonkore

// A small "i" beside a setting's title: hover it (or click it) to read the
// long explanation, so the row itself keeps a one-line description. The
// explanation opens under the "i", over its own row's description, never
// over the controls of the rows around it.
Icon {
    id: tip
    property string text
    // Opened with a click: stays until the pointer leaves or another click.
    property bool pinned: false
    glyph: "info"
    size: 14
    color: hover.hovered || pinned ? Theme.text : Theme.textFaint

    HoverHandler {
        id: hover
        cursorShape: Qt.WhatsThisCursor
        onHoveredChanged: if (!hovered) tip.pinned = false
    }
    TapHandler { onTapped: tip.pinned = !tip.pinned }

    ToolTip {
        visible: hover.hovered || tip.pinned
        delay: tip.pinned ? 0 : 250
        // Under the "i", starting a little to its left (Popup keeps it inside
        // the window when the "i" is near an edge).
        x: -8
        y: tip.height + 6
        // Wraps at a readable width instead of spanning the window.
        width: Math.min(implicitWidth, 380)
        contentItem: Label {
            text: tip.text
            wrapMode: Text.Wrap
            color: Theme.text
            font.pixelSize: Theme.fontSmall + 1
        }
        background: RoundRect {
            radius: Theme.radiusSmall
            color: Theme.surfaceHi
            borderWidth: 1
            borderColor: Theme.border
        }
    }
}
