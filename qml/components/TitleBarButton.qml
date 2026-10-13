import QtQuick
import QtQuick.Controls
import Sonkore

// Minimize / maximize / close button of an integrated title bar, in the style
// chosen in Settings › Appearance › Window buttons (`style`, the Theme's):
//   windows  flat, flush in the corner, red close
//   mac      traffic lights; the glyph shows on hover
//   gnome    grey round buttons (Adwaita)
//   breeze   chevron glyphs (v ^ x, ◇ to restore), a round highlight on hover,
//            red close (KDE Plasma)
//   cinnamon plain glyphs, close a blue disc with a white cross (Linux Mint, Mint-Y)
//   minimal  glyphs only, tinted on hover
AbstractButton {
    id: wb
    property string glyph
    property string tip
    property bool danger: false        // the close button
    property real cornerRadius: 0      // windows: close follows the window's corner
    property string style: Theme.windowButtons
    // traffic-light colour by role (mac)
    readonly property color light: danger ? "#ff5f57" : glyph === "win-min" ? "#febc2e" : "#28c840"

    implicitWidth: style === "windows" ? 39 : style === "mac" ? 20 : style === "cinnamon" ? 28 : 30
    implicitHeight: style === "windows" ? 27 : 30
    hoverEnabled: true
    focusPolicy: Qt.NoFocus

    background: Item {
        anchors.fill: parent
        // windows: the whole cell
        Rectangle {
            visible: wb.style === "windows"
            anchors.fill: parent
            topRightRadius: wb.cornerRadius
            bottomLeftRadius: 4
            bottomRightRadius: 4
            color: wb.pressed ? (wb.danger ? Qt.darker("#e81123", 1.15) : Theme.border)
                 : wb.hovered ? (wb.danger ? "#e81123" : Theme.hover) : "transparent"
        }
        // cinnamon: the close button is a blue disc; the others a faint
        // round highlight on hover
        Rectangle {
            visible: wb.style === "cinnamon"
            anchors.centerIn: parent
            width: wb.danger ? 18 : 22
            height: width
            radius: width / 2
            color: wb.danger ? (wb.pressed ? "#1883ba" : wb.hovered ? "#45b3ec" : "#1f9ede")
                 : Qt.rgba(Theme.text.r, Theme.text.g, Theme.text.b, wb.pressed ? 0.16 : wb.hovered ? 0.1 : 0)
        }
        // the round styles
        Rectangle {
            visible: wb.style !== "windows" && wb.style !== "minimal" && wb.style !== "cinnamon"
            anchors.centerIn: parent
            width: wb.style === "mac" ? 13 : wb.style === "gnome" ? 24 : 26
            height: width
            radius: width / 2
            color: wb.style === "mac" ? (wb.pressed ? Qt.darker(wb.light, 1.2) : wb.light)
                 : wb.style === "gnome" ? Qt.rgba(Theme.text.r, Theme.text.g, Theme.text.b, wb.pressed ? 0.2 : wb.hovered ? 0.15 : 0.09)
                 : wb.danger && wb.hovered ? (wb.pressed ? Qt.darker("#da4453", 1.15) : "#da4453")
                 : Qt.rgba(Theme.text.r, Theme.text.g, Theme.text.b, wb.pressed ? 0.18 : wb.hovered ? 0.12 : 0)
            border.width: wb.style === "mac" ? 1 : 0
            border.color: Qt.rgba(0, 0, 0, 0.18)
        }
    }
    contentItem: Item {
        Icon {
            anchors.centerIn: parent
            // Mint's minimize is a low bar ("_").
            anchors.verticalCenterOffset: wb.style === "cinnamon" && wb.glyph === "win-min" ? 4 : 0
            visible: wb.style !== "mac" || wb.hovered
            // Breeze draws its own: chevrons down / up, a diamond, a thin cross.
            glyph: wb.style !== "breeze" ? wb.glyph
                 : wb.glyph === "close" ? "breeze-close" : wb.glyph.replace("win-", "breeze-")
            size: wb.style === "mac" ? 8 : wb.style === "windows" || wb.style === "breeze" ? 14
                : wb.style === "cinnamon" && wb.danger ? 10 : 12
            color: wb.style === "mac" ? Qt.rgba(0, 0, 0, 0.6)
                 : wb.style === "cinnamon" && wb.danger ? "#ffffff"
                 : (wb.style === "windows" || wb.style === "breeze") && wb.danger && wb.hovered ? "#ffffff"
                 : wb.style === "minimal" && wb.hovered ? (wb.danger ? "#e81123" : Theme.accent)
                 : wb.hovered || wb.style === "gnome" || wb.style === "cinnamon" || wb.style === "breeze" ? Theme.text : Theme.textDim
        }
    }
    ToolTip.visible: tip.length > 0 && hovered
    ToolTip.delay: 600
    ToolTip.text: tip
}
