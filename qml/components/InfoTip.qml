import QtQuick
import QtQuick.Controls
import Sonkore

// A small "i" beside a setting's title: hover it to read the long
// explanation, so the row itself keeps a one-line description.
Icon {
    id: tip
    property string text
    glyph: "info"
    size: 14
    color: hover.hovered ? Theme.text : Theme.textFaint

    HoverHandler { id: hover; cursorShape: Qt.WhatsThisCursor }

    ToolTip {
        visible: hover.hovered
        delay: 250
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
