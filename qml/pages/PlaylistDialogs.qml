import QtQuick
import QtQuick.Dialogs
import Sonkore

// The playlist page's file dialogs, loaded (with their module) the first time
// one is opened.
Item {
    id: root

    signal folderChosen(url folder)
    signal filesChosen(var files)
    signal exportChosen(url file)

    function openAddFolder() { addFolderDialog.open() }
    function openAddFiles() { addFilesDialog.open() }
    function openExport() { exportDialog.open() }

    FileDialog {
        id: exportDialog
        title: qsTr("Export playlist")
        fileMode: FileDialog.SaveFile
        defaultSuffix: "m3u8"
        nameFilters: [qsTr("M3U playlist (*.m3u8 *.m3u)")]
        onAccepted: root.exportChosen(selectedFile)
    }
    // A local folder (sub-folders too) or loose files, in or out of the library.
    FolderDialog {
        id: addFolderDialog
        title: qsTr("Add a folder to the playlist")
        onAccepted: root.folderChosen(selectedFolder)
    }
    FileDialog {
        id: addFilesDialog
        title: qsTr("Add files to the playlist")
        fileMode: FileDialog.OpenFiles
        nameFilters: [qsTr("Audio files (%1)").arg(Library.audioNameFilter), qsTr("All files (*)")]
        onAccepted: root.filesChosen(selectedFiles)
    }
}
