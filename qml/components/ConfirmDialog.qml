import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Sonkore

// Small modal asking to confirm an action that is costly to undo
// (e.g. taking a whole music folder out of the library).
Popup {
    id: root

    property string title
    property string text
    property string acceptText: qsTr("Remove")
    signal accepted()

    parent: Overlay.overlay
    anchors.centerIn: parent
    // Narrower in the compact player.
    width: Math.min(420, parent.width - 24)
    modal: true
    focus: true
    padding: 20
    closePolicy: Popup.CloseOnEscape | Popup.CloseOnPressOutside

    background: RoundRect {
        radius: Theme.radius + 2
        color: Theme.surface
        borderWidth: 1
        borderColor: Theme.border
    }
    Overlay.modal: Rectangle { color: Qt.rgba(0, 0, 0, 0.45) }

    contentItem: ColumnLayout {
        spacing: 14
        Label {
            Layout.fillWidth: true
            text: root.title
            wrapMode: Text.Wrap
            font.pixelSize: Theme.fontTitle
            font.weight: Font.DemiBold
            color: Theme.text
        }
        Label {
            Layout.fillWidth: true
            text: root.text
            wrapMode: Text.Wrap
            color: Theme.textDim
        }
        RowLayout {
            Layout.alignment: Qt.AlignRight
            spacing: 8
            // Cancel is the default: Enter or a stray click outside keeps the folder.
            PillButton { text: qsTr("Cancel"); onClicked: root.close() }
            PillButton {
                primary: true
                text: root.acceptText
                onClicked: { root.close(); root.accepted() }
            }
        }
    }
}
