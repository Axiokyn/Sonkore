import QtQuick
import QtQuick.Controls.impl
import Sonkore

// Monochrome SVG icon tinted with `color`.
IconImage {
    property string glyph
    property int size: 20
    property string iconStyle // "" = Theme's (Settings previews pass their own)
    source: glyph ? Theme.icon(glyph, iconStyle || undefined) : ""
    sourceSize: Qt.size(size, size)
    width: size
    height: size
    color: Theme.text
}
