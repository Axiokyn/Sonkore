pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import Sonkore

// Track table shared by every track view:
//  * user-defined columns (right-click the header): drag a header to move
//    the column, drag a divider to resize (double-click resets), click a
//    header to sort, or pick an order from the button at its left (when
//    `filter` is set; `rememberSort` keeps it between sessions);
//  * multi-selection with Ctrl / Shift / Ctrl+A, Enter to play;
//  * drag the selection onto a playlist, the queue or "new playlist".
// `model` is a TrackFilterModel or a JS array of track maps.
Item {
    id: table

    property var model
    property TrackFilterModel filter: null     // enables sorting by header
    property bool rememberSort: false          // the order is kept (Prefs) and shared by such lists
    property string dragOrigin: "library"
    property string playlistName               // context menu: remove from playlist
    property bool queueMode: false             // context menu: remove from queue
    property alias list: list
    readonly property bool isArray: Array.isArray(model)
    readonly property int count: list.count

    // selection: row -> true; `selectionVersion` makes bindings re-evaluate
    property var selection: ({})
    property int selectionVersion: 0
    property int anchorRow: -1
    readonly property int selectedCount: { selectionVersion; return Object.keys(selection).length }

    signal activated(int row)
    signal removeRequested(var rows)

    readonly property real leadingWidth: 36
    // When even the narrowest columns leave no room for the covers (the
    // queue beside the large cover on the Now playing page), the covers go;
    // only below that does the list scroll sideways.
    readonly property real coverFull: Theme.listSpacious ? 62 : 52
    readonly property real narrowestRow: { // every column at its least, no covers
        let w = 14 + leadingWidth + trailingWidth + TrackColumns.widthOf(flexKey)
        for (const k of TrackColumns.keys)
            if (!TrackColumns.isFlex(k))
                w += k === primaryKey ? primaryMin : TrackColumns.minWidthOf(k)
        return w
    }
    readonly property real coverWidth: Theme.listCovers && list.width >= narrowestRow + coverFull ? coverFull : 0
    readonly property real fixedWidth: {
        let w = 0
        for (const k of TrackColumns.keys)
            if (!TrackColumns.isFlex(k))
                w += TrackColumns.widthOf(k)
        return w
    }
    readonly property real trailingWidth: 8 // a little room after the last column
    readonly property real availableFlex: list.width - 14 - leadingWidth - coverWidth - fixedWidth - trailingWidth
    // The last column fills the free space down to its own width. When the
    // window is too narrow for that, the first column (the title or file
    // name, whose text elides) gives up space first, down to primaryMin (200,
    // or a quarter of a narrow list); then the other columns, each in
    // proportion down to its title's width; then the covers (coverWidth);
    // only below that the list scrolls sideways.
    readonly property string flexKey: TrackColumns.flexKey
    readonly property string primaryKey: TrackColumns.keys.length > 1 ? TrackColumns.keys[0] : ""
    readonly property real primaryFull: primaryKey.length > 0 ? TrackColumns.widthOf(primaryKey) : 0
    readonly property real primaryMin: primaryKey.length > 0
        ? Math.min(primaryFull, Math.max(TrackColumns.minWidthOf(primaryKey), Math.min(200, Math.round(list.width / 4)))) : 0
    readonly property real shortfall: Math.max(0, TrackColumns.widthOf(flexKey) - availableFlex)
    readonly property real primaryWidth: Math.max(primaryMin, primaryFull - shortfall) // 0 without a separate first column
    readonly property real primaryShrink: primaryFull - primaryWidth
    // What the other columns can give up, and the share each one gives.
    readonly property real restSlack: {
        let w = 0
        for (const k of TrackColumns.keys)
            if (!TrackColumns.isFlex(k) && k !== primaryKey)
                w += TrackColumns.widthOf(k) - TrackColumns.minWidthOf(k)
        return w
    }
    readonly property real restRatio: restSlack > 0 ? Math.min(1, Math.max(0, shortfall - primaryShrink) / restSlack) : 0
    readonly property real restShrink: restSlack * restRatio
    readonly property bool overflowing: availableFlex + primaryShrink + restShrink < TrackColumns.widthOf(flexKey)
    readonly property real flexWidth: Math.max(TrackColumns.widthOf(flexKey), availableFlex + primaryShrink + restShrink)
    readonly property real rowWidth: leadingWidth + coverWidth + fixedWidth - primaryShrink - restShrink + flexWidth + trailingWidth

    function columnWidth(key) {
        if (TrackColumns.isFlex(key))
            return flexWidth
        if (key === primaryKey)
            return primaryWidth
        const w = TrackColumns.widthOf(key)
        return w - (w - TrackColumns.minWidthOf(key)) * restRatio
    }
    // x of the boundary before column `i` (i = keys.length: after the last).
    function columnEdge(i) {
        let x = leadingWidth + coverWidth
        const keys = TrackColumns.keys
        for (let j = 0; j < i && j < keys.length; ++j)
            x += columnWidth(keys[j])
        return x
    }

    // --- Resizing: one column at a time -------------------------------------
    // Each column's divider is its right edge: dragging it resizes that
    // column only, the divider stays under the pointer and the columns after
    // it move along (the last column, which fills the rest, gives or takes
    // the space). The last column has a divider only while the list scrolls
    // sideways.
    property var resizeState: null
    function beginResize(boundary, sceneX) {
        const widths = {}
        for (const k of TrackColumns.keys)
            widths[k] = columnWidth(k)
        resizeState = { boundary: boundary, startX: sceneX, widths: widths }
    }
    function updateResize(sceneX) {
        const st = resizeState
        if (!st)
            return
        const key = TrackColumns.keys[st.boundary]
        const map = {}
        map[key] = Math.max(TrackColumns.minWidthOf(key), st.widths[key] + sceneX - st.startX)
        TrackColumns.setWidths(map, false)
    }
    function endResize() {
        if (resizeState)
            TrackColumns.commitWidths()
        resizeState = null
    }

    // --- Moving a column by dragging its header -----------------------------
    property string dragKey: ""
    property real dragX: 0       // ghost position in header coordinates
    property real dragGrab: 0    // pointer offset inside the dragged header
    property int dropTarget: -1  // boundary index where the column would land
    function beginColumnDrag(key, grab) {
        dragKey = key
        dragGrab = grab
        dropTarget = -1
    }
    function updateColumnDrag(headerX) {
        dragX = headerX - dragGrab
        const keys = TrackColumns.keys
        let x = leadingWidth + coverWidth
        let target = 0
        for (let i = 0; i < keys.length; ++i) {
            const w = columnWidth(keys[i])
            if (headerX > x + w / 2)
                target = i + 1
            x += w
        }
        dropTarget = target
    }
    function endColumnDrag() {
        const from = TrackColumns.keys.indexOf(dragKey)
        if (from >= 0 && dropTarget >= 0 && dropTarget !== from && dropTarget !== from + 1)
            TrackColumns.moveTo(dragKey, dropTarget > from ? dropTarget - 1 : dropTarget)
        dragKey = ""
        dropTarget = -1
    }
    function setSort(key, descending) {
        if (!filter)
            return
        filter.sortField = key
        filter.descending = descending
        if (rememberSort) {
            Prefs.store.trackSort = key
            Prefs.store.trackSortDescending = descending
        }
    }
    function sortBy(key) {
        if (!filter)
            return
        // Favorites start with the favourites on top (descending), then flip.
        const first = key === "favorite"
        if (filter.sortField !== key)
            setSort(key, first)
        else if (filter.descending === first)
            setSort(key, !first)
        else
            setSort("", false)   // third click: back to library order
    }
    Component.onCompleted: if (filter && rememberSort) {
        filter.sortField = Prefs.store.trackSort
        filter.descending = Prefs.store.trackSortDescending
    }
    // The orders in the sort button's menu: [key, descending, label].
    readonly property var sortChoices: [
        ["", false, qsTr("Library order")],
        ["path", false, qsTr("As in the folder (A → Z)")],
        ["path", true, qsTr("As in the folder (Z → A)")],
        ["title", false, qsTr("Title")],
        ["artist", false, qsTr("Artist")],
        ["albumArtist", false, qsTr("Album artist")],
        ["album", false, qsTr("Album")],
        ["genre", false, qsTr("Genre")],
        ["year", false, qsTr("Year (oldest first)")],
        ["year", true, qsTr("Year (newest first)")],
        ["duration", false, qsTr("Duration (shortest first)")],
        ["duration", true, qsTr("Duration (longest first)")],
        ["fileName", false, qsTr("File name")],
        ["added", true, qsTr("Recently added")],
        ["modified", true, qsTr("Recently modified")],
        ["favorite", true, qsTr("Favorites first")]]
    LMenu {
        id: sortMenu
        width: 260 // the longest choice whole, never "…"
        Instantiator {
            // "Favorites first" means nothing among favourites only.
            model: table.sortChoices.filter(c => c[0] !== "favorite" || !(table.filter && table.filter.favoritesOnly))
            delegate: LMenuItem {
                id: sortItem
                required property var modelData
                readonly property bool current: table.filter !== null && table.filter.sortField === modelData[0]
                                                && (modelData[0] === "" || table.filter.descending === modelData[1])
                text: modelData[2]
                checkable: true
                checked: current
                onTriggered: {
                    table.setSort(modelData[0], modelData[1])
                    checked = Qt.binding(() => sortItem.current) // a click toggles it: follow the order again
                }
            }
            onObjectAdded: (index, object) => sortMenu.insertItem(index, object)
            onObjectRemoved: (index, object) => sortMenu.removeItem(object)
        }
    }

    function pathAt(row) {
        if (row < 0)
            return ""
        return isArray ? (row < model.length ? model[row].path : "") : model.pathAt(row)
    }
    function isSelected(row) { return selection[row] === true }
    function selectedRows() {
        return Object.keys(selection).map(Number).sort((a, b) => a - b)
    }
    function selectedPaths() {
        return selectedRows().map(r => pathAt(r)).filter(p => p.length > 0)
    }
    function setSelection(map) {
        selection = map
        selectionVersion++
    }
    function clearSelection() { setSelection({}); anchorRow = -1 }
    function selectAll() {
        const m = {}
        for (let i = 0; i < list.count; ++i) m[i] = true
        setSelection(m)
    }
    function clickRow(row, modifiers) {
        list.currentIndex = row
        list.forceActiveFocus()
        const m = (modifiers & Qt.ControlModifier) ? Object.assign({}, selection) : {}
        if ((modifiers & Qt.ShiftModifier) && anchorRow >= 0) {
            const a = Math.min(anchorRow, row), b = Math.max(anchorRow, row)
            for (let i = a; i <= b; ++i) m[i] = true
        } else if (modifiers & Qt.ControlModifier) {
            if (m[row]) delete m[row]; else m[row] = true
            anchorRow = row
        } else {
            m[row] = true
            anchorRow = row
        }
        setSelection(m)
    }
    function ensureSelected(row) {
        if (!isSelected(row)) {
            const m = {}
            m[row] = true
            setSelection(m)
            anchorRow = row
        }
    }
    function openMenu(row, item, x, y) {
        ensureSelected(row)
        menu.paths = selectedPaths()
        menu.rows = selectedRows()
        menu.popup(item, x, y)
    }

    // The playing track in the middle of the list, when the list holds it
    // (the player bar's title asks every open list: AppActions.revealPlaying).
    function revealPlaying() {
        const path = DspController.currentPath
        if (path.length === 0)
            return false
        const row = queueMode ? DspController.currentIndex
                  : isArray ? model.findIndex(t => t.path === path)
                  : model.rowOfPath(path)
        if (row < 0 || row >= list.count)
            return false
        list.positionViewAtIndex(row, ListView.Center)
        return true
    }
    Connections {
        target: AppActions
        enabled: table.visible
        function onRevealPlayingRequested() {
            if (table.revealPlaying())
                AppActions.revealed = true
        }
    }

    // Model changes invalidate row indices.
    onModelChanged: clearSelection()
    Connections {
        target: table.isArray ? null : table.model
        ignoreUnknownSignals: true
        function onModelReset() { table.clearSelection() }
        function onLayoutChanged() { table.clearSelection() }
        function onRowsRemoved() { table.clearSelection() }
    }

    TrackMenu {
        id: menu
        property var rows: []
        playlistName: table.playlistName
        playlistIndex: rows.length > 0 ? rows[0] : -1
        queueMode: table.queueMode
        onRemoveRequested: table.removeRequested(rows)
    }

    LMenu {
        id: columnsMenu
        Instantiator {
            model: TrackColumns.catalog
            delegate: LMenuItem {
                required property var modelData
                text: modelData.title
                checkable: true
                checked: TrackColumns.isVisible(modelData.key)
                enabled: !(checked && TrackColumns.keys.length === 1)
                onTriggered: TrackColumns.setVisible(modelData.key, checked)
            }
            onObjectAdded: (index, object) => columnsMenu.insertItem(index, object)
            onObjectRemoved: (index, object) => columnsMenu.removeItem(object)
        }
        LMenuSeparator {}
        LMenuItem { text: qsTr("Reset columns"); onTriggered: TrackColumns.reset() }
    }

    // Column header: outside the list, so rows never scroll underneath it
    // (they would show through it while a background style is on). A soft
    // rounded band, a little below the page title.
    readonly property int headerGap: 2
    RoundRect {
        id: headerBand
        // At the top of the page panel the band is the panel's top: edge to
        // edge, up to its rounded corner, a hairline under it. With something
        // above the list (sub-folders, a playlist's header...) it stays a band
        // of its own.
        readonly property Item panel: headerFade.panel
        // (the sizes and the table's place are there to re-map when they change)
        readonly property point offset: panel && panel.width + panel.height + table.width + table.y
                                                 + (table.parent ? table.parent.y : 0) > -1e9
                                        ? table.mapFromItem(panel, 0, 0) : Qt.point(0, 0)
        readonly property bool joined: panel !== null && offset.y > -16
        x: joined ? offset.x : 0
        y: joined ? offset.y : table.headerGap
        width: joined ? panel.width : parent.width - 12 // clear of the scroll bar
        height: header.height
        radius: joined ? 0 : Theme.radiusSmall + 2
        topLeftRadius: joined ? 14 : radius
        topRightRadius: joined ? 0 : radius
        color: Theme.surface
        borderWidth: 1
        borderColor: joined ? "transparent" : Qt.rgba(Theme.text.r, Theme.text.g, Theme.text.b, Theme.dark ? 0.05 : 0.08)
        Rectangle {
            visible: headerBand.joined
            anchors.bottom: parent.bottom
            width: parent.width
            height: 1
            color: Theme.border
        }
    }
    Item {
        id: headerStrip
        z: 3
        y: headerBand.y
        width: table.width - 12 // clear of the scroll bar
        height: header.height
        clip: true
        Rectangle {
            id: header
            // Follows the list's horizontal scroll.
            x: list.originX - list.contentX
            width: Math.max(list.width, table.rowWidth + 12)
            height: Theme.listMode ? 28 : 34
            color: "transparent"

            Row {
                height: parent.height
                // Sort button: the orders that are not a column (folder, added…).
                Item {
                    width: table.leadingWidth
                    height: parent.height
                    IconButton {
                        id: sortButton
                        visible: table.filter !== null
                        anchors.centerIn: parent
                        implicitWidth: Math.min(28, parent.height - 4)
                        implicitHeight: implicitWidth
                        tile: false
                        iconName: "sort"
                        iconSize: 14
                        active: table.filter !== null && table.filter.sortField.length > 0
                        tip: qsTr("Sort")
                        onClicked: sortMenu.popup(sortButton, 0, sortButton.height)
                    }
                }
                Item { width: table.coverWidth; height: 1 }
                Repeater {
                    model: TrackColumns.keys
                    delegate: Item {
                        id: hcell
                        required property string modelData
                        readonly property string key: modelData
                        readonly property bool sorted: table.filter !== null && table.filter.sortField === key
                        width: table.columnWidth(key)
                        height: header.height
                        opacity: table.dragKey === key ? 0.3 : 1

                        Rectangle {
                            anchors.fill: parent
                            anchors.topMargin: 4
                            anchors.bottomMargin: 4
                            anchors.leftMargin: 3
                            anchors.rightMargin: 3
                            radius: Theme.radiusSmall
                            color: headerMouse.pressed ? Theme.surfaceHi : headerMouse.containsMouse ? Theme.hover : "transparent"
                        }
                        Row {
                            anchors.verticalCenter: parent.verticalCenter
                            x: TrackColumns.cellPadding // from the column's left; its divider is its right edge
                            spacing: 4
                            // Favorite: the rows' heart instead of its name.
                            Icon {
                                anchors.verticalCenter: parent.verticalCenter
                                visible: hcell.key === "favorite"
                                glyph: "heart-filled"
                                size: 11
                                color: hcell.sorted ? Theme.accent : Theme.textDim
                            }
                            SectionLabel {
                                anchors.verticalCenter: parent.verticalCenter
                                visible: hcell.key !== "favorite"
                                width: Math.min(implicitWidth, hcell.width - 2 * TrackColumns.cellPadding - (hcell.sorted ? 15 : 0))
                                text: TrackColumns.title(hcell.key)
                                elide: Text.ElideRight
                                // quieter than the section labels: the rows matter
                                font.capitalization: Font.MixedCase
                                font.pixelSize: Theme.fontSmall
                                font.weight: Font.Medium
                                font.letterSpacing: 0
                                color: hcell.sorted ? Theme.accent : Theme.textDim
                            }
                            Icon {
                                anchors.verticalCenter: parent.verticalCenter
                                visible: hcell.sorted
                                glyph: "chevron-down"
                                size: 11
                                rotation: table.filter && table.filter.descending ? 0 : 180
                                color: Theme.accent
                            }
                        }
                        // Click: sort · drag: move the column · right-click: columns menu
                        MouseArea {
                            id: headerMouse
                            anchors.fill: parent
                            acceptedButtons: Qt.LeftButton | Qt.RightButton
                            hoverEnabled: true
                            preventStealing: true
                            cursorShape: table.dragKey.length > 0 ? Qt.ClosedHandCursor : Qt.PointingHandCursor
                            property real pressX
                            property bool moved: false
                            onPressed: (mouse) => {
                                pressX = mouse.x
                                moved = false
                            }
                            onPositionChanged: (mouse) => {
                                if (!(pressedButtons & Qt.LeftButton))
                                    return
                                const hx = mapToItem(header, mouse.x, 0).x
                                if (!moved && Math.abs(mouse.x - pressX) > 8) {
                                    moved = true
                                    table.beginColumnDrag(hcell.key, pressX)
                                }
                                if (moved)
                                    table.updateColumnDrag(hx)
                            }
                            onReleased: if (moved) table.endColumnDrag()
                            onCanceled: if (moved) { table.dragKey = ""; moved = false }
                            onClicked: (mouse) => {
                                if (moved)
                                    return
                                if (mouse.button === Qt.RightButton)
                                    columnsMenu.popup(hcell, mouse.x, mouse.y)
                                else
                                    table.sortBy(hcell.key)
                            }
                        }
                    }
                }
                Item { width: table.trailingWidth; height: header.height }
            }

            // Dividers between columns; they are also the resize grips.
            Repeater {
                model: TrackColumns.keys.length - (table.overflowing ? 0 : 1)
                delegate: MouseArea {
                    id: grip
                    required property int index
                    readonly property real edge: table.columnEdge(index + 1)
                    x: edge - width / 2
                    width: 16 // easy to catch; the line itself stays thin
                    height: header.height
                    visible: table.dragKey.length === 0
                    hoverEnabled: true
                    preventStealing: true
                    cursorShape: Qt.SplitHCursor
                    onPressed: (mouse) => table.beginResize(index, mapToItem(null, mouse.x, 0).x)
                    onPositionChanged: (mouse) => { if (pressed) table.updateResize(mapToItem(null, mouse.x, 0).x) }
                    onReleased: table.endResize()
                    onCanceled: table.endResize()
                    // resets the column this divider ends
                    onDoubleClicked: TrackColumns.resetWidth(TrackColumns.keys[index])
                    Rectangle {
                        anchors.horizontalCenter: parent.horizontalCenter
                        anchors.verticalCenter: parent.verticalCenter
                        width: grip.pressed || grip.containsMouse ? 2 : 1
                        height: grip.pressed || grip.containsMouse ? parent.height - 8 : parent.height * 0.42
                        radius: 1
                        color: grip.pressed || grip.containsMouse ? Theme.accent : Theme.border
                    }
                }
            }

            // Column being moved, and where it would land.
            RoundRect {
                visible: table.dragKey.length > 0
                x: table.dragX
                y: 3
                width: table.dragKey.length > 0 ? table.columnWidth(table.dragKey) : 0
                height: header.height - 6
                radius: Theme.radiusSmall
                color: Theme.surfaceHi
                borderWidth: 1
                borderColor: Theme.accent
                opacity: 0.95
                SectionLabel {
                    anchors.verticalCenter: parent.verticalCenter
                    x: TrackColumns.cellPadding
                    width: parent.width - 2 * TrackColumns.cellPadding
                    elide: Text.ElideRight
                    text: TrackColumns.title(table.dragKey)
                    color: Theme.text
                }
            }
            Rectangle {
                visible: table.dragKey.length > 0 && table.dropTarget >= 0
                x: table.columnEdge(table.dropTarget) - 1
                width: 3
                height: header.height
                radius: 1.5
                color: Theme.accent
            }

        }
    }
    // Rows fade out as they scroll under the header instead of being cut,
    // across the whole page panel (Main's "pagePanel"), not just the list.
    EdgeFade {
        id: headerFade
        readonly property Item panel: {
            for (let p = table.parent; p; p = p.parent)
                if (p.objectName === "pagePanel")
                    return p
            return null
        }
        z: 2
        anchors.top: headerBand.bottom
        x: headerBand.joined ? headerBand.offset.x : 0
        width: headerBand.joined ? panel.width : list.width
        height: 10
        tint: Theme.bg
        visible: list.contentY > list.originY
    }

    ListView {
        id: list
        WheelScroll { flickable: list }
        anchors.fill: parent
        anchors.topMargin: headerStrip.y + headerStrip.height + 4 // a little air under the header
        bottomMargin: 10 // the last row does not end flush against the player bar
        clip: true
        model: table.model
        spacing: Theme.listMode ? 0 : 1
        reuseItems: true
        focus: true
        keyNavigationEnabled: false
        boundsBehavior: Flickable.StopAtBounds
        flickableDirection: Flickable.AutoFlickIfNeeded
        contentWidth: table.rowWidth + 12
        highlightFollowsCurrentItem: false
        ScrollBar.vertical: LScrollBar {}
        ScrollBar.horizontal: LScrollBar {}

        Keys.onPressed: (event) => {
            if (event.key === Qt.Key_A && (event.modifiers & Qt.ControlModifier)) {
                table.selectAll()
                event.accepted = true
            } else if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter) {
                if (list.currentIndex >= 0)
                    table.activated(list.currentIndex)
                event.accepted = true
            } else if (event.key === Qt.Key_Delete && (table.playlistName.length > 0 || table.queueMode)) {
                if (table.selectedCount > 0)
                    table.removeRequested(table.selectedRows())
                event.accepted = true
            } else if (event.key === Qt.Key_Down || event.key === Qt.Key_Up) {
                const next = Math.max(0, Math.min(list.count - 1, list.currentIndex + (event.key === Qt.Key_Down ? 1 : -1)))
                table.clickRow(next, event.modifiers & Qt.ShiftModifier)
                list.positionViewAtIndex(next, ListView.Contain)
                event.accepted = true
            } else if (event.key === Qt.Key_Escape) {
                table.clearSelection()
                event.accepted = true
            }
        }

        delegate: TrackRow {
            id: rowItem
            required property int index
            required property var model
            width: table.rowWidth
            rowIndex: index
            rowData: table.isArray ? model.modelData : model
            leadingWidth: table.leadingWidth
            showCover: table.coverWidth > 0
            flexWidth: table.flexWidth
            primaryWidth: table.primaryWidth
            restRatio: table.restRatio
            selected: { table.selectionVersion; return table.isSelected(index) }

            onClicked: (modifiers) => table.clickRow(index, modifiers)
            onDoubleClicked: table.activated(index)
            onContextMenu: (mx, my) => table.openMenu(index, rowItem, mx, my)
            onDragStarted: (pos) => {
                table.ensureSelected(index)
                const paths = table.selectedPaths()
                const label = paths.length === 1 ? (rowData.title || "") : qsTr("%1 tracks").arg(paths.length)
                AppActions.beginTrackDrag(paths, label, pos, table.dragOrigin)
            }
            onDragMoved: (pos) => AppActions.moveTrackDrag(pos)
            onDragEnded: AppActions.endTrackDrag()
        }
    }
}
