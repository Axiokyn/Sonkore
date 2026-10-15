import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Sonkore

Item {
    id: page

    // The Favorites page: the same list, only the tracks with a heart.
    property bool favoritesOnly: false

    // Shown in the window's title bar.
    readonly property string pageTitle: favoritesOnly ? qsTr("Favorites") : qsTr("Tracks")
    readonly property string pageSubtitle: (favoritesOnly ? AppActions.tracksText(tracks.count)
          : Library.scanning
          ? qsTr("Scanning… %1 files").arg(Library.scannedFiles)
          : (tracks.count === Library.count
             ? AppActions.tracksText(Library.count) + " · " + page.hoursText(Library.totalDuration)
             : qsTr("%1 of %2 tracks").arg(tracks.count).arg(Library.count)))
          + (table.selectedCount > 1 ? qsTr("  ·  %1 selected").arg(table.selectedCount) : "")

    function hoursText(seconds) {
        const h = Math.floor(seconds / 3600), m = Math.round((seconds % 3600) / 60)
        return h > 0 ? qsTr("%1 h %2 min").arg(h).arg(m) : qsTr("%1 min").arg(m)
    }

    component Pane: ColumnBrowser {
        Layout.fillWidth: true
        Layout.fillHeight: true
        showDetail: false
        radius: Theme.radiusSmall
        color: Theme.surface
        border.width: 1
        border.color: Theme.border
    }

    TrackFilterModel {
        id: tracks
        search: AppActions.search
        favoritesOnly: page.favoritesOnly
        genre: Prefs.store.trackBrowser ? genreColumn.selected : ""
        artist: Prefs.store.trackBrowser ? artistColumn.selected : ""
        album: Prefs.store.trackBrowser ? albumColumn.selected : ""
    }

    ColumnLayout {
        anchors.fill: parent
        anchors.leftMargin: 8
        anchors.rightMargin: 0 // the scroll bar sits against the window edge
        anchors.topMargin: 4
        spacing: 12

        // Genre → Artist → Album panes (MusicBee, iTunes, JRiver): each one
        // narrows the next and the tracks below.
        RowLayout {
            Layout.fillWidth: true
            Layout.preferredHeight: Math.round(Math.min(240, page.height * 0.32))
            Layout.rightMargin: 8
            visible: Prefs.store.trackBrowser && Library.count > 0 && !page.favoritesOnly
            spacing: 6
            Pane {
                id: genreColumn
                title: qsTr("Genre")
                field: "genre"
                onPicked: (key) => { selected = key; artistColumn.selected = ""; albumColumn.selected = "" }
            }
            Pane {
                id: artistColumn
                title: qsTr("Artist")
                field: "artist"
                genre: genreColumn.selected
                onPicked: (key) => { selected = key; albumColumn.selected = "" }
            }
            Pane {
                id: albumColumn
                title: qsTr("Album")
                field: "album"
                genre: genreColumn.selected
                artist: artistColumn.selected
                onPicked: (key) => selected = key
            }
        }

        TrackTable {
            id: table
            Layout.fillWidth: true
            Layout.fillHeight: true
            visible: tracks.count > 0
            model: tracks
            filter: tracks
            rememberSort: true
            onActivated: (row) => DspController.playPaths(tracks.paths(), row)
        }
        Item { Layout.fillHeight: true; visible: tracks.count === 0 }
    }

    // No favourites yet
    ColumnLayout {
        anchors.centerIn: parent
        visible: page.favoritesOnly && Library.favoriteCount === 0
        spacing: 10
        Icon {
            Layout.alignment: Qt.AlignHCenter
            glyph: "heart"
            size: 48
            color: Theme.textFaint
        }
        Label {
            Layout.alignment: Qt.AlignHCenter
            text: qsTr("No favorites yet")
            color: Theme.text
            font.pixelSize: 20
            font.weight: Font.DemiBold
        }
        Label {
            Layout.alignment: Qt.AlignHCenter
            text: qsTr("Click the heart on any track to keep it here.")
            color: Theme.textDim
            font.pixelSize: Theme.fontBody
        }
    }

    // empty state
    ColumnLayout {
        anchors.centerIn: parent
        visible: Library.count === 0 && !Library.scanning && !page.favoritesOnly
        spacing: 14
        Image {
            Layout.alignment: Qt.AlignHCenter
            source: "qrc:/icons/sonkore.svg"
            sourceSize: Qt.size(72, 72)
        }
        Label {
            Layout.alignment: Qt.AlignHCenter
            text: qsTr("Your library is empty")
            color: Theme.text
            font.pixelSize: 20
            font.weight: Font.DemiBold
        }
        Label {
            Layout.alignment: Qt.AlignHCenter
            text: qsTr("Add a music folder — it's watched and kept in sync automatically.\nFLAC, WAV, MP3, ALAC, AAC, Opus, DSD, CUE sheets and more. You can also drop files here.")
            horizontalAlignment: Text.AlignHCenter
            color: Theme.textDim
            font.pixelSize: Theme.fontBody
        }
        RowLayout {
            Layout.alignment: Qt.AlignHCenter
            Layout.topMargin: 6
            spacing: 8
            PillButton { primary: true; iconName: "folder-plus"; text: qsTr("Add music folder"); onClicked: AppActions.addFolderRequested() }
            PillButton { iconName: "file-plus"; text: qsTr("Add files"); onClicked: AppActions.addFilesRequested() }
        }
    }

    Label {
        anchors.centerIn: parent
        visible: Library.count > 0 && tracks.count === 0 && (!page.favoritesOnly || Library.favoriteCount > 0)
        text: qsTr("No tracks match “%1”").arg(AppActions.search)
        color: Theme.textDim
        font.pixelSize: Theme.fontTitle
    }
}
