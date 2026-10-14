pragma Singleton
import QtQuick
import Sonkore

// Application-wide event bus: lets any page ask the main window for
// navigation, dialogs and toasts without reaching into its ids. Also carries
// the state of an in-app track drag (track lists -> playlists / queue).
QtObject {
    id: actions

    signal navigate(string page, string argument)
    signal newPlaylistRequested(var paths)
    signal newPlaylistFromFolderRequested()
    signal addFolderRequested()
    signal addFilesRequested()
    signal importPlaylistRequested()
    signal openSoundTouch()
    signal compactRequested(bool on)   // compact mode on / off (MiniPlayer)
    signal revealPlayingRequested()    // scroll the open track list to the playing track
    property bool revealed: false      // ... set by the list that did
    signal showPlayingRequested()      // the playing track in a list: the open one, else its folder
    signal removeFolderRequested(string path) // asks before taking a watched folder out
    property bool settingsOpen: false  // the settings window is shown
    property bool equalizerOpen: false // the equalizer window is shown
    signal toast(string text)

    // Text of the title bar's search field; filters the library pages.
    property string search

    // --- Track drag & drop -------------------------------------------------
    property bool trackDragActive: false
    property var dragPaths: []
    property string dragOrigin   // e.g. "playlist:<name>", "queue", "library"
    signal trackDragStarted(var paths, string label, point scenePos)
    signal trackDragMoved(point scenePos)
    signal trackDragFinished()

    function beginTrackDrag(paths, label, scenePos, origin) {
        dragPaths = paths
        dragOrigin = origin || "library"
        trackDragActive = true
        trackDragStarted(paths, label, scenePos)
    }
    function moveTrackDrag(scenePos) {
        if (trackDragActive)
            trackDragMoved(scenePos)
    }
    function endTrackDrag() {
        if (!trackDragActive)
            return
        trackDragFinished()
        trackDragActive = false
        dragPaths = []
    }

    function tracksText(n) {
        return n === 1 ? qsTr("1 track") : qsTr("%1 tracks").arg(n)
    }

    // Paths carried by a drop: an in-app track drag or files from outside.
    function dropPaths(drop) {
        if (drop.hasUrls) {
            Library.addUrls(drop.urls)
            return Library.expandAudioPaths(drop.urls)
        }
        return drop.source && drop.source.paths ? drop.source.paths : []
    }
}
