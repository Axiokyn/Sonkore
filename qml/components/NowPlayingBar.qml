import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Sonkore

// Bottom transport bar: artwork + title, transport and seek, signal path
// status, output device, EQ/DSP shortcuts and volume.
Rectangle {
    id: bar

    property string page        // current page, to highlight its button
    readonly property var track: DspController.currentTrack
    property real lastVolume: 1.0
    // Narrow windows (large interface scales): smaller side sections.
    readonly property bool narrow: width < 940
    readonly property bool veryNarrow: width < 820

    // Progress: the engine reports its position ~20x per second; the bar is
    // extrapolated from the last report and moved only when it advances a
    // device pixel (at least 4x per second for the time label), on a Clock
    // tick so it shares the visualizer's frames. Moving it every frame would
    // redraw the whole window at the display refresh rate while playing, for a
    // bar that moves 1 px every few hundred ms.
    property real smoothPos: DspController.position
    property real anchorPos: 0
    property double anchorTime: 0
    property double lastScrub: 0
    property double lastStep: 0

    function resync() {
        anchorPos = DspController.position
        anchorTime = Date.now()
        if (!DspController.playing || Math.abs(DspController.position - smoothPos) > 0.5)
            smoothPos = DspController.position
    }
    Connections {
        target: DspController
        function onPositionChanged() { bar.resync() }
        function onSeeked() { bar.smoothPos = DspController.position; bar.resync() }
        function onTrackChanged() { bar.smoothPos = DspController.position; bar.resync() }
    }
    Connections {
        target: Clock
        enabled: DspController.playing && bar.visible && !seek.pressed
        readonly property real pixels: Math.max(1, seek.availableWidth * Screen.devicePixelRatio)
        readonly property real interval: Math.min(250, 1000 * DspController.duration / (pixels * Math.max(0.1, DspController.speed)))
        function onTick() {
            const now = Date.now()
            if (now - bar.lastStep < interval)
                return
            bar.lastStep = now
            const target = bar.anchorPos + (now - bar.anchorTime) / 1000 * DspController.speed
            // never backwards by a fraction of a pixel; real jumps go through resync()
            bar.smoothPos = Math.min(DspController.duration, Math.max(bar.smoothPos, target))
        }
    }

    implicitHeight: Theme.compact ? 72 : Theme.spacious ? 100 : 88
    // y of the seek bar's middle line in the bar, and of the line's bottom
    // edge: the visualizer stands there, the same for every bar style (a
    // thicker bar or the waveform does not move it).
    readonly property real seekTrackCenter: transport.y + seekRow.y + seek.y + seek.topPadding + seek.availableHeight / 2
    readonly property real seekTrackBottom: seekTrackCenter + 1
    color: Theme.sidebar

    Rectangle {
        anchors.top: parent.top
        width: parent.width
        height: 1
        color: Theme.border
    }

    RowLayout {
        anchors.fill: parent
        anchors.leftMargin: 16
        anchors.rightMargin: 22 // a touch more air after the volume slider
        spacing: bar.narrow ? 12 : 28

        // --- Now playing ---------------------------------------------------
        // Its own slot: it never shrinks below what the cover and the
        // visualizer need, so nothing spills over the seek bar.
        RowLayout {
            id: nowInfo
            Layout.preferredWidth: bar.narrow ? 200 : (visualizer.visible ? 420 : 320)
            Layout.minimumWidth: bar.narrow ? 160 : (visualizer.visible ? 330 : 220)
            Layout.maximumWidth: visualizer.visible ? 470 : 360
            Layout.fillWidth: true
            // The full height of the bar: its y stays 0, so the visualizer's
            // margin below (measured from it) cannot move it in turn.
            Layout.fillHeight: true
            clip: true
            spacing: 12

            CoverArt {
                Layout.preferredWidth: Theme.compact ? 44 : Theme.spacious ? 66 : 58
                Layout.preferredHeight: Layout.preferredWidth
                path: DspController.currentPath
                radius: Theme.radiusSmall
            }
            ColumnLayout {
                Layout.fillWidth: true
                Layout.minimumWidth: 0
                spacing: 3
                Label {
                    Layout.fillWidth: true
                    text: bar.track.title || qsTr("Nothing playing")
                    elide: Text.ElideRight
                    color: Theme.text
                    font.pixelSize: Theme.fontBody + 1
                    font.weight: Font.DemiBold
                }
                Label {
                    Layout.fillWidth: true
                    text: bar.track.artist ? bar.track.artist + (bar.track.album ? "  ·  " + bar.track.album : "") : qsTr("Pick something from your library")
                    elide: Text.ElideRight
                    color: Theme.textDim
                    font.pixelSize: Theme.fontSmall + 1
                }
                Badge {
                    Layout.maximumWidth: parent.width // shortened with "…" when narrow
                    text: DspController.trackDescription
                    tone: Theme.textDim
                    // Compact: two lines only (title, artist), the bar is short.
                    visible: !Theme.compact && DspController.currentPath.length > 0 && text.length > 0
                }
            }
            Visualizer {
                id: visualizer
                visible: bar.width >= 1100 && Theme.visualizer !== "off" && DspController.currentPath.length > 0
                // Away while paused: its flat resting row read like a stray line.
                opacity: DspController.playing ? 1 : 0
                Behavior on opacity { NumberAnimation { duration: 250 } }
                style: Theme.visualizer
                Layout.preferredWidth: 96
                Layout.minimumWidth: 96
                Layout.preferredHeight: Theme.compact ? 28 : Theme.spacious ? 44 : 38
                // Its bottom edge on the seek line's last pixel row, whatever
                // the bar style; "mirror" grows both ways, so its middle on
                // the seek bar's middle line.
                Layout.alignment: Qt.AlignTop
                Layout.topMargin: Math.max(0, (style === "mirror" ? bar.seekTrackCenter + height / 2 : bar.seekTrackBottom) - nowInfo.y - height)
                Layout.rightMargin: 4
            }
        }

        // --- Transport + seek ----------------------------------------------
        ColumnLayout {
            id: transport
            Layout.fillWidth: true
            Layout.preferredWidth: 520
            spacing: 2

            RowLayout {
                Layout.alignment: Qt.AlignHCenter
                // The player buttons in the chosen set (Settings › Appearance ›
                // Player buttons: the play button's shape); stop only when asked for (Settings › General).
                spacing: 10
                IconButton {
                    iconName: "shuffle"
                    active: DspController.shuffle
                    tip: qsTr("Shuffle")
                    onClicked: DspController.shuffle = !DspController.shuffle
                }
                IconButton {
                    iconName: "prev"
                    iconSize: 18
                    tip: qsTr("Previous")
                    onClicked: DspController.previous()
                }
                PlayButton {
                    implicitWidth: Theme.compact ? 34 : Theme.spacious ? 48 : 42
                    iconSize: Theme.compact ? 16 : Theme.spacious ? 20 : 18
                }
                IconButton {
                    visible: Prefs.store.stopButton
                    iconName: "stop"
                    iconSize: 18
                    tip: qsTr("Stop")
                    onClicked: DspController.stop()
                }
                IconButton {
                    iconName: "next"
                    iconSize: 18
                    tip: qsTr("Next")
                    onClicked: DspController.next()
                }
                IconButton {
                    iconName: DspController.repeatMode === 2 ? "repeat-one" : "repeat"
                    active: DspController.repeatMode !== 0
                    tip: [qsTr("Repeat off"), qsTr("Repeat all"), qsTr("Repeat one")][DspController.repeatMode]
                    onClicked: DspController.repeatMode = (DspController.repeatMode + 1) % 3
                }
            }

            RowLayout {
                id: seekRow
                Layout.fillWidth: true
                spacing: 10
                Label {
                    text: DspController.formatTime(seek.pressed ? seek.value : bar.smoothPos)
                    color: Theme.textDim
                    font.pixelSize: Theme.fontSmall
                    font.features: { "tnum": 1 }
                    Layout.preferredWidth: 40
                    horizontalAlignment: Text.AlignRight
                }
                LSlider {
                    id: seek
                    Layout.fillWidth: true
                    seekBar: true
                    from: 0
                    to: Math.max(DspController.duration, 0.001)
                    enabled: DspController.currentPath.length > 0
                    alwaysShowHandle: pressed
                    value: pressed ? value : bar.smoothPos
                    // Seeking costs ~6 ms, so it can follow the pointer live.
                    onMoved: {
                        if (Prefs.store.liveSeek && Date.now() - bar.lastScrub > 45) {
                            bar.lastScrub = Date.now()
                            DspController.seek(value)
                        }
                    }
                    onPressedChanged: if (!pressed) { DspController.seek(value); bar.smoothPos = value }
                }
                Label {
                    text: DspController.formatTime(DspController.duration)
                    color: Theme.textDim
                    font.pixelSize: Theme.fontSmall
                    font.features: { "tnum": 1 }
                    Layout.preferredWidth: 40
                }
            }
        }

        // --- Signal path / output / volume ---------------------------------
        // Centred in the bar's height.
        RowLayout {
            Layout.preferredWidth: bar.narrow ? 236 : 390
            // Never below its buttons: the transport gives way instead, rather
            // than the buttons spilling out of the window's edge.
            Layout.minimumWidth: implicitWidth
            Layout.maximumWidth: 430
            Layout.fillWidth: true
            Layout.alignment: Qt.AlignRight | Qt.AlignVCenter
            spacing: bar.narrow ? 0 : 4

            Item { Layout.fillWidth: true }

            IconButton {
                visible: !bar.narrow
                iconName: "wave"
                active: DspController.soundTouchActive
                tip: qsTr("Pitch / tempo (SoundTouch)")
                onClicked: AppActions.openSoundTouch()
            }
            IconButton {
                iconName: "eq"
                active: Equalizer.enabled || AppActions.equalizerOpen
                tip: Equalizer.enabled ? qsTr("Equalizer on — click to open") : qsTr("Equalizer off — click to open")
                onClicked: AppActions.navigate("eq", "")
            }
            IconButton {
                id: pathButton
                visible: DspController.loaded
                iconName: "chip"
                active: DspController.bitPerfect
                tip: qsTr("Signal path")
                onClicked: signalPath.opened ? signalPath.close() : signalPath.open()
            }
            IconButton {
                id: outputButton
                iconName: "speaker"
                active: DspController.bitPerfect
                tip: DspController.outputDescription
                onClicked: {
                    DspController.refreshDevices()
                    outputMenu.openAbove()
                }
            }
            LMenu {
                id: outputMenu
                // As wide as the longest name, up to 400 (longer ones elide).
                readonly property real maxWidth: 400
                width: maxWidth
                function fitWidth() {
                    let w = 0
                    for (let i = 0; i < count; ++i) {
                        const it = itemAt(i)
                        if (it && it.visible)
                            w = Math.max(w, it.implicitWidth)
                    }
                    width = Math.min(maxWidth, Math.max(220, Math.ceil(w) + leftPadding + rightPadding))
                }
                // Always above the button, centred on it; the height is only
                // final once the menu has laid out, so it follows it.
                function openAbove() {
                    fitWidth()
                    popup(outputButton, Math.round((outputButton.width - width) / 2), -Math.max(implicitHeight, height) - 6)
                }
                function settle() {
                    if (visible)
                        y = -height - 6
                }
                onHeightChanged: settle()
                onOpened: { fitWidth(); x = Math.round((outputButton.width - width) / 2); settle() }
                Instantiator {
                    model: DspController.outputDevices
                    delegate: LMenuItem {
                        required property var modelData
                        text: (modelData.direct ? "◆ " : "") + modelData.name + "   —   " + modelData.backend
                        checkable: true
                        checked: modelData.key === DspController.currentDeviceKey
                        onTriggered: DspController.selectOutputDevice(modelData.key)
                    }
                    onObjectAdded: (index, object) => outputMenu.insertItem(index, object)
                    onObjectRemoved: (index, object) => outputMenu.removeItem(object)
                }
                LMenuSeparator { visible: DspController.outputReserved; height: visible ? implicitHeight : 0 }
                LMenuItem {
                    visible: DspController.outputReserved
                    height: visible ? implicitHeight : 0
                    text: qsTr("Release DAC to the system")
                    onTriggered: DspController.releaseOutput()
                }
            }

            IconButton {
                visible: !bar.veryNarrow // Ctrl+M still works
                iconName: "mini-player"
                tip: qsTr("Compact mode (Ctrl+M)")
                onClicked: AppActions.compactRequested(true)
            }
            IconButton {
                iconName: "settings"
                active: AppActions.settingsOpen
                tip: qsTr("Settings")
                onClicked: AppActions.navigate("settings", "")
            }
            IconButton {
                id: volumeButton
                TapHandler {
                    acceptedButtons: Qt.RightButton
                    onTapped: bar.openVolumeMenu()
                }
                iconName: DspController.volume === 0 ? "volume-mute" : (DspController.volume < 0.5 ? "volume-low" : "volume")
                iconColor: DspController.volumeFixed ? Theme.textFaint : (hovered ? Theme.text : Theme.textDim)
            tip: volumeMenu.opened ? "" : DspController.volumeFixed ? qsTr("Volume fixed at 100 %: set it on the DAC or the amplifier. Click to choose how the volume is set.")
                                           : qsTr("Mute · right-click: volume control")
                onClicked: {
                    if (DspController.volumeFixed) {
                        bar.openVolumeMenu()
                        return
                    }
                    if (DspController.volume > 0) {
                        bar.lastVolume = DspController.volume
                        DspController.volume = 0
                    } else {
                        DspController.volume = bar.lastVolume
                    }
                }
            }
            LockedVolume {
                id: lockedVolume
                menuOpen: volumeMenu.opened
                onClicked: bar.openVolumeMenu()
                visible: DspController.volumeFixed
                brief: bar.narrow
                Layout.preferredWidth: bar.narrow ? 60 : 110
            }
            // The slider moves the DAC's own control (Volume control: DAC).
            Label {
                visible: DspController.hardwareVolume && !bar.narrow
                text: "DAC"
                color: Theme.accent
                font.pixelSize: Theme.fontSmall - 1
                font.weight: Font.DemiBold
                leftPadding: 5
                rightPadding: 5
                topPadding: 1
                bottomPadding: 1
                background: Rectangle {
                    radius: height / 2
                    color: "transparent"
                    border.color: Theme.accent
                    opacity: 0.6
                }
            }
            LSlider {
                id: volume
                Layout.preferredWidth: bar.narrow ? 60 : 110
                // grey at rest, the accent while it is being set: the accent
                // stays for what plays
                fillColor: hovered || pressed ? Theme.accent : Theme.textDim
                WheelHandler {
                    onWheel: (event) => {
                        const step = event.angleDelta.y > 0 ? 0.04 : -0.04
                        const pos = Math.max(0, Math.min(1, Math.sqrt(DspController.volume) + step))
                        DspController.volume = pos >= 0.999 ? 1.0 : pos * pos
                    }
                }
                from: 0
                to: 1
                visible: !DspController.volumeFixed
                // Perceptual taper: slider position = sqrt(gain).
                value: Math.sqrt(DspController.volume)
                onMoved: DspController.volume = value >= 0.999 ? 1.0 : value * value
                ToolTip.visible: hovered || pressed
                ToolTip.text: DspController.hardwareVolume
                    ? qsTr("DAC volume · %1 · bit-perfect").arg(DspController.volume > 0 ? (20 * Math.log10(DspController.volume)).toFixed(1) + " dB" : "−∞ dB")
                    : Math.round(value * 100) + " %"
            }
        }
    }

    // Opens above its button in the bar.
    SignalPathPopup {
        id: signalPath
        parent: pathButton
        x: pathButton.width - width
        y: -implicitHeight - 8
    }

    // The volume control menu, its right edge at the end of the volume
    // controls (the slider, or the "fixed" pill).
    function openVolumeMenu() {
        volumeMenu.openAbove(lockedVolume.visible ? lockedVolume : volume.visible ? volume : volumeButton)
    }
    VolumeModeMenu { id: volumeMenu }
}
