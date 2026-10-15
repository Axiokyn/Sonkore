pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Sonkore

// Now playing: the track in large (cover, titles, format), the signal path
// under it, and the play queue beside it. Reached from the cover in the
// player bar and from the sidebar.
Item {
    id: page

    readonly property string pageTitle: qsTr("Now playing")
    readonly property string pageSubtitle: AppActions.tracksText(DspController.queue.length) + qsTr(" in the queue")
                                           + (DspController.shuffle ? qsTr(" · shuffle on") : "")
    readonly property var track: DspController.currentTrack
    readonly property bool hasTrack: DspController.currentPath.length > 0
    // Narrow pages: the queue goes under the track instead of beside it.
    readonly property bool stacked: width < 900
    // What the second column shows: the play queue or the lyrics (remembered).
    readonly property string pane: Prefs.store.playingPane === "lyrics" ? "lyrics" : "queue"

    TrackListModel {
        id: queueModel
        paths: DspController.queue
    }

    GridLayout {
        anchors.fill: parent
        anchors.leftMargin: 28
        anchors.rightMargin: 16
        columns: page.stacked ? 1 : 2
        columnSpacing: 28
        rowSpacing: 16

        // --- The track ------------------------------------------------------
        Flickable {
            id: trackPane
            Layout.preferredWidth: page.stacked ? page.width : Math.min(420, page.width * 0.4)
            Layout.minimumWidth: page.stacked ? 0 : 300
            Layout.fillHeight: !page.stacked
            Layout.preferredHeight: page.stacked ? Math.min(trackColumn.implicitHeight, page.height * 0.55) : -1
            Layout.fillWidth: page.stacked
            contentHeight: trackColumn.implicitHeight
            contentWidth: width
            clip: true
            boundsBehavior: Flickable.StopAtBounds
            WheelScroll { flickable: trackPane }
            ScrollBar.vertical: LScrollBar {}

            ColumnLayout {
                id: trackColumn
                width: trackPane.width
                spacing: 12

                CoverArt {
                    id: cover
                    Layout.topMargin: 4
                    Layout.preferredWidth: page.stacked ? Math.min(220, trackPane.width - 20) : Math.min(trackPane.width - 20, 380)
                    Layout.preferredHeight: Layout.preferredWidth
                    path: DspController.currentPath
                    radius: Theme.radius
                    iconSize: 64
                }
                Label {
                    Layout.fillWidth: true
                    text: page.hasTrack ? (page.track.title || "") : qsTr("Nothing playing")
                    wrapMode: Text.Wrap
                    maximumLineCount: 3
                    elide: Text.ElideRight
                    color: Theme.text
                    font.pixelSize: Theme.fontTitle + 7
                    font.weight: Font.Bold
                }
                Label {
                    Layout.fillWidth: true
                    visible: page.hasTrack
                    text: (page.track.artist || qsTr("Unknown artist")) + (page.track.album ? "  ·  " + page.track.album : "")
                    wrapMode: Text.Wrap
                    maximumLineCount: 2
                    elide: Text.ElideRight
                    color: Theme.textDim
                    font.pixelSize: Theme.fontBody + 2
                }
                Label {
                    Layout.fillWidth: true
                    visible: !page.hasTrack
                    text: qsTr("Pick something from your library: double-click a track, an album or a playlist.")
                    wrapMode: Text.Wrap
                    color: Theme.textDim
                    font.pixelSize: Theme.fontBody
                }
                Flow {
                    Layout.fillWidth: true
                    visible: page.hasTrack
                    spacing: 6
                    Badge {
                        text: DspController.trackDescription
                        tone: Theme.accent
                    }
                    Badge {
                        visible: DspController.outputSummary.length > 0
                        text: DspController.outputSummary
                        tone: DspController.bitPerfect ? Theme.good : Theme.textDim
                    }
                }
                // Go to where the track lives (its list or folder).
                PillButton {
                    visible: page.hasTrack
                    iconName: "locate"
                    text: qsTr("Show in library")
                    onClicked: AppActions.showPlayingRequested()
                }

                Rectangle {
                    Layout.fillWidth: true
                    Layout.topMargin: 6
                    height: 1
                    color: Theme.border
                }
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
                    Layout.rightMargin: 12
                    Layout.bottomMargin: 16
                }
            }
        }

        // --- The queue, or the lyrics ----------------------------------------
        ColumnLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            spacing: 8
            RowLayout {
                Layout.fillWidth: true
                Layout.topMargin: 4
                Label {
                    Layout.fillWidth: true
                    text: page.pane === "lyrics" ? qsTr("Lyrics") : qsTr("Play queue")
                    font.pixelSize: Theme.fontTitle
                    font.weight: Font.DemiBold
                    color: Theme.text
                }
                PillButton {
                    iconName: "queue"
                    text: qsTr("Queue")
                    primary: page.pane === "queue"
                    onClicked: Prefs.store.playingPane = "queue"
                }
                PillButton {
                    iconName: "tag"
                    text: qsTr("Lyrics")
                    primary: page.pane === "lyrics"
                    onClicked: Prefs.store.playingPane = "lyrics"
                }
                Rectangle { width: 1; height: 20; color: Theme.border; visible: page.pane === "queue" }
                PillButton {
                    visible: page.pane === "queue"
                    iconName: "save"
                    text: qsTr("Save as playlist")
                    enabled: DspController.queue.length > 0
                    onClicked: AppActions.newPlaylistRequested(DspController.queue)
                }
                PillButton {
                    visible: page.pane === "queue"
                    iconName: "trash"
                    text: qsTr("Clear")
                    enabled: DspController.queue.length > 0
                    onClicked: DspController.clearQueue()
                }
            }

            // The lyrics: timed lines follow the music and seek on a click.
            Item {
                visible: page.pane === "lyrics"
                Layout.fillWidth: true
                Layout.fillHeight: true

                ListView {
                    id: lyricsList
                    anchors.fill: parent
                    anchors.rightMargin: 4
                    clip: true
                    model: Lyrics.lines
                    spacing: 2
                    boundsBehavior: Flickable.StopAtBounds
                    readonly property int current: Lyrics.lines.length > 0 ? Lyrics.lineAt(DspController.position) : -1
                    currentIndex: current
                    highlightFollowsCurrentItem: Lyrics.synced
                    highlightRangeMode: Lyrics.synced ? ListView.ApplyRange : ListView.NoHighlightRange
                    preferredHighlightBegin: height * 0.38
                    preferredHighlightEnd: height * 0.62
                    highlightMoveDuration: 350
                    highlightMoveVelocity: -1
                    highlight: Item {}
                    header: Item { height: Lyrics.synced ? lyricsList.height * 0.38 : 8 }
                    footer: Item { height: Lyrics.synced ? lyricsList.height * 0.5 : 24 }
                    WheelScroll { flickable: lyricsList }
                    ScrollBar.vertical: LScrollBar {}

                    delegate: Item {
                        id: lineRow
                        required property var modelData
                        required property int index
                        readonly property bool sung: Lyrics.synced && index === lyricsList.current
                        readonly property bool past: Lyrics.synced && index < lyricsList.current
                        width: ListView.view.width
                        height: lineText.implicitHeight + 10
                        Label {
                            id: lineText
                            anchors.left: parent.left
                            anchors.right: parent.right
                            anchors.leftMargin: 8
                            anchors.rightMargin: 16
                            anchors.verticalCenter: parent.verticalCenter
                            text: lineRow.modelData.text.length > 0 ? lineRow.modelData.text : "♪"
                            wrapMode: Text.Wrap
                            color: lineRow.sung ? Theme.text : (lineRow.past ? Theme.textFaint : Theme.textDim)
                            font.pixelSize: Theme.fontTitle + (lineRow.sung ? 4 : 0)
                            font.weight: lineRow.sung ? Font.Bold : Font.Normal
                            Behavior on color { ColorAnimation { duration: 200 } }
                            Behavior on font.pixelSize { NumberAnimation { duration: 150 } }
                        }
                        HoverHandler { id: lineHover; enabled: lineRow.modelData.time >= 0; cursorShape: Qt.PointingHandCursor }
                        TapHandler {
                            enabled: lineRow.modelData.time >= 0
                            onTapped: DspController.seek(lineRow.modelData.time)
                        }
                    }
                }
                ColumnLayout {
                    anchors.centerIn: parent
                    width: Math.min(parent.width - 40, 420)
                    visible: Lyrics.lines.length === 0
                    spacing: 8
                    Label {
                        Layout.fillWidth: true
                        horizontalAlignment: Text.AlignHCenter
                        wrapMode: Text.Wrap
                        text: !page.hasTrack ? qsTr("Nothing playing")
                              : Lyrics.loading ? qsTr("Looking for the lyrics…")
                              : Lyrics.status.length > 0 ? Lyrics.status
                              : qsTr("No lyrics for this track")
                        color: Theme.textDim
                        font.pixelSize: Theme.fontBody + 1
                    }
                    Label {
                        Layout.fillWidth: true
                        visible: page.hasTrack && !Lyrics.loading && !Lyrics.onlineEnabled
                        horizontalAlignment: Text.AlignHCenter
                        wrapMode: Text.Wrap
                        text: qsTr("A .lrc file beside the track, its LYRICS tag, or LRCLIB with the switch in Settings › Library.")
                        color: Theme.textFaint
                        font.pixelSize: Theme.fontSmall + 1
                    }
                }
                Label {
                    anchors.right: parent.right
                    anchors.bottom: parent.bottom
                    anchors.margins: 8
                    visible: Lyrics.lines.length > 0
                    text: Lyrics.source === "file" ? qsTr("From a .lrc file beside the track")
                          : Lyrics.source === "embedded" ? qsTr("From the file's tags")
                          : qsTr("From LRCLIB")
                    color: Theme.textFaint
                    font.pixelSize: Theme.fontSmall
                }
            }

            TrackTable {
                id: table
                visible: page.pane === "queue"
                Layout.fillWidth: true
                Layout.fillHeight: true
                Layout.rightMargin: -16 // scroll bar against the window edge
                model: queueModel
                queueMode: true
                dragOrigin: "queue"
                onActivated: (row) => DspController.playIndex(row)
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
                            event.accept(Qt.CopyAction)
                        }
                    }
                }
            }
        }
    }
}
