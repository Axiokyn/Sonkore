import QtQuick
import Sonkore

// A Rectangle (colour, radius or a radius per corner, border) that stays
// clean with the software renderer at fractional scales: there RoundBox
// draws it as one anti-aliased path on the device pixels; the GPU renderers
// draw the Rectangle. Children go on top, as in a Rectangle. Used where the
// software renderer shows it: circles, small radii and every rounded shape
// with a border (knobs, switches, window buttons, menus, cards, fields).
Item {
    id: box

    property color color: "white"
    property real radius: 0
    property real borderWidth: 0
    property color borderColor: "transparent"
    // A corner of its own, as a Rectangle's (below 0: `radius`).
    property real topLeftRadius: -1
    property real topRightRadius: -1
    property real bottomLeftRadius: -1
    property real bottomRightRadius: -1

    readonly property bool software: GraphicsInfo.api === GraphicsInfo.Software

    Rectangle {
        anchors.fill: parent
        visible: !box.software
        color: box.color
        radius: box.radius
        topLeftRadius: box.topLeftRadius >= 0 ? box.topLeftRadius : box.radius
        topRightRadius: box.topRightRadius >= 0 ? box.topRightRadius : box.radius
        bottomLeftRadius: box.bottomLeftRadius >= 0 ? box.bottomLeftRadius : box.radius
        bottomRightRadius: box.bottomRightRadius >= 0 ? box.bottomRightRadius : box.radius
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
        topLeftRadius: box.topLeftRadius
        topRightRadius: box.topRightRadius
        bottomLeftRadius: box.bottomLeftRadius
        bottomRightRadius: box.bottomRightRadius
    }
}
