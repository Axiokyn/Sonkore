pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import Sonkore

// ComboBox drawn with the active skin.
ComboBox {
    id: control

    implicitHeight: Theme.compact ? 28 : Theme.spacious ? 38 : 34
    hoverEnabled: true
    focusPolicy: Qt.NoFocus
    font.pixelSize: Theme.fontBody

    background: RoundRect {
        implicitWidth: 120
        radius: Theme.squareShapes ? Theme.radiusSmall : Math.min(height / 2, Theme.radius)
        color: control.pressed ? Theme.border : (control.hovered ? Theme.hover : Theme.surfaceHi)
        borderWidth: 1
        borderColor: control.visualFocus ? Theme.accent : Theme.border
    }
    contentItem: Label {
        leftPadding: 12
        rightPadding: control.indicator.width + 6
        text: control.displayText
        font: control.font
        color: Theme.text
        verticalAlignment: Text.AlignVCenter
        elide: Text.ElideRight
    }
    indicator: Icon {
        x: control.width - width - 10
        y: (control.height - height) / 2
        glyph: "chevron-down"
        size: 14
        color: Theme.textDim
    }
    popup: Popup {
        y: control.height + 4
        width: Math.max(control.width, 160)
        implicitHeight: Math.min(contentItem.implicitHeight + 12, 360)
        padding: 6
        contentItem: ListView {
            id: popupList
            clip: true
            boundsBehavior: Flickable.StopAtBounds
            WheelScroll { flickable: popupList }
            implicitHeight: contentHeight
            model: control.popup.visible ? control.delegateModel : null
            currentIndex: control.highlightedIndex
            ScrollBar.vertical: LScrollBar {}
        }
        background: RoundRect {
            radius: Theme.radius
            color: Theme.surface
            borderWidth: 1
            borderColor: Theme.border
        }
    }
    delegate: ItemDelegate {
        id: item
        required property int index
        required property var modelData
        width: ListView.view ? ListView.view.width : control.width
        height: Theme.compact ? 26 : Theme.spacious ? 36 : 32
        highlighted: control.highlightedIndex === index
        contentItem: Label {
            text: control.textRole ? item.modelData[control.textRole] : (control.displayFor ? control.displayFor(item.modelData) : item.modelData)
            color: control.currentIndex === item.index ? Theme.accent : Theme.text
            font.weight: control.currentIndex === item.index ? Font.DemiBold : Font.Normal
            verticalAlignment: Text.AlignVCenter
            elide: Text.ElideRight
        }
        background: Rectangle {
            radius: Theme.radiusSmall
            color: item.highlighted || item.hovered ? Theme.hover : "transparent"
        }
    }

    // Optional formatter for plain string models (e.g. filter type codes).
    property var displayFor: null
}
