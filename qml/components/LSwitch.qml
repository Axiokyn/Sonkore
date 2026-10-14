import QtQuick
import QtQuick.Controls
import Sonkore

Switch {
    id: control
    focusPolicy: Qt.NoFocus
    hoverEnabled: true

    // Size of the switch, 1 = the usual 38 x 22. Drawn at that size rather
    // than scaled, so its rounded edges stay crisp with every renderer.
    property real size: 1

    indicator: RoundRect {
        implicitWidth: Math.round(38 * control.size)
        implicitHeight: Math.round(22 * control.size)
        x: control.leftPadding
        y: parent.height / 2 - height / 2
        radius: Theme.squareShapes ? 3 : height / 2
        color: control.checked ? Theme.accent : Theme.border
        Behavior on color { ColorAnimation { duration: 140 } }
        RoundRect {
            readonly property int inset: Math.round(3 * control.size)
            x: control.checked ? parent.width - width - inset : inset
            y: inset
            width: parent.height - 2 * inset
            height: width
            radius: Theme.squareShapes ? 2 : width / 2
            color: "#ffffff"
            Behavior on x { NumberAnimation { duration: 140; easing.type: Easing.OutCubic } }
        }
    }
    contentItem: Label {
        text: control.text
        leftPadding: control.text.length > 0 ? control.indicator.width + 10 : control.indicator.width
        verticalAlignment: Text.AlignVCenter
        color: Theme.text
        font.pixelSize: Theme.fontBody
    }
}
