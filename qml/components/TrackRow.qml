pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import Sonkore

// One track line with the user's columns. `rowData` is either a library
// model row or a track map (playlists, queue) — both expose the same keys.
Rectangle {
    id: row

    property var rowData
    property int rowIndex: -1
    property int number: rowIndex + 1
    property bool selected: false
    property bool showCover: Theme.listCovers
    property real flexWidth: 200
    property real primaryWidth: 0 // first column, narrowed when the list is short of room (TrackTable)
    property real restRatio: 0    // how far the other columns are narrowed towards their minimum (TrackTable)
    property real leadingWidth: 36
    readonly property string path: rowData && rowData.path ? rowData.path : ""
    readonly property bool missing: rowData && rowData.missing === true
    readonly property bool isCurrent: path.length > 0 && path === DspController.currentPath
    readonly property bool showArtistLine: !Theme.listCompact && !TrackColumns.isVisible("artist")

    signal clicked(int modifiers)
    signal doubleClicked()
    signal contextMenu(real x, real y)
    signal dragStarted(point scenePos)
    signal dragMoved(point scenePos)
    signal dragEnded()

    implicitHeight: Theme.rowHeight
    implicitWidth: content.implicitWidth + 40
    radius: Theme.listMode ? 0 : Theme.radiusSmall
    // The playing row stands out most; a selected one is a softer tint and
    // hover the faintest. All translucent, so every skin's background shows.
    color: isCurrent ? Qt.rgba(Theme.accent.r, Theme.accent.g, Theme.accent.b, Theme.dark ? 0.11 : 0.08)
         : selected ? Qt.rgba(Theme.accent.r, Theme.accent.g, Theme.accent.b, Theme.dark ? 0.045 : 0.035)
         : hover.hovered ? Qt.rgba(Theme.hover.r, Theme.hover.g, Theme.hover.b, 0.35)
         : (Theme.alternateRows && rowIndex % 2 === 1) ? Theme.rowAlt : "transparent"

    HoverHandler { id: hover }
    // One MouseArea handles click (with modifiers, no double-click delay),
    // double-click, context menu and dragging the row out of the list.
    MouseArea {
        anchors.fill: parent
        acceptedButtons: Qt.LeftButton | Qt.RightButton
        preventStealing: true          // the list must not turn a track drag into a scroll
        property point pressPos
        property bool dragging: false
        property bool dragged: false

        onPressed: (mouse) => {
            pressPos = Qt.point(mouse.x, mouse.y)
            dragging = false
            dragged = false
        }
        onPositionChanged: (mouse) => {
            if (!(mouse.buttons & Qt.LeftButton))
                return
            const scenePos = mapToItem(null, mouse.x, mouse.y)
            if (!dragging && Math.hypot(mouse.x - pressPos.x, mouse.y - pressPos.y) > 6) {
                dragging = true
                dragged = true
                row.dragStarted(scenePos)
            } else if (dragging) {
                row.dragMoved(scenePos)
            }
        }
        onReleased: if (dragging) { dragging = false; row.dragEnded() }
        onCanceled: if (dragging) { dragging = false; row.dragEnded() }
        onClicked: (mouse) => {
            if (dragged)
                return
            if (mouse.button === Qt.RightButton)
                row.contextMenu(mouse.x, mouse.y)
            else
                row.clicked(mouse.modifiers)
        }
        onDoubleClicked: (mouse) => { if (mouse.button === Qt.LeftButton) row.doubleClicked() }
    }

    Row {
        id: content
        anchors.verticalCenter: parent.verticalCenter
        x: 0
        spacing: 0

        // # / playing indicator / hover play button
        Item {
            width: row.leadingWidth
            height: row.height
            Label {
                anchors.centerIn: parent
                visible: !row.isCurrent && !hover.hovered
                text: row.number > 0 ? row.number : ""
                color: Theme.textFaint
                font.pixelSize: Theme.fontSmall
                font.features: { "tnum": 1 }
            }
            PlayingBars {
                anchors.centerIn: parent
                visible: row.isCurrent && !hover.hovered
                running: visible && DspController.playing
            }
            IconButton {
                anchors.centerIn: parent
                tile: false
                visible: hover.hovered
                iconName: row.isCurrent && DspController.playing ? "pause" : "play"
                iconSize: Theme.listMode ? 10 : 13
                implicitHeight: Math.min(iconSize + 16, row.height)
                iconColor: Theme.text
                onClicked: row.isCurrent ? DspController.togglePlayPause() : row.doubleClicked()
            }
        }

        Item {
            visible: row.showCover
            width: visible ? (Theme.listSpacious ? 62 : 52) : 0
            height: row.height
            CoverArt {
                anchors.verticalCenter: parent.verticalCenter
                width: Theme.listSpacious ? 52 : 40
                height: width
                path: row.path
            }
        }

        Repeater {
            model: TrackColumns.keys
            delegate: Item {
                id: cell
                required property string modelData
                required property int index
                readonly property string key: modelData
                readonly property bool primary: index === 0  // the main column reads like a title
                width: TrackColumns.isFlex(key) ? row.flexWidth
                       : primary && row.primaryWidth > 0 ? row.primaryWidth
                       : TrackColumns.widthOf(key) - (TrackColumns.widthOf(key) - TrackColumns.minWidthOf(key)) * row.restRatio
                height: row.height

                // Title (+ artist underneath when the artist column is hidden)
                Column {
                    visible: cell.key === "title"
                    anchors.verticalCenter: parent.verticalCenter
                    x: TrackColumns.cellPadding
                    width: parent.width - 2 * TrackColumns.cellPadding
                    spacing: 2
                    Label {
                        width: parent.width
                        text: row.rowData ? (row.rowData.title || "") : ""
                        elide: Text.ElideRight
                        color: row.missing ? Theme.textFaint : (row.isCurrent ? Theme.accent : Theme.text)
                        font.pixelSize: Theme.fontBody + (Theme.listCompact ? 0 : 1)
                        font.weight: Theme.listMode ? Font.Normal : Font.Medium
                        font.strikeout: row.missing
                    }
                    Label {
                        width: parent.width
                        visible: row.showArtistLine
                        text: row.rowData ? (row.rowData.artist || "") : ""
                        elide: Text.ElideRight
                        color: Theme.textDim
                        font.pixelSize: Theme.fontSmall + 1
                    }
                }
                Badge {
                    visible: cell.key === "format" && text.length > 0 && !Theme.listMode
                    anchors.verticalCenter: parent.verticalCenter
                    x: TrackColumns.cellPadding
                    width: Math.min(implicitWidth, cell.width - 2 * TrackColumns.cellPadding)
                    text: cell.key === "format" ? TrackColumns.text("format", row.rowData) : ""
                    // The accent only for what stands out (hi-res, DSD); CD
                    // quality and lossy files stay quiet, so a list of them
                    // is not a column of green.
                    readonly property bool special: !!row.rowData && (row.rowData.bits > 16 || row.rowData.sampleRate > 48000
                                                                     || text.indexOf("DSD") >= 0)
                    tone: special ? Theme.accent : Theme.textDim
                }
                // Heart: filled in the accent for a favourite; the outline only
                // while the row is under the pointer.
                IconButton {
                    // The revision is part of the expression (not a bare statement the
                    // QML compiler may drop), so the heart follows every change.
                    readonly property bool favorite: Library.favoritesRevision >= 0 && row.path.length > 0
                                                     && Library.isFavorite(row.path)
                    visible: cell.key === "favorite" && row.path.length > 0
                    anchors.verticalCenter: parent.verticalCenter
                    x: TrackColumns.cellPadding - 6
                    tile: false
                    iconName: favorite ? "heart-filled" : "heart-empty"
                    iconSize: Theme.listMode ? 12 : 15
                    implicitHeight: Math.min(iconSize + 12, row.height)
                    implicitWidth: iconSize + 12
                    iconColor: favorite ? Theme.accent : hovered ? Theme.text : hover.hovered ? Theme.textDim : Theme.textFaint
                    opacity: favorite || hover.hovered ? 1 : 0
                    tip: favorite ? qsTr("Remove from favorites") : qsTr("Add to favorites")
                    onClicked: Library.toggleFavorite(row.path)
                }
                Label {
                    visible: cell.key !== "title" && cell.key !== "favorite" && (cell.key !== "format" || Theme.listMode)
                    anchors.verticalCenter: parent.verticalCenter
                    x: TrackColumns.cellPadding
                    width: parent.width - 2 * TrackColumns.cellPadding
                    text: visible ? TrackColumns.text(cell.key, row.rowData) : ""
                    elide: cell.key === "folder" ? Text.ElideMiddle : Text.ElideRight
                    color: !cell.primary ? Theme.textDim
                           : row.missing ? Theme.textFaint : (row.isCurrent ? Theme.accent : Theme.text)
                    font.pixelSize: Theme.fontBody + (cell.primary && !Theme.listCompact ? 1 : 0)
                    font.weight: cell.primary && !Theme.listMode ? Font.Medium : Font.Normal
                    font.strikeout: cell.primary && row.missing
                    font.features: { "tnum": 1 }
                }
            }
        }
    }
}
