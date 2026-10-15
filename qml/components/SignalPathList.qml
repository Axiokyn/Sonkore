pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Sonkore

// The stages of the signal path (DspController.signalPath), file to DAC,
// each with its quality: the body of the signal path popup and of the Now
// playing page.
ColumnLayout {
    id: root

    spacing: 12

    // Lossless (untouched), high quality, enhanced (DSP chosen on purpose),
    // warning (a problem one setting fixes, e.g. clipping), low.
    function colorOf(quality) {
        switch (quality) {
        case "lossless": return "#9b6bff"
        case "high": return "#3fb950"
        case "enhanced": return "#4ea1ff"
        case "warning": return Theme.warn
        default: return "#e3a008"
        }
    }
    function labelOf(quality) {
        switch (quality) {
        case "lossless": return qsTr("Lossless")
        case "high": return qsTr("High quality")
        case "enhanced": return qsTr("Enhanced")
        case "warning": return qsTr("Warning")
        default: return qsTr("Low quality")
        }
    }
    // The verdict: the lowest stage wins.
    readonly property string overall: {
        const order = ["lossless", "high", "enhanced", "warning", "low"]
        let worst = 0
        for (const s of DspController.signalPath)
            worst = Math.max(worst, order.indexOf(s.quality))
        return order[worst]
    }

    Label {
        visible: DspController.signalPath.length === 0
        text: qsTr("Nothing is playing.")
        color: Theme.textDim
    }
    Repeater {
        model: DspController.signalPath
        delegate: RowLayout {
            id: stageRow
            required property var modelData
            required property int index
            Layout.fillWidth: true
            spacing: 12
            Item {
                Layout.preferredWidth: 12
                Layout.fillHeight: true
                // line to the next stage
                Rectangle {
                    visible: stageRow.index < DspController.signalPath.length - 1
                    x: 5; y: 12
                    width: 2
                    height: stageRow.height
                    color: Theme.border
                }
                Rectangle {
                    y: 4
                    width: 12; height: 12; radius: 6
                    color: root.colorOf(stageRow.modelData.quality)
                }
            }
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 1
                Label {
                    text: stageRow.modelData.stage
                    font.weight: Font.DemiBold
                    color: Theme.text
                }
                Label {
                    Layout.fillWidth: true
                    text: stageRow.modelData.detail
                    wrapMode: Text.Wrap
                    color: Theme.textDim
                    font.pixelSize: Theme.fontSmall
                }
                // A one-click fix, where the stage offers one.
                PillButton {
                    Layout.topMargin: 4
                    visible: stageRow.modelData.action === "headroomAuto"
                    text: qsTr("Use automatic headroom")
                    onClicked: DspController.headroomAuto = true
                }
            }
            Label {
                Layout.alignment: Qt.AlignTop
                text: root.labelOf(stageRow.modelData.quality)
                color: root.colorOf(stageRow.modelData.quality)
                font.pixelSize: Theme.fontSmall
            }
        }
    }
}
