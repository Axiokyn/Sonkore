import QtQuick
import QtQuick.Controls
import Sonkore

// Play / pause in the player bars, shaped as chosen in Settings ›
// Appearance › Player buttons (`style`, the Theme's); the glyph follows the
// icon style:
//   sonkore  the glyph alone, larger than its neighbours
//   round    a circle (light on dark themes, the accent on light ones)
//   outline  a thin ring around the glyph
AbstractButton {
    id: button

    property int iconSize: 18
    property bool playing: DspController.playing
    property string style: Theme.transportStyle // Settings previews pass their own
    readonly property bool round: style === "round"
    readonly property color face: Theme.dark ? Theme.text : Theme.accent

    implicitWidth: 42
    implicitHeight: implicitWidth
    hoverEnabled: true
    focusPolicy: Qt.NoFocus
    onClicked: DspController.togglePlayPause()

    // "Tile" icon style: the plain (sonkore) button on a soft square, like
    // its neighbours (IconButton).
    readonly property bool tile: style === "sonkore" && Theme.iconTiles

    background: Item {
        RoundRect {
            anchors.fill: parent
            // The round face grows a little under the pointer and shrinks when
            // pressed: by its size, not a scale transform, so its edge is
            // rasterised again (anti-aliased) at each size with every
            // renderer, the software one included (which scales a drawn
            // circle without smoothing).
            anchors.margins: button.tile ? 2 : button.round && button.pressed ? 1 : button.round && button.hovered ? -1 : 0
            radius: button.tile ? Math.round(Math.min(width, height) * 0.3) : width / 2
            color: button.round ? button.face
                 : button.pressed ? Theme.border : button.hovered ? Theme.hover
                 : button.tile ? Theme.surfaceHi : "transparent"
            borderWidth: button.style === "outline" ? 1.5 : 0
            borderColor: button.hovered ? Theme.accent : Theme.text
            Behavior on anchors.margins { NumberAnimation { duration: 100 } }
            Behavior on color { ColorAnimation { duration: 120 } }
        }
    }
    contentItem: Item {
        Icon {
            id: glyph
            anchors.centerIn: parent
            // A triangle's weight sits left of its box's middle: moved right
            // to look centred (not the line glyph, already drawn right in its
            // box).
            anchors.horizontalCenterOffset: button.playing ? 0 : (Theme.lineIcons ? 0 : Math.round(size * 0.08))
            glyph: button.playing ? "pause" : "play"
            size: button.tile ? button.iconSize + 2 : button.style === "sonkore" ? button.iconSize + 6 : button.iconSize
            color: button.round ? (Theme.dark ? Theme.solidBg : Theme.onAccent)
                 : button.hovered ? Theme.accent : Theme.text
        }
    }
}
