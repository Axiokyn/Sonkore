pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Sonkore

// Artists or Genres: the list on the left, the picked one's tracks on the
// right. The title bar's search narrows the list.
Item {
    id: page

    property string field: "artist"   // artist | genre
    property string selected          // "" = all of them

    // Shown in the window's title bar.
    readonly property string pageTitle: field === "genre" ? qsTr("Genres") : qsTr("Artists")
    readonly property string pageSubtitle: (field === "genre" ? qsTr("%1 genres") : qsTr("%1 artists")).arg(browser.entries.length)
                                           + (table.selectedCount > 1 ? qsTr("  ·  %1 selected").arg(table.selectedCount) : "")

    function play(shuffled) {
        const paths = tracks.paths()
        if (paths.length === 0)
            return
        DspController.shuffle = shuffled
        DspController.playPaths(paths, shuffled ? Math.floor(Math.random() * paths.length) : 0)
    }

    TrackFilterModel {
        id: tracks
        artist: page.field === "artist" ? page.selected : ""
        genre: page.field === "genre" ? page.selected : ""
    }

    RowLayout {
        anchors.fill: parent
        anchors.leftMargin: 8
        // Below the page heading, which reaches a little under the title bar.
        anchors.topMargin: 18
        spacing: 0

        ColumnBrowser {
            id: browser
            Layout.fillHeight: true
            Layout.preferredWidth: Math.min(300, Math.max(200, page.width * 0.26))
            Layout.minimumWidth: 200
            field: page.field
            filterText: AppActions.search
            selected: page.selected
            onPicked: (key) => page.selected = key
            onActivated: (key) => { page.selected = key; page.play(false) }
        }
        // Air on both sides of the divider: the list's scroll bar and the
        // track table never touch it.
        Rectangle { Layout.fillHeight: true; Layout.leftMargin: 6; implicitWidth: 1; color: Theme.border }

        ColumnLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            spacing: 8

            RowLayout {
                Layout.fillWidth: true
                Layout.leftMargin: 16
                Layout.rightMargin: 12
                spacing: 8
                ColumnLayout {
                    Layout.fillWidth: true
                    spacing: 0
                    Label {
                        Layout.fillWidth: true
                        text: page.selected.length === 0 ? browser.nameOf({ key: "" })
                              : browser.nameOf(browser.entries.find(e => e.key === page.selected) || { key: page.selected, name: page.selected })
                        elide: Text.ElideRight
                        color: Theme.text
                        font.pixelSize: Theme.fontTitle + 2
                        font.weight: Font.DemiBold
                    }
                    Label {
                        text: AppActions.tracksText(tracks.count)
                        color: Theme.textDim
                        font.pixelSize: Theme.fontSmall + 1
                    }
                }
                PillButton { primary: true; iconName: "play"; text: qsTr("Play"); enabled: tracks.count > 0; onClicked: page.play(false) }
                PillButton { iconName: "shuffle"; text: qsTr("Shuffle"); enabled: tracks.count > 0; onClicked: page.play(true) }
            }

            TrackTable {
                id: table
                Layout.fillWidth: true
                Layout.fillHeight: true
                Layout.leftMargin: 12
                model: tracks
                filter: tracks
                rememberSort: true
                onActivated: (row) => DspController.playPaths(tracks.paths(), row)
            }
        }
    }

    Label {
        anchors.centerIn: parent
        visible: Library.count === 0 && !Library.scanning
        text: qsTr("Add a music folder to browse your library")
        color: Theme.textDim
        font.pixelSize: Theme.fontTitle
    }
}
