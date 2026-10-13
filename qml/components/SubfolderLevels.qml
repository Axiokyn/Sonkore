import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Sonkore

// "Subfolders" row of a folder list (the compact player's folder menu, the
// sidebar's folder tree): a switch to show them, and how many levels deep
// (− N +, from 1 to the deepest there is). Inside a menu it keeps it open.
RowLayout {
    id: levels

    property bool shown: true
    property int value: 1     // levels shown, already within 1 … deepest
    property int deepest: 1   // the deepest subfolder level in the library
    signal shownToggled(bool on)
    signal valuePicked(int value)

    height: 28
    spacing: 6

    component StepButton: IconButton {
        anchors.verticalCenter: parent.verticalCenter
        iconSize: 10
        implicitWidth: 22
        implicitHeight: 22
        tile: false
        opacity: enabled ? 1 : 0.35
    }

    Label {
        Layout.leftMargin: 10
        Layout.fillWidth: true
        text: qsTr("Subfolders")
        color: Theme.textDim
        font.pixelSize: Theme.fontSmall + 1
    }
    Row {
        visible: levels.shown && levels.deepest > 1
        spacing: 0
        StepButton {
            iconName: "win-min"
            enabled: levels.value > 1
            tip: qsTr("Fewer levels")
            onClicked: levels.valuePicked(levels.value - 1)
        }
        Label {
            anchors.verticalCenter: parent.verticalCenter
            width: 18
            horizontalAlignment: Text.AlignHCenter
            text: levels.value
            color: Theme.text
            font.pixelSize: Theme.fontSmall + 1
            font.weight: Font.DemiBold
            font.features: { "tnum": 1 }
        }
        StepButton {
            iconName: "plus"
            enabled: levels.value < levels.deepest
            tip: qsTr("More levels")
            onClicked: levels.valuePicked(levels.value + 1)
        }
    }
    LSwitch {
        Layout.rightMargin: 4
        size: 0.8
        checked: levels.shown
        onToggled: levels.shownToggled(checked)
    }
}
