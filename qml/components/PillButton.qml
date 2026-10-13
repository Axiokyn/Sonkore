import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Sonkore

// Text button: `primary` = filled with the accent colour, otherwise subtle.
AbstractButton {
    id: control

    property bool primary: false
    property string iconName

    implicitHeight: Theme.compact ? 28 : 34
    implicitWidth: row.implicitWidth + (Theme.compact ? 20 : 28)
    hoverEnabled: true
    focusPolicy: Qt.NoFocus

    background: Rectangle {
        radius: Theme.squareShapes ? Theme.radiusSmall : height / 2
        color: control.primary
               ? (control.pressed ? Qt.darker(Theme.accent, 1.15) : (control.hovered ? Qt.lighter(Theme.accent, 1.08) : Theme.accent))
               : (control.pressed ? Theme.border : (control.hovered ? Theme.hover : Theme.surfaceHi))
        border.width: control.primary ? 0 : 1
        border.color: Theme.border
        Behavior on color { ColorAnimation { duration: 120 } }
    }
    contentItem: Item {
        RowLayout {
            id: row
            anchors.centerIn: parent
            spacing: 6
            Icon {
                visible: control.iconName.length > 0
                glyph: control.iconName
                size: 16
                color: control.primary ? Theme.onAccent : Theme.text
            }
            Label {
                text: control.text
                font.pixelSize: Theme.fontBody
                font.weight: Font.DemiBold
                color: control.primary ? Theme.onAccent : Theme.text
            }
        }
    }
    opacity: enabled ? 1 : 0.45
}
