import QtQuick
import QtQuick.Controls
import Sonkore

// Themed menu (every context and pop-up menu): rounded, with a soft shadow,
// LMenuItem rows. Matches the LComboBox pop-up.
Menu {
    id: menu
    padding: 5
    delegate: LMenuItem {} // items made from Actions, and sub-menus
    // The card's colour (a menu may sit darker, like the compact player's).
    property color tone: Theme.surface
    // A long menu scrolls without bouncing past its ends.
    Component.onCompleted: if (contentItem && contentItem.boundsBehavior !== undefined)
                               contentItem.boundsBehavior = Flickable.StopAtBounds
    // In a pop-up window of its own (Popup.Window, Qt 6.8+) the window is
    // only as big as the card: the shadow would be cut off at the sides and
    // left as dark crescents in the corners, so it is not drawn: a dark
    // outline sets it apart from what lies behind instead.
    readonly property bool ownWindow: menu.popupType !== undefined && menu.popupType !== 0
    background: Item {
        implicitWidth: 220
        // soft shadow: two faint rings outside the edge, lower than it
        RoundRect {
            visible: !menu.ownWindow
            anchors.fill: parent
            anchors.margins: -3
            anchors.topMargin: -1
            radius: card.radius + 3
            color: "transparent"
            borderWidth: 3
            borderColor: Qt.rgba(0, 0, 0, Theme.dark ? 0.18 : 0.06)
        }
        RoundRect {
            visible: !menu.ownWindow
            anchors.fill: parent
            anchors.margins: -1
            anchors.bottomMargin: -2
            radius: card.radius + 1
            color: "transparent"
            borderWidth: 1
            borderColor: Qt.rgba(0, 0, 0, Theme.dark ? 0.35 : 0.1)
        }
        RoundRect {
            id: card
            anchors.fill: parent
            radius: Theme.radius
            color: menu.tone
            borderWidth: 1
            borderColor: menu.ownWindow ? Qt.rgba(0, 0, 0, Theme.dark ? 0.85 : 0.3) : Theme.border
            // a faint light edge just inside the outline
            RoundRect {
                visible: menu.ownWindow
                anchors.fill: parent
                anchors.margins: 1
                radius: parent.radius - 1
                color: "transparent"
                borderWidth: 1
                borderColor: Theme.dark ? Qt.rgba(1, 1, 1, 0.07) : Qt.rgba(1, 1, 1, 0.6)
            }
        }
    }
    enter: Transition {
        NumberAnimation { property: "opacity"; from: 0; to: 1; duration: 110 }
        NumberAnimation { property: "scale"; from: 0.97; to: 1; duration: 110; easing.type: Easing.OutCubic }
    }
    exit: Transition {
        NumberAnimation { property: "opacity"; from: 1; to: 0; duration: 80 }
    }
}
