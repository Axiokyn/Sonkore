import QtQuick
import Sonkore

// Album art with rounded corners and a placeholder. The provider decodes the
// cover at the displayed size and rounds the corners itself, so this is a
// plain Image (no shader effect, no offscreen layer) and works the same with
// every renderer.
Item {
    id: root

    property string path
    property real radius: Theme.radiusSmall
    property int iconSize: Math.max(14, Math.round(width * 0.32))
    readonly property bool hasCover: image.status === Image.Ready && image.implicitWidth > 1

    implicitWidth: 48
    implicitHeight: 48

    RoundRect {
        anchors.fill: parent
        radius: root.radius
        color: Theme.surfaceHi
        borderWidth: root.hasCover ? 0 : 1
        borderColor: Theme.border
        Icon {
            anchors.centerIn: parent
            visible: !root.hasCover
            glyph: "music"
            size: root.iconSize
            color: Theme.textFaint
        }
    }

    // A cover fetched from the internet for this track: shown now.
    Connections {
        target: OnlineArt
        function onCoverArrived(paths) {
            if (paths.indexOf(root.path) >= 0)
                image.generation += 1
        }
    }

    Image {
        id: image
        readonly property real dpr: Screen.devicePixelRatio
        property int generation: 0
        anchors.fill: parent
        source: root.path.length > 0 && root.width > 0
                ? "image://cover/r" + Math.round(root.radius * dpr) + "/" + encodeURIComponent(root.path)
                  + (generation > 0 ? "?v=" + generation : "") : ""
        sourceSize: Qt.size(Math.ceil(root.width * dpr), Math.ceil(root.height * dpr))
        asynchronous: true
        cache: true
        visible: root.hasCover
        opacity: visible ? 1 : 0
        Behavior on opacity { NumberAnimation { duration: 160 } }
    }
}
