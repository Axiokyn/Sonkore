import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Sonkore

// Top of a tool window (Equalizer, Pitch & tempo): what is set up, in large
// type, its details under it, and the on / off switch with its state named.
Rectangle {
    id: card
    property string title
    property string subtitle
    property bool checked: false
    property string tip
    signal toggled(bool on)

    Layout.fillWidth: true
    implicitHeight: row.implicitHeight + 24
    radius: Theme.radius
    color: Theme.surface
    border.color: Qt.rgba(Theme.text.r, Theme.text.g, Theme.text.b, Theme.dark ? 0.06 : 0.09)

    RowLayout {
        id: row
        anchors.fill: parent
        anchors.leftMargin: 16
        anchors.rightMargin: 14
        spacing: 12
        ColumnLayout {
            Layout.fillWidth: true
            spacing: 2
            Label {
                Layout.fillWidth: true
                text: card.title
                elide: Text.ElideRight
                color: Theme.text
                font.pixelSize: Theme.fontTitle
                font.weight: Font.DemiBold
            }
            Label {
                Layout.fillWidth: true
                visible: text.length > 0
                text: card.subtitle
                elide: Text.ElideRight
                color: Theme.textDim
                font.pixelSize: Theme.fontSmall
            }
        }
        Label {
            text: card.checked ? qsTr("On") : qsTr("Off")
            color: card.checked ? Theme.accent : Theme.textDim
            font.weight: Font.DemiBold
        }
        LSwitch {
            checked: card.checked
            onToggled: card.toggled(checked)
            ToolTip.visible: hovered && card.tip.length > 0
            ToolTip.delay: 500
            ToolTip.text: card.tip
        }
    }
}
