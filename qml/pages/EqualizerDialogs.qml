import QtQuick
import QtQuick.Dialogs
import Sonkore

// The equalizer page's profile file dialogs, loaded (with their module) the
// first time one is opened.
Item {
    id: root

    signal importChosen(url file)
    signal exportChosen(url file)

    function openImport() { importDialog.open() }
    function openExport() { exportDialog.open() }

    FileDialog {
        id: importDialog
        title: qsTr("Import parametric EQ profile")
        nameFilters: [qsTr("Parametric EQ (*.txt)"), qsTr("All files (*)")]
        onAccepted: root.importChosen(selectedFile)
    }
    FileDialog {
        id: exportDialog
        title: qsTr("Export EQ profile")
        fileMode: FileDialog.SaveFile
        defaultSuffix: "txt"
        nameFilters: [qsTr("Parametric EQ (*.txt)")]
        onAccepted: root.exportChosen(selectedFile)
    }
}
