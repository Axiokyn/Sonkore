import QtQuick
import QtQuick.Controls
import Sonkore

Switch {
    id: control
    focusPolicy: Qt.NoFocus
    hoverEnabled: true

    indicator: Rectangle {
        implicitWidth: 38
        implicitHeight: 22
        x: control.leftPadding
        y: parent.height / 2 - height / 2
        radius: Theme.squareShapes ? 3 : 11
        color: control.checked ? Theme.accent : Theme.border
        Behavior on color { ColorAnimation { duration: 140 } }
        Rectangle {
            x: control.checked ? parent.width - width - 3 : 3
            y: 3
            width: 16
            height: 16
            radius: Theme.squareShapes ? 2 : 8
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
