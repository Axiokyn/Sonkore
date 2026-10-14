import QtQuick
import QtQuick.Controls
import Sonkore

// Small rounded label: format tags, bit-perfect status...
RoundRect {
    id: root

    property alias text: label.text
    property color tone: Theme.textDim
    property bool filled: false
    // Widest text the badge may show: its width stays put when the text
    // changes, so the row around it does not shift.
    property string reserveText: ""

    implicitWidth: Math.max(label.implicitWidth, reserve.implicitWidth) + 12
    implicitHeight: 18
    radius: Theme.squareShapes ? 2 : 9
    color: filled ? tone : Qt.rgba(tone.r, tone.g, tone.b, 0.14)
    borderWidth: filled ? 0 : 1
    borderColor: Qt.rgba(tone.r, tone.g, tone.b, 0.35)

    Label {
        id: label
        anchors.centerIn: parent
        width: Math.min(implicitWidth, root.width - 10)
        elide: Text.ElideRight
        font.pixelSize: 10
        font.weight: Font.DemiBold
        font.letterSpacing: 0.4
        color: root.filled ? Theme.onAccent : root.tone
    }
    Label {
        id: reserve
        visible: false
        text: root.reserveText
        font: label.font
    }
}
