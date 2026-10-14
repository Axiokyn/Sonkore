pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Sonkore

// Tracks inside the folder picked in the sidebar's folder tree (recursively).
Item {
    id: page

    property string folder
    // The sidebar shows icons only (no tree): browse sub-folders from here.
    property bool showSubfolders: false
    readonly property string folderName: folder.length > 0 ? folder.split("/").pop() : qsTr("All folders")

    // Shown in the window's title bar. No subtitle: the folder tree already
    // shows where the folder is and how many tracks it holds.
    readonly property string pageTitle: folderName

    TrackFilterModel {
        id: tracks
        folder: page.folder
        search: AppActions.search
    }

    // The playing track in the middle of the list, when it is in this folder.
    function revealPlaying() {
        const row = tracks.rowOfPath(DspController.currentPath)
        if (row >= 0)
            trackTable.list.positionViewAtIndex(row, ListView.Center)
    }
    Component.onCompleted: Qt.callLater(revealPlaying)
    onFolderChanged: Qt.callLater(revealPlaying)


        // --- Tracks of the selected folder (the tree is in the sidebar) -------
        Item {
            anchors.fill: parent

            ColumnLayout {
                anchors.fill: parent
                anchors.leftMargin: 8
                anchors.rightMargin: 0 // the scroll bar sits against the window edge
                anchors.topMargin: 4
                spacing: 12

                // Sub-folders of the current folder, plus a way back up.
                Loader {
                    Layout.fillWidth: true
                    active: page.showSubfolders
                    visible: active
                    sourceComponent: ListView {
                        id: subfolders
                        property var entries: []
                        function refresh() {
                            let list = folderModel.childrenOf(page.folder)
                            // A single music folder: start one level down.
                            if (page.folder.length === 0 && list.length === 1)
                                list = list.concat(folderModel.childrenOf(list[0].path))
                            if (page.folder.length > 0) {
                                const roots = Library.folders
                                const up = roots.indexOf(page.folder) >= 0 ? "" : page.folder.substring(0, page.folder.lastIndexOf("/"))
                                list.unshift({ name: qsTr("Up"), path: up, count: -1 })
                            }
                            entries = list
                        }
                        FolderTreeModel {
                            id: folderModel
                            onFoldersChanged: subfolders.refresh()
                        }
                        Connections {
                            target: page
                            function onFolderChanged() { subfolders.refresh() }
                        }
                        Component.onCompleted: refresh()
                        implicitHeight: count > 0 ? (Theme.compact ? 28 : 34) : 0
                        orientation: ListView.Horizontal
                        spacing: 6
                        clip: true
                        boundsBehavior: Flickable.StopAtBounds
                        model: entries
                        WheelScroll { flickable: subfolders; vertical: false }
                        delegate: PillButton {
                            required property var modelData
                            iconName: modelData.count < 0 ? "back" : "folder"
                            text: modelData.count < 0 ? modelData.name : modelData.name + "  " + modelData.count
                            onClicked: AppActions.navigate("folders", modelData.path)
                        }
                    }
                }

                TrackTable {
                    id: trackTable
                    visible: Library.count > 0 || Library.scanning
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    model: tracks
                    filter: tracks
                    rememberSort: true
                    onActivated: (row) => DspController.playPaths(tracks.paths(), row)
                }
            }
        }

    // No music at all (the last folder was removed): the way to add some.
    ColumnLayout {
        anchors.centerIn: parent
        visible: Library.count === 0 && !Library.scanning
        spacing: 14
        Label {
            Layout.alignment: Qt.AlignHCenter
            text: qsTr("Your library is empty")
            color: Theme.text
            font.pixelSize: 20
            font.weight: Font.DemiBold
        }
        PillButton {
            Layout.alignment: Qt.AlignHCenter
            primary: true
            iconName: "folder-plus"
            text: qsTr("Add music folder")
            onClicked: AppActions.addFolderRequested()
        }
    }
    Label {
        anchors.centerIn: parent
        visible: Library.count > 0 && tracks.count === 0 && AppActions.search.length > 0
        text: qsTr("No tracks match “%1”").arg(AppActions.search)
        color: Theme.textDim
        font.pixelSize: Theme.fontTitle
    }
}
