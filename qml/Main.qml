pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Qt.labs.platform as Platform
import Sonkore

ApplicationWindow {
    id: window

    property string page: "tracks"
    property string playlist
    property string folder      // folder picked in the sidebar tree ("" = all)
    property var pendingPlaylistPaths: []
    readonly property var pageNames: ["tracks", "albums", "folders", "playlist", "queue", "artists", "genres", "favorites"]
    // Pages filtered by the title bar's search.
    readonly property bool libraryPage: ["tracks", "albums", "folders", "artists", "genres", "favorites"].indexOf(page) >= 0

    // Opened in compact mode, the window starts at its last compact size, so
    // nothing is resized in view (and GNOME does not maximize a window that
    // starts nearly screen-sized). Assigned sizes replace these bindings.
    width: Prefs.store.compactMode ? Math.max(340, Prefs.store.compactWidth) : 1320
    height: !Prefs.store.compactMode ? 840
            : Prefs.store.compactLastHeight > 0 ? Prefs.store.compactLastHeight
            : Prefs.store.compactPlaylist ? Prefs.store.compactHeight : 150
    // Small enough for a 1920×1080 screen at 200 % interface scale. In
    // compact mode the window is the mini player: its height is fixed, its
    // width free from a small minimum.
    readonly property bool compact: Prefs.store.compactMode
    minimumWidth: compact ? 340 : 760
    // With its play queue open the compact player grows downwards, like
    // Winamp's playlist window, but never past the list's own length;
    // without it its height is fixed.
    readonly property bool compactList: compact && Prefs.store.compactPlaylist
    // The size limits are plain values set only inside the resize below,
    // each on its own side of it: a limit that changed first would make the
    // desktop clamp the window by itself (GNOME shifts it for a few frames)
    // and that snap would be saved as the queue's height.
    property real compactMin: 0
    property real compactMax: 16777215
    minimumHeight: compact ? Math.max(compactMin, miniPlayer.playerHeight) : 480
    maximumHeight: compact ? Math.max(compactMax, miniPlayer.playerHeight) : 16777215
    // Lowers the maximum once the window has already shrunk.
    Timer {
        id: compactMaxDelay
        interval: 150
        property real value
        onTriggered: window.compactMax = value
    }
    // Full-view size and state to come back to after compact mode.
    property int fullWidth: 1320
    property int fullHeight: 840
    property bool fullMaximized: false
    property bool compactReady: false // the window has reached its compact size

    function setCompact(on) {
        if (on === compact)
            return
        compactReady = false
        if (on) {
            fullMaximized = maximized
            if (!maximized) {
                fullWidth = width
                fullHeight = height
            }
            Prefs.store.compactMode = true
            if (maximized) {
                // GNOME would restore a maximized window to where it was
                // before it was maximized, maybe on another monitor (the app
                // cannot place it on Wayland). Shown again instead, it opens
                // where the pointer is (the monitor you are on), already at
                // the compact size.
                hide()
                width = Math.max(340, Prefs.store.compactWidth)
                height = compactList ? Math.max(miniPlayer.listMinHeight, Prefs.store.compactHeight)
                                     : miniPlayer.playerHeight
                showNormal()
            }
            compactResize.restart()
        } else {
            Prefs.store.compactMode = false
            if (fullMaximized) {
                showMaximized() // on the screen it is on
            } else {
                if (maximized)
                    showNormal() // the mini player was maximized
                // Never larger than the screen the player is on (Screen is
                // this window's screen). On Wayland the app cannot place its
                // window: GNOME keeps a resized window whole on one monitor,
                // and one too big for this screen is moved to another.
                width = Math.min(fullWidth, Math.max(minimumWidth, Screen.width - 48))
                height = Math.min(fullHeight, Math.max(minimumHeight, Screen.height - 96))
            }
        }
    }
    // Applied once more shortly after: leaving the maximized state is
    // asynchronous on some desktops, and their reply would undo the size.
    Timer {
        id: compactResize
        interval: 120
        onTriggered: {
            // GNOME maximizes a window that starts nearly screen-sized:
            // restore it, and size it once that is done (it is asynchronous).
            if (window.maximized) {
                window.showNormal()
                compactResize.restart()
                return
            }
            window.width = Math.max(340, Prefs.store.compactWidth)
            window.resizeCompact()
        }
    }
    onWidthChanged: {
        if (compact && compactReady && !maximized)
            Prefs.store.compactWidth = width
        pixelSnap.restart()
    }
    onHeightChanged: {
        pixelSnap.restart()
        if (compactList && compactReady && !maximized)
            Prefs.store.compactHeight = height
        if (compact && compactReady && !maximized)
            Prefs.store.compactLastHeight = height // the size to open with next time
    }
    // Fractional scaling (125 %, 133 %…): the full view is kept at a size that
    // is a whole number of screen pixels. Otherwise its last pixel row and
    // column are only partly drawn: the desktop shows through there, and the
    // outline's bottom and right sides look unlike its top and left. Once a
    // resize settles, the size steps down to the nearest such size (at 133 %
    // a multiple of 3, so a pixel or two). Not when maximized or tiled: the
    // desktop sets that size.
    onDevicePixelRatioChanged: pixelSnap.restart()
    Timer {
        id: pixelSnap
        interval: 500
        onTriggered: {
            if (window.compact || window.visibility !== Window.Windowed || App.tilingWindowManager)
                return
            const d = window.devicePixelRatio
            let step = 1 // the fewest logical pixels that make whole screen pixels
            while (step < 8 && Math.abs(d * step - Math.round(d * step)) > 0.01)
                ++step
            if (step === 1 || step === 8)
                return
            const snap = (size, least) => Math.max(Math.floor(size / step) * step, Math.ceil(least / step) * step)
            const w = snap(window.width, window.minimumWidth)
            const h = snap(window.height, window.minimumHeight)
            if (w !== window.width)
                window.width = w
            if (h !== window.height)
                window.height = h
        }
    }
    // Opening or closing the queue resizes in one step, once no frame is
    // being drawn. With the threaded render loop, a frame of the old size
    // still in flight when the size changes is shown stretched (Wayland with
    // fractional scaling): so the clock is held, the window waits until it
    // has drawn nothing for a few milliseconds, and only then resizes.
    onCompactListChanged: if (compact) scheduleCompactResize()
    // Tracks are decoded for their waveform only while a seek bar shows it.
    Binding {
        target: Waveform
        property: "active"
        value: Theme.sliderStyle === "wave"
    }
    // The list's length (another folder, tracks queued) never resizes the
    // window: the height is the one you gave it. Only the least height
    // follows the player's (the seek bar joins the controls on wide windows).
    Connections {
        target: miniPlayer
        enabled: window.compactList
        function onListMinHeightChanged() { if (window.compactReady) window.compactMin = miniPlayer.listMinHeight }
    }
    // Without the list the window is the player: it follows the player's
    // height (the seek bar joins the controls on wide windows).
    Connections {
        target: miniPlayer
        enabled: window.compact && !window.compactList
        function onPlayerHeightChanged() { if (window.compactReady) window.scheduleCompactResize() }
    }
    // Resizes once no frame is being drawn: with the threaded render loop, a
    // frame of the old size still in flight when the size changes is shown
    // stretched (Wayland with fractional scaling). So the clock is held and
    // the window waits until it has drawn nothing for a few milliseconds.
    function scheduleCompactResize() {
        Clock.held = true
        idleWait.restart()
        idleGiveUp.restart()
    }
    onFrameSwapped: if (idleWait.running) idleWait.restart()
    Timer {
        id: idleWait
        interval: 30 // no frame for this long: the render thread is idle
        onTriggered: window.resizeCompact()
    }
    // Gives up waiting after a while (something keeps animating).
    Timer {
        id: idleGiveUp
        interval: 300
        onTriggered: window.resizeCompact()
    }
    // The compact window's height: the player alone, or with the list at the
    // height you gave it, but no taller than the list needs.
    function resizeCompact() {
        idleWait.stop()
        idleGiveUp.stop()
        Clock.held = false
        if (!compact)
            return
        // With the list: the height you gave it, any height from a few rows
        // up (not capped at the list's length). Without: the player's.
        const least = compactList ? miniPlayer.listMinHeight : miniPlayer.playerHeight
        const most = compactList ? 16777215 : miniPlayer.playerHeight
        const target = compactList ? Math.max(least, Prefs.store.compactHeight) : most
        compactReady = false
        compactMaxDelay.stop()
        if (target >= height) {
            // Growing: lift the maximum, grow, then set the minimum.
            compactMax = most
            height = target
            compactMin = least
        } else {
            // Shrinking: drop the minimum, shrink, and only then lower the
            // maximum (a moment later, once the desktop has the new size).
            compactMin = least
            height = target
            compactMaxDelay.value = most
            compactMaxDelay.restart()
        }
        compactSettle.restart()
    }
    // Sizes are saved again only once the desktop has applied this one: on
    // Wayland its answer comes later, and a late one still at the full
    // view's size would be saved as the compact player's.
    Timer {
        id: compactSettle
        interval: 600
        onTriggered: window.compactReady = true
    }
    Component.onCompleted: {
        if (compact)
            compactResize.restart()
        // The page chosen in Settings (or a layout), else where you were.
        const start = Prefs.store.startPage
        if (start === "last" || start === "folders")
            showStartupFolder()
        else if (pageNames.indexOf(start) >= 0)
            page = start
    }

    // Back where you were: the folder last opened in the tree or, without
    // one, the folder of the track restored from the last session (only
    // folders that still hold music in the library).
    function showStartupFolder() {
        let dir = Prefs.store.lastFolder
        if (dir.length === 0 || Library.folderPaths(dir).length === 0) {
            const path = DspController.currentPath
            const slash = path.lastIndexOf("/")
            dir = slash > 0 ? path.substring(0, slash) : ""
            if (dir.length === 0 || Library.folderPaths(dir).length === 0)
                return
        }
        // Not navigate(): that would also leave the compact player.
        folder = dir
        page = "folders"
    }
    // Folded when asked to, when narrow (automatic), and always in a very
    // small window, where the pages need every pixel.
    readonly property bool sidebarCollapsed: Prefs.store.sidebar === "collapsed"
                                             || (Prefs.store.sidebar !== "expanded" && width < 1080)
                                             || width < 860
    visible: true
    // Integrated title bar: no system decoration; moving and resizing are
    // handed to the compositor (startSystemMove/Resize), like native apps.
    flags: Qt.Window | Qt.FramelessWindowHint
           | (compact && Prefs.store.alwaysOnTop ? Qt.WindowStaysOnTopHint : 0)
    readonly property bool maximized: visibility === Window.Maximized || visibility === Window.FullScreen
    function toggleMaximized() { if (maximized) showNormal(); else showMaximized() }
    title: DspController.currentTrack.title
           ? DspController.currentTrack.title + (DspController.currentTrack.artist ? "  ·  " + DspController.currentTrack.artist : "")
           : "Sonkore"
    // Rounded corners (integrated title bar, not maximized): the corners stay
    // see-through, so the window needs an alpha channel (chosen at start-up).
    readonly property real cornerRadius: App.alphaWindow && !maximized
                                         ? Theme.windowRadius : 0
    // A clear window: the sidebar, pages and bar paint their own backgrounds.
    readonly property bool clearBackground: Theme.translucent || App.alphaWindow
    // As in Gmail: the top bar and the sidebar are the window's ground, the
    // page a panel on it, standing on the player bar.
    readonly property int panelGap: 8
    // Maximized, the clear window is filled in: with fractional scaling its
    // last row of pixels is only partly covered by the bar, and would show
    // the desktop through it. Never fully opaque: an opaque colour drops the
    // window's alpha channel, the desktop keeps treating it as opaque once
    // restored, and the rounded corners turn black.
    readonly property color groundColor: compact ? Theme.bg : Theme.sidebar
    color: !clearBackground ? groundColor
         : maximized && !Theme.translucent ? Qt.rgba(groundColor.r, groundColor.g, groundColor.b, 254 / 255)
         : "transparent"
    font.pixelSize: Theme.fontBody
    font.family: App.fontFamily.length > 0 ? App.fontFamily : Qt.application.font.family // Settings › Appearance

    palette {
        window: Theme.solidBg
        windowText: Theme.text
        base: Theme.surface
        alternateBase: Theme.surfaceHi
        text: Theme.text
        button: Theme.surfaceHi
        buttonText: Theme.text
        highlight: Theme.accent
        highlightedText: Theme.onAccent
        placeholderText: Theme.textFaint
        light: Theme.surfaceHi
        midlight: Theme.surfaceHi
        mid: Theme.border
        dark: Theme.border
        shadow: "#000000"
        toolTipBase: Theme.surfaceHi
        toolTipText: Theme.text
        brightText: Theme.text
        link: Theme.accent
    }


    component PageLoader: Loader {
        property string name   // documents the page; the order matches pageNames
        property bool keepAlive: true
        property bool opened: false
        readonly property bool current: StackLayout.isCurrentItem
        onCurrentChanged: if (current) opened = true
        Component.onCompleted: opened = current
        active: current || (keepAlive && opened)
        onActiveChanged: if (!active) Qt.callLater(App.releaseMemory)
    }

    function navigate(target, argument) {
        // Settings have their own window: the track list stays in view.
        if (target === "settings") {
            // Opened from the mini player: Appearance edits the mini player's.
            if (!settingsWindow.visible)
                Theme.editingCompact = compact
            settingsWindow.open()
            return
        }
        if (target === "eq") {
            equalizerWindow.open()
            return
        }
        setCompact(false) // pages live in the full view
        if (target === "playlist")
            playlist = argument
        if (target === "folders" && argument !== undefined) {
            // The open folder picked again: jump to the track that is playing.
            if (page === "folders" && folder === argument)
                AppActions.revealPlayingRequested()
            folder = argument
            if (argument.length > 0)
                Prefs.store.lastFolder = argument
        }
        page = target
    }

    // The playing track in a list (the player bar's title, the mini
    // player's locate button): in the open list when it holds it, else in its
    // folder; a file outside the library in the play queue.
    function showPlaying() {
        const path = DspController.currentPath
        if (path.length === 0)
            return
        if (compact) {
            miniPlayer.showPlaying()
            return
        }
        AppActions.revealed = false
        AppActions.revealPlayingRequested()
        if (AppActions.revealed)
            return
        if (Library.contains(path))
            navigate("folders", path.substring(0, path.lastIndexOf("/")))
        else
            navigate("queue", "")
        // A page that was already open shows it once it is laid out.
        Qt.callLater(AppActions.revealPlayingRequested)
    }

    function showToast(text) {
        toastLabel.text = text
        toast.opacity = 1
        toastTimer.restart()
    }

    function bringToFront() {
        window.show()
        if (window.visibility === Window.Minimized)
            window.showNormal()
        window.raise()
        window.requestActivate()
    }

    // Closing hides to the tray when the user asked for it.
    onClosing: (close) => {
        if (Prefs.store.trayIcon && Prefs.store.closeToTray && tray.available) {
            close.accepted = false
            window.hide()
        }
    }

    Connections {
        target: AppActions
        function onNavigate(target, argument) { window.navigate(target, argument) }
        function onCompactRequested(on) { window.setCompact(on) }
        function onShowPlayingRequested() { window.showPlaying() }
        function onNewPlaylistRequested(paths) {
            window.pendingPlaylistPaths = paths
            newPlaylistDialog.ask("")
        }
        function onNewPlaylistFromFolderRequested() {
            window.pendingPlaylistPaths = []
            dialogs.open("openPlaylistFolder")
        }
        function onAddFolderRequested() { dialogs.open("openFolder") }
        function onAddFilesRequested() { dialogs.open("openFiles") }
        function onImportPlaylistRequested() { dialogs.open("openImportPlaylist") }
        function onRemoveFolderRequested(path) {
            removeFolderDialog.path = path
            removeFolderDialog.open()
        }
        function onOpenSoundTouch() {
            soundTouchLoader.active = true
            const w = soundTouchLoader.item as ApplicationWindow
            w.show()
            w.raise()
            w.requestActivate()
        }
        function onToast(text) { window.showToast(text) }
        function onTrackDragStarted(paths, label, pos) {
            dragGhost.paths = paths
            dragGhost.label = label
            dragGhost.x = pos.x
            dragGhost.y = pos.y
            dragGhost.visible = true
        }
        function onTrackDragMoved(pos) {
            dragGhost.x = pos.x
            dragGhost.y = pos.y
        }
        function onTrackDragFinished() {
            dragGhost.Drag.drop()
            dragGhost.visible = false
        }
    }
    Connections {
        target: DspController
        function onErrorOccurred() { if (DspController.lastError.length > 0) window.showToast(DspController.lastError) }
        function onOutputTakenByOther() { window.showToast(qsTr("Another app asked for the DAC — playback paused")) }
        function onOutputDevicesHotplugged(added, removed) {
            if (added.length > 0)
                window.showToast(qsTr("Sound device connected: %1").arg(added.join(", ")))
            else if (removed.length > 0)
                window.showToast(qsTr("Sound device disconnected: %1").arg(removed.join(", ")))
        }
    }
    Connections {
        target: App
        function onRaiseRequested() { window.bringToFront() }
    }

    Shortcut { sequence: "Space"; onActivated: DspController.togglePlayPause() }
    Shortcut { sequences: ["Ctrl+Right", "Media Next"]; onActivated: DspController.next() }
    Shortcut { sequences: ["Ctrl+Left", "Media Previous"]; onActivated: DspController.previous() }
    Shortcut { sequence: "Media Play"; onActivated: DspController.togglePlayPause() }
    Shortcut {
        sequence: "Ctrl+F"
        onActivated: {
            if (window.compact) {
                miniPlayer.openSearch()
                return
            }
            if (!window.libraryPage)
                window.page = "tracks"
            titleSearch.summoned = true
            titleSearch.forceActiveFocus()
            titleSearch.selectAll()
        }
    }
    Shortcut { sequence: "Ctrl+E"; onActivated: window.navigate("eq", "") }
    Shortcut { sequence: "Ctrl+,"; onActivated: window.navigate("settings", "") }
    Shortcut { sequence: "Ctrl+Up"; onActivated: DspController.stepVolume(1) }
    Shortcut { sequence: "Ctrl+Down"; onActivated: DspController.stepVolume(-1) }
    Shortcut { sequence: "Ctrl+Q"; onActivated: Qt.quit() }
    Shortcut { sequence: "Ctrl+M"; onActivated: window.setCompact(!window.compact) }

    // Taking a watched folder out empties that part of the library at once
    // (and its "date added" history): always confirm first. A folder inside
    // a watched one is skipped by the scan instead (Settings › Library).
    ConfirmDialog {
        id: removeFolderDialog
        property string path
        readonly property bool watched: Library.folders.indexOf(path) >= 0
        title: qsTr("Remove “%1” from the library?").arg(path.split("/").pop())
        text: watched
              ? qsTr("Its %1 tracks disappear from Sonkore; the files stay on disk. You can add the folder again with +.")
                .arg(Library.folderPaths(path).length)
              : qsTr("Its %1 tracks disappear from Sonkore; the files stay on disk. You can bring it back in Settings › Library › Skipped folders.")
                .arg(Library.folderPaths(path).length)
        acceptText: qsTr("Remove folder")
        onAccepted: {
            if (watched)
                Library.removeFolder(path)
            else
                Library.excludeFolder(path)
            // Nothing from that folder is resumed or reopened on the next start.
            DspController.forgetFolder(path)
            const last = Prefs.store.lastFolder
            if (last === path || last.startsWith(path + "/"))
                Prefs.store.lastFolder = ""
            window.showToast(qsTr("“%1” removed from the library (the files stay on disk)").arg(path.split("/").pop()))
        }
    }
    // The file and folder dialogs (MainDialogs.qml), loaded with their module
    // the first time one is opened.
    Loader {
        id: dialogs
        active: false
        source: "MainDialogs.qml"
        function open(which) {
            active = true
            item[which]()
        }
    }
    Connections {
        target: dialogs.item
        function onFolderChosen(folder) {
            Library.addFolder(folder)
            window.showToast(qsTr("Scanning folder…"))
        }
        function onFilesChosen(files) {
            Library.addUrls(files)
            DspController.playPaths(Library.audioPaths(files), 0)
        }
        function onPlaylistFileChosen(file) {
            const name = Playlists.importM3u(file)
            if (name.length > 0)
                window.navigate("playlist", name)
        }
        function onPlaylistFolderChosen(folder) {
            const paths = Library.expandAudioPaths([folder])
            if (paths.length === 0) {
                window.showToast(qsTr("No audio files in that folder"))
                return
            }
            window.pendingPlaylistPaths = window.pendingPlaylistPaths.concat(paths)
            const url = folder.toString()
            newPlaylistDialog.suggest(decodeURIComponent(url.substring(url.lastIndexOf("/") + 1)))
            if (!newPlaylistDialog.opened)
                newPlaylistDialog.open()
        }
    }
    NameDialog {
        id: newPlaylistDialog
        title: qsTr("New playlist")
        placeholder: qsTr("Playlist name")
        acceptText: qsTr("Create")
        note: window.pendingPlaylistPaths.length > 0
              ? qsTr("With %1.").arg(AppActions.tracksText(window.pendingPlaylistPaths.length)) : ""
        onClosed: if (!(dialogs.item && dialogs.item.playlistFolderOpen) && !created) window.pendingPlaylistPaths = []
        property bool created: false
        onAboutToShow: created = false
        onAccepted: (name) => {
            newPlaylistDialog.created = true
            const created = Playlists.create(name)
            if (window.pendingPlaylistPaths.length > 0) {
                Playlists.addTracks(created, window.pendingPlaylistPaths)
                window.showToast(qsTr("Added %1 to “%2”").arg(AppActions.tracksText(window.pendingPlaylistPaths.length)).arg(created))
            }
            window.pendingPlaylistPaths = []
            window.navigate("playlist", created)
        }
    }


    // Created on first use and released when closed: a second window has its
    // own render thread and graphics resources.
    Loader {
        id: soundTouchLoader
        active: false
        sourceComponent: Component {
            EditSoundtouch { visible: false; transientParent: window }
        }
    }
    Connections {
        target: soundTouchLoader.item as ApplicationWindow
        function onVisibleChanged() {
            const w = soundTouchLoader.item as ApplicationWindow
            if (w && !w.visible)
                Qt.callLater(() => soundTouchLoader.active = false)
        }
    }

    // --- Tray icon: playback controls from the panel ---------------------------
    Platform.SystemTrayIcon {
        id: tray
        visible: Prefs.store.trayIcon && available
        // Only once shown: reading a PNG by URL makes Qt load every image
        // plugin to detect the format, and they would stay for the session.
        icon.source: visible ? "qrc:/icons/png/64.png" : ""
        tooltip: DspController.currentTrack.title
                 ? DspController.currentTrack.title + " — " + (DspController.currentTrack.artist || "")
                 : "Sonkore"
        onActivated: (reason) => {
            if (reason === Platform.SystemTrayIcon.Trigger || reason === Platform.SystemTrayIcon.DoubleClick) {
                if (window.visible && window.active)
                    window.hide()
                else
                    window.bringToFront()
            } else if (reason === Platform.SystemTrayIcon.MiddleClick) {
                DspController.togglePlayPause()
            }
        }
        menu: Platform.Menu {
            Platform.MenuItem {
                text: DspController.currentTrack.title
                      ? DspController.currentTrack.title + (DspController.currentTrack.artist ? " — " + DspController.currentTrack.artist : "")
                      : qsTr("Nothing playing")
                enabled: false
            }
            Platform.MenuSeparator {}
            Platform.MenuItem {
                text: DspController.playing ? qsTr("Pause") : qsTr("Play")
                onTriggered: DspController.togglePlayPause()
            }
            Platform.MenuItem { text: qsTr("Next"); onTriggered: DspController.next() }
            Platform.MenuItem { text: qsTr("Previous"); onTriggered: DspController.previous() }
            Platform.MenuSeparator {}
            Platform.MenuItem {
                text: qsTr("Shuffle")
                checkable: true
                checked: DspController.shuffle
                onTriggered: DspController.shuffle = !DspController.shuffle
            }
            Platform.MenuItem {
                text: qsTr("Equalizer")
                checkable: true
                checked: Equalizer.enabled
                onTriggered: Equalizer.enabled = !Equalizer.enabled
            }
            Platform.MenuSeparator {}
            Platform.MenuItem { text: qsTr("Show Sonkore"); onTriggered: window.bringToFront() }
            Platform.MenuItem { text: qsTr("Quit"); onTriggered: Qt.quit() }
        }
    }

    // --- External drops anywhere (below the specific drop targets) -------------
    DropArea {
        id: globalDrop
        anchors.fill: parent
        keys: ["text/uri-list"]
        onDropped: (event) => {
            if (!event.hasUrls)
                return
            Library.addUrls(event.urls)
            const files = Library.audioPaths(event.urls)
            if (files.length > 0)
                DspController.playPaths(files, 0)
            else
                window.showToast(qsTr("Folder added — scanning…"))
        }
    }

    // --- Background style (Settings › Appearance) --------------------------------
    // Soft colour fields from the skin and its accent, under the panels (which
    // let them through, Theme.panelAlpha). Drawn by one shader in float with
    // dithering (no 8-bit banding), rounded with the window.
    // The software renderer runs no shaders: there the same picture is drawn
    // on the CPU (BackdropItem), from these same inputs.
    readonly property bool softwareRendering: GraphicsInfo.api === GraphicsInfo.Software
    ShaderEffect {
        id: backdrop
        anchors.fill: parent
        z: -1
        visible: Theme.decorated && !window.softwareRendering
        fragmentShader: "qrc:/shaders/backdrop.frag.qsb"
        blending: true
        // Cached: drawn into a texture only when an input changes (size, skin,
        // accent, style). Each frame then copies one texture instead of
        // running three glows and the dither over every pixel of the window.
        layer.enabled: visible

        readonly property string style: Theme.backdrop
        readonly property bool aurora: style === "aurora"
        readonly property bool glowStyle: style === "glow"
        readonly property real span: Math.max(width, height)
        readonly property real k: Theme.dark ? 1 : 0.7 // softer on light skins
        // the accent and two neighbouring hues
        readonly property color a1: Theme.accent
        readonly property color a2: Qt.hsla((Theme.accent.hslHue + 0.11) % 1, Math.max(0.4, Theme.accent.hslSaturation), Math.max(0.35, Theme.accent.hslLightness), 1)
        readonly property color a3: Qt.hsla((Theme.accent.hslHue + 0.86) % 1, Math.max(0.4, Theme.accent.hslSaturation), Math.max(0.35, Theme.accent.hslLightness), 1)
        function tone(c, strength) { return Qt.vector4d(c.r, c.g, c.b, strength * k) }

        // shader uniforms (qml/shaders/backdrop.frag)
        property vector2d area: Qt.vector2d(width, height)
        property real radius: window.cornerRadius
        property real linearMode: style === "gradient" ? 1 : 0
        readonly property color skinBg: Theme.skin.bg // skin entries are strings; a color has .r/.g/.b
        property vector4d bg: Qt.vector4d(skinBg.r, skinBg.g, skinBg.b, 1)
        property vector4d tone1: aurora ? tone(a1, 0.34) : glowStyle ? tone(a1, 0.32) : style === "gradient" ? tone(a1, 0.3) : tone(a1, 0)
        property vector4d tone2: aurora ? tone(a2, 0.28) : glowStyle ? tone(a1, 0.1) : style === "gradient" ? tone(a2, 0.1) : tone(a2, 0)
        property vector4d tone3: aurora ? tone(a3, 0.2) : style === "gradient" ? tone(a3, 0.22) : tone(a3, 0)
        property vector4d blob1: glowStyle
            ? Qt.vector4d(width * 0.5, height * 1.05, span * 0.6, 0)
            : Qt.vector4d(width * 0.12, height * 0.2, span * 0.7, 0)
        property vector4d blob2: glowStyle
            ? Qt.vector4d(0, 0, span * 0.4, 0)
            : Qt.vector4d(width * 0.804, height * 0.858, span * 0.65, 0)
        property vector4d blob3: Qt.vector4d(width * 0.559, height * 0.091, span * 0.45, 0)
    }
    Loader {
        anchors.fill: parent
        z: -1
        active: Theme.decorated && window.softwareRendering
        sourceComponent: BackdropItem {
            radius: backdrop.radius
            linearMode: backdrop.linearMode
            bg: backdrop.bg
            tone1: backdrop.tone1
            tone2: backdrop.tone2
            tone3: backdrop.tone3
            blob1: backdrop.blob1
            blob2: backdrop.blob2
            blob3: backdrop.blob3
        }
    }

    // Clear window: the ground is painted here (the window itself is clear
    // for the rounded corners).
    Rectangle {
        visible: window.clearBackground && !window.compact
        anchors.fill: parent
        radius: window.cornerRadius
        color: Theme.sidebar
    }

    // the page's panel
    Rectangle {
        visible: !window.compact
        x: pageColumn.x
        y: topBar.height + pageColumn.y
        width: pageColumn.width
        height: pageColumn.height
        // out to the window's right edge and down onto the player bar: only
        // the corner by the sidebar is rounded
        topLeftRadius: 14
        color: Theme.pane
    }

    // --- Top bar, the window's whole width, as in Spotify: the logo on the
    // left, the search in the middle of the window, the window buttons in the
    // corner. It stays put when the sidebar is resized or folded; its empty
    // space moves / maximizes the window.
    Rectangle {
        id: topBar
        visible: !window.compact
        width: parent.width
        height: Theme.compact ? 42 : Theme.spacious ? 54 : 48
        topLeftRadius: window.cornerRadius
        topRightRadius: window.cornerRadius
        color: Theme.sidebar
        DragHandler {
            target: null
            onActiveChanged: if (active) window.startSystemMove()
        }
        TapHandler { onDoubleTapped: window.toggleMaximized() }

        // The logo 16 px in, level with the sidebar's section labels and
        // icons; folded, the logo alone over the icon column, and a click on
        // it unfolds the sidebar.
        Brand {
            id: topBrand
            x: window.sidebarCollapsed ? Math.round((sidebar.width - width) / 2) : 13
            logoSize: 34
            textSize: 21
            anchors.verticalCenter: parent.verticalCenter
            logoOnly: window.sidebarCollapsed
            clickable: window.sidebarCollapsed
            logoTip: qsTr("Expand sidebar")
            onLogoClicked: Prefs.store.sidebar = "expanded"
        }

        Row {
            id: pageHeading
            visible: pageHeader.pageTitle.length > 0
            // Over the pages, level with the list (28 px in), or centred
            // for `titleCentered` pages.
            x: pageHeader.headingOnly ? Math.round((parent.width - width) / 2) : sidebar.width + 1 + 28
            anchors.verticalCenter: parent.verticalCenter
            spacing: 10
            Icon {
                visible: pageHeader.pageIcon.length > 0
                anchors.verticalCenter: parent.verticalCenter
                glyph: pageHeader.pageIcon
                size: 22
                color: Theme.accent
            }
            Column {
                anchors.verticalCenter: parent.verticalCenter
                // Shares the bar with the search: at most 30 % of the window.
                width: Math.min(Math.max(pageTitleLabel.implicitWidth, pageSubtitleLabel.implicitWidth), 300,
                                Math.max(100, topBar.width * 0.3))
                spacing: 0
                Label {
                    id: pageTitleLabel
                    width: parent.width
                    text: pageHeader.pageTitle
                    elide: Text.ElideRight
                    color: Theme.text
                    font.pixelSize: Theme.fontTitle + 2
                    font.weight: Font.Bold
                }
                Label {
                    id: pageSubtitleLabel
                    width: parent.width
                    visible: text.length > 0
                    text: pageHeader.pageSubtitle
                    elide: Text.ElideMiddle
                    color: Theme.textDim
                    font.pixelSize: Theme.fontSmall
                }
            }
        }

        // Search, centred over the list below, never over the logo, the page
        // heading or the window buttons.
        readonly property real searchMinX: (pageHeading.visible ? pageHeading.x + pageHeading.width : topBrand.x + topBrand.width) + 24
        readonly property real searchMaxX: (windowButtons.visible ? windowButtons.x : width) - 24
        SearchField {
            id: titleSearch
            // Turned off in Settings: Ctrl+F still brings it up until it is
            // left empty.
            property bool summoned: false
            visible: !pageHeader.headingOnly && width >= 120
                     && (Prefs.store.searchBar || summoned || text.length > 0)
            onActiveFocusChanged: if (!activeFocus && text.length === 0) summoned = false
            anchors.verticalCenter: parent.verticalCenter
            // Shrinks with the window; hidden when there is no room left.
            width: Math.min(440, topBar.searchMaxX - topBar.searchMinX)
            // centred over the list (the pages' column), not the whole window
            readonly property real listCentre: sidebar.width + 1 + (parent.width - sidebar.width - 1) / 2
            x: Math.max(topBar.searchMinX, Math.min(Math.round(listCentre - width / 2), topBar.searchMaxX - width))
            implicitHeight: Theme.compact ? 28 : 32
            placeholderText: window.page === "albums" ? qsTr("Search albums or artists…")
                           : window.page === "folders" ? qsTr("Search in folder…")
                           : window.page === "artists" ? qsTr("Search artists…")
                           : window.page === "genres" ? qsTr("Search genres…")
                           : qsTr("Search title, artist, album, genre…")
            onTextChanged: AppActions.search = text
            onTextEdited: if (!window.libraryPage) window.navigate("tracks", "")
        }

        // Window buttons flush in the top-right corner.
        WindowButtons {
            id: windowButtons
            anchors.top: parent.top
            anchors.right: parent.right
            // Windows-style buttons sit flush in the corner; the round styles
            // a little inside it.
            anchors.topMargin: flush ? 0 : 4
            anchors.rightMargin: flush ? 0 : 8
            cornerRadius: window.cornerRadius
            maximized: window.maximized
            onMinimizeClicked: window.showMinimized()
            onMaximizeClicked: window.toggleMaximized()
            onCloseClicked: window.close()
        }
    }

    // --- Layout ------------------------------------------------------------------
    RowLayout {
        visible: !window.compact
        anchors.fill: parent
        anchors.topMargin: topBar.height
        anchors.bottomMargin: nowPlaying.height
        spacing: 0

        Sidebar {
            id: sidebar
            // Room for the pages: the sidebar may take up to 45 % of the window.
            readonly property int maxWidth: Math.max(200, Math.min(520, window.width * 0.45))
            Layout.fillHeight: true
            Layout.preferredWidth: collapsed ? 68 : Math.max(200, Math.min(maxWidth, Prefs.store.sidebarWidth))
            // Folding / unfolding slides; while the edge is dragged it follows
            // the pointer almost at once.
            Behavior on Layout.preferredWidth {
                NumberAnimation { duration: gripDrag.active ? 60 : 200; easing.type: Easing.OutCubic }
            }
            clip: true // the labels while it slides
            collapsed: window.sidebarCollapsed
            page: window.page
            playlist: window.playlist
            folder: window.folder
            onNavigate: (target, argument) => window.navigate(target, argument)
        }
        Rectangle {
            Layout.fillHeight: true
            implicitWidth: 1
            z: 2 // the grip overlaps the page edge
            color: sidebarGrip.active ? Theme.accent : "transparent"
            // Drag the edge to resize the sidebar: narrower than `foldAt` it
            // folds to icons, wider again it unfolds. Double-click resets it.
            Item {
                id: sidebarGrip
                readonly property bool active: gripHover.hovered || gripDrag.active
                readonly property int foldAt: 140
                property int startWidth: 0
                anchors.horizontalCenter: parent.horizontalCenter
                width: 9
                height: parent.height
                z: 10
                HoverHandler { id: gripHover; cursorShape: Qt.SplitHCursor }
                DragHandler {
                    id: gripDrag
                    target: null
                    xAxis.enabled: true
                    yAxis.enabled: false
                    cursorShape: Qt.SplitHCursor
                    onActiveChanged: if (active) sidebarGrip.startWidth = sidebar.width
                    onTranslationChanged: {
                        if (!active)
                            return
                        const w = sidebarGrip.startWidth + translation.x
                        if (w < sidebarGrip.foldAt) {
                            Prefs.store.sidebar = "collapsed"
                        } else {
                            Prefs.store.sidebar = "expanded"
                            Prefs.store.sidebarWidth = Math.round(Math.max(200, Math.min(sidebar.maxWidth, w)))
                        }
                    }
                }
                TapHandler {
                    onDoubleTapped: {
                        Prefs.store.sidebar = "expanded"
                        Prefs.store.sidebarWidth = 248
                    }
                }
            }
        }

        ColumnLayout {
            id: pageColumn
            objectName: "pagePanel"
            Layout.fillWidth: true
            Layout.fillHeight: true
            Layout.leftMargin: window.panelGap - 1
            clip: true
            spacing: 0

        // --- The page's heading (shown in the top bar): pages may set
        // `pageTitle`, `pageSubtitle` and `pageIcon`.
        Item {
            id: pageHeader
            Layout.fillWidth: true
            readonly property Item pageItem: pages.children[pages.currentIndex]
                                             ? (pages.children[pages.currentIndex] as Loader).item : null
            readonly property string pageTitle: pageItem && pageItem.pageTitle !== undefined ? pageItem.pageTitle : ""
            readonly property string pageSubtitle: pageItem && pageItem.pageSubtitle !== undefined ? pageItem.pageSubtitle : ""
            readonly property string pageIcon: pageItem && pageItem.pageIcon !== undefined ? pageItem.pageIcon : ""
            // `titleCentered` pages: only their heading, in the middle.
            readonly property bool headingOnly: pageItem !== null && pageItem.titleCentered === true
            implicitHeight: 6 // the heading itself is in the top bar
        }

        StackLayout {
            id: pages
            Layout.fillWidth: true
            Layout.fillHeight: true
            currentIndex: window.pageNames.indexOf(window.page)

            // Pages are created when first opened. The library views stay alive
            // (search, scroll and selection survive); the others are released
            // when you leave them.
            PageLoader { name: "tracks"; sourceComponent: Component { TracksPage {} } }
            PageLoader { name: "albums"; sourceComponent: Component { AlbumsPage {} } }
            PageLoader { name: "folders"; sourceComponent: Component { FoldersPage { folder: window.folder; showSubfolders: window.sidebarCollapsed } } }
            PageLoader { name: "playlist"; sourceComponent: Component { PlaylistPage { name: window.playlist } } }
            PageLoader { name: "queue"; keepAlive: false; sourceComponent: Component { QueuePage {} } }
            PageLoader { name: "artists"; sourceComponent: Component { BrowsePage { field: "artist" } } }
            PageLoader { name: "genres"; sourceComponent: Component { BrowsePage { field: "genre" } } }
            PageLoader { name: "favorites"; sourceComponent: Component { TracksPage { favoritesOnly: true } } }
        }
        }
    }

    NowPlayingBar {
        id: nowPlaying
        visible: !window.compact
        bottomLeftRadius: window.cornerRadius
        bottomRightRadius: window.cornerRadius
        page: window.page
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: parent.bottom
    }

    // --- Settings: a window of their own, so the lists stay in view -----------------
    AppWindow {
        id: settingsWindow
        transientParent: window
        title: qsTr("Settings") + "  ·  Sonkore"
        iconName: "settings"
        width: 920
        height: Math.min(820, Math.max(480, window.height - 80))
        minimumWidth: 560
        minimumHeight: 420
        onVisibleChanged: AppActions.settingsOpen = visible

        function open() {
            if (!visible) {
                // Centred over the main window the first time it is shown.
                x = window.x + Math.round((window.width - width) / 2)
                y = window.y + Math.round((window.height - height) / 2)
            }
            show()
            raise()
            requestActivate()
        }

        Shortcut { sequence: "Escape"; onActivated: settingsWindow.close() }
        Shortcut { sequence: "Ctrl+,"; onActivated: settingsWindow.close() }
        Shortcut { sequence: "Ctrl+Q"; onActivated: Qt.quit() }

        // Created while shown, released when closed.
        Loader {
            anchors.fill: parent
            active: settingsWindow.visible
            sourceComponent: Component { SettingsPage {} }
            onActiveChanged: if (!active) Qt.callLater(App.releaseMemory)
        }
    }

    // --- Equalizer: a window of its own too (Ctrl+E, the bar's EQ button) ----------
    AppWindow {
        id: equalizerWindow
        transientParent: window
        title: qsTr("Equalizer")
        iconName: "eq"
        // Never taller than its content (it grows with the AutoEQ results).
        readonly property real fullHeight: eqLoader.item ? Math.ceil(eqLoader.item.fullHeight) + (header ? header.height : 0) : 900
        readonly property real fitHeight: Math.min(fullHeight, Math.max(480, Screen.desktopAvailableHeight - 80))
        width: 1000
        height: 900
        minimumWidth: 720
        minimumHeight: Math.min(420, fullHeight)
        maximumHeight: fullHeight
        onVisibleChanged: AppActions.equalizerOpen = visible
        // Sized once the page is laid out.
        Connections {
            target: eqLoader
            function onLoaded() { Qt.callLater(() => equalizerWindow.height = equalizerWindow.fitHeight) }
        }

        function open() {
            if (!visible) {
                x = window.x + Math.round((window.width - width) / 2)
                y = window.y + Math.round((window.height - height) / 2)
            }
            show()
            raise()
            requestActivate()
        }

        Shortcut { sequence: "Escape"; onActivated: equalizerWindow.close() }
        Shortcut { sequence: "Ctrl+E"; onActivated: equalizerWindow.close() }
        Shortcut { sequence: "Ctrl+Q"; onActivated: Qt.quit() }

        // Created while shown, released when closed.
        Loader {
            id: eqLoader
            anchors.fill: parent
            active: equalizerWindow.visible
            sourceComponent: Component { EqualizerPage {} }
            onActiveChanged: if (!active) Qt.callLater(App.releaseMemory)
        }
    }

    // Compact mode: the whole window is the mini player.
    MiniPlayer {
        id: miniPlayer
        visible: window.compact
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        height: Math.max(implicitHeight, parent.height)
        cornerRadius: window.cornerRadius
        maximized: window.maximized
        onExpandRequested: window.setCompact(false)
        onMaximizeRequested: window.toggleMaximized()
        onSettingsRequested: window.navigate("settings", "")
        onMinimizeRequested: window.showMinimized()
        onCloseRequested: window.close()
    }

    RoundRect {
        anchors.fill: parent
        anchors.margins: 10
        visible: globalDrop.containsDrag
        radius: Theme.radius + 4
        color: Theme.accentSoft
        borderWidth: 2
        borderColor: Theme.accent
        Label {
            anchors.centerIn: parent
            text: qsTr("Drop music to add it to your library and play it")
            color: Theme.text
            font.pixelSize: 20
            font.weight: Font.DemiBold
        }
    }

    // --- Drag ghost for in-app track drags -------------------------------------
    Item {
        id: dragGhost
        property var paths: []
        property string label
        visible: false
        z: 1000
        width: 1
        height: 1
        Drag.active: visible
        Drag.keys: ["sonkore/tracks"]
        Drag.source: dragGhost
        Drag.hotSpot.x: 0
        Drag.hotSpot.y: 0

        Rectangle {
            x: 14
            y: 10
            width: ghostRow.implicitWidth + 22
            height: 30
            radius: Theme.squareShapes ? 3 : 15
            color: Theme.accent
            Row {
                id: ghostRow
                anchors.centerIn: parent
                spacing: 6
                Icon { glyph: "music"; size: 14; color: Theme.onAccent }
                Label {
                    text: dragGhost.label
                    color: Theme.onAccent
                    font.weight: Font.DemiBold
                    width: Math.min(implicitWidth, 260)
                    elide: Text.ElideRight
                }
            }
        }
    }

    // --- Graphics settings were rolled back after a failed start -------------------
    RoundRect {
        id: recoveryBanner
        visible: App.recoveryMessage.length > 0
        anchors.top: parent.top
        anchors.topMargin: 14
        anchors.horizontalCenter: parent.horizontalCenter
        width: Math.min(720, window.width - 64)
        height: recoveryRow.implicitHeight + 24
        z: 998
        radius: Theme.radius
        color: Theme.surfaceHi
        borderWidth: 1
        borderColor: Theme.warn
        RowLayout {
            id: recoveryRow
            anchors.fill: parent
            anchors.margins: 12
            anchors.leftMargin: 16
            spacing: 12
            Icon { glyph: "info"; size: 18; color: Theme.warn }
            Label {
                Layout.fillWidth: true
                text: App.recoveryMessage
                wrapMode: Text.Wrap
                color: Theme.text
            }
            PillButton {
                text: qsTr("Settings")
                onClicked: { recoveryBanner.visible = false; Prefs.store.settingsTab = "general"; window.navigate("settings", "") }
            }
            IconButton {
                iconName: "close"
                iconSize: 14
                tip: qsTr("Dismiss")
                onClicked: recoveryBanner.visible = false
            }
        }
    }

    // --- Toast -------------------------------------------------------------------
    Rectangle {
        id: toast
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.bottom: nowPlaying.top
        anchors.bottomMargin: 16
        width: Math.min(toastLabel.implicitWidth + 32, window.width - 80)
        height: toastLabel.implicitHeight + 18
        radius: Theme.squareShapes ? 3 : height / 2
        color: Theme.dark ? "#f2f2f4" : "#1d1d1f"
        opacity: 0
        visible: opacity > 0
        Behavior on opacity { NumberAnimation { duration: 180 } }
        Label {
            id: toastLabel
            anchors.centerIn: parent
            width: Math.min(implicitWidth, window.width - 112)
            elide: Text.ElideRight
            color: Theme.dark ? "#111111" : "#ffffff"
            font.weight: Font.Medium
        }
        Timer {
            id: toastTimer
            interval: 3200
            onTriggered: toast.opacity = 0
        }
    }

    // --- Resize edges for the integrated title bar ------------------------------
    component ResizeEdge: MouseArea {
        property int edges
        z: 2000
        enabled: Window.window !== null
                 && Window.window.visibility !== Window.Maximized && Window.window.visibility !== Window.FullScreen
        visible: enabled
        hoverEnabled: true
        acceptedButtons: Qt.LeftButton
        cursorShape: edges === (Qt.LeftEdge | Qt.TopEdge) || edges === (Qt.RightEdge | Qt.BottomEdge) ? Qt.SizeFDiagCursor
                   : edges === (Qt.RightEdge | Qt.TopEdge) || edges === (Qt.LeftEdge | Qt.BottomEdge) ? Qt.SizeBDiagCursor
                   : (edges & (Qt.LeftEdge | Qt.RightEdge)) ? Qt.SizeHorCursor : Qt.SizeVerCursor
        onPressed: Window.window.startSystemResize(edges)
    }
    readonly property int grip: 5
    ResizeEdge { edges: Qt.LeftEdge; x: 0; y: window.grip; width: window.grip; height: parent.height - 2 * window.grip }
    ResizeEdge { edges: Qt.RightEdge; x: parent.width - window.grip; y: window.grip; width: window.grip; height: parent.height - 2 * window.grip }
    // Stops at the window buttons, which sit flush in the top-right corner.
    ResizeEdge { edges: Qt.TopEdge; x: window.grip; y: 0; width: parent.width - window.grip - (windowButtons.visible ? windowButtons.width : window.grip); height: window.grip }
    ResizeEdge { edges: Qt.BottomEdge; x: window.grip; y: parent.height - window.grip; width: parent.width - 2 * window.grip; height: window.grip }
    ResizeEdge { edges: Qt.LeftEdge | Qt.TopEdge; x: 0; y: 0; width: 2 * window.grip; height: 2 * window.grip }
    ResizeEdge { edges: Qt.RightEdge | Qt.TopEdge; x: parent.width - 2 * window.grip; y: 0; width: 2 * window.grip; height: 2 * window.grip }
    ResizeEdge { edges: Qt.LeftEdge | Qt.BottomEdge; x: 0; y: parent.height - 2 * window.grip; width: 2 * window.grip; height: 2 * window.grip }
    ResizeEdge { edges: Qt.RightEdge | Qt.BottomEdge; x: parent.width - 2 * window.grip; y: parent.height - 2 * window.grip; width: 2 * window.grip; height: 2 * window.grip }

    // Window outline (Settings › Appearance) so the borderless window stays
    // defined on any wallpaper.
    WindowOutline {
        anchors.fill: parent
        z: 1999
        visible: !window.maximized
        radius: window.cornerRadius
        windowEdge: true
    }
    // A square window (maximized, or no rounded corners): its last rows and
    // columns of pixels, which the software renderer leaves unpainted at a
    // fractional scale (the desktop showed through a line along the bottom).
    WindowEdges {
        anchors.fill: parent
        z: 2000
        visible: window.cornerRadius === 0
    }

    // --- FPS counter (Settings › Performance) ----------------------------------
    Rectangle {
        id: fpsBadge
        anchors.top: parent.top
        anchors.right: parent.right
        anchors.margins: 8
        anchors.topMargin: topBar.height + 4 // below the top bar
        visible: Prefs.store.showFps
        width: fpsLabel.implicitWidth + 16
        height: fpsLabel.implicitHeight + 8
        radius: 6
        color: Qt.rgba(0, 0, 0, 0.6)
        z: 999
        // The frames the window really shows, counted as they are presented
        // (App.countFrames), read every half second. It asks for no frame
        // itself: an idle window reads 0, and the label's own change adds at
        // most two frames a second.
        property real fps: 0
        property double last: 0
        readonly property bool counting: Prefs.store.showFps
        function restart() {
            App.countFrames(window, counting)
            last = Date.now()
            fps = 0
        }
        onCountingChanged: restart()
        Component.onCompleted: restart()
        Timer {
            interval: 500
            repeat: true
            running: fpsBadge.counting
            onTriggered: {
                const now = Date.now()
                fpsBadge.fps = App.takeFrameCount() * 1000 / Math.max(1, now - fpsBadge.last)
                fpsBadge.last = now
            }
        }
        Label {
            id: fpsLabel
            anchors.centerIn: parent
            text: Math.round(fpsBadge.fps) + " fps · " + Math.round(App.refreshRate(window)) + " Hz"
            color: "#ffffff"
            font.pixelSize: 11
            font.features: { "tnum": 1 }
        }
    }
}
