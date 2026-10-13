pragma ComponentBehavior: Bound
import QtQuick
import Sonkore

// The integrated title bar's minimize / maximize / close buttons, in the
// chosen look (Theme.windowButtons). Placed by the parent in its top-right corner;
// `edge` is the margin to leave around it there (flush for Windows-style
// buttons, a little air for the others). Only the buttons the desktop shows
// (App.systemWindowButtons); none under a tiling window manager, which
// handles the window itself, unless `keepClose` (dialogs, which float there).
Row {
    id: buttons

    property real cornerRadius: 0       // the window's: a flush close button follows it
    property bool maximizable: true
    property bool maximized: false
    property string maximizeTip: maximized ? qsTr("Restore") : qsTr("Maximize")
    property bool keepClose: false
    signal minimizeClicked()
    signal maximizeClicked()
    signal closeClicked()

    readonly property var order: ["minimize", "maximize", "close"]
    readonly property var shown: order.filter(b => App.tilingWindowManager ? keepClose && b === "close"
                                                   : App.systemWindowButtons.indexOf(b) >= 0
                                                     && (b !== "maximize" || maximizable))
    readonly property bool flush: Theme.windowButtons === "windows"
    readonly property real edge: flush ? 0 : 6
    spacing: Theme.windowButtons === "mac" ? 4 : flush ? 0 : 2
    visible: shown.length > 0

    Repeater {
        model: buttons.shown
        TitleBarButton {
            required property string modelData
            required property int index
            glyph: modelData === "minimize" ? "win-min"
                 : modelData === "close" ? "close"
                 : buttons.maximized ? "win-restore" : "win-max"
            danger: modelData === "close"
            // Only the one in the corner follows the window's corner.
            cornerRadius: index === buttons.shown.length - 1 ? buttons.cornerRadius : 0
            tip: modelData === "minimize" ? qsTr("Minimize") : modelData === "close" ? qsTr("Close") : buttons.maximizeTip
            onClicked: {
                if (modelData === "minimize")
                    buttons.minimizeClicked()
                else if (modelData === "close")
                    buttons.closeClicked()
                else
                    buttons.maximizeClicked()
            }
        }
    }
}
