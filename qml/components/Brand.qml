import QtQuick
import QtQuick.Controls
import Sonkore

// The app's logo and its wordmark ("son" in the text colour, "kore" in the
// accent, Sora Bold: fonts/sora, used only here), at the top left of the
// window. `logoOnly` leaves the wordmark out (folded sidebar); `clickable`
// makes the logo a button (logoClicked).
Row {
    id: brand
    property bool logoOnly: false
    property bool clickable: false
    property string logoTip
    property int logoSize: 42
    property int textSize: 25
    signal logoClicked()
    spacing: 8

    Item {
        id: logo
        width: brand.logoSize + 5
        height: brand.logoSize + 5
        anchors.verticalCenter: parent.verticalCenter
        // A faint dark rim all round, like a muted shadow: a few rounded
        // layers, each a little larger and fainter, so the logo's dark face
        // stands off the bar behind it.
        Repeater {
            model: 3
            Rectangle {
                required property int index
                anchors.centerIn: parent
                width: Math.round(logoImage.width * 0.9375) + (index + 1) * 2
                height: width
                radius: width * 0.32
                color: Qt.rgba(0, 0, 0, Theme.dark ? 0.11 : 0.04)
            }
        }
        Image {
            id: logoImage
            anchors.centerIn: parent
            source: "qrc:/icons/sonkore.svg"
            sourceSize: Qt.size(brand.logoSize, brand.logoSize)
            smooth: true
        }
        HoverHandler {
            id: logoHover
            enabled: brand.clickable
            cursorShape: Qt.PointingHandCursor
        }
        TapHandler {
            enabled: brand.clickable
            onTapped: brand.logoClicked()
        }
        ToolTip.visible: brand.clickable && brand.logoTip.length > 0 && logoHover.hovered
        ToolTip.delay: 350
        ToolTip.text: brand.logoTip
    }
    FontLoader {
        id: wordmarkFont
        source: "qrc:/fonts/sora/700.ttf"
    }
    Label {
        visible: !brand.logoOnly
        anchors.verticalCenter: parent.verticalCenter
        text: "son<font color=\"" + Theme.accent + "\">kore</font>"
        textFormat: Text.StyledText
        color: Theme.text
        font.family: wordmarkFont.name
        font.pixelSize: brand.textSize
    }
}
