import QtQuick
import QtQuick.Controls
import Sonkore

TextField {
    id: control

    implicitWidth: 260
    implicitHeight: Theme.compact ? 28 : 36
    leftPadding: 34
    rightPadding: clearButton.visible ? 30 : 12
    color: Theme.text
    placeholderTextColor: Theme.textFaint
    selectionColor: Theme.accent
    selectedTextColor: Theme.onAccent
    font.pixelSize: Theme.fontBody
    selectByMouse: true

    background: RoundRect {
        radius: Theme.squareShapes ? Theme.radiusSmall : height / 2
        color: Theme.surfaceHi
        borderWidth: control.activeFocus ? 1.5 : 1
        borderColor: control.activeFocus ? Theme.accent : Theme.border
    }

    Icon {
        anchors.left: parent.left
        anchors.leftMargin: 12
        anchors.verticalCenter: parent.verticalCenter
        glyph: "search"
        size: 15
        color: Theme.textFaint
    }
    IconButton {
        id: clearButton
        tile: false
        anchors.right: parent.right
        anchors.rightMargin: 4
        anchors.verticalCenter: parent.verticalCenter
        iconName: "close"
        iconSize: 12
        visible: control.text.length > 0
        onClicked: control.clear()
    }
    Keys.onEscapePressed: { clear(); focus = false }
}
