pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import Sonkore

// The library's folder tree (one root per watched folder, track counts on
// the right). Click a folder to list its tracks; right-click for actions.
Item {
    id: root

    property string folder            // the selected folder
    // How far past this item's right edge the scroll bar goes (to sit
    // against the sidebar's edge, beyond its margin).
    property real scrollBarOutset: 0
    signal folderPicked(string path)

    // Subfolders (the Folders header's levels menu): the library folders'
    // own folders always; below them none, or down to the chosen level.
    // `deepest`: the deepest subfolder level there is (1 = folders inside
    // the top folders).
    readonly property int deepest: {
        Library.count
        Library.scanning
        return Library.trackFolders().reduce((m, f) => Math.max(m, f.name.split("/").length - 1), 1)
    }
    readonly property int levels: Math.max(1, Math.min(Prefs.store.treeFolderDepth, deepest))
    // Deepest tree depth that may show (0 = the library folders).
    readonly property int maxDepth: 1 + (Prefs.store.treeSubfolders ? levels : 0)
    // A limit you change folds what is past it and unfolds down to it.
    function applyLevels() {
        tree.collapseRecursively()
        tree.expandRecursively(-1, maxDepth)
        Qt.callLater(tree.reveal)
    }
    Connections {
        target: Prefs.store
        function onTreeSubfoldersChanged() { Qt.callLater(root.applyLevels) }
        function onTreeFolderDepthChanged() { Qt.callLater(root.applyLevels) }
    }

    TrackMenu {
        id: folderMenu
        fromLibrary: false
    }
    // The open folder was removed from the library: show all folders instead.
    Connections {
        target: Library
        function onCountChanged() {
            if (root.folder.length > 0 && Library.folderPaths(root.folder).length === 0)
                root.folderPicked("")
        }
    }

    // Once the tree is scrolled, the folders fade out under the section
    // head instead of being cut off by it.
    EdgeFade {
        z: 2
        width: root.width + root.scrollBarOutset
        height: 10
        tint: Theme.sidebar
        opacity: tree.atYBeginning ? 0 : 1
        Behavior on opacity { NumberAnimation { duration: 150 } }
    }

    TreeView {
        id: tree
        property string selected: root.folder
        WheelScroll { flickable: tree }
        anchors.fill: parent
        clip: true
        boundsBehavior: Flickable.StopAtBounds
        model: FolderTreeModel {
            id: folderModel
            onModelReset: Qt.callLater(tree.expandAndReveal)
        }
        // Unfolds the selected folder's parents and scrolls it into view
        // (e.g. the folder of the track restored from the last session).
        function reveal() {
            if (root.folder.length === 0)
                return
            let index = folderModel.indexOfPath(root.folder)
            if (!index.valid)
                return
            // A folder past the limit: its nearest shown parent instead.
            let depth = 0
            for (let i = index.parent; i.valid; i = i.parent)
                ++depth
            for (; depth > root.maxDepth; --depth)
                index = index.parent
            tree.expandToIndex(index)
            if (depth === root.maxDepth) // the limit's folders stay folded
                Qt.callLater(() => { const r = tree.rowAtIndex(index); if (r >= 0) tree.collapse(r) })
            Qt.callLater(() => {
                const r = tree.rowAtIndex(index)
                if (r >= 0)
                    tree.positionViewAtRow(r, TableView.Contain)
            })
        }
        function expandAndReveal() {
            tree.expandRecursively(-1, 1)
            reveal()
        }
        Connections {
            target: root
            function onFolderChanged() { Qt.callLater(tree.reveal) }
        }
        columnWidthProvider: () => tree.width
        // Plain geometry instead of anchors: the bar is created inside the tree
        // and only then moved to `root`, and anchors set before the move fail.
        ScrollBar.vertical: LScrollBar {
            parent: root
            x: root.width + root.scrollBarOutset - width
            y: 0
            height: root.height
        }
        Component.onCompleted: Qt.callLater(tree.expandAndReveal)

        delegate: Item {
            id: node
            required property TreeView treeView
            required property bool isTreeNode
            required property bool expanded
            required property bool hasChildren
            required property int depth
            required property int row
            required property string name
            required property string path
            required property int count
            readonly property bool selectedNode: tree.selected === path
            // Folders at the depth limit do not unfold.
            readonly property bool canExpand: hasChildren && depth < root.maxDepth
            // The playing track is somewhere inside / directly in this folder.
            readonly property string playingPath: DspController.currentPath
            readonly property bool holdsPlaying: playingPath.length > 0 && playingPath.startsWith(path + "/")
            readonly property bool playingHere: holdsPlaying && playingPath.lastIndexOf("/") === path.length
            // The bars go on the deepest folder you can see: the track's own
            // folder, or a folded one that hides it. Not on the open folder:
            // its track list already shows the playing track.
            readonly property bool showBars: !selectedNode && (playingHere || (holdsPlaying && hasChildren && (!expanded || !canExpand)))

            implicitWidth: tree.width
            implicitHeight: Theme.compact ? 26 : 32

            Rectangle {
                anchors.fill: parent
                anchors.rightMargin: 4
                radius: Theme.radiusSmall
                color: node.selectedNode ? Theme.accentSoft : (nodeHover.hovered ? Theme.hover : "transparent")
            }
            HoverHandler { id: nodeHover }
            TapHandler {
                acceptedButtons: Qt.LeftButton
                onSingleTapped: root.folderPicked(node.path)
                onDoubleTapped: if (node.canExpand) node.treeView.toggleExpanded(node.row)
            }
            TapHandler {
                acceptedButtons: Qt.RightButton
                onTapped: (point) => {
                    folderMenu.paths = Library.folderPaths(node.path)
                    folderMenu.folderPath = node.path
                    folderMenu.popup(node, point.position.x, point.position.y)
                }
            }

            Row {
                anchors.verticalCenter: parent.verticalCenter
                x: 6 + node.depth * 16
                spacing: 6
                Item {
                    width: 16
                    height: 16
                    anchors.verticalCenter: parent.verticalCenter
                    Icon {
                        anchors.centerIn: parent
                        visible: node.canExpand
                        glyph: "chevron-down"
                        size: 13
                        rotation: node.expanded ? 0 : -90
                        color: Theme.textDim
                        Behavior on rotation { NumberAnimation { duration: 120 } }
                    }
                    TapHandler { onTapped: if (node.canExpand) node.treeView.toggleExpanded(node.row) }
                }
                // The folder's icon, or in its place the playing bars: they
                // move while it plays with another folder open, and stand
                // still otherwise. The open folder's list, or the list of
                // every track, already shows the playing track moving: never
                // twice.
                Item {
                    width: 15
                    height: 16
                    anchors.verticalCenter: parent.verticalCenter
                    Icon {
                        anchors.centerIn: parent
                        visible: !node.showBars
                        glyph: "folder"
                        size: 15
                        color: node.selectedNode || node.holdsPlaying ? Theme.accent : Theme.textDim
                    }
                    PlayingBars {
                        anchors.centerIn: parent
                        visible: node.showBars
                        running: visible && DspController.playing && tree.selected.length > 0
                                 && !node.playingPath.startsWith(tree.selected + "/")
                        time: 0.2
                    }
                }
                Label {
                    anchors.verticalCenter: parent.verticalCenter
                    width: tree.width - 6 - node.depth * 16 - 16 - 15 - 12 - countLabel.width - 18
                    text: node.name
                    elide: Text.ElideRight
                    color: node.holdsPlaying ? Theme.accent : (node.selectedNode ? Theme.text : Theme.textDim)
                    font.weight: node.selectedNode || node.playingHere ? Font.DemiBold : Font.Normal
                    font.pixelSize: Theme.fontBody
                }
                Label {
                    id: countLabel
                    anchors.verticalCenter: parent.verticalCenter
                    text: node.count
                    color: Theme.textFaint
                    font.pixelSize: Theme.fontSmall
                    font.features: { "tnum": 1 }
                }
            }
        }
    }

    Label {
        anchors.fill: parent
        anchors.margins: 8
        visible: Library.folders.length === 0
        text: qsTr("No music folders yet — add one with +.")
        wrapMode: Text.Wrap
        color: Theme.textFaint
        font.pixelSize: Theme.fontSmall
    }
}
