import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Sonkore

// Small modal asking for a name (playlists, EQ profiles).
Popup {
    id: root

    property string title
    property string placeholder
    property string acceptText: qsTr("Save")
    // Optional line of text under the field.
    property string note
    signal accepted(string name)

    // Fills the field only while the user has not typed a name.
    function suggest(name) {
        if (field.text.trim().length === 0)
            field.text = name
    }

    function ask(initial) {
        field.text = initial || ""
        open()
        field.forceActiveFocus()
        field.selectAll()
    }

    parent: Overlay.overlay
    anchors.centerIn: parent
    width: 380
    modal: true
    focus: true
    padding: 20
    closePolicy: Popup.CloseOnEscape | Popup.CloseOnPressOutside

    background: Rectangle {
        radius: Theme.radius + 2
        color: Theme.surface
        border.color: Theme.border
    }
    Overlay.modal: Rectangle { color: Qt.rgba(0, 0, 0, 0.45) }

    contentItem: ColumnLayout {
        spacing: 14
        Label {
            text: root.title
            font.pixelSize: Theme.fontTitle
            font.weight: Font.DemiBold
            color: Theme.text
        }
        TextField {
            id: field
            Layout.fillWidth: true
            placeholderText: root.placeholder
            color: Theme.text
            placeholderTextColor: Theme.textFaint
            selectByMouse: true
            background: Rectangle {
                implicitHeight: 36
                radius: Theme.radiusSmall
                color: Theme.surfaceHi
                border.color: field.activeFocus ? Theme.accent : Theme.border
            }
            onAccepted: if (text.trim().length > 0) { root.accepted(text.trim()); root.close() }
        }
        Label {
            Layout.fillWidth: true
            visible: root.note.length > 0
            text: root.note
            wrapMode: Text.Wrap
            color: Theme.textDim
            font.pixelSize: Theme.fontSmall + 1
        }
        RowLayout {
            Layout.alignment: Qt.AlignRight
            spacing: 8
            PillButton { text: qsTr("Cancel"); onClicked: root.close() }
            PillButton {
                primary: true
                text: root.acceptText
                enabled: field.text.trim().length > 0
                onClicked: { root.accepted(field.text.trim()); root.close() }
            }
        }
    }
}
