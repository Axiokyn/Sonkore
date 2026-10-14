import QtQuick
import QtQuick.Controls
import Sonkore

// Secondary window (Settings, SoundTouch) in the app's style. It follows
// Settings › Appearance › Title bar like the main window: "Integrated" draws
// a slim bar in the skin's colours with its own buttons, rounded corners and
// outline, and the desktop moves / resizes it; "System" keeps the desktop's
// title bar. Children go below the bar (it is the window's header).
ApplicationWindow {
    id: win

    property int baseFlags: Qt.Window   // flags without the frameless hint
    property bool maximizable: true
    property string iconName           // shown before the title in the integrated bar
    readonly property bool maximized: visibility === Window.Maximized || visibility === Window.FullScreen
    readonly property real cornerRadius: App.alphaWindow && !maximized ? Theme.windowRadius : 0
    function toggleMaximized() { if (maximized) showNormal(); else showMaximized() }

    flags: baseFlags | Qt.FramelessWindowHint
    color: App.alphaWindow ? "transparent" : Theme.solidBg
    background: Rectangle { color: Theme.solidBg; radius: win.cornerRadius }
    font.pixelSize: Theme.fontBody
    font.family: App.fontFamily.length > 0 ? App.fontFamily : Qt.application.font.family

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

    // --- Integrated title bar ----------------------------------------------
    header: Item {
        implicitHeight: 38
        // The desktop moves the window from the first pixels of a drag.
        DragHandler {
            target: null
            dragThreshold: 2
            onActiveChanged: if (active) win.startSystemMove()
        }
        TapHandler { onDoubleTapped: if (win.maximizable) win.toggleMaximized() }
        Row {
            x: 16
            anchors.verticalCenter: parent.verticalCenter
            width: buttons.x - x - 12
            spacing: 9
            Icon {
                anchors.verticalCenter: parent.verticalCenter
                visible: win.iconName.length > 0
                glyph: win.iconName
                size: 16
                color: Theme.accent
            }
            Label {
                anchors.verticalCenter: parent.verticalCenter
                width: parent.width - (win.iconName.length > 0 ? 25 : 0)
                text: win.title
                elide: Text.ElideRight
                color: Theme.text
                font.pixelSize: Theme.fontBody + 1
                font.weight: Font.DemiBold
            }
        }
        WindowButtons {
            id: buttons
            keepClose: true
            anchors.top: parent.top
            anchors.right: parent.right
            anchors.topMargin: flush ? 0 : 4
            anchors.rightMargin: flush ? 0 : 8
            cornerRadius: win.cornerRadius
            maximizable: win.maximizable
            maximized: win.maximized
            onMinimizeClicked: win.showMinimized()
            onMaximizeClicked: win.toggleMaximized()
            onCloseClicked: win.close()
        }
    }

    // --- Resizing from the edges (integrated bar: no desktop frame) ----------
    // On the overlay layer: above the header and the content.
    component ResizeEdge: MouseArea {
        property int edges
        z: 2000
        enabled: !win.maximized
        visible: enabled
        hoverEnabled: true
        acceptedButtons: Qt.LeftButton
        cursorShape: edges === (Qt.LeftEdge | Qt.TopEdge) || edges === (Qt.RightEdge | Qt.BottomEdge) ? Qt.SizeFDiagCursor
                   : edges === (Qt.RightEdge | Qt.TopEdge) || edges === (Qt.LeftEdge | Qt.BottomEdge) ? Qt.SizeBDiagCursor
                   : (edges & (Qt.LeftEdge | Qt.RightEdge)) ? Qt.SizeHorCursor : Qt.SizeVerCursor
        onPressed: win.startSystemResize(edges)
    }
    readonly property int grip: 5
    Item {
        parent: Overlay.overlay
        anchors.fill: parent
        z: 1000
        ResizeEdge { edges: Qt.LeftEdge; x: 0; y: win.grip; width: win.grip; height: parent.height - 2 * win.grip }
        ResizeEdge { edges: Qt.RightEdge; x: parent.width - win.grip; y: win.grip; width: win.grip; height: parent.height - 2 * win.grip }
        // Stops at the buttons, which sit flush in the top-right corner.
        ResizeEdge { edges: Qt.TopEdge; x: win.grip; y: 0; width: parent.width - win.grip - buttons.width; height: win.grip }
        ResizeEdge { edges: Qt.BottomEdge; x: win.grip; y: parent.height - win.grip; width: parent.width - 2 * win.grip; height: win.grip }
        ResizeEdge { edges: Qt.LeftEdge | Qt.TopEdge; x: 0; y: 0; width: 2 * win.grip; height: 2 * win.grip }
        ResizeEdge { edges: Qt.LeftEdge | Qt.BottomEdge; x: 0; y: parent.height - 2 * win.grip; width: 2 * win.grip; height: 2 * win.grip }
        ResizeEdge { edges: Qt.RightEdge | Qt.BottomEdge; x: parent.width - 2 * win.grip; y: parent.height - 2 * win.grip; width: 2 * win.grip; height: 2 * win.grip }

        // Outline, as on the main window (Settings › Appearance › Window outline).
        WindowOutline {
            anchors.fill: parent
            z: 1999
            visible: !win.maximized
            radius: win.cornerRadius
            windowEdge: true
        }
        // A square window: the edge pixels the software renderer leaves
        // unpainted at a fractional scale (Main.qml).
        WindowEdges {
            anchors.fill: parent
            z: 2000
            visible: win.cornerRadius === 0
        }
    }
}
