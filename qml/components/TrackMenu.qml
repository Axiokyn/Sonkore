pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import Sonkore

// Context menu for one or more tracks.
LMenu {
    id: trackMenu

    property var paths: []
    property string playlistName      // set when the track lives in a playlist
    property int playlistIndex: -1
    property bool fromLibrary: true
    property bool queueMode: false        // the menu belongs to the play queue
    property string folderPath            // set when the menu is for a folder of the tree
    signal removeRequested()

    LMenuItem {
        text: qsTr("Play")
        onTriggered: DspController.playPaths(trackMenu.paths, 0)
    }
    LMenuItem {
        text: qsTr("Add to queue")
        onTriggered: {
            DspController.enqueue(trackMenu.paths)
            AppActions.toast(qsTr("Added to queue"))
        }
    }
    LMenu {
        id: addTo
        title: qsTr("Add to playlist")
        LMenuItem {
            text: qsTr("New playlist…")
            onTriggered: AppActions.newPlaylistRequested(trackMenu.paths)
        }
        LMenuSeparator { visible: Playlists.names.length > 0 }
        Instantiator {
            model: Playlists.names
            delegate: LMenuItem {
                id: playlistItem
                required property string modelData
                text: modelData
                onTriggered: {
                    Playlists.addTracks(playlistItem.modelData, trackMenu.paths)
                    AppActions.toast(qsTr("Added to “%1”").arg(playlistItem.modelData))
                }
            }
            onObjectAdded: (index, object) => addTo.insertItem(index + 2, object)
            onObjectRemoved: (index, object) => addTo.removeItem(object)
        }
    }
    LMenuItem {
        text: qsTr("Open containing folder")
        enabled: trackMenu.paths.length > 0 || trackMenu.folderPath.length > 0
        // A folder of the tree opens itself; tracks open the folder they are in.
        onTriggered: App.openContainingFolder(trackMenu.folderPath.length > 0 ? trackMenu.folderPath : trackMenu.paths[0])
    }
    // Only when a removal entry follows (a sub-folder of the tree has none).
    LMenuSeparator {
        visible: trackMenu.playlistName.length > 0 || trackMenu.queueMode || trackMenu.fromLibrary
                 || (trackMenu.folderPath.length > 0 && Library.folders.indexOf(trackMenu.folderPath) >= 0)
        height: visible ? implicitHeight : 0
    }
    LMenuItem {
        text: trackMenu.queueMode ? qsTr("Remove from queue") : qsTr("Remove from playlist")
        visible: trackMenu.playlistName.length > 0 || trackMenu.queueMode
        height: visible ? implicitHeight : 0
        onTriggered: trackMenu.removeRequested()
    }
    LMenuItem {
        text: trackMenu.paths.length > 1 ? qsTr("Remove %1 tracks from library").arg(trackMenu.paths.length) : qsTr("Remove from library")
        visible: trackMenu.fromLibrary && trackMenu.playlistName.length === 0 && !trackMenu.queueMode
        height: visible ? implicitHeight : 0
        onTriggered: Library.removeTracks(trackMenu.paths)
    }
    // A watched (root) folder of the tree stops being watched. The files
    // stay on disk.
    LMenuItem {
        text: qsTr("Remove folder from library")
        visible: trackMenu.folderPath.length > 0 && Library.folders.indexOf(trackMenu.folderPath) >= 0
        height: visible ? implicitHeight : 0
        onTriggered: AppActions.removeFolderRequested(trackMenu.folderPath) // asks first
    }
}
