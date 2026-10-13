import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Sonkore

// High-precision numeric editor:  [label]  [−]  [ -0.3176660 ]  [+]  [suffix]
//
// * Text is validated by a C-locale DoubleValidator (StandardNotation, N
//   decimals) and parsed in C++ (DspController.parseDecimal), so "0.0000001"
//   becomes exactly the nearest IEEE-754 double. No float32 anywhere.
// * Valid keystrokes are committed live; the field is only re-formatted when
//   it loses focus, so the caret never jumps while typing.
// * −/+ (and Up/Down keys) step by `step` on the decimal grid; hold to repeat.
RowLayout {
    id: root

    property string label
    property string suffix
    property real value: 0
    property real from: -1e9
    property real to: 1e9
    property int decimals: 7
    property real step: 0.000001

    // Emitted with an exact double; the owner writes it to the C++ property.
    signal valueEdited(real newValue)

    spacing: 6

    function syncText(force) {
        if (force || !field.activeFocus)
            field.text = DspController.formatDecimal(root.value, root.decimals)
    }

    function commit(text) {
        const v = DspController.parseDecimal(text)
        if (v !== undefined && v >= root.from && v <= root.to)
            root.valueEdited(v)
    }

    function nudge(direction) {
        root.valueEdited(DspController.stepValue(root.value, root.step, direction,
                                                 root.decimals, root.from, root.to))
        syncText(true)
    }

    onValueChanged: syncText(false)
    onDecimalsChanged: syncText(true)
    Component.onCompleted: syncText(true)

    Label {
        text: root.label
        Layout.preferredWidth: 72
        color: Theme.text
        font.weight: Font.DemiBold
    }

    Button {
        text: "−"
        implicitWidth: 36
        implicitHeight: 36
        background: Rectangle { radius: Theme.radiusSmall; color: parent.pressed ? Theme.border : (parent.hovered ? Theme.hover : Theme.surfaceHi); border.color: Theme.border }
        contentItem: Label { text: parent.text; color: Theme.text; font.pixelSize: 16; horizontalAlignment: Text.AlignHCenter; verticalAlignment: Text.AlignVCenter }
        autoRepeat: true
        autoRepeatDelay: 350
        autoRepeatInterval: 35
        focusPolicy: Qt.NoFocus
        enabled: root.value > root.from
        onClicked: root.nudge(-1)
        ToolTip.visible: hovered
        ToolTip.delay: 600
        ToolTip.text: qsTr("−%1").arg(DspController.formatDecimal(root.step, root.decimals))
    }

    TextField {
        id: field
        Layout.fillWidth: true
        Layout.minimumWidth: 140
        horizontalAlignment: TextInput.AlignRight
        font.family: "monospace"
        font.pixelSize: 15
        selectByMouse: true
        inputMethodHints: Qt.ImhFormattedNumbersOnly
        color: acceptableInput ? Theme.text : Theme.warn
        selectionColor: Theme.accent
        selectedTextColor: Theme.onAccent
        background: Rectangle {
            implicitHeight: 36
            radius: Theme.radiusSmall
            color: Theme.surfaceHi
            border.width: field.activeFocus ? 1.5 : 1
            border.color: field.activeFocus ? Theme.accent : Theme.border
        }

        validator: DoubleValidator {
            bottom: root.from
            top: root.to
            decimals: root.decimals
            notation: DoubleValidator.StandardNotation
            locale: "C"                 // '.' decimal separator on every OS locale
        }

        onTextEdited: if (acceptableInput) root.commit(text)
        onEditingFinished: {
            root.commit(text)
            root.syncText(true)
        }
        onActiveFocusChanged: if (!activeFocus) root.syncText(true)

        Keys.onUpPressed: root.nudge(+1)
        Keys.onDownPressed: root.nudge(-1)
        Keys.onEscapePressed: {
            root.syncText(true)
            focus = false
        }
    }

    Button {
        text: "+"
        implicitWidth: 36
        implicitHeight: 36
        background: Rectangle { radius: Theme.radiusSmall; color: parent.pressed ? Theme.border : (parent.hovered ? Theme.hover : Theme.surfaceHi); border.color: Theme.border }
        contentItem: Label { text: parent.text; color: Theme.text; font.pixelSize: 16; horizontalAlignment: Text.AlignHCenter; verticalAlignment: Text.AlignVCenter }
        autoRepeat: true
        autoRepeatDelay: 350
        autoRepeatInterval: 35
        focusPolicy: Qt.NoFocus
        enabled: root.value < root.to
        onClicked: root.nudge(+1)
        ToolTip.visible: hovered
        ToolTip.delay: 600
        ToolTip.text: qsTr("+%1").arg(DspController.formatDecimal(root.step, root.decimals))
    }

    Label {
        text: root.suffix
        Layout.preferredWidth: 24
        color: Theme.textDim
    }
}
