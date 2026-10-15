pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Sonkore

Rectangle {
    id: sidebar

    property string page
    property string playlist
    property string folder
    // Icons only (narrow windows / large interface scales).
    property bool collapsed: false
    // Short windows: tighter spacing so the tree and the playlists keep room.
    readonly property bool short: height < 600
    // Inner margin (slim, to give the names room); the lists' scroll bars
    // sit beyond it, against the edge.
    readonly property real edgeInset: collapsed ? 8 : 6

    signal navigate(string page, string argument)

    // Section title on a soft band, with a "+" action (and optionally a
    // rescan button): each section starts visibly.
    component SectionHeader: Rectangle {
        id: header
        property string title
        property string addTip
        property bool rescan: false
        property bool canAdd: true
        property bool levels: false // a button for the folder tree's subfolder levels
        property bool levelsActive: false
        readonly property Item addButton: addButton
        signal add()
        signal levelsRequested(Item button)
        Layout.fillWidth: true
        Layout.bottomMargin: 4
        implicitHeight: Theme.compact ? 26 : Theme.spacious ? 34 : 30
        // A soft band, no outline: each section starts visibly.
        radius: Theme.radiusSmall + 2
        color: Theme.surface
        // Its buttons fit inside the band, hover circle included.
        readonly property real buttonSize: implicitHeight - 6
        RowLayout {
            anchors.fill: parent
            anchors.leftMargin: 10
            anchors.rightMargin: 3
            spacing: 2
        SectionLabel { text: header.title; Layout.fillWidth: true }
        IconButton {
            id: levelsButton
            visible: header.levels
            implicitWidth: header.buttonSize
            implicitHeight: header.buttonSize
            iconName: "sliders"
            iconSize: 14
            active: header.levelsActive
            tip: qsTr("Subfolders")
            onClicked: header.levelsRequested(levelsButton)
        }
        IconButton {
            visible: header.rescan
            implicitWidth: header.buttonSize
            implicitHeight: header.buttonSize
            iconName: "refresh"
            iconSize: 14
            tip: Library.scanning ? qsTr("Scanning… %1 files").arg(Library.scannedFiles) : qsTr("Rescan library")
            enabled: !Library.scanning
            spinning: Library.scanning
            onClicked: Library.rescan()
        }
        IconButton {
            id: addButton
            visible: header.canAdd
            implicitWidth: header.buttonSize
            implicitHeight: header.buttonSize
            iconName: "plus"
            iconSize: 14
            tip: header.addTip
            onClicked: header.add()
        }
        }
    }

    color: Theme.sidebar

    component NavItem: AbstractButton {
        id: item
        property string iconName
        property string badge
        property bool current: false
        property bool acceptsDrops: false
        property bool collapsed: false
        signal tracksDropped(var paths)
        Layout.fillWidth: true
        implicitHeight: collapsed ? 38 : (Theme.compact ? 28 : Theme.spacious ? 42 : 36)
        hoverEnabled: true
        focusPolicy: Qt.NoFocus
        ToolTip.visible: collapsed && hovered
        ToolTip.delay: 350
        ToolTip.text: text + (badge.length > 0 ? "  ·  " + badge : "")
        background: RoundRect {
            radius: Theme.radiusSmall
            color: navDrop.containsDrag ? Theme.accentSoft : (item.current ? Theme.accentSoft : (item.hovered ? Theme.hover : "transparent"))
            borderWidth: navDrop.containsDrag ? 1.5 : 0
            borderColor: Theme.accent
        }
        DropArea {
            id: navDrop
            anchors.fill: parent
            enabled: item.acceptsDrops
            keys: ["sonkore/tracks", "text/uri-list"]
            onDropped: (event) => {
                const paths = AppActions.dropPaths(event)
                if (paths.length > 0)
                    item.tracksDropped(paths)
                event.accept(Qt.CopyAction)
            }
        }
        contentItem: Item {
            implicitHeight: navRow.implicitHeight
            RowLayout {
                id: navRow
                anchors.fill: parent
                visible: !item.collapsed
                spacing: 10
                Item { implicitWidth: Theme.iconTiles ? 0 : 2 }
                // "Tile" icon style: the icon on a soft square.
                Rectangle {
                    implicitWidth: Theme.iconTiles ? 26 : 17
                    implicitHeight: implicitWidth
                    radius: 7
                    color: !Theme.iconTiles ? "transparent" : item.current ? Theme.accentSoft : Theme.surfaceHi
                    Icon {
                        anchors.centerIn: parent
                        glyph: item.iconName
                        size: Theme.iconTiles ? 15 : 17
                        color: item.current ? Theme.accent : Theme.textDim
                    }
                }
                Label {
                    Layout.fillWidth: true
                    text: item.text
                    elide: Text.ElideRight
                    color: item.current ? Theme.text : Theme.textDim
                    font.pixelSize: Theme.fontBody
                    font.weight: item.current ? Font.DemiBold : Font.Normal
                }
                Label {
                    text: item.badge
                    color: Theme.textFaint
                    font.pixelSize: Theme.fontSmall
                    font.features: { "tnum": 1 }
                }
                Item { implicitWidth: 4 }
            }
            Icon {
                anchors.centerIn: parent
                visible: item.collapsed
                glyph: item.iconName
                size: 19
                color: item.current ? Theme.accent : Theme.textDim
            }
        }
    }

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: sidebar.edgeInset
        spacing: 2

        // Library entries. Dragging the bar under them up lets the folder
        // tree cover them (they scroll); a double-click folds them away, and
        // another unfolds them back to where they were.
        Item {
            id: libraryNav
            Layout.fillWidth: true
            readonly property real fullHeight: navColumn.implicitHeight
            Layout.preferredHeight: sidebar.collapsed || Prefs.store.libraryHeight < 0
                                    ? fullHeight : Math.min(Prefs.store.libraryHeight, fullHeight)
            Behavior on Layout.preferredHeight {
                enabled: !libraryDrag.active
                NumberAnimation { duration: 180; easing.type: Easing.OutCubic }
            }
            clip: true
            Flickable {
                id: navFlick
                anchors.fill: parent
                contentHeight: libraryNav.fullHeight
                interactive: contentHeight > height
                boundsBehavior: Flickable.StopAtBounds
                WheelScroll { flickable: navFlick }
                ColumnLayout {
                    id: navColumn
                    width: navFlick.width
                    spacing: 2
                    SectionHeader { title: qsTr("Library"); canAdd: false; visible: !sidebar.collapsed }
                    NavItem {
                        collapsed: sidebar.collapsed
                        text: qsTr("Now playing")
                        iconName: "headphones"
                        badge: DspController.queue.length > 0 ? DspController.queue.length : ""
                        current: sidebar.page === "playing"
                        acceptsDrops: true
                        onClicked: sidebar.navigate("playing", "")
                        onTracksDropped: (paths) => DspController.enqueue(paths)
                    }
                    NavItem {
                        collapsed: sidebar.collapsed
                        text: qsTr("Tracks")
                        iconName: "music"
                        badge: Library.count > 0 ? Library.count : ""
                        current: sidebar.page === "tracks"
                        onClicked: sidebar.navigate("tracks", "")
                    }
                    NavItem {
                        collapsed: sidebar.collapsed
                        text: qsTr("Favorites")
                        iconName: "heart"
                        badge: Library.favoriteCount > 0 ? Library.favoriteCount : ""
                        current: sidebar.page === "favorites"
                        onClicked: sidebar.navigate("favorites", "")
                    }
                    NavItem {
                        collapsed: sidebar.collapsed
                        text: qsTr("Albums")
                        iconName: "album"
                        current: sidebar.page === "albums"
                        onClicked: sidebar.navigate("albums", "")
                    }
                    NavItem {
                        collapsed: sidebar.collapsed
                        text: qsTr("Artists")
                        iconName: "user"
                        current: sidebar.page === "artists"
                        onClicked: sidebar.navigate("artists", "")
                    }
                    NavItem {
                        collapsed: sidebar.collapsed
                        text: qsTr("Genres")
                        iconName: "tag"
                        current: sidebar.page === "genres"
                        onClicked: sidebar.navigate("genres", "")
                    }
                    // Icons-only rail: the tree does not fit, one entry opens the folder view.
                    NavItem {
                        visible: sidebar.collapsed
                        collapsed: true
                        text: qsTr("Folders")
                        iconName: "folder"
                        current: sidebar.page === "folders"
                        onClicked: sidebar.navigate("folders", sidebar.folder)
                    }
                }
            }
        }
        Item {
            id: libraryGrip
            visible: !sidebar.collapsed
            Layout.fillWidth: true
            Layout.topMargin: 2
            implicitHeight: 9
            property real startHeight: 0
            readonly property bool active: libraryHover.hovered || libraryDrag.active
            Rectangle { // the line, only while it is grabbed or hovered
                anchors.verticalCenter: parent.verticalCenter
                width: parent.width
                height: 1
                visible: libraryGrip.active
                color: Theme.accent
            }
            RoundRect {
                anchors.centerIn: parent
                width: libraryGrip.active ? 40 : 28
                height: 5
                radius: 2.5
                color: libraryGrip.active ? Theme.accent : Theme.sidebar
                borderWidth: 1
                borderColor: libraryGrip.active ? Theme.accent : Qt.rgba(Theme.text.r, Theme.text.g, Theme.text.b, Theme.dark ? 0.28 : 0.32)
                Behavior on width { NumberAnimation { duration: 120 } }
            }
            HoverHandler { id: libraryHover; cursorShape: Qt.SplitVCursor }
            DragHandler {
                id: libraryDrag
                target: null
                xAxis.enabled: false
                cursorShape: Qt.SplitVCursor
                onActiveChanged: if (active) libraryGrip.startHeight = libraryNav.height
                onTranslationChanged: if (active) {
                    const h = Math.round(Math.max(0, libraryGrip.startHeight + translation.y))
                    Prefs.store.libraryHeight = h >= libraryNav.fullHeight ? -1 : h
                }
            }
            // Fold / unfold, back to the height it had.
            TapHandler {
                onDoubleTapped: {
                    if (Prefs.store.libraryHeight !== 0) {
                        Prefs.store.libraryHeightRestore = Prefs.store.libraryHeight
                        Prefs.store.libraryHeight = 0
                    } else {
                        Prefs.store.libraryHeight = Prefs.store.libraryHeightRestore !== 0 ? Prefs.store.libraryHeightRestore : -1
                    }
                }
            }
        }

        // --- Folder tree -------------------------------------------------------
        SectionHeader {
            visible: !sidebar.collapsed
            title: qsTr("Folders")
            addTip: qsTr("Add music folder")
            rescan: true
            levels: true
            // Lit while subfolders are hidden or cut short.
            levelsActive: !Prefs.store.treeSubfolders || folderTree.levels < folderTree.deepest
            Layout.topMargin: sidebar.short ? 4 : 6
            onAdd: AppActions.addFolderRequested()
            onLevelsRequested: (button) => levelsMenu.popup(button, button.width - levelsMenu.width, button.height + 2)
        }
        LMenu {
            id: levelsMenu
            width: 250
            SubfolderLevels {
                width: levelsMenu.availableWidth
                shown: Prefs.store.treeSubfolders
                value: folderTree.levels
                deepest: folderTree.deepest
                onShownToggled: (on) => Prefs.store.treeSubfolders = on
                onValuePicked: (v) => Prefs.store.treeFolderDepth = v
            }
        }
        FolderTree {
            id: folderTree
            visible: !sidebar.collapsed
            Layout.fillWidth: true
            Layout.fillHeight: true
            Layout.minimumHeight: 64
            folder: sidebar.page === "folders" ? sidebar.folder : ""
            scrollBarOutset: sidebar.edgeInset
            onFolderPicked: (path) => sidebar.navigate("folders", path)
        }

        // --- Playlists -----------------------------------------------------------
        // Drag this bar to share the height between folders and playlists.
        Item {
            id: splitGrip
            objectName: "splitGrip"
            visible: !sidebar.collapsed
            Layout.fillWidth: true
            Layout.topMargin: 4
            implicitHeight: 9
            property real startHeight: 0
            readonly property bool active: splitHover.hovered || splitDrag.active
            // Full-width line while grabbed or hovered: where the playlists
            // start covering the folders.
            Rectangle {
                anchors.verticalCenter: parent.verticalCenter
                width: parent.width
                height: 1
                visible: splitGrip.active
                color: Theme.accent
            }
            // Grip in the middle of the line.
            RoundRect {
                anchors.centerIn: parent
                width: splitGrip.active ? 40 : 28
                height: 5
                radius: 2.5
                color: splitGrip.active ? Theme.accent : Theme.sidebar
                borderWidth: 1
                borderColor: splitGrip.active ? Theme.accent : Qt.rgba(Theme.text.r, Theme.text.g, Theme.text.b, Theme.dark ? 0.28 : 0.32)
                Behavior on width { NumberAnimation { duration: 120 } }
            }
            HoverHandler { id: splitHover; cursorShape: Qt.SplitVCursor }
            DragHandler {
                id: splitDrag
                target: null
                xAxis.enabled: false
                cursorShape: Qt.SplitVCursor
                onActiveChanged: if (active) splitGrip.startHeight = playlistList.height
                onTranslationChanged: if (active) Prefs.store.playlistsHeight =
                    Math.round(Math.max(0, Math.min(sidebar.height * 0.7, splitGrip.startHeight - translation.y)))
            }
            // Fold / unfold, back to the height it had.
            TapHandler {
                onDoubleTapped: {
                    if (Prefs.store.playlistsHeight > 0) {
                        Prefs.store.playlistsHeightRestore = Prefs.store.playlistsHeight
                        Prefs.store.playlistsHeight = 0
                    } else {
                        Prefs.store.playlistsHeight = Prefs.store.playlistsHeightRestore > 0 ? Prefs.store.playlistsHeightRestore : 150
                    }
                }
            }
        }
        SectionHeader {
            visible: !sidebar.collapsed
            id: playlistsHeader
            title: qsTr("Playlists")
            addTip: qsTr("New playlist")
            onAdd: addPlaylistMenu.popup(playlistsHeader.addButton, 0, playlistsHeader.addButton.height)
        }
        IconButton {
            visible: sidebar.collapsed
            Layout.alignment: Qt.AlignHCenter
            Layout.topMargin: 6
            iconName: "plus"
            iconSize: 14
            tip: qsTr("New playlist")
            onClicked: addPlaylistMenu.popup(this, width, 0)
        }
        LMenu {
            id: addPlaylistMenu
            LMenuItem { text: qsTr("New playlist…"); onTriggered: AppActions.newPlaylistRequested([]) }
            LMenuItem { text: qsTr("New playlist from a folder…"); onTriggered: AppActions.newPlaylistFromFolderRequested() }
            LMenuItem { text: qsTr("Import M3U…"); onTriggered: AppActions.importPlaylistRequested() }
        }
        // Right-click on a playlist.
        LMenu {
            id: playlistMenu
            property string name
            LMenuItem { text: qsTr("Play"); onTriggered: { DspController.shuffle = false; DspController.playPaths(Playlists.paths(playlistMenu.name), 0) } }
            LMenuItem { text: qsTr("Rename…"); onTriggered: renamePlaylistDialog.ask(playlistMenu.name) }
            LMenuSeparator {}
            LMenuItem { text: qsTr("Delete playlist…"); onTriggered: deletePlaylistDialog.open() }
        }
        NameDialog {
            id: renamePlaylistDialog
            title: qsTr("Rename playlist")
            acceptText: qsTr("Rename")
            onAccepted: (newName) => {
                const old = playlistMenu.name
                const renamed = Playlists.rename(old, newName)
                if (sidebar.page === "playlist" && sidebar.playlist === old)
                    sidebar.navigate("playlist", renamed)
            }
        }
        ConfirmDialog {
            id: deletePlaylistDialog
            title: qsTr("Delete “%1”?").arg(playlistMenu.name)
            text: qsTr("The playlist is removed; its tracks stay in the library and on disk.")
            acceptText: qsTr("Delete playlist")
            onAccepted: {
                const name = playlistMenu.name
                Playlists.remove(name)
                if (sidebar.page === "playlist" && sidebar.playlist === name)
                    sidebar.navigate("tracks", "")
            }
        }
        ListView {
            id: playlistList
            Layout.fillWidth: true
            // Out to the sidebar's edge (scroll bar there); rows keep the margin.
            Layout.rightMargin: -sidebar.edgeInset
            // Collapsed (dragged to 0) in the rail; otherwise the dragged height.
            Layout.preferredHeight: sidebar.collapsed ? -1 : Prefs.store.playlistsHeight
            Behavior on Layout.preferredHeight {
                enabled: !splitDrag.active && !sidebar.collapsed
                NumberAnimation { duration: 180; easing.type: Easing.OutCubic }
            }
            Layout.fillHeight: sidebar.collapsed
            clip: true
            WheelScroll { flickable: playlistList }
            ScrollBar.vertical: LScrollBar {}
            spacing: 2
            model: Playlists.names
            boundsBehavior: Flickable.StopAtBounds
            // Nothing yet: say what goes here.
            Label {
                anchors.top: parent.top
                anchors.topMargin: 10
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.rightMargin: sidebar.edgeInset + 10
                anchors.leftMargin: 10
                visible: playlistList.count === 0 && !sidebar.collapsed
                text: qsTr("No playlists yet. Press + or drop tracks here.")
                wrapMode: Text.WordWrap
                color: Theme.textFaint
                font.pixelSize: Theme.fontSmall
            }
            delegate: NavItem {
                required property string modelData
                width: playlistList.width - sidebar.edgeInset
                collapsed: sidebar.collapsed
                text: modelData
                iconName: "playlist"
                current: sidebar.page === "playlist" && sidebar.playlist === modelData
                acceptsDrops: true
                onClicked: sidebar.navigate("playlist", modelData)
                TapHandler {
                    acceptedButtons: Qt.RightButton
                    onTapped: {
                        playlistMenu.name = modelData
                        playlistMenu.popup()
                    }
                }
                onTracksDropped: (paths) => {
                    Playlists.addTracks(modelData, paths)
                    AppActions.toast(qsTr("Added %1 to “%2”").arg(AppActions.tracksText(paths.length)).arg(modelData))
                }
            }
            footer: Item {
                width: playlistList.width - sidebar.edgeInset
                height: AppActions.trackDragActive ? 46 : 0
                visible: AppActions.trackDragActive
                RoundRect {
                    anchors.fill: parent
                    anchors.topMargin: 6
                    radius: Theme.radiusSmall
                    color: newDrop.containsDrag ? Theme.accentSoft : "transparent"
                    borderWidth: 1.5
                    borderColor: newDrop.containsDrag ? Theme.accent : Theme.textFaint
                    Row {
                        anchors.centerIn: parent
                        spacing: 6
                        Icon { glyph: "plus"; size: 14; color: newDrop.containsDrag ? Theme.accent : Theme.textDim }
                        Label {
                            visible: !sidebar.collapsed
                            text: qsTr("Drop to create a playlist")
                            color: newDrop.containsDrag ? Theme.accent : Theme.textDim
                            font.pixelSize: Theme.fontSmall + 1
                        }
                    }
                    DropArea {
                        id: newDrop
                        anchors.fill: parent
                        keys: ["sonkore/tracks"]
                        onDropped: (event) => {
                            AppActions.newPlaylistRequested(AppActions.dropPaths(event))
                            event.accept(Qt.CopyAction)
                        }
                    }
                }
            }
            Label {
                anchors.fill: parent
                anchors.margins: 8
                visible: playlistList.count === 0 && !AppActions.trackDragActive && !sidebar.collapsed
                text: qsTr("Drag tracks here, or right-click a track → Add to playlist")
                wrapMode: Text.Wrap
                color: Theme.textFaint
                font.pixelSize: Theme.fontSmall
            }
        }
    }
}
