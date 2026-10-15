pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Sonkore

// The signal path, file to DAC: every stage the audio goes through and how it
// affects quality, with the overall verdict on top (the lowest stage wins).
Popup {
    id: root

    width: 380
    padding: 16
    closePolicy: Popup.CloseOnEscape | Popup.CloseOnPressOutside

    background: RoundRect {
        radius: Theme.radius + 2
        color: Theme.surface
        borderWidth: 1
        borderColor: Theme.border
    }

    contentItem: ColumnLayout {
        spacing: 12
        RowLayout {
            Layout.fillWidth: true
            Label {
                Layout.fillWidth: true
                text: qsTr("Signal path")
                font.pixelSize: Theme.fontTitle
                font.weight: Font.DemiBold
                color: Theme.text
            }
            Badge {
                visible: DspController.signalPath.length > 0
                text: DspController.bitPerfect ? qsTr("BIT-PERFECT") : stages.labelOf(stages.overall).toUpperCase()
                tone: stages.colorOf(stages.overall)
            }
        }
        SignalPathList {
            id: stages
            Layout.fillWidth: true
        }
    }
}
