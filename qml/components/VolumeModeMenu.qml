import QtQuick
import QtQuick.Controls
import Sonkore

// Where the volume is set, chosen right from the player bar (the volume
// button while the volume is fixed, or a right-click on it): the same three
// modes as Settings › Audio › Volume control.
LMenu {
    id: menu

    width: 320
    // In the window, where it can be placed exactly. The compact player is
    // shorter than the menu, so there it is a pop-up window of its own
    // (Qt 6.8+), positioned once: Wayland does not move an open pop-up.
    property bool ownWindow: false
    Component.onCompleted: if (ownWindow && "popupType" in menu) menu.popupType = 1 // Popup.Window

    // Opens above `item`, right edges lined up. The height is only final
    // once the descriptions have wrapped: it follows the height until then.
    property Item anchorItem: null
    function openAbove(item) {
        anchorItem = item
        popup(item, item.width - width, -Math.max(implicitHeight, height) - 8)
    }
    function settle() {
        if (visible && anchorItem && !ownWindow)
            y = -height - 8
    }
    onHeightChanged: settle()
    onOpened: settle()

    Label {
        leftPadding: 10
        topPadding: 4
        bottomPadding: 4
        text: qsTr("Volume control")
        color: Theme.textFaint
        font.pixelSize: Theme.fontSmall
        font.weight: Font.DemiBold
        font.capitalization: Font.AllUppercase
        font.letterSpacing: 0.6
    }
    // VolumeMode values: 0 Digital, 1 DAC, 2 Automatic.
    LMenuItem {
        text: qsTr("Automatic")
        description: qsTr("Fixed at 100 % on a direct output (set it on the DAC or the amplifier); Sonkore's own volume otherwise.")
        checkable: true
        checked: DspController.volumeMode === 2
        onTriggered: {
            DspController.volumeMode = 2
            checked = Qt.binding(() => DspController.volumeMode === 2) // a click toggles it otherwise
        }
    }
    LMenuItem {
        text: qsTr("Digital")
        description: qsTr("Sonkore scales the samples: below 100 % the path is no longer bit-perfect.")
        checkable: true
        checked: DspController.volumeMode === 0
        onTriggered: {
            DspController.volumeMode = 0
            checked = Qt.binding(() => DspController.volumeMode === 0) // a click toggles it otherwise
        }
    }
    LMenuItem {
        text: qsTr("DAC")
        // Chosen but not possible with this output: say so, it is fixed then.
        description: DspController.volumeMode === 1 && !DspController.hardwareVolume
                     ? DspController.hardwareVolumeInfo
                     : qsTr("Moves the DAC's own volume control: the audio stays bit-perfect.")
        checkable: true
        checked: DspController.volumeMode === 1
        onTriggered: {
            DspController.volumeMode = 1
            checked = Qt.binding(() => DspController.volumeMode === 1) // a click toggles it otherwise
        }
    }
    LMenuSeparator {}
    LMenuItem {
        text: qsTr("Audio settings…")
        onTriggered: {
            Prefs.store.settingsTab = "audio"
            AppActions.navigate("settings", "")
        }
    }
}
