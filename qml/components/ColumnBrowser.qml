pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Sonkore

// One browser column: the genres, artists or albums of the library (within
// the given genre / artist), with an "All" row on top. Used by the Artists
// and Genres pages and by the Genre → Artist → Album panes above the tracks.
Rectangle {
    id: column

    property string title
    property string field: "artist"      // genre | artist | album
    property string genre                 // narrows the list ("" = any)
    property string artist
    property string filterText            // case-insensitive match on the names
    property string selected              // key of the picked entry ("" = All)
    property bool showDetail: true        // "3 albums · 41 tracks" under each name

    signal picked(string key)
    signal activated(string key)          // double-click: play it

    // Re-read whenever the library changes.
    readonly property var entries: {
        Library.count
        Library.scanning
        const all = Library.browse(field, genre, artist)
        const words = filterText.toLowerCase().split(" ").filter(w => w.length > 0)
        return words.length === 0 ? all
             : all.filter(e => words.every(w => e.name.toLowerCase().indexOf(w) >= 0))
    }

    function nameOf(entry) {
        if (entry.key.length === 0)
            return field === "genre" ? qsTr("All genres") : field === "album" ? qsTr("All albums") : qsTr("All artists")
        if (entry.key === Library.unknownKey)
            return field === "genre" ? qsTr("No genre") : field === "album" ? qsTr("No album") : qsTr("Unknown artist")
        return entry.name
    }
    function detailOf(entry) {
        if (entry.key.length === 0)
            return qsTr("%1 items").arg(column.entries.length)
        if (field === "album")
            return AppActions.tracksText(entry.count)
        return (entry.albums === 1 ? qsTr("1 album") : qsTr("%1 albums").arg(entry.albums)) + " · " + AppActions.tracksText(entry.count)
    }

    // A filter that no longer matches anything falls back to All.
    onEntriesChanged: if (selected.length > 0 && !entries.some(e => e.key === selected)) picked("")

    color: "transparent"
    border.width: 0

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: column.border.width > 0 ? 4 : 0
        anchors.topMargin: column.border.width > 0 ? 8 : 0
        spacing: 4

        SectionLabel {
            visible: column.title.length > 0
            Layout.leftMargin: 10
            Layout.topMargin: 2
            text: column.title
        }
        ListView {
            id: list
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            boundsBehavior: Flickable.StopAtBounds
            WheelScroll { flickable: list }
            ScrollBar.vertical: LScrollBar { id: vbar }
            spacing: 3
            model: [{ key: "", name: "", count: 0, albums: 0 }].concat(column.entries)
            delegate: AbstractButton {
                id: row
                required property var modelData
                required property int index
                readonly property bool current: column.selected === modelData.key
                // Room for the scroll bar while there is one, so it never
                // sits on the highlight.
                width: list.width - x - (vbar.scrollable ? vbar.width + 2 : 6)
                x: 4
                implicitHeight: column.showDetail && !Theme.compact ? 50 : (Theme.compact ? 26 : 32)
                hoverEnabled: true
                focusPolicy: Qt.NoFocus
                onClicked: column.picked(modelData.key)
                onDoubleClicked: column.activated(modelData.key)
                background: Rectangle {
                    radius: Theme.radiusSmall
                    color: row.current ? Theme.accentSoft : (row.hovered ? Theme.hover : "transparent")
                }
                // Name and detail as one block, centred in the row (not spread
                // to its edges).
                contentItem: Item {
                    Column {
                        anchors.verticalCenter: parent.verticalCenter
                        x: 8
                        width: parent.width - 16
                        spacing: 2
                        Label {
                            width: parent.width
                            text: column.nameOf(row.modelData)
                            elide: Text.ElideRight
                            color: row.current ? Theme.text : (row.modelData.key === Library.unknownKey ? Theme.textDim : Theme.text)
                            font.weight: row.current || row.index === 0 ? Font.DemiBold : Font.Normal
                        }
                        Label {
                            width: parent.width
                            visible: column.showDetail && !Theme.compact
                            text: column.detailOf(row.modelData)
                            elide: Text.ElideRight
                            color: Theme.textFaint
                            font.pixelSize: Theme.fontSmall
                        }
                    }
                }
            }
        }
    }
}
