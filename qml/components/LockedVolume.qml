import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Sonkore

// Stands in for the volume slider while the volume is fixed at 100 %
// (Automatic on a direct output): a padlock pill, so it reads as "not
// adjustable here" at a glance rather than as a faded slider. Click: the
// volume control menu (VolumeModeMenu, opened by the bar).
Rectangle {
    id: pill

    signal clicked()
    property bool menuOpen: false // no tooltip over the open menu

    property bool brief: false // narrow places: only "100 %"

    implicitHeight: 22
    radius: height / 2
    color: area.containsMouse ? Theme.surfaceHi : "transparent"
    border.width: 1
    border.color: area.containsMouse ? Theme.textFaint : Theme.border

    RowLayout {
        anchors.centerIn: parent
        spacing: 5
        Icon {
            glyph: "lock"
            size: 12
            color: Theme.textDim
        }
        Label {
            text: pill.brief ? "100 %" : qsTr("Fixed · 100 %")
            color: Theme.textDim
            font.pixelSize: Theme.fontSmall
            font.features: { "tnum": 1 }
            elide: Text.ElideRight
            Layout.maximumWidth: pill.width - 30
        }
    }

    MouseArea {
        id: area
        anchors.fill: parent
        hoverEnabled: true
        cursorShape: Qt.PointingHandCursor
        onClicked: pill.clicked()
    }

    ToolTip.visible: area.containsMouse && !area.pressed && !pill.menuOpen
    ToolTip.delay: 400
    ToolTip.text: qsTr("Volume fixed at 100 %: set it on the DAC or the amplifier. Click to choose how the volume is set.")
}
