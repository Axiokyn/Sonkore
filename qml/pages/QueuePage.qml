import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Sonkore

Item {
    id: page

    readonly property string pageTitle: qsTr("Play queue")
    readonly property string pageSubtitle: AppActions.tracksText(DspController.queue.length)
                                           + (DspController.shuffle ? qsTr(" · shuffle on") : "")

    TrackListModel {
        id: queueModel
        paths: DspController.queue
    }

    ColumnLayout {
        anchors.fill: parent
        anchors.leftMargin: 28
        anchors.rightMargin: 16
        anchors.topMargin: 0
        spacing: 8

        // Title and count in the window's title bar, as on the other pages.
        RowLayout {
            Layout.fillWidth: true
            Item { Layout.fillWidth: true }
            PillButton {
                iconName: "save"
                text: qsTr("Save as playlist")
                enabled: DspController.queue.length > 0
                onClicked: AppActions.newPlaylistRequested(DspController.queue)
            }
            PillButton {
                iconName: "trash"
                text: qsTr("Clear")
                enabled: DspController.queue.length > 0
                onClicked: DspController.clearQueue()
            }
        }

        TrackTable {
            id: table
            Layout.fillWidth: true
            Layout.fillHeight: true
            // out to the page edges: scroll bar against the window edge
            Layout.leftMargin: -20
            Layout.rightMargin: -16
            model: queueModel
            queueMode: true
            dragOrigin: "queue"
            onActivated: (row) => DspController.playIndex(row)
            // Opened: the playing track in view.
            Component.onCompleted: Qt.callLater(revealPlaying)
            onRemoveRequested: (rows) => DspController.removeFromQueue(rows)

            Label {
                anchors.centerIn: parent
                visible: DspController.queue.length === 0
                text: qsTr("The queue is empty.\nDouble-click any track, album or playlist — or drag tracks here.")
                horizontalAlignment: Text.AlignHCenter
                color: Theme.textDim
                font.pixelSize: Theme.fontBody + 1
            }
            DropArea {
                id: drop
                anchors.fill: parent
                keys: ["sonkore/tracks", "text/uri-list"]
                onDropped: (event) => {
                    // Rows of the queue itself: move them to the drop position.
                    if (!event.hasUrls && AppActions.dragOrigin === "queue") {
                        const p = table.list.mapFromItem(drop, event.x, event.y)
                        const row = table.list.indexAt(p.x + table.list.contentX, p.y + table.list.contentY)
                        DspController.moveInQueue(table.selectedRows(), row < 0 ? queueModel.count : row)
                        event.accept(Qt.MoveAction)
                        return
                    }
                    const paths = AppActions.dropPaths(event)
                    if (paths.length > 0) {
                        DspController.enqueue(paths)
                        AppActions.toast(qsTr("Added %1 to the queue").arg(AppActions.tracksText(paths.length)))
                    }
                    event.accept(Qt.CopyAction)
                }
                Rectangle {
                    anchors.fill: parent
                    visible: drop.containsDrag
                    radius: Theme.radius
                    color: "transparent"
                    border.width: 2
                    border.color: Theme.accent
                }
            }
        }
    }
}
