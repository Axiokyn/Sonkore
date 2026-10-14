import QtQuick
import Sonkore

// A Rectangle (colour, radius, border) that stays clean with the software
// renderer at fractional scales, for the small round shapes that show it
// most (slider knobs, switches, check boxes): there RoundBox draws it as one
// anti-aliased path on the device pixels; the GPU renderers draw the
// Rectangle. Children go on top, as in a Rectangle.
Item {
    id: box

    property color color: "white"
    property real radius: 0
    property real borderWidth: 0
    property color borderColor: "transparent"

    readonly property bool software: GraphicsInfo.api === GraphicsInfo.Software

    Rectangle {
        anchors.fill: parent
        visible: !box.software
        color: box.color
        radius: box.radius
        border.width: box.borderWidth
        border.color: box.borderColor
    }
    RoundBox {
        anchors.fill: parent
        visible: box.software
        color: box.color
        radius: box.radius
        borderWidth: box.borderWidth
        borderColor: box.borderColor
    }
}
