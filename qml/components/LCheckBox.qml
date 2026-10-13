import QtQuick
import QtQuick.Controls
import Sonkore

// Themed check box: an accent square with a check mark when on, a clear
// outline when off. The text (optional) is part of the click area.
CheckBox {
    id: control

    hoverEnabled: true
    focusPolicy: Qt.NoFocus
    spacing: 10
    opacity: enabled ? 1 : 0.45

    indicator: Rectangle {
        implicitWidth: 18
        implicitHeight: 18
        x: control.leftPadding
        y: control.topPadding + (control.availableHeight - height) / 2
        radius: 5
        color: control.checked ? Theme.accent : (control.hovered ? Theme.hover : "transparent")
        border.width: control.checked ? 0 : 1.5
        border.color: control.hovered ? Theme.textDim : Theme.textFaint
        Behavior on color { ColorAnimation { duration: 90 } }
        Icon {
            anchors.centerIn: parent
            visible: control.checked
            glyph: "check"
            iconStyle: "line"
            size: 14
            color: Theme.onAccent
        }
    }

    contentItem: Label {
        leftPadding: control.text.length > 0 ? control.indicator.width + control.spacing : 0
        text: control.text
        color: control.checked ? Theme.text : Theme.textDim
        verticalAlignment: Text.AlignVCenter
        elide: Text.ElideRight
    }
}
