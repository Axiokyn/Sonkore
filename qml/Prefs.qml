pragma Singleton
import QtQuick
import QtCore

// Behaviour preferences that live purely in the UI layer.
QtObject {
    property Settings store: Settings {
        category: "ui"
        property bool trayIcon: false
        property bool closeToTray: false
        property bool showFps: false
        property bool liveSeek: true
        property bool stopButton: false       // a stop button next to play / pause in the player bars
        property bool searchBar: true        // the search in the title bar (Ctrl+F shows it anyway)
        property string sidebar: "auto"     // auto (icons when the window is narrow) | expanded | collapsed (set by dragging its edge)
        property int wheelStep: 180         // px per mouse-wheel notch
        property string visualizer: "bars"     // off | bars | mirror | line | dots
        property int sidebarWidth: 248        // dragged with the sidebar's edge
        property int playlistsHeight: 150     // split between folders and playlists
        property int libraryHeight: -1        // Library entries above the folders (-1 = all of them)
        property int libraryHeightRestore: -1 // ... the height a double-click on its bar unfolds back to
        property int playlistsHeightRestore: 150 // ... same for the playlists
        property bool compactMode: false      // the window is a small player (MiniPlayer)
        property int compactWidth: 480        // window width in compact mode
        property bool compactPlaylist: false  // compact mode shows the play queue under the player
        property int compactHeight: 460       // window height in compact mode with the play queue open
        property int compactLastHeight: 0     // the compact window's last height, to open with (0 = not yet)
        property bool alwaysOnTop: false      // the compact player stays above other windows
        property string compactSource: ""     // compact list: "" the play queue, else a library folder
        property string compactSort: "path"   // compact list: a folder's sort field ("path" = as on disk)
        property bool compactSortDesc: false  // compact list: that sort reversed
        property bool treeSubfolders: true    // sidebar folder tree: subfolders can be unfolded
        property int treeFolderDepth: 99      // ... down to this many levels (at least 1; past the deepest = all)
        property bool compactSubfolders: true // compact list's folder menu: subfolders shown
        property int compactFolderDepth: 2    // ... down to this many levels (at least 1)
        property string compactColumns: ""    // compact list: its columns (JSON keys; "" = take the full view's, once)
        property string lastFolder: ""        // folder last opened in the folder tree (reopened at start-up)
        property string startPage: "last"     // last | tracks | favorites | albums | artists | genres | folders | queue
        property bool trackBrowser: false     // Genre → Artist → Album panes above the track list
        property string settingsTab: "general" // the Settings tab last open
        property string trackSort: "path"      // order of the track lists (TrackTable.rememberSort): "" library | path (as in the folder) | a column key
        property bool trackSortDescending: false
    }
}
