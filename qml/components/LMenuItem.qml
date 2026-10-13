import QtQuick
import QtQuick.Controls
import Sonkore

// Row of an LMenu: rounded highlight, a check mark in the accent for the
// chosen option (no box), an arrow for sub-menus.
MenuItem {
    id: item

    property string description // optional second line, dimmer
    property bool dense: false  // shorter rows, smaller text (long lists)
    property bool alignChecks: false // not checkable, but lined up with the items that are

    implicitHeight: !visible ? 0
                  : description.length > 0 ? titleLabel.implicitHeight + 1 + descriptionLabel.implicitHeight + 14
                  : dense ? 24
                  : (Theme.compact ? 28 : Theme.spacious ? 36 : 32)
    leftPadding: 10
    rightPadding: 10
    spacing: 8

    indicator: Item {
        x: item.leftPadding
        // Beside the title (the first line when there is a description).
        y: item.description.length > 0 ? item.topPadding + Math.round((titleLabel.height - height) / 2)
                                        : Math.round((item.height - height) / 2)
        width: item.checkable || item.alignChecks ? (item.dense ? 12 : 16) : 0
        height: 16
        Icon {
            anchors.centerIn: parent
            visible: item.checked
            glyph: "check"
            size: item.dense ? 12 : 14
            color: Theme.accent
        }
    }
    arrow: Icon {
        x: item.width - width - item.rightPadding
        anchors.verticalCenter: parent.verticalCenter
        visible: item.subMenu !== null
        glyph: "chevron-down"
        rotation: -90
        size: 12
        color: Theme.textDim
    }
    contentItem: Item {
        readonly property real textX: item.checkable || item.alignChecks ? item.indicator.width + item.spacing : 0
        readonly property real textWidth: width - textX - (item.subMenu ? 20 : 0)
        implicitWidth: textX + titleLabel.implicitWidth + (item.subMenu ? 20 : 0)
        Label {
            id: titleLabel
            x: parent.textX
            // One line: centred. With a description: at the top.
            y: item.description.length > 0 ? 0 : Math.round((parent.height - height) / 2)
            width: parent.textWidth
            text: item.text
            elide: Text.ElideRight
            color: !item.enabled ? Theme.textFaint : item.checked ? Theme.accent : Theme.text
            font.weight: item.checked ? Font.DemiBold : Font.Normal
            font.pixelSize: item.dense ? Theme.fontSmall + 1 : Theme.fontBody
        }
        Label {
            id: descriptionLabel
            x: parent.textX
            y: titleLabel.height + 1
            width: parent.textWidth
            visible: item.description.length > 0
            text: item.description
            wrapMode: Text.Wrap
            color: Theme.textDim
            font.pixelSize: Theme.fontSmall
        }
    }
    background: Rectangle {
        implicitWidth: 200
        radius: Theme.radiusSmall
        color: item.highlighted && item.enabled ? Theme.hover : "transparent"
    }
}
