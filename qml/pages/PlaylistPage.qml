import QtQuick
import QtQuick.Controls
import QtQuick.Dialogs
import QtQuick.Layouts
import Sonkore

Item {
    id: page

    property string name
    // Short windows: a smaller header leaves room for the tracks.
    readonly property bool shortHeader: Theme.compact || height < 560

    function reload() { playlistModel.paths = name.length > 0 ? Playlists.paths(name) : [] }

    onNameChanged: reload()
    Component.onCompleted: reload()
    Connections {
        target: Playlists
        function onPlaylistChanged(changed) { if (changed === page.name) page.reload() }
    }
    TrackListModel { id: playlistModel }

    NameDialog {
        id: renameDialog
        title: qsTr("Rename playlist")
        acceptText: qsTr("Rename")
        onAccepted: (newName) => AppActions.navigate("playlist", Playlists.rename(page.name, newName))
    }
    FileDialog {
        id: exportDialog
        title: qsTr("Export playlist")
        fileMode: FileDialog.SaveFile
        defaultSuffix: "m3u8"
        nameFilters: [qsTr("M3U playlist (*.m3u8 *.m3u)")]
        onAccepted: AppActions.toast(Playlists.exportM3u(page.name, selectedFile) ? qsTr("Playlist exported") : qsTr("Export failed"))
    }

    function addPaths(paths) {
        if (paths.length === 0) {
            AppActions.toast(qsTr("No audio files in that folder"))
            return
        }
        Playlists.addTracks(page.name, paths)
        AppActions.toast(qsTr("Added %1 to “%2”").arg(AppActions.tracksText(paths.length)).arg(page.name))
    }
    // A local folder (sub-folders too) or loose files, in or out of the library.
    FolderDialog {
        id: addFolderDialog
        title: qsTr("Add a folder to the playlist")
        onAccepted: page.addPaths(Library.expandAudioPaths([selectedFolder]))
    }
    FileDialog {
        id: addFilesDialog
        title: qsTr("Add files to the playlist")
        fileMode: FileDialog.OpenFiles
        nameFilters: [qsTr("Audio files (%1)").arg(Library.audioNameFilter), qsTr("All files (*)")]
        onAccepted: page.addPaths(Library.audioPaths(selectedFiles))
    }

    ColumnLayout {
        anchors.fill: parent
        anchors.leftMargin: 28
        anchors.rightMargin: 20
        anchors.topMargin: (page.shortHeader ? 2 : 8)
        spacing: page.shortHeader ? 12 : 18

        RowLayout {
            Layout.fillWidth: true
            spacing: page.shortHeader ? 16 : 22
            Rectangle {
                implicitWidth: page.shortHeader ? 104 : 160
                implicitHeight: implicitWidth
                radius: Theme.radius
                gradient: Gradient {
                    GradientStop { position: 0; color: Theme.accent }
                    GradientStop { position: 1; color: Qt.darker(Theme.accent, 2.2) }
                }
                Icon {
                    anchors.centerIn: parent
                    glyph: "playlist"
                    size: parent.width * 0.38
                    color: Theme.onAccent
                }
            }
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 6
                SectionLabel { text: qsTr("Playlist") }
                Label {
                    Layout.fillWidth: true
                    text: page.name
                    elide: Text.ElideRight
                    color: Theme.text
                    font.pixelSize: page.shortHeader ? 24 : 34
                    font.weight: Font.Bold
                }
                Label {
                    Layout.fillWidth: true
                    elide: Text.ElideRight
                    text: AppActions.tracksText(playlistModel.count) + " · " + DspController.formatTime(Playlists.duration(page.name))
                    color: Theme.textDim
                    font.pixelSize: Theme.fontBody
                }
                RowLayout {
                    Layout.topMargin: page.shortHeader ? 4 : 10
                    spacing: 8
                    PillButton {
                        primary: true
                        iconName: "play"
                        text: qsTr("Play")
                        enabled: playlistModel.count > 0
                        onClicked: { DspController.shuffle = false; DspController.playPaths(Playlists.paths(page.name), 0) }
                    }
                    PillButton {
                        iconName: "shuffle"
                        text: qsTr("Shuffle")
                        enabled: playlistModel.count > 0
                        onClicked: {
                            const paths = Playlists.paths(page.name)
                            DspController.shuffle = true
                            DspController.playPaths(paths, Math.floor(Math.random() * paths.length))
                        }
                    }
                    IconButton { iconName: "folder-plus"; tip: qsTr("Add a folder…"); onClicked: addFolderDialog.open() }
                    IconButton { iconName: "file-plus"; tip: qsTr("Add files…"); onClicked: addFilesDialog.open() }
                    IconButton { iconName: "edit"; tip: qsTr("Rename"); onClicked: renameDialog.ask(page.name) }
                    IconButton { iconName: "download"; tip: qsTr("Export as M3U"); onClicked: exportDialog.open() }
                    IconButton {
                        iconName: "trash"
                        tip: qsTr("Delete playlist")
                        onClicked: {
                            Playlists.remove(page.name)
                            AppActions.navigate("tracks", "")
                        }
                    }
                }
            }
        }

        TrackTable {
            id: table
            Layout.fillWidth: true
            Layout.fillHeight: true
            // out to the page edges: scroll bar against the window edge
            Layout.leftMargin: -20
            Layout.rightMargin: -20
            model: playlistModel
            dragOrigin: "playlist:" + page.name
            playlistName: page.name
            onActivated: (row) => DspController.playPaths(Playlists.paths(page.name), row)
            onRemoveRequested: (rows) => Playlists.removeRows(page.name, rows)

            ColumnLayout {
                anchors.centerIn: parent
                visible: playlistModel.count === 0
                spacing: 14
                Label {
                    Layout.alignment: Qt.AlignHCenter
                    text: qsTr("This playlist is empty.\nDrag tracks here from your library, or drop files and folders.")
                    horizontalAlignment: Text.AlignHCenter
                    color: Theme.textDim
                    font.pixelSize: Theme.fontBody + 1
                }
                RowLayout {
                    Layout.alignment: Qt.AlignHCenter
                    spacing: 8
                    PillButton { primary: true; iconName: "folder-plus"; text: qsTr("Add a folder…"); onClicked: addFolderDialog.open() }
                    PillButton { iconName: "file-plus"; text: qsTr("Add files…"); onClicked: addFilesDialog.open() }
                }
            }

            // Drops from the library, other playlists or the file manager;
            // dragging rows of this playlist reorders them.
            DropArea {
                id: drop
                anchors.fill: parent
                keys: ["sonkore/tracks", "text/uri-list"]
                onDropped: (event) => {
                    const p = table.list.mapFromItem(drop, event.x, event.y)
                    const row = table.list.indexAt(p.x + table.list.contentX, p.y + table.list.contentY)
                    if (!event.hasUrls && AppActions.dragOrigin === "playlist:" + page.name) {
                        const rows = table.selectedRows()
                        const moved = playlistModel.pathsOf(rows)
                        const target = row < 0 ? playlistModel.count : row
                        const before = rows.filter(r => r < target).length
                        Playlists.removeRows(page.name, rows)
                        Playlists.insertTracks(page.name, target - before, moved)
                    } else {
                        const paths = AppActions.dropPaths(event)
                        if (paths.length === 0)
                            return
                        if (row < 0)
                            Playlists.addTracks(page.name, paths)
                        else
                            Playlists.insertTracks(page.name, row, paths)
                        AppActions.toast(qsTr("Added %1 to “%2”").arg(AppActions.tracksText(paths.length)).arg(page.name))
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
