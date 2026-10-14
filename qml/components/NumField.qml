import QtQuick
import QtQuick.Controls
import Sonkore

// Compact numeric editor (C locale, fixed decimals). Re-formats when it loses
// focus; emits edited() with a clamped double.
TextField {
    id: field

    property real value: 0
    property real from: -1e9
    property real to: 1e9
    property int decimals: 2
    property string suffix
    signal edited(real newValue)

    function sync(force) {
        if (force || !activeFocus)
            text = DspController.formatDecimal(value, decimals)
    }

    implicitWidth: 84
    implicitHeight: Theme.compact ? 26 : 32
    horizontalAlignment: TextInput.AlignRight
    rightPadding: suffix.length > 0 ? 30 : 8
    color: acceptableInput ? Theme.text : Theme.warn
    selectionColor: Theme.accent
    selectedTextColor: Theme.onAccent
    selectByMouse: true
    font.pixelSize: Theme.fontBody
    font.features: { "tnum": 1 }
    inputMethodHints: Qt.ImhFormattedNumbersOnly
    validator: DoubleValidator {
        bottom: field.from
        top: field.to
        decimals: field.decimals
        notation: DoubleValidator.StandardNotation
        locale: "C"
    }
    background: RoundRect {
        radius: Theme.radiusSmall
        color: Theme.surfaceHi
        borderWidth: 1
        borderColor: field.activeFocus ? Theme.accent : Theme.border
    }

    Label {
        anchors.right: parent.right
        anchors.rightMargin: 8
        anchors.verticalCenter: parent.verticalCenter
        text: field.suffix
        color: Theme.textFaint
        font.pixelSize: Theme.fontSmall
    }

    onValueChanged: sync(false)
    onActiveFocusChanged: if (!activeFocus) sync(true)
    Component.onCompleted: sync(true)
    onEditingFinished: {
        const v = DspController.parseDecimal(text)
        if (v !== undefined)
            field.edited(Math.min(field.to, Math.max(field.from, v)))
        sync(true)
    }
}
