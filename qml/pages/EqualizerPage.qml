pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Dialogs
import QtQuick.Layouts
import Sonkore

Item {
    id: page

    readonly property bool graphic: Equalizer.mode === "graphic"
    // Height that shows everything (its window is never taller).
    readonly property real fullHeight: pageFlick.contentHeight
    readonly property bool isUserProfile: Equalizer.userProfiles.indexOf(Equalizer.currentProfile) >= 0

    NameDialog {
        id: saveDialog
        title: qsTr("Save EQ profile")
        placeholder: qsTr("e.g. My headphones — warm")
        onAccepted: (name) => AppActions.toast(qsTr("Saved “%1”").arg(Equalizer.saveProfile(name)))
    }
    FileDialog {
        id: importDialog
        title: qsTr("Import parametric EQ profile")
        nameFilters: [qsTr("Parametric EQ (*.txt)"), qsTr("All files (*)")]
        onAccepted: Equalizer.importFile(selectedFile)
    }
    FileDialog {
        id: exportDialog
        title: qsTr("Export EQ profile")
        fileMode: FileDialog.SaveFile
        defaultSuffix: "txt"
        nameFilters: [qsTr("Parametric EQ (*.txt)")]
        onAccepted: AppActions.toast(Equalizer.exportFile(selectedFile) ? qsTr("Profile exported") : qsTr("Export failed"))
    }
    Connections {
        target: Equalizer
        function onMessage(text) { AppActions.toast(text) }
    }

    LMenu {
        id: profileMenu
        LMenuItem { text: qsTr("Built-in"); enabled: false }
        Instantiator {
            model: Equalizer.builtinProfiles
            delegate: LMenuItem {
                required property string modelData
                text: "    " + modelData
                checkable: true
                checked: Equalizer.currentProfile === modelData
                onTriggered: Equalizer.loadProfile(modelData)
            }
            onObjectAdded: (index, object) => profileMenu.insertItem(1 + index, object)
            onObjectRemoved: (index, object) => profileMenu.removeItem(object)
        }
        LMenuSeparator {}
        LMenuItem {
            text: Equalizer.userProfiles.length > 0 ? qsTr("My profiles") : qsTr("My profiles — none saved yet")
            enabled: false
        }
        Instantiator {
            model: Equalizer.userProfiles
            delegate: LMenuItem {
                required property string modelData
                text: "    " + modelData
                checkable: true
                checked: Equalizer.currentProfile === modelData
                onTriggered: Equalizer.loadProfile(modelData)
            }
            onObjectAdded: (index, object) => profileMenu.insertItem(Equalizer.builtinProfiles.length + 3 + index, object)
            onObjectRemoved: (index, object) => profileMenu.removeItem(object)
        }
    }

    component VSlider: Slider {
        id: vs
        orientation: Qt.Vertical
        implicitWidth: 28 // the whole column around the thin track grabs the fader
        hoverEnabled: true
        focusPolicy: Qt.NoFocus
        background: Rectangle {
            x: vs.leftPadding + vs.availableWidth / 2 - width / 2
            y: vs.topPadding
            width: 4
            height: vs.availableHeight
            radius: 2
            color: Theme.border
            Rectangle { // 0 dB tick
                anchors.horizontalCenter: parent.horizontalCenter
                y: parent.height / 2
                width: 12
                height: 1
                color: Theme.textFaint
            }
            Rectangle { // fill from 0 dB
                anchors.horizontalCenter: parent.horizontalCenter
                width: parent.width
                radius: 2
                color: Equalizer.enabled ? Theme.accent : Theme.textFaint
                readonly property real zero: parent.height / 2
                readonly property real pos: vs.visualPosition * parent.height
                y: Math.min(zero, pos)
                height: Math.abs(zero - pos)
            }
        }
        handle: Rectangle {
            x: vs.leftPadding + vs.availableWidth / 2 - width / 2
            y: vs.topPadding + vs.visualPosition * (vs.availableHeight - height)
            width: 18
            height: 10
            radius: Theme.squareShapes ? 1 : 5
            color: Theme.dark ? "#ffffff" : Theme.surface
            border.color: vs.pressed ? Theme.accent : Theme.border
            border.width: vs.pressed ? 2 : 1
        }
    }

    Flickable {
        id: pageFlick
        WheelScroll { flickable: pageFlick }
        anchors.fill: parent
        contentHeight: column.y + column.implicitHeight + 20
        // Wheel and scroll bar only: dragging would steal the vertical drags
        // of the faders and of the response graph's points.
        interactive: false
        boundsBehavior: Flickable.StopAtBounds
        clip: true
        ScrollBar.vertical: LScrollBar {}

        ColumnLayout {
            id: column
            x: 20
            y: 6 // the window's title bar names it
            width: parent.width - 40
            spacing: 14

            // --- Profile summary + on / off (off: everything below is dimmed) ----
            StatusCard {
                title: Equalizer.currentProfile.length > 0 ? Equalizer.currentProfile : qsTr("Custom")
                subtitle: (page.graphic ? qsTr("10-band graphic") : qsTr("%1-band parametric").arg(Equalizer.bands.length))
                          + "  ·  " + (Equalizer.linearPhase ? qsTr("linear phase (FIR)") : qsTr("64-bit biquads"))
                checked: Equalizer.enabled
                onToggled: (on) => Equalizer.enabled = on
                tip: checked ? qsTr("Turn the equalizer off (the settings are kept)") : qsTr("Turn the equalizer on")
            }

            // --- Toolbar --------------------------------------------------------
            RowLayout {
                Layout.fillWidth: true
                enabled: Equalizer.enabled
                opacity: enabled ? 1 : 0.45
                spacing: 8
                PillButton {
                    id: profileButton
                    iconName: "chevron-down"
                    text: Equalizer.currentProfile.length > 0 ? Equalizer.currentProfile : qsTr("Custom")
                    Layout.maximumWidth: 320
                    onClicked: profileMenu.popup(profileButton, 0, profileButton.height + 4)
                }
                PillButton { iconName: "save"; text: qsTr("Save as…"); onClicked: saveDialog.ask(page.isUserProfile ? Equalizer.currentProfile : "") }
                IconButton {
                    iconName: "trash"
                    tip: qsTr("Delete this profile")
                    visible: page.isUserProfile
                    onClicked: Equalizer.deleteProfile(Equalizer.currentProfile)
                }
                Item { Layout.fillWidth: true }
                PillButton { iconName: "upload"; text: qsTr("Import"); onClicked: importDialog.open() }
                PillButton { iconName: "download"; text: qsTr("Export"); onClicked: exportDialog.open() }
                PillButton { iconName: "refresh"; text: qsTr("Reset"); onClicked: Equalizer.reset() }
            }

            // --- Response -------------------------------------------------------
            Card {
                Layout.fillWidth: true
                enabled: Equalizer.enabled
                opacity: enabled ? 1 : 0.45
                implicitHeight: 300
                ResponseGraph {
                    anchors.fill: parent
                    anchors.margins: 14
                    anchors.topMargin: 18
                }
                Label {
                    anchors.right: parent.right
                    anchors.top: parent.top
                    anchors.margins: 12
                    text: page.graphic ? qsTr("Drag points · double click = standard frequency · right click = reset") : qsTr("Drag points · wheel = Q · right click = reset")
                    color: Theme.textFaint
                    font.pixelSize: Theme.fontSmall
                }
            }

            // --- Preamp ---------------------------------------------------------
            Card {
                Layout.fillWidth: true
                enabled: Equalizer.enabled
                opacity: enabled ? 1 : 0.45
                implicitHeight: preampRow.implicitHeight + 24
                RowLayout {
                    id: preampRow
                    anchors.fill: parent
                    anchors.margins: 12
                    anchors.leftMargin: 16
                    spacing: 14
                    Label {
                        text: qsTr("Preamp")
                        color: Theme.text
                        font.weight: Font.DemiBold
                    }
                    LSlider {
                        Layout.fillWidth: true
                        from: -24
                        to: 6
                        stepSize: 0.1
                        value: Equalizer.preamp
                        alwaysShowHandle: true
                        onMoved: Equalizer.preamp = Math.round(value * 10) / 10
                    }
                    NumField {
                        value: Equalizer.preamp
                        from: -30
                        to: 12
                        decimals: 1
                        suffix: "dB"
                        onEdited: (v) => Equalizer.preamp = v
                    }
                    PillButton { text: qsTr("Auto"); onClicked: Equalizer.autoPreamp() }
                    LCheckBox {
                        text: qsTr("Linear phase")
                        checked: Equalizer.linearPhase
                        onToggled: Equalizer.linearPhase = checked
                        ToolTip.visible: hovered
                        ToolTip.delay: 600
                        ToolTip.text: qsTr("The same curve as a linear-phase FIR: every frequency is delayed alike, so transients and the stereo image keep their timing. Changes are heard about half a second later.")
                    }
                    Badge {
                        readonly property bool clipping: Equalizer.enabled && Equalizer.peakGain > 0.05
                        text: clipping ? qsTr("CLIP RISK +%1 dB").arg(Equalizer.peakGain.toFixed(1)) : qsTr("NO CLIPPING")
                        // Room for the longest of both texts (gains up to +99.9 dB).
                        reserveText: qsTr("CLIP RISK +%1 dB").arg("99.9").length >= qsTr("NO CLIPPING").length
                                     ? qsTr("CLIP RISK +%1 dB").arg("99.9") : qsTr("NO CLIPPING")
                        tone: clipping ? Theme.warn : Theme.good
                    }
                }
            }

            // --- Bands ----------------------------------------------------------
            Card {
                Layout.fillWidth: true
                enabled: Equalizer.enabled
                opacity: enabled ? 1 : 0.45
                implicitHeight: bandsColumn.implicitHeight + 28

                ColumnLayout {
                    id: bandsColumn
                    anchors.fill: parent
                    anchors.margins: 14
                    spacing: 12

                    RowLayout {
                        spacing: 4
                        Repeater {
                            model: [{ key: "graphic", text: qsTr("Graphic") }, { key: "parametric", text: qsTr("Parametric") }]
                            delegate: AbstractButton {
                                required property var modelData
                                readonly property bool current: Equalizer.mode === modelData.key
                                implicitWidth: tabLabel.implicitWidth + 28
                                implicitHeight: 30
                                onClicked: Equalizer.mode = modelData.key
                                background: Rectangle {
                                    radius: Theme.squareShapes ? 2 : 15
                                    color: parent.current ? Theme.accentSoft : (parent.hovered ? Theme.hover : "transparent")
                                    border.color: parent.current ? Theme.accent : Theme.border
                                }
                                contentItem: Label {
                                    id: tabLabel
                                    text: parent.modelData.text
                                    horizontalAlignment: Text.AlignHCenter
                                    verticalAlignment: Text.AlignVCenter
                                    color: parent.current ? Theme.accent : Theme.textDim
                                    font.weight: Font.DemiBold
                                }
                            }
                        }
                        Item { Layout.fillWidth: true }
                        PillButton {
                            visible: !page.graphic
                            iconName: "plus"
                            text: qsTr("Add band")
                            enabled: Equalizer.bands.length < Equalizer.maxBands
                            onClicked: Equalizer.addBand()
                        }
                    }

                    // Graphic: vertical faders spread evenly across the card
                    Item {
                        id: faders
                        visible: page.graphic
                        Layout.fillWidth: true
                        Layout.preferredHeight: 250
                        readonly property int count: page.graphic ? Equalizer.bands.length : 0
                        Repeater {
                            model: faders.count
                            delegate: Item {
                                id: fader
                                required property int index
                                readonly property var band: Equalizer.bands[index] || ({ gain: 0, freq: 1000 })
                                width: faders.width / Math.max(1, faders.count)
                                height: faders.height
                                x: index * width
                                Label {
                                    id: gainLabel
                                    anchors.horizontalCenter: parent.horizontalCenter
                                    text: (fader.band.gain > 0 ? "+" : "") + fader.band.gain.toFixed(1)
                                    color: fader.band.gain !== 0 ? Theme.accent : Theme.textFaint
                                    font.pixelSize: Theme.fontSmall
                                    font.features: { "tnum": 1 }
                                }
                                VSlider {
                                    anchors.horizontalCenter: parent.horizontalCenter
                                    anchors.top: gainLabel.bottom
                                    anchors.bottom: freqLabel.top
                                    anchors.topMargin: 4
                                    anchors.bottomMargin: 4
                                    from: -12
                                    to: 12
                                    stepSize: 0.5
                                    value: fader.band.gain
                                    onMoved: Equalizer.setGain(fader.index, value)
                                    TapHandler {
                                        onDoubleTapped: Equalizer.setGain(fader.index, 0)
                                    }
                                }
                                Label {
                                    id: freqLabel
                                    anchors.horizontalCenter: parent.horizontalCenter
                                    anchors.bottom: parent.bottom
                                    text: Equalizer.frequencyLabel(fader.band.freq)
                                    color: Theme.textDim
                                    font.pixelSize: Theme.fontSmall
                                    font.weight: Font.DemiBold
                                }
                            }
                        }
                    }

                    // Parametric: one row per band
                    ColumnLayout {
                        visible: !page.graphic
                        Layout.fillWidth: true
                        spacing: 4

                        RowLayout {
                            Layout.fillWidth: true
                            spacing: 10
                            SectionLabel { text: "#"; Layout.preferredWidth: 22 }
                            SectionLabel { text: qsTr("On"); Layout.preferredWidth: 40 }
                            SectionLabel { text: qsTr("Type"); Layout.preferredWidth: 130 }
                            SectionLabel { text: qsTr("Frequency"); Layout.preferredWidth: 104 }
                            SectionLabel { text: qsTr("Gain"); Layout.preferredWidth: 90 }
                            SectionLabel { text: qsTr("Q"); Layout.preferredWidth: 80 }
                            Item { Layout.fillWidth: true }
                        }
                        Repeater {
                            model: page.graphic ? 0 : Equalizer.bands.length
                            delegate: RowLayout {
                                id: bandRow
                                required property int index
                                readonly property var band: Equalizer.bands[index] || ({ type: "PK", freq: 1000, gain: 0, q: 1, enabled: true })
                                readonly property bool hasGain: band.type === "PK" || band.type === "LSC" || band.type === "HSC"
                                function update(type, freq, gain, q, enabled) {
                                    Equalizer.setBand(index, type, freq, gain, q, enabled)
                                }
                                Layout.fillWidth: true
                                spacing: 10
                                Label {
                                    text: bandRow.index + 1
                                    color: Theme.textFaint
                                    Layout.preferredWidth: 22
                                    font.features: { "tnum": 1 }
                                }
                                LSwitch {
                                    Layout.preferredWidth: 40
                                    checked: bandRow.band.enabled
                                    onToggled: bandRow.update(bandRow.band.type, bandRow.band.freq, bandRow.band.gain, bandRow.band.q, checked)
                                }
                                LComboBox {
                                    Layout.preferredWidth: 130
                                    model: Equalizer.filterTypes
                                    currentIndex: Equalizer.filterTypes.indexOf(bandRow.band.type)
                                    displayText: Equalizer.typeLabel(currentText)
                                    displayFor: (code) => Equalizer.typeLabel(code)
                                    onActivated: bandRow.update(currentText, bandRow.band.freq, bandRow.band.gain, bandRow.band.q, bandRow.band.enabled)
                                }
                                NumField {
                                    Layout.preferredWidth: 104
                                    value: bandRow.band.freq
                                    from: 10
                                    to: 24000
                                    decimals: 1
                                    suffix: "Hz"
                                    onEdited: (v) => bandRow.update(bandRow.band.type, v, bandRow.band.gain, bandRow.band.q, bandRow.band.enabled)
                                }
                                NumField {
                                    Layout.preferredWidth: 90
                                    enabled: bandRow.hasGain
                                    opacity: enabled ? 1 : 0.4
                                    value: bandRow.band.gain
                                    from: -24
                                    to: 24
                                    decimals: 1
                                    suffix: "dB"
                                    onEdited: (v) => bandRow.update(bandRow.band.type, bandRow.band.freq, v, bandRow.band.q, bandRow.band.enabled)
                                }
                                NumField {
                                    Layout.preferredWidth: 80
                                    value: bandRow.band.q
                                    from: 0.05
                                    to: 30
                                    decimals: 2
                                    onEdited: (v) => bandRow.update(bandRow.band.type, bandRow.band.freq, bandRow.band.gain, v, bandRow.band.enabled)
                                }
                                Item { Layout.fillWidth: true }
                                IconButton {
                                    iconName: "trash"
                                    iconSize: 15
                                    tip: qsTr("Remove band")
                                    onClicked: Equalizer.removeBand(bandRow.index)
                                }
                            }
                        }
                        Label {
                            visible: Equalizer.bands.length === 0
                            text: qsTr("No bands. Add one, import a profile or pick an AutoEQ correction below.")
                            color: Theme.textDim
                        }
                    }
                }
            }

            // --- AutoEQ -------------------------------------------------------------
            Card {
                enabled: Equalizer.enabled
                opacity: enabled ? 1 : 0.45
                Layout.fillWidth: true
                implicitHeight: autoEqColumn.implicitHeight + 32

                Timer {
                    id: searchDelay
                    interval: 250
                    onTriggered: Equalizer.autoEqSearch(autoEqSearch.text)
                }

                ColumnLayout {
                    id: autoEqColumn
                    anchors.fill: parent
                    anchors.margins: 16
                    spacing: 10

                    RowLayout {
                        spacing: 10
                        Icon { glyph: "headphones"; size: 22; color: Theme.accent }
                        Label {
                            text: qsTr("AutoEQ headphone correction")
                            color: Theme.text
                            font.pixelSize: Theme.fontTitle
                            font.weight: Font.DemiBold
                        }
                        Item { Layout.fillWidth: true }
                        BusyIndicator {
                            running: Equalizer.autoEqBusy
                            visible: running
                            implicitWidth: 22
                            implicitHeight: 22
                        }
                        Label {
                            text: Equalizer.autoEqStatus
                            color: Theme.textDim
                            font.pixelSize: Theme.fontSmall + 1
                            elide: Text.ElideRight
                            Layout.maximumWidth: 380
                        }
                    }
                    Label {
                        Layout.fillWidth: true
                        text: qsTr("Measurements of thousands of headphones and in-ear monitors. Find yours and apply its parametric correction to a neutral target — it is saved to “My profiles”.")
                        wrapMode: Text.Wrap
                        color: Theme.textDim
                        font.pixelSize: Theme.fontBody
                    }
                    RowLayout {
                        Layout.fillWidth: true
                        spacing: 8
                        SearchField {
                            id: autoEqSearch
                            Layout.fillWidth: true
                            placeholderText: qsTr("Search your headphones by brand or model")
                            onTextChanged: searchDelay.restart()
                        }
                        PillButton {
                            iconName: "refresh"
                            text: qsTr("Update database")
                            enabled: !Equalizer.autoEqBusy
                            onClicked: Equalizer.autoEqRefreshIndex()
                        }
                    }
                    ListView {
                        id: results
                        Layout.fillWidth: true
                        Layout.preferredHeight: Math.min(count * 46, 320)
                        visible: count > 0
                        clip: true
                        model: Equalizer.autoEqResults
                        boundsBehavior: Flickable.StopAtBounds
                        ScrollBar.vertical: LScrollBar {}
                        delegate: Rectangle {
                            id: result
                            required property int index
                            required property var modelData
                            width: results.width - 10
                            height: 46
                            radius: Theme.radiusSmall
                            color: resultHover.hovered ? Theme.hover : "transparent"
                            HoverHandler { id: resultHover }
                            RowLayout {
                                anchors.fill: parent
                                anchors.leftMargin: 10
                                anchors.rightMargin: 6
                                spacing: 10
                                Icon { glyph: "headphones"; size: 16; color: Theme.textDim }
                                ColumnLayout {
                                    Layout.fillWidth: true
                                    spacing: 1
                                    Label {
                                        Layout.fillWidth: true
                                        text: result.modelData.name
                                        elide: Text.ElideRight
                                        color: Theme.text
                                        font.weight: Font.Medium
                                    }
                                    Label {
                                        Layout.fillWidth: true
                                        text: qsTr("by %1").arg(result.modelData.source) + (result.modelData.rig ? " · " + result.modelData.rig : "")
                                        elide: Text.ElideRight
                                        color: Theme.textDim
                                        font.pixelSize: Theme.fontSmall
                                    }
                                }
                                PillButton {
                                    primary: resultHover.hovered
                                    text: qsTr("Apply")
                                    enabled: !Equalizer.autoEqBusy
                                    onClicked: Equalizer.autoEqApply(result.index)
                                }
                            }
                        }
                    }
                    Label {
                        visible: autoEqSearch.text.length > 1 && results.count === 0 && !Equalizer.autoEqBusy
                        text: qsTr("No headphones match “%1”").arg(autoEqSearch.text)
                        color: Theme.textDim
                    }
                }
            }
        }
    }
}
