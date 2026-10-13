import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Sonkore

// Compact mode (Settings › Appearance, Ctrl+M): the whole window is a small
// player, like Winamp or Audacious — cover, title, transport, seek and
// volume, then a strip of buttons (equalizer, pitch, play queue, options)
// and, when opened, a plain play queue below. Drag the player to move the
// window; the expand button returns to the full view.
// `playerHeight` is the window's height without the queue.
Rectangle {
    id: mini

    signal expandRequested()
    signal maximizeRequested()        // the window itself, not the full view
    signal minimizeRequested()
    signal closeRequested()
    signal settingsRequested()
    property real cornerRadius: 0
    property bool maximized: false
    readonly property var track: DspController.currentTrack
    readonly property bool hasTrack: DspController.currentPath.length > 0
    property real lastVolume: 1.0
    property double lastScrub: 0
    // The engine position, followed only while shown (the hidden player is
    // always created) and 4x per second on a Clock tick, so it shares the
    // visualizer's frames instead of redrawing the window 20x per second.
    property real position: 0
    property double lastStep: 0
    function syncPosition() { position = DspController.position; lastStep = Date.now() }
    onVisibleChanged: if (visible) { syncPosition(); adoptColumns() }
    Connections {
        target: Clock
        enabled: mini.visible
        function onTick() { if (Date.now() - mini.lastStep >= 250) mini.syncPosition() }
    }
    Connections {
        target: DspController
        enabled: mini.visible
        function onSeeked() { mini.syncPosition() }
        function onTrackChanged() { mini.syncPosition() }
    }
    // What fits, by width: the spectrum and the stream's mono / stereo
    // lights on wide players; the cover, then the equalizer and pitch
    // buttons, then the volume slider go as it narrows (the mute button and
    // the wheel stay).
    readonly property bool wide: width >= 640
    readonly property bool showSpectrum: width >= 500 && Theme.visualizer !== "off"
    readonly property bool showCover: width >= 400
    readonly property bool showStrip: width >= 470
    readonly property bool narrow: width < 430
    // Wide: the seek bar joins the controls' row (Winamp's single line).
    readonly property bool inlineSeek: width >= 560
    readonly property var activeSeek: inlineSeek ? inlineSeekBar : seek
    // Settings › Appearance › Interface density: the player's controls and
    // display grow or shrink with it.
    readonly property real ui: Theme.compact ? 0.88 : Theme.spacious ? 1.14 : 1
    readonly property real displayHeight: Math.round((wide ? 80 : 72) * ui)

    // --- List columns: chosen for this player (right-click the column heads,
    // or the sort menu), starting out as the full view's -------------------
    readonly property var columnKeys: {
        try {
            const k = JSON.parse(Prefs.store.compactColumns)
            if (Array.isArray(k)) {
                const valid = k.filter(x => TrackColumns.info(x) !== null)
                if (valid.length > 0)
                    return valid
            }
        } catch (e) {}
        return TrackColumns.keys
    }
    // The first time the player is shown it takes the full view's columns.
    function adoptColumns() {
        if (Prefs.store.compactColumns.length === 0)
            Prefs.store.compactColumns = JSON.stringify(TrackColumns.keys)
    }
    Component.onCompleted: {
        if (visible)
            adoptColumns()
        checkSource()
    }
    function setColumn(key, on) {
        let k = columnKeys.filter(x => x !== key)
        if (on)
            k.push(key)
        if (k.length > 0)
            Prefs.store.compactColumns = JSON.stringify(k)
    }
    // Short columns keep a width; the text ones share the rest (the first
    // a little more).
    readonly property var columnFixed: ({ duration: 44, year: 40, trackNo: 34, discNo: 34, format: 92, codec: 52,
                                          sampleRate: 66, bits: 54, channels: 60, bitrate: 82, fileSize: 64,
                                          added: 84, modified: 84, favorite: 26 })
    readonly property int columnGap: 8
    // Row width left for the columns: the frame's and the row's margins, the
    // number, the scroll bar.
    readonly property real columnsSpace: listFrame.width - 8 - 12 - 30 - columnGap
                                         - (listCover > 0 ? listCover + columnGap : 0)
                                         - (queueBar.scrollable ? queueBar.width + 2 : 0)
    // The chosen columns that fit, in their order. The first (at least
    // 140 px) and the time come first; then the others while they fit (one
    // that does not is left out, a shorter one after it may still come in;
    // text columns need 90 px).
    readonly property var shownColumns: {
        if (columnKeys.length === 0)
            return []
        const first = columnKeys[0]
        const byPriority = [first].concat(columnKeys.filter(k => k === "duration" && k !== first),
                                          columnKeys.filter(k => k !== first && k !== "duration"))
        const chosen = {}
        let used = 0
        for (const k of byPriority) {
            const w = (columnFixed[k] || (k === first ? 140 : 90)) + columnGap
            if (k !== first && used + w > columnsSpace + columnGap)
                continue
            chosen[k] = true
            used += w
        }
        return columnKeys.filter(k => chosen[k])
    }
    readonly property var columnWidths: {
        const out = {}
        let fixed = 0, weights = 0
        for (const k of shownColumns) {
            if (columnFixed[k])
                fixed += columnFixed[k]
            else
                weights += weights === 0 ? 1.5 : 1
        }
        const rest = Math.max(0, columnsSpace - fixed - columnGap * (shownColumns.length - 1))
        let first = true
        for (const k of shownColumns) {
            if (columnFixed[k]) {
                out[k] = columnFixed[k]
            } else {
                out[k] = Math.floor(rest * (first ? 1.5 : 1) / weights)
                first = false
            }
        }
        return out
    }
    // Column heads once there is more than one column.
    readonly property bool listColumns: shownColumns.length > 1
    // A track field's name, as the track lists' columns call it.
    function fieldTitle(field) {
        return field === "path" ? qsTr("As in the folder") : TrackColumns.title(field)
    }
    // The queue is reordered once; a folder keeps the order.
    // Favorites sort with the favourites on top.
    function sortBy(field) {
        if (showingQueue) {
            DspController.sortQueue(field, field === "favorite")
        } else {
            Prefs.store.compactSort = field
            Prefs.store.compactSortDesc = field === "favorite"
        }
    }
    // The display's clock: elapsed, or (a click on it) remaining.
    property bool showRemaining: false
    // Codec · rate · depth · bitrate of what plays (the format label's first
    // word is the codec: "FLAC 16/44.1").
    readonly property string streamText: [String(track.format || "").split(" ")[0]]
        .concat(["sampleRate", "bits", "bitrate"].map(k => TrackColumns.text(k, track)))
        .filter(t => t.length > 0 && t !== "—").join("  ·  ")

    readonly property real playerHeight: Math.ceil(toolbar.y + toolbar.height + 12)
    readonly property bool listOpen: Prefs.store.compactPlaylist
    // The least window height with the list open (Main): a few rows.
    // Frame margins 8 + source button 4 + 28 + 2 + list 4.
    // Settings › Appearance › List density, covers and alternate rows.
    readonly property int listRowHeight: Theme.listMode ? 22 : Theme.listCompact ? 26 : Theme.listSpacious ? 44 : 34
    readonly property int listCover: Theme.listCovers ? listRowHeight - 8 : 0
    readonly property real listMinHeight: playerHeight + 52 + 3 * listRowHeight
    implicitHeight: playerHeight
    radius: cornerRadius
    color: Theme.sidebar

    // The player's empty space moves the window; the wheel sets the volume.
    // (Not over the queue, which scrolls.)
    Item {
        width: parent.width
        height: mini.playerHeight
        DragHandler {
            target: null
            onActiveChanged: if (active) mini.Window.window.startSystemMove()
        }
        WheelHandler {
            onWheel: (event) => {
                const step = event.angleDelta.y > 0 ? 0.04 : -0.04
                const pos = Math.max(0, Math.min(1, Math.sqrt(DspController.volume) + step))
                DspController.volume = pos >= 0.999 ? 1.0 : pos * pos
            }
        }
    }

    // Title text that scrolls while it does not fit, like Winamp's: a pause,
    // one lap, a pause. Only while music plays (it costs frames); otherwise
    // it is elided.
    component Marquee: Item {
        id: marquee
        property string text
        property color color: Theme.text
        property int pixelSize: Theme.fontBody
        property int weight: Font.Normal
        property real offset: 0
        readonly property real gap: 48
        readonly property bool overflow: first.implicitWidth > width + 1
        implicitHeight: first.implicitHeight
        clip: true
        onTextChanged: { offset = 0; if (lap.running) lap.restart() }
        Row {
            x: -Math.round(marquee.offset) // whole pixels: the text stays sharp
            spacing: marquee.gap
            Label {
                id: first
                width: lap.running ? implicitWidth : marquee.width
                text: marquee.text
                elide: Text.ElideRight
                color: marquee.color
                font.pixelSize: marquee.pixelSize
                font.weight: marquee.weight
            }
            Label {
                visible: lap.running
                text: marquee.text
                color: marquee.color
                font.pixelSize: marquee.pixelSize
                font.weight: marquee.weight
            }
        }
        SequentialAnimation {
            id: lap
            running: marquee.overflow && marquee.visible && DspController.playing
                     && marquee.Window.visibility !== Window.Minimized
            loops: Animation.Infinite
            onRunningChanged: if (!running) marquee.offset = 0
            PauseAnimation { duration: 2500 }
            NumberAnimation {
                target: marquee
                property: "offset"
                from: 0
                to: first.implicitWidth + marquee.gap
                duration: (first.implicitWidth + marquee.gap) * 28 // about 35 px/s
            }
        }
    }

    // --- Display: cover, time and spectrum, title and stream details, as in
    // Winamp's main window, straight on the player's background --------------
    RowLayout {
        id: content
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        anchors.margins: 12
        spacing: 4

        Rectangle {
            id: display
            Layout.fillWidth: true
            Layout.minimumWidth: 0
            Layout.preferredHeight: mini.displayHeight
            // No panel of its own: it is the player's background.
            color: "transparent"

            RowLayout {
                anchors.fill: parent
                anchors.margins: 4
                spacing: 14

                CoverArt {
                    visible: mini.showCover
                    Layout.preferredWidth: parent.height
                    Layout.preferredHeight: parent.height
                    path: DspController.currentPath
                    radius: Math.max(2, Theme.radius - 4)
                }

                // State and time (click: elapsed / remaining), the spectrum under it.
                ColumnLayout {
                    Layout.fillHeight: true
                    Layout.leftMargin: mini.showCover ? 0 : 3
                    spacing: 2
                    RowLayout {
                        spacing: 6
                        Icon {
                            Layout.alignment: Qt.AlignVCenter
                            glyph: DspController.playing ? "play" : (mini.hasTrack ? "pause" : "music")
                            size: 10
                            color: DspController.playing ? Theme.accent : Theme.textFaint
                        }
                        Label {
                            readonly property real shown: mini.activeSeek.pressed ? mini.activeSeek.value : mini.position
                            text: !mini.hasTrack ? "0:00"
                                  : mini.showRemaining ? "−" + DspController.formatTime(Math.max(0, DspController.duration - shown))
                                  : DspController.formatTime(shown)
                            color: mini.hasTrack ? Theme.accent : Theme.textFaint
                            font.pixelSize: Math.round((mini.wide ? 30 : 26) * mini.ui)
                            font.weight: Font.Medium
                            font.features: { "tnum": 1 }
                            TapHandler { onTapped: mini.showRemaining = !mini.showRemaining }
                            HoverHandler { cursorShape: Qt.PointingHandCursor }
                        }
                    }
                    Visualizer {
                        visible: mini.showSpectrum
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        Layout.minimumWidth: 72
                        // no wider than a normal mini player's: on a wide
                        // (maximized) window the title takes the rest
                        Layout.maximumWidth: Math.round(220 * mini.ui)
                        Layout.minimumHeight: 10
                        style: Theme.visualizer
                    }
                }

                // Title, artist, and the stream: format, rate, depth, bitrate,
                // queue place and Winamp's mono / stereo lights. Down at the
                // bottom: the column fills the height, a spacer takes the room
                // above, and the last line's baseline sits on the cover's
                // bottom edge (moved down by the room a line keeps under its
                // letters).
                ColumnLayout {
                    FontMetrics {
                        id: lastLine
                        font: streamLabel.visible ? streamLabel.font : artistLabel.font
                    }
                    transform: Translate { y: Math.round(lastLine.descent) }
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    Layout.minimumWidth: 0
                    spacing: 2
                    Item { Layout.fillHeight: true }
                    Marquee {
                        Layout.fillWidth: true
                        // clear of the window's buttons in the corner
                        Layout.rightMargin: windowButtons.visible ? Math.max(0, windowButtons.width + windowButtons.anchors.margins - 16 + 8) : 0 // the column ends 16 px in
                        text: mini.track.title || qsTr("Nothing playing")
                        pixelSize: Theme.fontBody + (mini.wide ? 2 : 1)
                        weight: Font.DemiBold
                    }
                    Label {
                        id: artistLabel
                        Layout.fillWidth: true
                        text: mini.track.artist || (mini.hasTrack ? "" : qsTr("Pick something from your library"))
                        visible: text.length > 0
                        elide: Text.ElideRight
                        color: Theme.textDim
                        font.pixelSize: Theme.fontSmall + 1
                    }
                    RowLayout {
                        Layout.fillWidth: true
                        Layout.topMargin: 2
                        visible: mini.hasTrack
                        spacing: 8
                        Label {
                            id: streamLabel
                            // the line's labels share one baseline
                            Layout.alignment: Qt.AlignBaseline
                            Layout.fillWidth: true
                            Layout.minimumWidth: 0
                            text: mini.streamText
                            elide: Text.ElideRight
                            color: Theme.textFaint
                            font.pixelSize: Theme.fontSmall
                            font.features: { "tnum": 1 }
                        }
                        Label {
                            text: DspController.queue.length > 0
                                  ? (DspController.currentIndex + 1) + " / " + DspController.queue.length : ""
                            visible: text.length > 0
                            Layout.alignment: Qt.AlignBaseline
                            color: Theme.textFaint
                            font.pixelSize: Theme.fontSmall
                            font.features: { "tnum": 1 }
                        }
                        Row {
                            id: channelsRow
                            visible: mini.wide && (mini.track.channels || 0) > 0
                            Layout.alignment: Qt.AlignBaseline
                            baselineOffset: children.length > 0 ? children[0].baselineOffset : 0
                            spacing: 6
                            Repeater {
                                model: [1, 2]
                                Label {
                                    required property int modelData
                                    readonly property bool lit: modelData === 1 ? mini.track.channels === 1 : mini.track.channels >= 2
                                    text: TrackColumns.text("channels", { channels: modelData }).toUpperCase()
                                    color: lit ? Theme.accent : Theme.textFaint
                                    opacity: lit ? 1 : 0.45
                                    font.pixelSize: Theme.fontSmall - 2
                                    font.weight: Font.Bold
                                    font.letterSpacing: 0.6
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // The window's buttons, as in the full view's title bar (Settings ›
    // Appearance › Window buttons), in the window's corner: the title stops
    // short of them, the lines under it run beneath them. Maximize maximizes
    // the mini player's own window (only wider while the queue is closed: its
    // height is fixed); the expand button and Ctrl+M lead to the full view.
    // With the system's title bar the desktop draws them: none here (the
    // expand button and Ctrl+M still lead to the full view).
    WindowButtons {
        id: windowButtons
        anchors.top: parent.top
        anchors.right: parent.right
        anchors.margins: edge
        cornerRadius: mini.cornerRadius
        maximized: mini.maximized
        onMinimizeClicked: mini.minimizeRequested()
        onMaximizeClicked: mini.maximizeRequested()
        onCloseClicked: mini.closeRequested()
    }

    // --- Seek: under the display, or (wide) in the controls' row -------------
    component SeekBar: LSlider {
        seekBar: true
        from: 0
        to: Math.max(DspController.duration, 0.001)
        enabled: mini.hasTrack
        alwaysShowHandle: pressed
        value: pressed ? value : mini.position
        onMoved: {
            if (Prefs.store.liveSeek && Date.now() - mini.lastScrub > 45) {
                mini.lastScrub = Date.now()
                DspController.seek(value)
            }
        }
        onPressedChanged: if (!pressed) DspController.seek(value)
    }
    SeekBar {
        id: seek
        visible: !mini.inlineSeek
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: content.bottom
        anchors.leftMargin: 10
        anchors.rightMargin: 10
        anchors.topMargin: 6
        height: visible ? implicitHeight : 0
    }

    // --- Controls: transport, then equalizer, pitch, play queue, volume,
    // options. The equalizer and pitch buttons go on narrow windows (they
    // are in the options menu too), then the volume slider.
    RowLayout {
        id: toolbar
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: mini.inlineSeek ? content.bottom : seek.bottom
        anchors.topMargin: mini.inlineSeek ? 12 : 6
        anchors.leftMargin: 10
        anchors.rightMargin: 10
        spacing: 0
        // The icon style's tiles too (Settings › Appearance › Icon style).
        component StripButton: IconButton {
            iconSize: Math.round(15 * mini.ui)
            implicitWidth: Math.round(30 * mini.ui)
            implicitHeight: Math.round(28 * mini.ui)
        }
        StripButton {
            iconName: "shuffle"
            active: DspController.shuffle
            tip: qsTr("Shuffle")
            onClicked: DspController.shuffle = !DspController.shuffle
        }
        // The player buttons in the chosen set (Settings › Appearance ›
        // Player buttons: the play button's shape); stop only when asked for (Settings › General).
        StripButton {
            iconName: "prev"
            iconSize: Math.round(16 * mini.ui)
            tip: qsTr("Previous")
            onClicked: DspController.previous()
        }
        PlayButton {
            Layout.leftMargin: 2
            Layout.rightMargin: 2
            implicitWidth: Math.round(32 * mini.ui)
            iconSize: Math.round(14 * mini.ui)
        }
        StripButton {
            visible: Prefs.store.stopButton
            iconName: "stop"
            tip: qsTr("Stop")
            onClicked: DspController.stop()
        }
        StripButton {
            iconName: "next"
            iconSize: Math.round(16 * mini.ui)
            tip: qsTr("Next")
            onClicked: DspController.next()
        }
        StripButton {
            iconName: DspController.repeatMode === 2 ? "repeat-one" : "repeat"
            active: DspController.repeatMode !== 0
            tip: [qsTr("Repeat off"), qsTr("Repeat all"), qsTr("Repeat one")][DspController.repeatMode]
            onClicked: DspController.repeatMode = (DspController.repeatMode + 1) % 3
        }
        SeekBar {
            id: inlineSeekBar
            visible: mini.inlineSeek
            Layout.fillWidth: true
            Layout.leftMargin: 8
            Layout.rightMargin: 8
        }
        Rectangle {
            visible: !mini.inlineSeek
            Layout.leftMargin: 6
            Layout.rightMargin: 6
            implicitWidth: 1
            implicitHeight: 16
            color: Theme.border
        }
        StripButton {
            visible: mini.showStrip
            iconName: "eq"
            active: Equalizer.enabled
            tip: Equalizer.enabled ? qsTr("Equalizer on — click to open") : qsTr("Equalizer off — click to open")
            onClicked: AppActions.navigate("eq", "")
        }
        StripButton {
            visible: mini.showStrip
            iconName: "wave"
            active: DspController.soundTouchActive
            tip: qsTr("Pitch / tempo (%1)").arg(DspController.pitchEngineName)
            onClicked: AppActions.openSoundTouch()
        }
        StripButton {
            iconName: "queue"
            active: mini.listOpen
            tip: mini.listOpen ? qsTr("Hide play queue") : qsTr("Show play queue")
            onClicked: Prefs.store.compactPlaylist = !Prefs.store.compactPlaylist
        }
        Item { visible: !mini.inlineSeek; Layout.fillWidth: true }
        IconButton {
            id: volumeButton
            TapHandler {
                acceptedButtons: Qt.RightButton
                onTapped: mini.openVolumeMenu()
            }
            iconName: DspController.volume === 0 ? "volume-mute" : (DspController.volume < 0.5 ? "volume-low" : "volume")
            iconSize: 15
            implicitWidth: 28
            implicitHeight: 28
            iconColor: DspController.volumeFixed ? Theme.textFaint : (hovered ? Theme.text : Theme.textDim)
            tip: volumeMenu.opened ? "" : DspController.volumeFixed ? qsTr("Volume fixed at 100 %: set it on the DAC or the amplifier. Click to choose how the volume is set.")
                                       : qsTr("Mute · right-click: volume control")
            onClicked: {
                if (DspController.volumeFixed) {
                    mini.openVolumeMenu()
                    return
                }
                if (DspController.volume > 0) {
                    mini.lastVolume = DspController.volume
                    DspController.volume = 0
                } else {
                    DspController.volume = mini.lastVolume
                }
            }
        }
        LockedVolume {
            id: lockedVolume
            menuOpen: volumeMenu.opened
            onClicked: mini.openVolumeMenu()
            visible: !mini.narrow && DspController.volumeFixed
            brief: true
            Layout.preferredWidth: mini.wide ? 96 : 72
        }
        LSlider {
            id: volume
            visible: !mini.narrow && !DspController.volumeFixed
            fillColor: hovered || pressed ? Theme.accent : Theme.textDim // the accent stays for what plays
            Layout.preferredWidth: mini.wide ? 96 : 72
            from: 0
            to: 1
            // Perceptual taper: slider position = sqrt(gain).
            value: Math.sqrt(DspController.volume)
            onMoved: DspController.volume = value >= 0.999 ? 1.0 : value * value
            ToolTip.visible: hovered || pressed
            ToolTip.text: DspController.hardwareVolume
            ? qsTr("DAC volume · %1 · bit-perfect").arg(DspController.volume > 0 ? (20 * Math.log10(DspController.volume)).toFixed(1) + " dB" : "−∞ dB")
            : Math.round(value * 100) + " %"
        }
        StripButton {
            Layout.leftMargin: 4
            iconName: "expand"
            iconSize: 14
            tip: qsTr("Full view (Ctrl+M)")
            onClicked: mini.expandRequested()
        }
        StripButton {
            id: optionsButton
            iconName: "more"
            tip: qsTr("Options")
            onClicked: optionsMenu.popup(optionsButton, optionsButton.width - optionsMenu.width, optionsButton.height + 4)
        }
    }

    LMenu {
        id: optionsMenu
        Component.onCompleted: if ("popupType" in optionsMenu) optionsMenu.popupType = 1 // own window: the player is small
        LMenuItem { text: qsTr("Always on top"); checkable: true; checked: Prefs.store.alwaysOnTop; onTriggered: Prefs.store.alwaysOnTop = !Prefs.store.alwaysOnTop }
        LMenuItem { text: mini.listOpen ? qsTr("Hide play queue") : qsTr("Show play queue"); onTriggered: Prefs.store.compactPlaylist = !Prefs.store.compactPlaylist }
        LMenuItem { text: qsTr("Clear queue"); enabled: DspController.queue.length > 0; onTriggered: DspController.clearQueue() }
        LMenuSeparator {}
        LMenuItem { text: qsTr("Equalizer"); onTriggered: AppActions.navigate("eq", "") }
        LMenuItem { text: qsTr("Pitch / tempo"); onTriggered: AppActions.openSoundTouch() }
        LMenuItem { text: qsTr("Settings"); onTriggered: mini.settingsRequested() }
        LMenuSeparator {}
        LMenuItem { text: qsTr("Full view (Ctrl+M)"); onTriggered: mini.expandRequested() }
    }

    // --- Track list: the play queue or a library folder, one plain line per
    // track, as in Winamp's playlist -------------------------------------------
    // "" = the play queue, else the folder shown (kept between sessions).
    readonly property string source: Prefs.store.compactSource
    readonly property bool showingQueue: source.length === 0
    // Folders that hold music, for the source menu (read while compact).
    readonly property var folders: {
        Library.count
        Library.scanning
        return Prefs.store.compactMode ? Library.trackFolders() : []
    }
    // The folders as a tree, in order: each folder that holds music and the
    // folders between it and its library folder (a folder shows its
    // subfolders' tracks too, so `total` counts them).
    readonly property var folderTree: {
        const nodes = {}
        const roots = []
        for (const f of folders) {
            const base = f.path.slice(0, f.path.length - f.name.length)
            const parts = f.name.split("/")
            let parent = null
            for (let i = 0; i < parts.length; ++i) {
                const path = base + parts.slice(0, i + 1).join("/")
                let n = nodes[path]
                if (!n) {
                    n = nodes[path] = { path: path, name: parts[i], label: parts.slice(0, i + 1).join("  ›  "),
                                        depth: i, total: 0, children: [] }
                    if (parent)
                        parent.children.push(n)
                    else
                        roots.push(n)
                }
                n.total += f.count
                parent = n
            }
        }
        const out = []
        const byName = (a, b) => a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: "base" })
        const walk = (list) => {
            list.sort(byName)
            list.forEach((n, i) => {
                out.push({ path: n.path, name: n.name, label: n.label, depth: n.depth, total: n.total,
                           last: i === list.length - 1 })
                walk(n.children)
            })
        }
        walk(roots)
        return out
    }
    readonly property int folderDeepest: folderTree.reduce((m, n) => Math.max(m, n.depth), 0)
    // Subfolder levels shown in the source menu: none, or 1 up to the chosen.
    readonly property int folderLevels: Prefs.store.compactSubfolders
                                        ? Math.max(1, Math.min(Prefs.store.compactFolderDepth, folderDeepest)) : 0
    readonly property var shownFolders: folderTree.filter(n => n.depth <= folderLevels)
    // A folder that left the library (removed, or the last one gone, also
    // between sessions) falls back to the queue, once the library is read.
    function checkSource() {
        if (!showingQueue && Prefs.store.compactMode && !Library.scanning && !folderTree.some(f => f.path === source))
            Prefs.store.compactSource = ""
    }
    onFolderTreeChanged: checkSource()
    readonly property string sourceName: {
        if (showingQueue)
            return qsTr("Play queue")
        const f = folderTree.find(e => e.path === source)
        return f ? f.label : source.substring(source.lastIndexOf("/") + 1)
    }

    // Loaded with compact mode, not when the list opens: it is ready at once.
    TrackListModel {
        id: queueModel
        paths: Prefs.store.compactMode ? DspController.queue : []
    }
    TrackFilterModel {
        id: folderModel
        // Kept empty while the queue is shown ("" would be the whole library).
        folder: mini.showingQueue ? "/\u0001" : mini.source
        // "path": as the folder holds them, like the Folders page
        sortField: Prefs.store.compactSort
        descending: Prefs.store.compactSortDesc
    }

    Rectangle {
        id: listFrame
        // Shown by the room there is, not by the toggle: the window resizes
        // just after it (once no frame is in flight, Main), and in between the
        // list follows the window's edge instead of leaving a blank strip.
        visible: mini.height - mini.playerHeight >= 48
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        anchors.topMargin: mini.playerHeight
        anchors.bottom: parent.bottom
        anchors.leftMargin: 6
        anchors.rightMargin: 6
        anchors.bottomMargin: 6
        radius: Theme.radiusSmall
        // Concentric with the window's rounded corners, 6 px inside them.
        bottomLeftRadius: Math.max(Theme.radiusSmall, mini.cornerRadius - 6)
        bottomRightRadius: Math.max(Theme.radiusSmall, mini.cornerRadius - 6)
        // the lists' darker panel colour, as the full view's page
        color: Theme.pane
        border.color: Qt.rgba(Theme.text.r, Theme.text.g, Theme.text.b, Theme.dark ? 0.045 : 0.07)

        // As the full view's: the panel's top is a band, edge to edge, under
        // the source and the column heads, a hairline under it.
        Rectangle {
            x: 1
            y: 1
            width: parent.width - 2
            height: (listHead.visible ? listHead.y + listHead.height : sourceButton.y + sourceButton.height) + 2 - y
            topLeftRadius: listFrame.radius - 1
            topRightRadius: listFrame.radius - 1
            color: Theme.surface
            Rectangle {
                anchors.bottom: parent.bottom
                width: parent.width
                height: 1
                color: Theme.border
            }
        }

        // What the list shows: the queue or a folder.
        AbstractButton {
            id: sourceButton
            anchors.top: parent.top
            anchors.left: parent.left
            anchors.right: sortButton.left
            anchors.margins: 4
            anchors.rightMargin: 2
            height: 28
            hoverEnabled: true
            focusPolicy: Qt.NoFocus
            onClicked: sourceMenu.popup(sourceButton, 0, sourceButton.height + 2)
            // no box on the head's band: hovered, the name and its chevron light up
            background: Item {}
            opacity: pressed ? 0.7 : 1
            contentItem: Item {
                Icon {
                    id: sourceIcon
                    anchors.verticalCenter: parent.verticalCenter
                    x: 6
                    glyph: mini.showingQueue ? "queue" : "folder"
                    size: 14
                    color: Theme.accent
                }
                Label {
                    anchors.verticalCenter: parent.verticalCenter
                    anchors.left: sourceIcon.right
                    anchors.leftMargin: 8
                    anchors.right: sourceChevron.left
                    anchors.rightMargin: 6
                    text: mini.sourceName
                    elide: Text.ElideMiddle
                    color: sourceButton.hovered ? Theme.accent : Theme.text
                    font.pixelSize: Theme.fontSmall + 1
                    font.weight: Font.DemiBold
                }
                Icon {
                    id: sourceChevron
                    anchors.verticalCenter: parent.verticalCenter
                    anchors.right: parent.right
                    anchors.rightMargin: 6
                    glyph: "chevron-down"
                    size: 13
                    color: sourceButton.hovered ? Theme.accent : Theme.textDim
                }
            }
        }
        // The source: the queue, or a folder of the tree (a folder plays its
        // subfolders too). Subfolders can be hidden, or shown down to a level.
        component FolderItem: MenuItem {
            id: folderItem
            required property var modelData
            readonly property int depth: modelData.depth
            readonly property bool selected: modelData.path === mini.source
            // A hidden subfolder of it is the one shown.
            readonly property bool holdsSelected: !selected && mini.source.startsWith(modelData.path + "/")
            implicitHeight: 26
            leftPadding: 8
            rightPadding: 10
            indicator: Item {}
            arrow: Item {}
            onTriggered: Prefs.store.compactSource = modelData.path
            background: Rectangle {
                radius: Theme.radiusSmall
                color: folderItem.selected ? Theme.accentSoft : folderItem.highlighted ? Theme.hover : "transparent"
            }
            contentItem: Item {
                readonly property real indent: folderItem.depth * 16
                implicitWidth: indent + 20 + folderName.implicitWidth + 12 + folderCount.implicitWidth
                // tree guides: ├ / └ from the parent's folder icon
                Rectangle {
                    visible: folderItem.depth > 0
                    x: parent.indent - 10
                    width: 1
                    height: folderItem.modelData.last ? parent.height / 2 : parent.height
                    color: Theme.border
                }
                Rectangle {
                    visible: folderItem.depth > 0
                    x: parent.indent - 10
                    y: Math.round(parent.height / 2)
                    width: 7
                    height: 1
                    color: Theme.border
                }
                Icon {
                    id: folderIcon
                    x: parent.indent
                    anchors.verticalCenter: parent.verticalCenter
                    glyph: "folder"
                    size: folderItem.depth === 0 ? 14 : 12
                    color: folderItem.selected || folderItem.holdsSelected ? Theme.accent
                         : folderItem.depth === 0 ? Theme.textDim : Theme.textFaint
                }
                Label {
                    id: folderName
                    anchors.verticalCenter: parent.verticalCenter
                    anchors.left: folderIcon.right
                    anchors.leftMargin: 7
                    anchors.right: folderCount.left
                    anchors.rightMargin: 10
                    text: folderItem.modelData.name
                    elide: Text.ElideRight
                    color: folderItem.selected ? Theme.accent : folderItem.depth === 0 ? Theme.text : Theme.textDim
                    font.pixelSize: Theme.fontSmall + 1
                    font.weight: folderItem.depth === 0 || folderItem.selected ? Font.DemiBold : Font.Normal
                }
                Label {
                    id: folderCount
                    anchors.verticalCenter: parent.verticalCenter
                    anchors.right: parent.right
                    // The remove button takes its place under the pointer.
                    opacity: folderItem.hovered ? 0 : 1
                    text: folderItem.modelData.total
                    color: folderItem.selected ? Theme.accent : Theme.textFaint
                    font.pixelSize: Theme.fontSmall
                    font.features: { "tnum": 1 }
                }
                IconButton {
                    anchors.verticalCenter: parent.verticalCenter
                    anchors.right: parent.right
                    anchors.rightMargin: -6
                    visible: folderItem.hovered
                    implicitWidth: 22
                    implicitHeight: 22
                    tile: false
                    iconName: "trash"
                    iconSize: 12
                    tip: qsTr("Remove from library")
                    onClicked: {
                        sourceMenu.removeNext = folderItem.modelData.path
                        sourceMenu.close()
                    }
                }
            }
        }
        LMenu {
            id: sourceMenu
            width: 290
            // Long libraries scroll inside it instead of filling the screen.
            height: Math.min(implicitHeight, 520)
            // Own window: the player is small, and the folders are many.
            Component.onCompleted: if ("popupType" in sourceMenu) sourceMenu.popupType = 1
            LMenuItem {
                dense: true
                text: qsTr("Play queue")
                checkable: true
                checked: mini.showingQueue
                onTriggered: Prefs.store.compactSource = ""
            }
            LMenuItem {
                dense: true
                alignChecks: true
                text: qsTr("Add music folder…")
                onTriggered: sourceMenu.addNext = true
            }
            LMenuSeparator {}
            // Subfolders on / off, and how deep (these keep the menu open).
            SubfolderLevels {
                width: sourceMenu.availableWidth
                shown: Prefs.store.compactSubfolders
                value: mini.folderLevels
                deepest: mini.folderDeepest
                onShownToggled: (on) => Prefs.store.compactSubfolders = on
                onValuePicked: (v) => Prefs.store.compactFolderDepth = v
            }
            LMenuSeparator {}
            Instantiator {
                model: sourceMenu.opened || sourceMenu.visible ? mini.shownFolders : []
                delegate: FolderItem {}
                onObjectAdded: (index, object) => sourceMenu.insertItem(index + 5, object)
                onObjectRemoved: (index, object) => sourceMenu.removeItem(object)
            }
            // Dialogs open once this menu has closed (a pop-up window asked
            // for while another one is still closing does not show).
            property bool addNext: false
            property string removeNext
            onClosed: {
                if (addNext)
                    AppActions.addFolderRequested()
                else if (removeNext.length > 0)
                    AppActions.removeFolderRequested(removeNext) // asks first
                addNext = false
                removeNext = ""
            }
        }

        // Sort: a folder keeps its order (Settings-free, remembered); the
        // queue is reordered once, the current track playing on.
        IconButton {
            id: sortButton
            anchors.top: parent.top
            anchors.right: parent.right
            anchors.margins: 4
            implicitWidth: 28
            implicitHeight: 28
            iconName: "sort"
            iconSize: 14
            active: !mini.showingQueue && (Prefs.store.compactSort !== "path" || Prefs.store.compactSortDesc)
            tip: qsTr("Sort by")
            onClicked: sortMenu.popup(sortButton, sortButton.width - sortMenu.width, sortButton.height + 2)
        }
        LMenu {
            id: sortMenu
            Component.onCompleted: if ("popupType" in sortMenu) sortMenu.popupType = 1
            component SortItem: LMenuItem {
                property string field
                text: mini.fieldTitle(field)
                checkable: !mini.showingQueue
                checked: !mini.showingQueue && Prefs.store.compactSort === field
                onTriggered: mini.sortBy(field)
            }
            SortItem { field: "path" }
            SortItem { field: "fileName" }
            SortItem { field: "title" }
            SortItem { field: "artist" }
            SortItem { field: "albumArtist" }
            SortItem { field: "album" }
            SortItem { field: "genre" }
            SortItem { field: "duration" }
            SortItem { field: "year" }
            SortItem { field: "trackNo" }
            SortItem { field: "added" }
            SortItem { field: "favorite" }
            LMenuSeparator {}
            LMenuItem {
                text: qsTr("Reverse order")
                checkable: !mini.showingQueue
                checked: !mini.showingQueue && Prefs.store.compactSortDesc
                onTriggered: {
                    if (mini.showingQueue)
                        DspController.reverseQueue()
                    else
                        Prefs.store.compactSortDesc = !Prefs.store.compactSortDesc
                }
            }
            LMenuSeparator {}
            LMenuItem {
                text: qsTr("Columns…")
                alignChecks: !mini.showingQueue
                // Opened once this menu has closed: a pop-up window asked
                // for while another one is still closing does not show.
                onTriggered: sortMenu.columnsNext = true
            }
            property bool columnsNext: false
            onClosed: if (columnsNext) {
                columnsNext = false
                columnsMenu.popup(sortButton, sortButton.width - columnsMenu.width, sortButton.height + 2)
            }
        }

        // Column heads: a click sorts by that column (again: reversed), a
        // right-click chooses the columns.
        Item {
            id: listHead
            visible: mini.listColumns && queueList.count > 0
            anchors.top: sourceButton.bottom
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.leftMargin: 4
            anchors.rightMargin: 4
            height: visible ? 26 : 0
            TapHandler {
                acceptedButtons: Qt.RightButton
                onTapped: (point) => columnsMenu.popup(listHead, point.position.x, point.position.y)
            }

            Row {
                x: 6
                anchors.verticalCenter: parent.verticalCenter
                spacing: mini.columnGap
                Label { width: 30; text: "#"; color: Theme.textDim; font.pixelSize: Theme.fontSmall; font.weight: Font.Medium }
                Item { visible: mini.listCover > 0; width: mini.listCover; height: 1 } // over the covers
                Repeater {
                    model: mini.shownColumns
                    Label {
                        id: head
                        required property string modelData
                        readonly property bool sortable: true
                        readonly property bool sorted: !mini.showingQueue && Prefs.store.compactSort === modelData
                        width: mini.columnWidths[modelData] || 0
                        text: modelData === "favorite" ? ""
                              : mini.fieldTitle(modelData) + (sorted ? (Prefs.store.compactSortDesc ? "  ↓" : "  ↑") : "")
                        elide: Text.ElideRight
                        color: sorted ? Theme.accent : (headHover.hovered && sortable ? Theme.text : Theme.textDim)
                        font.pixelSize: Theme.fontSmall
                        font.weight: Font.Medium
                        HoverHandler { id: headHover; cursorShape: head.sortable ? Qt.PointingHandCursor : Qt.ArrowCursor }
                        // Favorite: a heart over the rows' hearts, named on hover.
                        Icon {
                            visible: head.modelData === "favorite"
                            anchors.centerIn: parent
                            glyph: "heart-filled"
                            size: 11
                            color: head.sorted ? Theme.accent : headHover.hovered ? Theme.text : Theme.textDim
                        }
                        ToolTip.visible: head.modelData === "favorite" && headHover.hovered
                        ToolTip.delay: 500
                        ToolTip.text: mini.fieldTitle("favorite")
                        TapHandler {
                            enabled: head.sortable
                            onTapped: {
                                if (head.sorted)
                                    Prefs.store.compactSortDesc = !Prefs.store.compactSortDesc
                                else
                                    mini.sortBy(head.modelData)
                            }
                        }
                    }
                }
            }
        }
        // The columns to show: two columns of check boxes, compact.
        LMenu {
            id: columnsMenu
            width: 2 * 150 + 2 + 2 * padding
            tone: Theme.sidebar
            Component.onCompleted: if ("popupType" in columnsMenu) columnsMenu.popupType = 1
            Grid {
                columns: 2
                columnSpacing: 2
                rowSpacing: 0
                Repeater {
                    model: TrackColumns.catalog
                    AbstractButton {
                        id: columnBox
                        required property var modelData
                        readonly property bool on: mini.columnKeys.indexOf(modelData.key) >= 0
                        width: 150
                        height: 24
                        hoverEnabled: true
                        focusPolicy: Qt.NoFocus
                        enabled: !(on && mini.columnKeys.length === 1) // at least one
                        onClicked: mini.setColumn(modelData.key, !on)
                        background: Rectangle {
                            radius: Theme.radiusSmall
                            color: columnBox.hovered ? Theme.hover : "transparent"
                        }
                        contentItem: Row {
                            leftPadding: 6
                            spacing: 6
                            Rectangle { // the box
                                anchors.verticalCenter: parent.verticalCenter
                                width: 13
                                height: 13
                                radius: 3
                                color: columnBox.on ? Theme.accent : "transparent"
                                border.width: columnBox.on ? 0 : 1
                                border.color: Theme.textFaint
                                Icon {
                                    anchors.centerIn: parent
                                    visible: columnBox.on
                                    glyph: "check"
                                    size: 10
                                    color: Theme.onAccent
                                }
                            }
                            Label {
                                anchors.verticalCenter: parent.verticalCenter
                                width: columnBox.width - 31
                                text: columnBox.modelData.title
                                elide: Text.ElideRight
                                color: !columnBox.enabled ? Theme.textFaint : columnBox.on ? Theme.text : Theme.textDim
                                font.pixelSize: Theme.fontSmall + 1
                            }
                        }
                    }
                }
            }
            LMenuSeparator {}
            // Back to the full view's columns.
            LMenuItem { dense: true; text: qsTr("Reset columns"); onTriggered: Prefs.store.compactColumns = JSON.stringify(TrackColumns.keys) }
        }

        ListView {
            id: queueList
            anchors.top: listHead.bottom
            anchors.topMargin: 2
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.bottom: parent.bottom
            anchors.margins: 4
            clip: true
            boundsBehavior: Flickable.StopAtBounds
            model: mini.showingQueue ? queueModel : folderModel
            WheelScroll { flickable: queueList }
            // the scroll bar against the panel's right edge, not the list's
            ScrollBar.vertical: LScrollBar {
                id: queueBar
                parent: listFrame
                x: listFrame.width - width - 1
                y: queueList.y
                height: queueList.height
            }
            // The playing track in view when the list opens or changes.
            function revealCurrent() {
                const row = mini.showingQueue ? DspController.currentIndex : folderModel.rowOfPath(DspController.currentPath)
                if (row >= 0 && row < count)
                    positionViewAtIndex(row, ListView.Contain)
            }
            onCountChanged: Qt.callLater(revealCurrent)
            onModelChanged: Qt.callLater(revealCurrent)
            onVisibleChanged: if (visible) Qt.callLater(revealCurrent)
            Connections {
                target: DspController
                enabled: queueList.visible
                function onTrackChanged() { Qt.callLater(queueList.revealCurrent) }
            }
            delegate: Rectangle {
                id: line
                required property int index
                required property var model
                readonly property bool current: mini.showingQueue ? index === DspController.currentIndex
                                                                  : model.path === DspController.currentPath
                readonly property bool missing: model.missing === true
                width: queueList.width - (queueBar.scrollable ? queueBar.width + 2 : 0)
                height: mini.listRowHeight
                radius: Theme.listMode ? 0 : Theme.radiusSmall
                color: current ? Theme.accentSoft : lineHover.hovered ? Theme.hover
                     : Theme.alternateRows && index % 2 === 1 ? Theme.rowAlt : "transparent"
                HoverHandler { id: lineHover }
                TapHandler {
                    onDoubleTapped: mini.playRow(line.index)
                }
                TapHandler {
                    acceptedButtons: Qt.RightButton
                    onTapped: (point) => {
                        lineMenu.row = line.index
                        lineMenu.path = line.model.path
                        lineMenu.popup(line, point.position.x, point.position.y)
                    }
                }
                Row {
                    x: 6
                    height: parent.height
                    spacing: mini.columnGap
                    Label {
                        anchors.verticalCenter: parent.verticalCenter
                        width: 30
                        text: (line.index + 1) + "."
                        color: line.current ? Theme.accent : Theme.textFaint
                        font.pixelSize: Theme.fontSmall
                        font.features: { "tnum": 1 }
                    }
                    CoverArt {
                        visible: mini.listCover > 0
                        anchors.verticalCenter: parent.verticalCenter
                        width: mini.listCover
                        height: mini.listCover
                        radius: 3
                        path: visible ? line.model.path : ""
                    }
                    Repeater {
                        model: mini.shownColumns
                        Item {
                            id: cell
                            required property string modelData
                            readonly property string key: modelData
                            readonly property bool primary: index === 0
                            required property int index
                            width: mini.columnWidths[key] || 0
                            height: line.height
                            // The title with the artist before it while the
                            // artist column is not shown (as the full view's
                            // artist line).
                            Label {
                                visible: cell.key !== "favorite" && !(cell.key === "format" && formatBadge.visible)
                                anchors.verticalCenter: parent.verticalCenter
                                width: parent.width
                                text: !visible ? ""
                                      : cell.key === "title" && mini.shownColumns.indexOf("artist") < 0 && (line.model.artist || "").length > 0
                                        ? line.model.artist + " — " + (line.model.title || "")
                                      : TrackColumns.text(cell.key, line.model)
                                elide: Text.ElideRight
                                color: line.missing ? Theme.textFaint
                                     : line.current ? Theme.accent
                                     : cell.primary ? Theme.text : Theme.textDim
                                font.pixelSize: cell.primary ? Theme.fontSmall + 1 : Theme.fontSmall
                                font.weight: line.current && cell.primary ? Font.DemiBold : Font.Normal
                                font.strikeout: line.missing && cell.primary
                                font.features: { "tnum": 1 }
                            }
                            Badge {
                                id: formatBadge
                                visible: cell.key === "format" && text.length > 0 && !Theme.listMode
                                anchors.verticalCenter: parent.verticalCenter
                                width: Math.min(implicitWidth, cell.width)
                                text: cell.key === "format" ? TrackColumns.text("format", line.model) : ""
                                // as the full view: the accent only for hi-res and DSD
                                tone: line.model.bits > 16 || line.model.sampleRate > 48000 || text.indexOf("DSD") >= 0
                                      ? Theme.accent : Theme.textDim
                            }
                            IconButton {
                                readonly property bool favorite: Library.favoritesRevision >= 0 && Library.isFavorite(line.model.path)
                                visible: cell.key === "favorite"
                                anchors.centerIn: parent
                                tile: false
                                iconName: favorite ? "heart-filled" : "heart-empty"
                                iconSize: 12
                                implicitWidth: 24
                                implicitHeight: Math.min(24, line.height)
                                iconColor: favorite ? Theme.accent : hovered ? Theme.text : lineHover.hovered ? Theme.textDim : Theme.textFaint
                                opacity: favorite || lineHover.hovered ? 1 : 0.6
                                tip: favorite ? qsTr("Remove from favorites") : qsTr("Add to favorites")
                                onClicked: Library.toggleFavorite(line.model.path)
                            }
                        }
                    }
                }
            }
        }
        // Rows fade out as they scroll up under the column heads instead of
        // being cut, no line between.
        Rectangle {
            anchors.top: queueList.top
            anchors.left: queueList.left
            anchors.right: queueList.right
            height: 14
            gradient: Gradient {
                GradientStop { position: 0; color: Theme.pane }
                GradientStop { position: 1; color: Qt.rgba(Theme.pane.r, Theme.pane.g, Theme.pane.b, 0) }
            }
            opacity: queueList.contentY > queueList.originY + 1 ? 1 : 0
            Behavior on opacity { NumberAnimation { duration: 120 } }
        }
        // No music at all: the way to add some.
        Column {
            anchors.centerIn: queueList
            visible: queueList.count === 0 && Library.count === 0 && !Library.scanning
            spacing: 10
            Label {
                anchors.horizontalCenter: parent.horizontalCenter
                text: qsTr("Your library is empty")
                color: Theme.textDim
                font.pixelSize: Theme.fontSmall + 1
            }
            PillButton {
                anchors.horizontalCenter: parent.horizontalCenter
                primary: true
                iconName: "folder-plus"
                text: qsTr("Add music folder")
                onClicked: AppActions.addFolderRequested()
            }
        }
        Label {
            anchors.centerIn: queueList
            width: parent.width - 24
            visible: queueList.count === 0 && (Library.count > 0 || Library.scanning)
            text: mini.showingQueue ? qsTr("The queue is empty.") : qsTr("No tracks in this folder.")
            horizontalAlignment: Text.AlignHCenter
            wrapMode: Text.Wrap
            color: Theme.textDim
            font.pixelSize: Theme.fontSmall + 1
        }
        LMenu {
            id: lineMenu
            property int row: -1
            property string path
            Component.onCompleted: if ("popupType" in lineMenu) lineMenu.popupType = 1
            LMenuItem { text: qsTr("Play"); onTriggered: mini.playRow(lineMenu.row) }
            LMenuItem {
                visible: mini.showingQueue
                height: visible ? implicitHeight : 0
                text: qsTr("Remove from queue")
                onTriggered: DspController.removeFromQueue([lineMenu.row])
            }
            LMenuItem {
                visible: !mini.showingQueue
                height: visible ? implicitHeight : 0
                text: qsTr("Add to queue")
                onTriggered: DspController.enqueue([lineMenu.path])
            }
        }
    }
    // Double-click: a queue row plays from the queue; a folder row plays the
    // folder from that track (it becomes the queue).
    function playRow(row) {
        if (row < 0)
            return
        if (showingQueue)
            DspController.playIndex(row)
        else
            DspController.playPaths(folderModel.paths(), row)
    }

    // The volume control menu, its right edge at the end of the volume
    // controls (the slider, or the "fixed" pill).
    function openVolumeMenu() {
        volumeMenu.openAbove(lockedVolume.visible ? lockedVolume : volume.visible ? volume : volumeButton)
    }
    VolumeModeMenu {
        id: volumeMenu
        ownWindow: true
    }
}
