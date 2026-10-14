pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Sonkore

Item {
    id: page

    property string openAlbum
    readonly property var album: openAlbum.length > 0 ? albums.byKey(openAlbum) : ({})

    // Shown in the window's title bar.
    readonly property string pageTitle: qsTr("Albums")
    readonly property string pageSubtitle: qsTr("%1 albums").arg(albums.count)

    function playAlbum(key, shuffled) {
        const paths = Library.albumPaths(key)
        if (paths.length === 0)
            return
        DspController.shuffle = shuffled
        DspController.playPaths(paths, shuffled ? Math.floor(Math.random() * paths.length) : 0)
    }

    AlbumModel {
        id: albums
        search: AppActions.search
    }
    TrackFilterModel {
        id: albumTracks
        albumKey: page.openAlbum
    }
    TrackMenu { id: menu }

    StackLayout {
        anchors.fill: parent
        currentIndex: page.openAlbum.length > 0 ? 1 : 0

        // --- Grid ------------------------------------------------------------
        ColumnLayout {
            spacing: 0

            GridView {
                id: grid
                WheelScroll { flickable: grid }
                Layout.fillWidth: true
                Layout.fillHeight: true
                Layout.leftMargin: 18
                clip: true
                model: albums
                readonly property int columns: Math.max(2, Math.floor(width / (Theme.compact ? 150 : 196)))
                cellWidth: Math.floor((width - 8) / columns)
                cellHeight: cellWidth + 58
                boundsBehavior: Flickable.StopAtBounds
                ScrollBar.vertical: LScrollBar {}

                delegate: Item {
                    id: cell
                    required property var model
                    width: grid.cellWidth
                    height: grid.cellHeight

                    HoverHandler { id: cellHover }
                    TapHandler { onTapped: page.openAlbum = cell.model.albumKey }

                    Column {
                        anchors.fill: parent
                        anchors.margins: 10
                        spacing: 8

                        Item {
                            width: parent.width
                            height: width
                            CoverArt {
                                anchors.fill: parent
                                path: cell.model.coverPath
                                radius: Theme.radius
                                scale: cellHover.hovered ? 1.02 : 1.0
                                Behavior on scale { NumberAnimation { duration: 140 } }
                            }
                            AbstractButton {
                                anchors.right: parent.right
                                anchors.bottom: parent.bottom
                                anchors.margins: 10
                                width: 44
                                height: 44
                                opacity: cellHover.hovered ? 1 : 0
                                Behavior on opacity { NumberAnimation { duration: 140 } }
                                onClicked: page.playAlbum(cell.model.albumKey, false)
                                background: RoundRect {
                                    radius: width / 2
                                    color: Theme.accent
                                }
                                contentItem: Item {
                                    Icon {
                                        anchors.centerIn: parent
                                        glyph: "play"
                                        size: 18
                                        color: Theme.onAccent
                                    }
                                }
                            }
                        }
                        Label {
                            width: parent.width
                            text: cell.model.album
                            elide: Text.ElideRight
                            color: Theme.text
                            font.pixelSize: Theme.fontBody + 1
                            font.weight: Font.DemiBold
                        }
                        Label {
                            width: parent.width
                            text: cell.model.artist + (cell.model.year > 0 ? " · " + cell.model.year : "")
                            elide: Text.ElideRight
                            color: Theme.textDim
                            font.pixelSize: Theme.fontSmall + 1
                        }
                    }
                }
            }
        }

        // --- Album detail ----------------------------------------------------
        ColumnLayout {
            spacing: 18

            IconButton {
                Layout.leftMargin: 20
                Layout.topMargin: 4
                iconName: "back"
                tip: qsTr("Back to albums")
                onClicked: page.openAlbum = ""
            }

            RowLayout {
                Layout.fillWidth: true
                Layout.leftMargin: 28
                Layout.rightMargin: 28
                spacing: 26

                CoverArt {
                    Layout.preferredWidth: Theme.compact ? 140 : 210
                    Layout.preferredHeight: Theme.compact ? 140 : 210
                    path: page.album.coverPath || ""
                    radius: Theme.radius
                }
                ColumnLayout {
                    Layout.fillWidth: true
                    spacing: 6
                    SectionLabel { text: qsTr("Album") }
                    Label {
                        Layout.fillWidth: true
                        text: page.album.album || ""
                        wrapMode: Text.Wrap
                        maximumLineCount: 2
                        elide: Text.ElideRight
                        color: Theme.text
                        font.pixelSize: Theme.compact ? 24 : 36
                        font.weight: Font.Bold
                    }
                    Label {
                        text: page.album.artist || ""
                        color: Theme.text
                        font.pixelSize: Theme.fontTitle + 1
                        font.weight: Font.DemiBold
                    }
                    Label {
                        text: [page.album.year > 0 ? page.album.year : "",
                               qsTr("%1 tracks").arg(page.album.trackCount || 0),
                               DspController.formatTime(page.album.duration || 0),
                               page.album.format || ""].filter(s => String(s).length > 0).join("  ·  ")
                        color: Theme.textDim
                        font.pixelSize: Theme.fontBody
                    }
                    RowLayout {
                        Layout.topMargin: 12
                        spacing: 8
                        PillButton { primary: true; iconName: "play"; text: qsTr("Play"); onClicked: page.playAlbum(page.openAlbum, false) }
                        PillButton { iconName: "shuffle"; text: qsTr("Shuffle"); onClicked: page.playAlbum(page.openAlbum, true) }
                        IconButton {
                            id: albumMore
                            iconName: "more"
                            onClicked: {
                                menu.paths = Library.albumPaths(page.openAlbum)
                                menu.popup(albumMore, 0, albumMore.height)
                            }
                        }
                    }
                }
            }

            TrackTable {
                Layout.fillWidth: true
                Layout.fillHeight: true
                Layout.leftMargin: 8
                Layout.rightMargin: 0 // the scroll bar sits against the window edge
                model: albumTracks
                filter: albumTracks
                onActivated: (row) => DspController.playPaths(albumTracks.paths(), row)
            }
        }
    }

    Label {
        anchors.centerIn: parent
        visible: albums.count === 0 && page.openAlbum.length === 0
        text: Library.count === 0 ? qsTr("Add a music folder to see your albums") : qsTr("No albums match")
        color: Theme.textDim
        font.pixelSize: Theme.fontTitle
    }
}
