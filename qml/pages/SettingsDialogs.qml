import QtQuick
import QtQuick.Dialogs
import Sonkore

// The Settings page's file dialogs, loaded (with their module) the first time
// one is opened.
Item {
    id: root

    signal impulseChosen(url file)
    signal skipFolderChosen(url folder)

    function openImpulse() { irDialog.open() }
    function openSkipFolder() { skipFolderDialog.open() }

    FileDialog {
        id: irDialog
        title: qsTr("Impulse response")
        nameFilters: [qsTr("Impulse responses (*.wav *.flac *.aif *.aiff)"), qsTr("All files (*)")]
        onAccepted: root.impulseChosen(selectedFile)
    }
    FolderDialog {
        id: skipFolderDialog
        title: qsTr("Folder to skip")
        currentFolder: Library.folders.length > 0 ? "file://" + Library.folders[0] : ""
        onAccepted: root.skipFolderChosen(selectedFolder)
    }
}
