import QtQuick
import QtQuick.Dialogs
import Sonkore

// The main window's file and folder dialogs, in a file of their own: Main.qml
// loads it (and with it the QtQuick.Dialogs module, three libraries) the first
// time one of them is asked for, not at start-up.
Item {
    id: root

    signal folderChosen(url folder)
    signal filesChosen(var files)
    signal playlistFileChosen(url file)
    signal playlistFolderChosen(url folder)
    readonly property bool playlistFolderOpen: playlistFolderDialog.visible

    function openFolder() { folderDialog.open() }
    function openFiles() { filesDialog.open() }
    function openImportPlaylist() { importPlaylistDialog.open() }
    function openPlaylistFolder() { playlistFolderDialog.open() }

    FolderDialog {
        id: folderDialog
        title: qsTr("Add music folder")
        onAccepted: root.folderChosen(selectedFolder)
    }
    FileDialog {
        id: filesDialog
        title: qsTr("Add audio files")
        fileMode: FileDialog.OpenFiles
        nameFilters: [qsTr("Audio files (%1)").arg(Library.audioNameFilter), qsTr("All files (*)")]
        onAccepted: root.filesChosen(selectedFiles)
    }
    FileDialog {
        id: importPlaylistDialog
        title: qsTr("Import playlist")
        nameFilters: [qsTr("M3U playlist (*.m3u *.m3u8)")]
        onAccepted: root.playlistFileChosen(selectedFile)
    }
    // A local folder's audio (sub-folders too) for the playlist being created.
    FolderDialog {
        id: playlistFolderDialog
        title: qsTr("Folder for the playlist")
        onAccepted: root.playlistFolderChosen(selectedFolder)
    }
}
