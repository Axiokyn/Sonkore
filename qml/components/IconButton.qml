import QtQuick
import QtQuick.Controls
import Sonkore

// Round, flat icon button. `active` tints it with the accent colour.
AbstractButton {
    id: control

    property string iconName
    property int iconSize: 18
    property bool active: false
    property color iconColor: active ? Theme.accent : (hovered ? Theme.text : Theme.textDim)
    property string tip
    // Turns the icon (not the button: its tooltip would turn with it).
    property bool spinning: false
    // "Tile" icon style: a soft square behind the icon, tinted when active.
    // Off for buttons inside rows and fields, where a square on every row
    // would be noise.
    property bool tile: Theme.iconTiles

    implicitWidth: iconSize + 16
    implicitHeight: iconSize + 16
    hoverEnabled: true
    focusPolicy: Qt.NoFocus

    background: RoundRect {
        anchors.fill: parent
        anchors.margins: control.tile ? 2 : 0
        radius: control.tile ? Math.round(Math.min(width, height) * 0.3) : Math.min(width, height) / 2
        color: control.pressed ? Theme.border
             : control.tile && control.active ? Theme.accentSoft
             : control.hovered ? Theme.hover
             : control.tile ? Theme.surfaceHi : "transparent"
        Behavior on color { ColorAnimation { duration: 120 } }
    }
    contentItem: Item {
        Icon {
            id: icon
            anchors.centerIn: parent
            RotationAnimation on rotation {
                running: control.spinning
                loops: Animation.Infinite
                from: 0; to: 360; duration: 1000
                onStopped: icon.rotation = 0
            }
            glyph: control.iconName
            size: control.iconSize
            color: control.enabled ? control.iconColor : Theme.textFaint
        }
    }

    ToolTip.visible: tip.length > 0 && hovered
    ToolTip.delay: 500
    ToolTip.text: tip
}
