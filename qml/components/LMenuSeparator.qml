import QtQuick
import QtQuick.Controls
import Sonkore

// Divider between groups of an LMenu.
MenuSeparator {
    topPadding: 4
    bottomPadding: 4
    leftPadding: 8
    rightPadding: 8
    contentItem: Rectangle {
        implicitHeight: 1
        color: Theme.border
    }
}
