pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import QtQuick.Shapes
import Sonkore

Item {
    id: page

    // Shown in the window's title bar.
    readonly property string pageIcon: "settings"
    readonly property bool titleCentered: true // no search here
    readonly property string pageTitle: qsTr("Settings")

    // The saved output is not on the system (a DAC unplugged...): playback
    // falls back to the system default, so that row shows as the one in use.
    readonly property bool savedDeviceMissing: DspController.currentDeviceKey.length > 0
        && !DspController.outputDevices.some(d => d.key === DspController.currentDeviceKey)
    readonly property string effectiveDeviceKey: savedDeviceMissing ? "" : DspController.currentDeviceKey

    // Devices grouped by backend for display.
    readonly property bool directSelected: DspController.outputDevices.some(
        d => d.direct && d.key === DspController.currentDeviceKey)
    readonly property var deviceGroups: {
        const groups = []
        for (const d of DspController.outputDevices) {
            let g = groups.find(x => x.backend === d.backend)
            if (!g) {
                g = { backend: d.backend, devices: [] }
                groups.push(g)
            }
            g.devices.push(d)
        }
        return groups
    }

    function backendHint(backend) {
        if (backend === "ALSA") return qsTr("Direct hardware (hw:) — exclusive, no mixer, bit-perfect capable. PipeWire must not be using the device.")
        if (backend === "PipeWire") return qsTr("The desktop's sound server, spoken to directly — shared with other apps; it switches to each track's sample rate (Follow the track's rate).")
        if (backend.indexOf("PipeWire") === 0) return qsTr("Desktop sound server — shared with other apps; sample rate follows PipeWire's clock settings.")
        if (backend === "WASAPI") return qsTr("Windows audio — exclusive mode bypasses the system mixer.")
        if (backend === "ASIO") return qsTr("The DAC's own ASIO driver — direct, bit-perfect capable, and native DSD where the driver offers it.")
        if (backend === "Core Audio") return qsTr("macOS — the device switches to the track's sample rate.")
        return ""
    }

    // The file dialogs (SettingsDialogs.qml), loaded the first time one opens.
    Loader {
        id: dialogs
        active: false
        source: "SettingsDialogs.qml"
        function open(which) {
            active = true
            item[which]()
        }
    }
    Connections {
        target: dialogs.item
        function onImpulseChosen(file) { DspController.loadConvolution(file) }
        function onSkipFolderChosen(folder) { Library.addExcludedFolder(folder) }
    }

    component SettingsCard: Card {
        id: card
        default property alias content: body.data
        property string title
        property string iconName
        property string subtitle
        // Folded behind its title until clicked.
        property bool collapsible: false
        property bool expanded: !collapsible
        property string tab // the tab it is shown in (page.tabs)
        visible: tab === page.tab
        Layout.fillWidth: true
        implicitHeight: inner.implicitHeight + 32
        ColumnLayout {
            id: inner
            anchors.fill: parent
            anchors.margins: 16
            spacing: 12
            RowLayout {
                Layout.fillWidth: true
                spacing: 10
                Icon { glyph: card.iconName; size: 20; color: Theme.accent }
                ColumnLayout {
                    Layout.fillWidth: true
                    spacing: 1
                    Label {
                        text: card.title
                        color: Theme.text
                        font.pixelSize: Theme.fontTitle
                        font.weight: Font.DemiBold
                    }
                    Label {
                        Layout.fillWidth: true
                        visible: card.subtitle.length > 0
                        text: card.subtitle
                        color: Theme.textDim
                        font.pixelSize: Theme.fontSmall + 1
                        wrapMode: Text.Wrap
                    }
                }
                Icon {
                    visible: card.collapsible
                    glyph: "chevron-down"
                    size: 16
                    color: Theme.textDim
                    rotation: card.expanded ? 180 : 0
                    Behavior on rotation { NumberAnimation { duration: 120 } }
                }
                TapHandler {
                    enabled: card.collapsible
                    onTapped: card.expanded = !card.expanded
                }
                HoverHandler { enabled: card.collapsible; cursorShape: Qt.PointingHandCursor }
            }
            ColumnLayout {
                id: body
                Layout.fillWidth: true
                visible: card.expanded
                spacing: 6
            }
        }
    }

    // One labelled setting: title + description on the left, control on the
    // right; in a narrow card the control goes underneath.
    component SettingRow: GridLayout {
        id: settingRow
        default property alias control: holder.data
        property string title
        property string description
        property string details // long explanation, behind the "i"
        readonly property bool stacked: width > 0 && width < 520
        Layout.fillWidth: true
        columns: stacked ? 1 : 2
        columnSpacing: 16
        rowSpacing: 8
        ColumnLayout {
            Layout.fillWidth: true
            spacing: 2
            Row {
                Layout.fillWidth: true
                spacing: 6
                Label {
                    id: settingRowTitle
                    width: Math.min(implicitWidth, parent.width - (settingRowInfo.visible ? settingRowInfo.width + 6 : 0))
                    text: settingRow.title
                    wrapMode: Text.Wrap
                    color: Theme.text
                    font.weight: Font.Medium
                }
                InfoTip {
                    id: settingRowInfo
                    y: Math.round((settingRowTitle.font.pixelSize * 1.35 - height) / 2)
                    visible: text.length > 0
                    text: settingRow.details
                }
            }
            Label {
                Layout.fillWidth: true
                visible: settingRow.description.length > 0
                text: settingRow.description
                color: Theme.textDim
                font.pixelSize: Theme.fontSmall + 1
                wrapMode: Text.Wrap
            }
        }
        Item {
            id: holder
            Layout.alignment: settingRow.stacked ? Qt.AlignLeft : (Qt.AlignRight | Qt.AlignVCenter)
            implicitWidth: childrenRect.width
            implicitHeight: childrenRect.height
        }
    }

    // A setting chosen from pictures: its title and description, then a tile
    // per option with a small drawing of what it looks like. `preview` is
    // created in each tile and gets the option's `value`.
    // Under a setting while the mini player is edited: "Same as the full
    // view", or, once changed for it, a link back to the full view's.
    component InheritNote: Label {
        id: note
        property string key
        readonly property bool own: Theme.isOwn(key)
        visible: key.length > 0 && Theme.editingCompact
        text: own ? qsTr("Use the full view's") : qsTr("Same as the full view")
        color: own ? Theme.accent : Theme.textFaint
        font.pixelSize: Theme.fontSmall
        font.underline: own && noteHover.hovered
        HoverHandler { id: noteHover; enabled: note.own; cursorShape: Qt.PointingHandCursor }
        TapHandler { enabled: note.own; onTapped: Theme.inherit(note.key) }
    }
    // Appearance: which view the settings below change.
    component ScopeSwitch: ColumnLayout {
        Layout.fillWidth: true
        spacing: 6
        RoundRect {
            implicitWidth: scopeRow.implicitWidth + 6
            implicitHeight: 34
            radius: Theme.radiusSmall + 3
            color: Theme.surfaceHi
            borderWidth: 1
            borderColor: Theme.border
            Row {
                id: scopeRow
                anchors.centerIn: parent
                spacing: 2
                Repeater {
                    model: [{ compact: false, text: qsTr("Full view"), icon: "expand" },
                            { compact: true, text: qsTr("Mini player"), icon: "mini-player" }]
                    AbstractButton {
                        id: scopeButton
                        required property var modelData
                        readonly property bool on: Theme.editingCompact === modelData.compact
                        width: scopeContent.implicitWidth + 24
                        height: 28
                        hoverEnabled: true
                        focusPolicy: Qt.NoFocus
                        onClicked: Theme.editingCompact = modelData.compact
                        background: RoundRect {
                            radius: Theme.radiusSmall + 1
                            color: scopeButton.on ? Theme.accentSoft : scopeButton.hovered ? Theme.hover : "transparent"
                            borderWidth: scopeButton.on ? 1 : 0
                            borderColor: Theme.accent
                        }
                        contentItem: Item {
                            Row {
                                id: scopeContent
                                anchors.centerIn: parent
                                spacing: 7
                                Icon {
                                    anchors.verticalCenter: parent.verticalCenter
                                    glyph: scopeButton.modelData.icon
                                    size: 14
                                    color: scopeButton.on ? Theme.accent : Theme.textDim
                                }
                                Label {
                                    anchors.verticalCenter: parent.verticalCenter
                                    text: scopeButton.modelData.text
                                    color: scopeButton.on ? Theme.accent : Theme.textDim
                                    font.weight: scopeButton.on ? Font.DemiBold : Font.Normal
                                }
                            }
                        }
                    }
                }
            }
        }
        Label {
            Layout.fillWidth: true
            text: Theme.editingCompact
                  ? qsTr("The mini player uses the full view's appearance, except what you change here for it.")
                  : qsTr("Changes here apply to the full view, and to the mini player where it has no setting of its own.")
            wrapMode: Text.Wrap
            color: Theme.textDim
            font.pixelSize: Theme.fontSmall + 1
        }
    }

    component VisualPicker: ColumnLayout {
        id: picker
        property string title
        property string description
        property string details
        property var options: []        // [{ value, text }]
        // A Theme setting (Theme.compactKeys): read and written for the view
        // being edited (full view or mini player).
        property string key
        property var current: key.length > 0 ? Theme.edited(key) : undefined
        property Component preview
        property int tileWidth: 112
        property int tileHeight: 86
        signal picked(var value)
        onPicked: (value) => { if (key.length > 0) Theme.edit(key, value) }
        Layout.fillWidth: true
        Layout.topMargin: 6
        spacing: 8
        opacity: enabled ? 1 : 0.5
        ColumnLayout {
            Layout.fillWidth: true
            spacing: 2
            Row {
                Layout.fillWidth: true
                spacing: 6
                Label {
                    id: pickerTitle
                    width: Math.min(implicitWidth, parent.width - (pickerInfo.visible ? pickerInfo.width + 6 : 0))
                    text: picker.title
                    wrapMode: Text.Wrap
                    color: Theme.text
                    font.weight: Font.Medium
                }
                InfoTip {
                    id: pickerInfo
                    y: Math.round((pickerTitle.font.pixelSize * 1.35 - height) / 2)
                    visible: text.length > 0
                    text: picker.details
                }
            }
            // Editing the mini player: inherited, or its own (back with a click).
            InheritNote { key: picker.key }
            Label {
                Layout.fillWidth: true
                visible: picker.description.length > 0
                text: picker.description
                color: Theme.textDim
                font.pixelSize: Theme.fontSmall + 1
                wrapMode: Text.Wrap
            }
        }
        Flow {
            Layout.fillWidth: true
            spacing: 8
            Repeater {
                model: picker.options
                delegate: AbstractButton {
                    id: tile
                    required property var modelData
                    readonly property bool selected: picker.current === modelData.value
                    width: picker.tileWidth
                    height: picker.tileHeight
                    hoverEnabled: true
                    focusPolicy: Qt.NoFocus
                    onClicked: picker.picked(modelData.value)
                    background: RoundRect {
                        radius: Theme.radiusSmall + 2
                        color: tile.selected ? Theme.accentSoft : Theme.surfaceHi
                        borderWidth: tile.selected ? 2 : 1
                        borderColor: tile.selected ? Theme.accent : (tile.hovered ? Theme.textFaint : Theme.border)
                    }
                    contentItem: Item {
                        Item { // the drawing, centred above the label
                            anchors.top: parent.top
                            anchors.topMargin: 8
                            anchors.bottom: tileLabel.top
                            anchors.bottomMargin: 4
                            anchors.left: parent.left
                            anchors.right: parent.right
                            Loader {
                                anchors.centerIn: parent
                                sourceComponent: picker.preview
                                onLoaded: {
                                    item.value = Qt.binding(() => tile.modelData.value)
                                    if ("hovered" in item) // previews that come alive under the mouse
                                        item.hovered = Qt.binding(() => tile.hovered)
                                }
                            }
                        }
                        Label {
                            id: tileLabel
                            anchors.bottom: parent.bottom
                            anchors.bottomMargin: 7
                            anchors.horizontalCenter: parent.horizontalCenter
                            width: parent.width - 10
                            horizontalAlignment: Text.AlignHCenter
                            elide: Text.ElideRight
                            text: tile.modelData.text
                            color: tile.selected ? Theme.text : Theme.textDim
                            font.pixelSize: Theme.fontSmall + 1
                            font.weight: tile.selected ? Font.DemiBold : Font.Normal
                        }
                    }
                }
            }
        }
    }

    // --- Previews for the pictures above (each gets `value`) ----------------
    // The window's background: the skin's colour with the accent's soft
    // glows or wash, as drawn by qml/shaders/backdrop.frag.
    component BackdropPreview: RoundRect {
        id: bp
        property var value
        readonly property color a1: Theme.accent
        readonly property color a2: Qt.hsla((a1.hslHue + 0.11) % 1, Math.max(0.4, a1.hslSaturation), Math.max(0.35, a1.hslLightness), 1)
        readonly property color a3: Qt.hsla((a1.hslHue + 0.86) % 1, Math.max(0.4, a1.hslSaturation), Math.max(0.35, a1.hslLightness), 1)
        readonly property real k: Theme.dark ? 1 : 0.7
        // [x, y, radius (of the longer side), colour, strength]
        readonly property var glows: value === "aurora" ? [[0.12, 0.2, 0.7, a1, 0.34], [0.8, 0.86, 0.65, a2, 0.28], [0.56, 0.09, 0.45, a3, 0.2]]
                                   : value === "glow" ? [[0.5, 1.05, 0.6, a1, 0.32], [0, 0, 0.4, a1, 0.1]] : []
        width: 84
        height: 48
        radius: 6
        color: Theme.solidBg
        borderWidth: 1
        borderColor: Theme.border
        clip: true
        Rectangle {
            anchors.fill: parent
            anchors.margins: 1
            radius: parent.radius - 1
            visible: bp.value === "gradient"
            gradient: Gradient {
                orientation: Gradient.Horizontal
                GradientStop { position: 0; color: Qt.rgba(bp.a1.r, bp.a1.g, bp.a1.b, 0.3 * bp.k) }
                GradientStop { position: 0.55; color: Qt.rgba(bp.a2.r, bp.a2.g, bp.a2.b, 0.1 * bp.k) }
                GradientStop { position: 1; color: Qt.rgba(bp.a3.r, bp.a3.g, bp.a3.b, 0.22 * bp.k) }
            }
        }
        Repeater {
            model: bp.glows
            Shape {
                id: glowShape
                required property var modelData
                anchors.fill: parent
                ShapePath {
                    strokeWidth: -1
                    fillGradient: RadialGradient {
                        centerX: glowShape.modelData[0] * bp.width
                        centerY: glowShape.modelData[1] * bp.height
                        centerRadius: glowShape.modelData[2] * Math.max(bp.width, bp.height)
                        focalX: centerX
                        focalY: centerY
                        GradientStop { position: 0; color: Qt.rgba(glowShape.modelData[3].r, glowShape.modelData[3].g, glowShape.modelData[3].b, glowShape.modelData[4] * bp.k) }
                        GradientStop { position: 1; color: "transparent" }
                    }
                    PathRectangle { width: bp.width; height: bp.height }
                }
            }
        }
    }

    // The player bar's spectrum, frozen.
    component VisualizerPreview: Item {
        id: vz
        property var value
        property bool hovered: false
        property real time: 0 // seconds of hover, drives the levels
        readonly property var still: [0.35, 0.6, 0.95, 0.7, 0.45, 0.8, 0.55, 0.3, 0.5, 0.25]
        // Under the mouse each bar rises and falls at its own pace, like music.
        readonly property var levels: hovered
            ? still.map((l, i) => 0.15 + 0.85 * l * (0.55 + 0.45 * Math.sin(time * (5 + (i * 1.7) % 4) + i)))
            : still
        FrameAnimation {
            running: vz.hovered && vz.value !== "off"
            onTriggered: vz.time += frameTime
        }
        width: 70
        height: 30
        Rectangle {
            visible: vz.value === "off"
            anchors.centerIn: parent
            width: parent.width
            height: 2
            radius: 1
            color: Theme.border
        }
        // The visualizer's own drawing, with fixed levels.
        SpectrumView {
            anchors.fill: parent
            visible: vz.value !== "off"
            style: vz.value
            color: Theme.accent
            levels: vz.levels
            barFraction: 4 / 7
        }
    }

    // A window corner at its real radius.
    component CornerPreview: RoundRect {
        property var value
        width: 64
        height: 44
        radius: value
        color: Theme.solidBg
        borderWidth: 2
        borderColor: Theme.textDim
    }

    // A small window on a wallpaper, with the outline as the app draws it.
    component OutlinePreview: Rectangle {
        id: op
        property var value
        width: 88
        height: 54
        radius: 4
        gradient: Gradient {
            GradientStop { position: 0; color: Theme.dark ? "#4a5568" : "#c3cfe2" }
            GradientStop { position: 1; color: Theme.dark ? "#1a202c" : "#8fa3c4" }
        }
        Rectangle {
            id: mini
            anchors.centerIn: parent
            width: 64
            height: 38
            radius: 6
            color: Theme.solidBg
            WindowOutline { anchors.fill: parent; style: op.value; radius: mini.radius }
        }
    }

    // The Tracks page: the list alone, or three filter columns above it.
    component BrowserPreview: RoundRect {
        id: bp
        property var value
        width: 84
        height: 48
        radius: 5
        color: Theme.solidBg
        borderWidth: 1
        borderColor: Theme.border
        Row { // Genre · Artist · Album
            visible: bp.value === true
            x: 5
            y: 5
            spacing: 3
            Repeater {
                model: 3
                Rectangle {
                    id: pane
                    required property int index
                    width: 22
                    height: 17
                    radius: 2
                    color: Theme.surfaceHi
                    Column {
                        x: 3
                        y: 3
                        spacing: 2
                        Repeater {
                            model: 3
                            Rectangle {
                                required property int index
                                width: 14 - index * 3
                                height: 2
                                radius: 1
                                color: index === 0 && pane.index === 0 ? Theme.accent : Theme.textFaint
                            }
                        }
                    }
                }
            }
        }
        Column { // the tracks
            x: 6
            y: bp.value === true ? 27 : 7
            spacing: 3
            Repeater {
                model: bp.value === true ? 3 : 6
                Rectangle {
                    required property int index
                    width: 62 - (index % 2) * 12
                    height: 3
                    radius: 1.5
                    color: Theme.textFaint
                }
            }
        }
    }

    // A window corner with the three buttons in that style.
    // Seek and volume bars: a seek bar over a short volume bar, at rest.
    // Progress bar: a seek bar (the waveform a made-up one).
    component SliderStylePreview: RoundRect {
        id: ssp
        property var value
        width: 92
        height: 44
        radius: 5
        color: Theme.solidBg
        borderWidth: 1
        borderColor: Theme.border
        LSlider {
            anchors.centerIn: parent
            width: 76
            seekBar: true
            sliderStyle: ssp.value
            peaks: Array.from({ length: 60 }, (_, i) => 0.25 + 0.75 * Math.abs(Math.sin(i * 0.7) * Math.cos(i * 0.23)))
            enabled: false
            opacity: 1
            value: 0.45
        }
    }

    // Volume bar: the speaker and a short bar.
    component VolumeStylePreview: RoundRect {
        id: vsp
        property var value
        width: 92
        height: 44
        radius: 5
        color: Theme.solidBg
        borderWidth: 1
        borderColor: Theme.border
        Row {
            anchors.centerIn: parent
            spacing: 6
            Icon {
                anchors.verticalCenter: parent.verticalCenter
                glyph: "volume"
                iconStyle: Theme.edited("iconStyle")
                size: 13
                color: Theme.textDim
            }
            LSlider {
                anchors.verticalCenter: parent.verticalCenter
                width: 54
                sliderStyle: vsp.value
                enabled: false
                opacity: 1
                value: 0.7
            }
        }
    }

    // Previous, play and next, the play button in that shape, the glyphs
    // (and tiles) in the icon style chosen above.
    component TransportStylePreview: RoundRect {
        id: tsp
        property var value
        readonly property bool round: value === "round"
        readonly property string iconStyle: Theme.edited("iconStyle")
        readonly property bool tiles: iconStyle === "tile"
        width: 92
        height: 44
        radius: 5
        color: Theme.solidBg
        borderWidth: 1
        borderColor: Theme.border
        Row {
            anchors.centerIn: parent
            spacing: 4
            Repeater {
                model: ["prev", "play", "next"]
                Item {
                    id: cell
                    required property string modelData
                    readonly property bool main: modelData === "play"
                    anchors.verticalCenter: parent.verticalCenter
                    // the play button's own shape, else a tile or nothing
                    readonly property bool shaped: main && tsp.value !== "sonkore"
                    width: shaped || (main && tsp.tiles) ? 22 : tsp.tiles ? 19 : 16
                    height: width
                    RoundRect { // a tile (IconButton)
                        visible: tsp.tiles && !cell.shaped
                        anchors.fill: parent
                        radius: 5
                        color: Theme.surfaceHi
                    }
                    RoundRect { // as PlayButton draws it
                        visible: cell.main && tsp.value !== "sonkore"
                        anchors.fill: parent
                        radius: width / 2
                        color: tsp.round ? (Theme.dark ? Theme.text : Theme.accent) : "transparent"
                        borderWidth: tsp.value === "outline" ? 1 : 0
                        borderColor: Theme.text
                    }
                    Icon {
                        anchors.centerIn: parent
                        // the triangle's weight, centred as in PlayButton
                        anchors.horizontalCenterOffset: cell.main ? (tsp.iconStyle === "line" || tsp.iconStyle === "tile" ? 0 : Math.round(size * 0.08)) : 0
                        glyph: cell.modelData
                        iconStyle: tsp.iconStyle
                        size: cell.main && tsp.value === "sonkore" ? (tsp.tiles ? 13 : 16) : 11
                        color: tsp.round && cell.main ? (Theme.dark ? Theme.solidBg : Theme.onAccent) : Theme.text
                    }
                }
            }
        }
    }

    component WindowButtonsPreview: RoundRect {
        id: wbp
        property var value
        width: 92
        height: 44
        radius: 5
        color: Theme.solidBg
        borderWidth: 1
        borderColor: Theme.border
        clip: true
        Row {
            anchors.top: parent.top
            anchors.right: parent.right
            anchors.topMargin: wbp.value === "windows" ? 1 : 4
            anchors.rightMargin: wbp.value === "windows" ? 1 : 5
            spacing: wbp.value === "mac" ? 1 : 0
            scale: 0.68
            transformOrigin: Item.TopRight
            Repeater {
                model: ["minimize", "maximize", "close"]
                TitleBarButton {
                    required property string modelData
                    style: wbp.value
                    glyph: modelData === "minimize" ? "win-min" : modelData === "close" ? "close" : "win-max"
                    danger: modelData === "close"
                    enabled: false
                }
            }
        }
        Column { // a page under the bar
            x: 8
            y: 24
            spacing: 4
            Rectangle { width: 46; height: 3; radius: 1.5; color: Theme.textFaint }
            Rectangle { width: 34; height: 3; radius: 1.5; color: Theme.textFaint }
        }
    }

    // A few icons of the bar and the sidebar in that style.
    component IconStylePreview: RoundRect {
        id: isp
        property var value
        readonly property bool tiles: value === "tile"
        width: 92
        height: 50
        radius: 5
        color: Theme.solidBg
        borderWidth: 1
        borderColor: Theme.border
        Row {
            anchors.centerIn: parent
            spacing: isp.tiles ? 4 : 7
            Repeater {
                model: ["music", "folder", "eq", "settings"]
                Rectangle {
                    id: cell
                    required property string modelData
                    required property int index
                    width: isp.tiles ? 19 : 14
                    height: width
                    radius: 5
                    color: !isp.tiles ? "transparent" : cell.index === 2 ? Theme.accentSoft : Theme.surfaceHi
                    Icon {
                        anchors.centerIn: parent
                        glyph: cell.modelData
                        iconStyle: isp.value
                        size: isp.tiles ? 12 : 14
                        color: cell.index === 2 ? Theme.accent : Theme.text
                    }
                }
            }
        }
    }

    // Buttons in a bar: roomy or tight.
    component DensityPreview: RoundRect {
        id: dp
        property var value
        readonly property bool compact: value === "compact"
        readonly property bool spacious: value === "spacious"
        width: 84
        height: 44
        radius: 5
        color: Theme.solidBg
        borderWidth: 1
        borderColor: Theme.border
        Rectangle { // a bar
            anchors.bottom: parent.bottom
            anchors.bottomMargin: 1
            x: 1
            width: parent.width - 2
            height: dp.compact ? 15 : dp.spacious ? 28 : 22
            bottomLeftRadius: 4
            bottomRightRadius: 4
            color: Theme.skin.sidebar
            Row {
                anchors.centerIn: parent
                spacing: dp.compact ? 4 : dp.spacious ? 8 : 7
                Repeater {
                    model: 4
                    RoundRect {
                        required property int index
                        width: dp.compact ? 8 : dp.spacious ? 14 : 12
                        height: width
                        radius: width / 2
                        color: index === 1 ? Theme.accent : Theme.textFaint
                    }
                }
            }
        }
    }

    // Rows of a track list as set up: density (`density`), covers and
    // alternating colours. The three list pickers each vary one of them and
    // draw the other two as they are now, so every preview follows the others.
    component ListPreview: RoundRect {
        id: lp
        property var value
        property string density: "comfortable"
        property bool covers: true
        property bool alternate: false
        readonly property bool roomy: density === "spacious" || density === "comfortable"
        readonly property bool showCovers: covers && roomy
        readonly property int rows: density === "list" ? 7 : density === "compact" ? 5 : density === "spacious" ? 3 : 4
        readonly property real rowHeight: density === "list" ? 5 : density === "compact" ? 7 : density === "spacious" ? 13 : 9
        width: 84
        height: 48
        radius: 5
        color: Theme.solidBg
        borderWidth: 1
        borderColor: Theme.border
        clip: true
        Column {
            x: 3
            y: 4
            width: parent.width - 6
            Repeater {
                model: lp.rows
                Rectangle {
                    id: listRow
                    required property int index
                    width: parent.width
                    height: lp.rowHeight + (lp.density === "list" ? 0.5 : 1.5)
                    radius: 2
                    // alternating rows, stronger than in the app so the tile shows it
                    color: lp.alternate && listRow.index % 2 === 1 ? Qt.rgba(Theme.text.r, Theme.text.g, Theme.text.b, 0.14) : "transparent"
                    Row {
                        x: 3
                        anchors.verticalCenter: parent.verticalCenter
                        spacing: 4
                        Rectangle { // cover
                            visible: lp.showCovers
                            anchors.verticalCenter: parent.verticalCenter
                            width: lp.rowHeight
                            height: lp.rowHeight
                            radius: 2
                            color: listRow.index === 0 ? Theme.accent : Theme.textFaint
                            opacity: listRow.index === 0 ? 0.9 : 0.5
                        }
                        Rectangle {
                            anchors.verticalCenter: parent.verticalCenter
                            width: (lp.showCovers ? 56 - lp.rowHeight : 70) - (listRow.index % 2) * 10
                            height: lp.density === "list" ? 2 : 3
                            radius: 1
                            color: listRow.index === 0 ? Theme.accent : Theme.textFaint
                        }
                    }
                }
            }
        }
    }
    component ListDensityPreview: ListPreview {
        density: value
        covers: Theme.edited("listCovers")
        alternate: Theme.edited("alternateRows")
    }
    component ListCoversPreview: ListPreview {
        density: Theme.editedListDensity === "compact" || Theme.editedListDensity === "list" ? "comfortable" : Theme.editedListDensity
        covers: value === true
        alternate: Theme.edited("alternateRows")
    }
    component AlternateRowsPreview: ListPreview {
        density: Theme.editedListDensity
        covers: Theme.edited("listCovers") && Theme.editedListDensity !== "compact" && Theme.editedListDensity !== "list"
        alternate: value === true
    }

    // Shown next to settings that only apply after a restart.
    // Play / Stop of one test signal row (Settings › Output › Test signals).
    component TestButton: PillButton {
        property string row
        readonly property bool on: testCard.anyRunning && testCard.running === row
        primary: on
        iconName: on ? "stop" : "play"
        text: on ? qsTr("Stop") : qsTr("Play")
    }

    component RestartButton: PillButton {
        property bool needed: App.restartRequired
        visible: needed
        primary: true
        iconName: "refresh"
        text: qsTr("Restart")
        onClicked: App.restart()
    }

    component Segmented: Row {
        id: seg
        property var options: []      // [{ value, text }]
        property var current
        signal picked(var value)
        spacing: 6
        Repeater {
            model: seg.options
            delegate: AbstractButton {
                required property var modelData
                required property int index
                readonly property bool active: seg.current === modelData.value
                implicitWidth: segLabel.implicitWidth + 24
                implicitHeight: 30
                onClicked: seg.picked(modelData.value)
                background: RoundRect {
                    radius: Theme.radiusSmall
                    color: parent.active ? Theme.accent : (parent.hovered ? Theme.hover : Theme.surfaceHi)
                    borderWidth: 1
                    borderColor: parent.active ? Theme.accent : Theme.border
                }
                contentItem: Label {
                    id: segLabel
                    text: parent.modelData.text
                    color: parent.active ? Theme.onAccent : Theme.text
                    horizontalAlignment: Text.AlignHCenter
                    verticalAlignment: Text.AlignVCenter
                    font.pixelSize: Theme.fontBody
                    font.weight: parent.active ? Font.DemiBold : Font.Normal
                }
            }
        }
    }

    // --- Tabs ---------------------------------------------------------------
    readonly property var tabs: [
        { id: "general", text: qsTr("General"), icon: "globe" },
        { id: "appearance", text: qsTr("Appearance"), icon: "palette" },
        { id: "audio", text: qsTr("Audio"), icon: "speaker" },
        { id: "library", text: qsTr("Library"), icon: "folder" }]
    // The tab last open, kept between sessions.
    readonly property string tab: tabs.some(t => t.id === Prefs.store.settingsTab) ? Prefs.store.settingsTab : "general"
    onTabChanged: pageFlick.contentY = 0

    // The tabs share the width of the page: never wider than the window;
    // their icons go when there is no room for them. A segmented control:
    // one track, the open tab a raised piece on it.
    RoundRect {
        id: tabTrack
        anchors.top: parent.top
        anchors.topMargin: 8 // right under the window's title bar
        x: column.x
        width: column.width
        height: tabStrip.height + 8
        radius: Theme.radiusSmall + 4
        color: Theme.surface
        borderWidth: 1
        borderColor: Theme.border
    }
    RowLayout {
        id: tabStrip
        anchors.top: tabTrack.top
        anchors.topMargin: 4
        x: tabTrack.x + 4
        width: tabTrack.width - 8
        spacing: 4
        // All tabs show their icon, or none: room for the longest label.
        readonly property bool roomForIcons: {
            let widest = 0
            for (const label of labelMeasure.children)
                widest = Math.max(widest, label.implicitWidth)
            return (width - (page.tabs.length - 1) * spacing) / page.tabs.length >= widest + 16 + 7 + 40
        }
        Row {
            id: labelMeasure
            visible: false
            Repeater {
                model: page.tabs
                Label { required property var modelData; text: modelData.text; font.weight: Font.DemiBold }
            }
        }
        Repeater {
            model: page.tabs
            delegate: AbstractButton {
                id: tabButton
                required property var modelData
                readonly property bool active: page.tab === modelData.id
                Layout.fillWidth: true
                Layout.preferredWidth: 1 // equal shares
                implicitHeight: 34
                hoverEnabled: true
                focusPolicy: Qt.NoFocus
                onClicked: Prefs.store.settingsTab = modelData.id
                background: RoundRect {
                    radius: Theme.radiusSmall + 1
                    color: tabButton.active ? Theme.hover
                         : tabButton.hovered ? Qt.rgba(Theme.text.r, Theme.text.g, Theme.text.b, 0.04) : "transparent"
                    borderWidth: tabButton.active ? 1 : 0
                    borderColor: Qt.rgba(Theme.text.r, Theme.text.g, Theme.text.b, Theme.dark ? 0.08 : 0.12)
                    Behavior on color { ColorAnimation { duration: 120 } }
                }
                contentItem: Item {
                    Row {
                        anchors.centerIn: parent
                        width: Math.min(implicitWidth, tabButton.width - 12)
                        spacing: 7
                        Icon {
                            anchors.verticalCenter: parent.verticalCenter
                            visible: tabStrip.roomForIcons
                            glyph: tabButton.modelData.icon
                            size: 16
                            color: tabButton.active ? Theme.accent : Theme.textDim
                        }
                        Label {
                            anchors.verticalCenter: parent.verticalCenter
                            width: Math.min(implicitWidth, tabButton.width - 12)
                            elide: Text.ElideRight
                            text: tabButton.modelData.text
                            color: tabButton.active ? Theme.text : Theme.textDim
                            font.weight: tabButton.active ? Font.DemiBold : Font.Normal
                        }
                    }
                }
            }
        }
    }
    Rectangle { // divider under the tabs
        anchors.top: tabTrack.bottom
        anchors.topMargin: 8
        x: column.x
        width: column.width
        height: 1
        color: Theme.border
    }

    Flickable {
        id: pageFlick
        WheelScroll { flickable: pageFlick }
        anchors.top: tabTrack.bottom
        anchors.topMargin: 9 // under the divider
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        contentHeight: column.y + column.implicitHeight + 20
        boundsBehavior: Flickable.StopAtBounds
        clip: true
        ScrollBar.vertical: LScrollBar {}

        ColumnLayout {
            id: column
            // Centred in the page, at a comfortable reading width.
            width: Math.min(parent.width - 56, 900)
            x: Math.round((parent.width - width) / 2)
            y: 14
            spacing: 16

            // Restart banner (scale / renderer changes)
            RoundRect {
                Layout.fillWidth: true
                visible: App.restartRequired
                implicitHeight: restartRow.implicitHeight + 20
                radius: Theme.radius
                color: Theme.accentSoft
                borderWidth: 1
                borderColor: Theme.accent
                RowLayout {
                    id: restartRow
                    anchors.fill: parent
                    anchors.margins: 10
                    anchors.leftMargin: 16
                    Label {
                        Layout.fillWidth: true
                        text: qsTr("Restart Sonkore to apply the new interface scale / renderer.")
                        color: Theme.text
                        wrapMode: Text.Wrap
                    }
                    PillButton { primary: true; iconName: "refresh"; text: qsTr("Restart now"); onClicked: App.restart() }
                }
            }

            // --- General ------------------------------------------------------
            SettingsCard {
                tab: "general"
                title: qsTr("General")
                iconName: "globe"
                SettingRow {
                    title: qsTr("Interface language")
                    LComboBox {
                        implicitWidth: 220
                        readonly property var entries: [{ code: "", name: qsTr("Automatic (system)") }].concat(App.languages)
                        model: entries.map(e => e.name)
                        currentIndex: Math.max(0, entries.findIndex(e => e.code === App.language))
                        onActivated: (index) => App.language = entries[index].code
                    }
                }
                SettingRow {
                    title: qsTr("Font")
                    LComboBox {
                        implicitWidth: 220
                        readonly property var entries: [{ id: "", name: qsTr("System") }].concat(App.fonts)
                        model: entries.map(e => e.name)
                        currentIndex: Math.max(0, entries.findIndex(e => e.id === App.font))
                        onActivated: (index) => App.font = entries[index].id
                    }
                }
                SettingRow {
                    title: qsTr("Interface scale")
                    description: qsTr("Needs a restart.")
                    Row {
                        spacing: 8
                        LComboBox {
                            implicitWidth: 140
                            model: [0.75, 0.85, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0]
                            currentIndex: Math.max(0, model.indexOf(App.uiScale))
                            displayText: Math.round(currentValue * 100) + " %"
                            displayFor: (v) => Math.round(v * 100) + " %"
                            onActivated: App.uiScale = currentValue
                        }
                        RestartButton {}
                    }
                }
                SettingRow {
                    title: qsTr("Resume last session")
                    LSwitch { checked: DspController.resumeSession; onToggled: DspController.resumeSession = checked }
                }
                SettingRow {
                    title: qsTr("Live seeking")
                    description: qsTr("Hear the audio while dragging the bar.")
                    LSwitch { checked: Prefs.store.liveSeek; onToggled: Prefs.store.liveSeek = checked }
                }
                SettingRow {
                    title: qsTr("Stop button")
                    description: qsTr("A stop button next to play in the player bars.")
                    LSwitch { checked: Prefs.store.stopButton; onToggled: Prefs.store.stopButton = checked }
                }
                SettingRow {
                    title: qsTr("Search bar")
                    details: qsTr("Ctrl + F shows it even when it is off.")
                    LSwitch { checked: Prefs.store.searchBar; onToggled: Prefs.store.searchBar = checked }
                }
                SettingRow {
                    title: qsTr("Start page")
                    details: qsTr("What Sonkore shows when it opens. Last place: the folder you had open, or the playing track's.")
                    LComboBox {
                        implicitWidth: 160
                        readonly property var pages: ["last", "tracks", "favorites", "albums", "artists", "genres", "folders", "queue"]
                        model: [qsTr("Last place"), qsTr("Tracks"), qsTr("Favorites"), qsTr("Albums"), qsTr("Artists"), qsTr("Genres"), qsTr("Folders"), qsTr("Play queue")]
                        currentIndex: Math.max(0, pages.indexOf(Prefs.store.startPage))
                        onActivated: (index) => Prefs.store.startPage = pages[index]
                    }
                }
                VisualPicker {
                    title: qsTr("Filters on the Tracks page")
                    options: [{ value: false, text: qsTr("List only") }, { value: true, text: qsTr("With filters") }]
                    current: Prefs.store.trackBrowser
                    onPicked: (v) => Prefs.store.trackBrowser = v
                    preview: Component { BrowserPreview {} }
                }
                SettingRow {
                    title: qsTr("Mouse wheel speed")
                    Segmented {
                        options: [{ value: 100, text: qsTr("Normal") }, { value: 180, text: qsTr("Fast") },
                                  { value: 300, text: qsTr("Very fast") }]
                        current: Prefs.store.wheelStep
                        onPicked: (v) => Prefs.store.wheelStep = v
                    }
                }
            }

            // --- Desktop integration -------------------------------------------------
            SettingsCard {
                tab: "general"
                title: qsTr("Desktop integration")
                iconName: "chip"
                SettingRow {
                    title: qsTr("Tray icon")
                    details: qsTr("Play / pause, next and previous from the panel (some desktops need a tray extension).")
                    LSwitch { checked: Prefs.store.trayIcon; onToggled: Prefs.store.trayIcon = checked }
                }
                SettingRow {
                    title: qsTr("Close to tray")
                    enabled: Prefs.store.trayIcon
                    opacity: enabled ? 1 : 0.5
                    LSwitch { checked: Prefs.store.closeToTray; onToggled: Prefs.store.closeToTray = checked }
                }
                SettingRow {
                    visible: App.mprisAvailable
                    title: qsTr("System media controls (MPRIS)")
                    details: qsTr("The desktop's media controls, the lock screen, the dock, media keys and playerctl control Sonkore. Off: they no longer see it, and opening a file while Sonkore is running starts a second window.")
                    LSwitch { checked: App.mprisEnabled; onToggled: App.mprisEnabled = checked }
                }
                SettingRow {
                    visible: App.platform === "windows"
                    title: qsTr("Taskbar buttons")
                    details: qsTr("Previous / play / next in the taskbar thumbnail and the track progress on the taskbar button.")
                    Badge { text: qsTr("ON"); tone: Theme.good }
                }
            }

            // --- Rendering ---------------------------------------------------------
            SettingsCard {
                tab: "general"
                title: qsTr("Rendering")
                iconName: "mini-player"
                SettingRow {
                    title: qsTr("Renderer")
                    details: qsTr("Software draws on the CPU and uses the least power; Vulkan or OpenGL use the graphics card and reach high refresh rates. If Sonkore fails to start with a renderer, it switches back on its own (or start it with --safe-mode).")
                    description: qsTr("Currently: %1.")
                                 .arg(["Unknown", "Software", "OpenVG", "OpenGL", "Direct3D 11", "Vulkan", "Metal", "Null", "Direct3D 12"][GraphicsInfo.api] || "?")
                    Row {
                        spacing: 8
                        LComboBox {
                            implicitWidth: 160
                            textRole: "text"
                            valueRole: "value"
                            model: [{ value: "auto", text: qsTr("Automatic") }, { value: "opengl", text: "OpenGL" },
                                    { value: "vulkan", text: "Vulkan" }, { value: "software", text: qsTr("Software") }]
                            currentIndex: Math.max(0, ["auto", "opengl", "vulkan", "software"].indexOf(App.renderer))
                            onActivated: App.renderer = currentValue
                        }
                        RestartButton {}
                    }
                }
                SettingRow {
                    title: qsTr("Threaded rendering")
                    description: App.renderer === "software"
                                 ? qsTr("Not used by the software renderer, which always draws on the main thread.")
                                 : qsTr("Smoother animations. Recommended.")
                    details: App.renderer === "software" ? ""
                             : qsTr("Renders on its own thread so animations follow the display refresh rate (144 Hz and up). Recommended.")
                    enabled: App.renderer !== "software"
                    opacity: enabled ? 1 : 0.5
                    LSwitch { checked: App.threadedRendering; onToggled: App.threadedRendering = checked }
                }
                SettingRow {
                    title: qsTr("Frame rate limit")
                    description: qsTr("Fewer frames use less CPU.")
                    LComboBox {
                        implicitWidth: 160
                        textRole: "text"
                        valueRole: "value"
                        model: [{ value: 30, text: "30 fps" }, { value: 60, text: "60 fps" },
                                { value: 120, text: "120 fps" }, { value: 0, text: qsTr("No limit") }]
                        currentIndex: Math.max(0, [30, 60, 120, 0].indexOf(App.frameLimit))
                        onActivated: App.frameLimit = currentValue
                    }
                }
                SettingRow {
                    title: qsTr("Show frame rate")
                    LSwitch { checked: Prefs.store.showFps; onToggled: Prefs.store.showFps = checked }
                }
            }

            // --- Shortcuts ---------------------------------------------------------
            SettingsCard {
                tab: "general"
                title: qsTr("Keyboard shortcuts")
                iconName: "info"
                // Two columns when they fit, one on narrow windows; long
                // descriptions wrap inside their column.
                GridLayout {
                    Layout.fillWidth: true
                    columns: width >= 640 ? 2 : 1
                    columnSpacing: 24
                    rowSpacing: 6
                    Repeater {
                        model: [
                            ["Space", qsTr("Play / pause")], ["Ctrl + → / ←", qsTr("Next / previous track")],
                            ["Ctrl + ↑ / ↓", qsTr("Volume")], ["Ctrl + F", qsTr("Search")],
                            ["Ctrl + A", qsTr("Select all tracks in a list")], ["Enter", qsTr("Play the selected track")],
                            ["Delete", qsTr("Remove from playlist / queue")], ["Ctrl + E", qsTr("Equalizer")],
                            ["Ctrl + ,", qsTr("Settings")], ["Ctrl + Q", qsTr("Quit")]
                        ]
                        delegate: RowLayout {
                            id: shortcutItem
                            required property var modelData
                            Layout.fillWidth: true
                            Layout.preferredWidth: 1 // equal columns
                            spacing: 10
                            Badge { text: shortcutItem.modelData[0]; tone: Theme.accent; Layout.preferredWidth: 110 }
                            Label {
                                Layout.fillWidth: true
                                text: shortcutItem.modelData[1]
                                color: Theme.textDim
                                wrapMode: Text.Wrap
                            }
                        }
                    }
                }
            }

            // --- About ----------------------------------------------------------
            SettingsCard {
                tab: "general"
                title: qsTr("About")
                iconName: "info"
                RowLayout {
                    spacing: 14
                    Image { source: "qrc:/icons/sonkore.svg"; sourceSize: Qt.size(48, 48) }
                    Label {
                        Layout.fillWidth: true
                        text: qsTr("Sonkore %1 — 32-bit integer bit-perfect path, 64-bit floating-point DSP.")
                              .arg(App.version) + "\n" + DspController.audioComponents
                        wrapMode: Text.Wrap
                        color: Theme.textDim
                    }
                }
            }

            // --- Reset -----------------------------------------------------------
            SettingsCard {
                tab: "general"
                title: qsTr("Restore defaults")
                iconName: "refresh"
                SettingRow {
                    title: qsTr("Restore all settings")
                    description: qsTr("Library, playlists and EQ profiles are kept.")
                    details: qsTr("Audio, equalizer, pitch, appearance and behaviour go back to how a new installation starts, which plays on any computer. The library, playlists, play queue and saved EQ profiles are kept. Sonkore restarts.")
                    PillButton { iconName: "refresh"; text: qsTr("Restore…"); onClicked: resetDialog.open() }
                }
                ConfirmDialog {
                    id: resetDialog
                    title: qsTr("Restore all settings?")
                    text: qsTr("Every setting goes back to its default and Sonkore restarts. Your library, playlists, play queue and saved EQ profiles are kept. This cannot be undone.")
                    acceptText: qsTr("Restore and restart")
                    onAccepted: App.restoreDefaults()
                }
            }

            // --- Appearance -----------------------------------------------------
            SettingsCard {
                tab: "appearance"
                title: qsTr("Appearance")
                iconName: "palette"

                ScopeSwitch {}
                SectionLabel { text: qsTr("Theme"); Layout.topMargin: 8 }
                InheritNote { key: "skinIndex" }
                // Every card has the same size, whatever the skin's name.
                Item {
                    id: skinGrid
                    Layout.fillWidth: true
                    readonly property int spacing: 12
                    readonly property int columns: Math.max(2, Math.floor((width + spacing) / (168 + spacing)))
                    readonly property real cardWidth: Math.floor((width - (columns - 1) * spacing) / columns)
                    readonly property real cardHeight: 136
                    implicitHeight: skinFlow.implicitHeight

                    Grid {
                        id: skinFlow
                        columns: skinGrid.columns
                        columnSpacing: skinGrid.spacing
                        rowSpacing: skinGrid.spacing

                        Repeater {
                            model: Theme.skinOrder.map(i => Object.assign({ skin: i }, Theme.skins[i]))
                            delegate: AbstractButton {
                                id: skinCard
                                required property var modelData // a skin, plus `skin`: its index in Theme.skins
                                readonly property bool selected: Theme.edited("skinIndex") === modelData.skin
                                width: skinGrid.cardWidth
                                height: skinGrid.cardHeight
                                hoverEnabled: true
                                onClicked: Theme.setSkin(skinCard.modelData.skin)
                                background: RoundRect {
                                    radius: 10
                                    color: Theme.surfaceHi
                                    borderWidth: skinCard.selected ? 2 : 1
                                    borderColor: skinCard.selected ? Theme.accent : (skinCard.hovered ? Theme.textFaint : Theme.border)
                                }
                                contentItem: Item {
                                    // miniature of the skin, laid out as the window: the frame
                                    // (top bar, sidebar, player bar) and the list's panel in it
                                    RoundRect {
                                        id: miniature
                                        readonly property var tones: Theme.store.invertedSkins.length >= 0 ? Theme.tones(skinCard.modelData) : ({})
                                        readonly property real side: Math.round(width * 0.28)
                                        x: 7
                                        y: 7
                                        width: parent.width - 14
                                        height: 74
                                        radius: Math.min(8, skinCard.modelData.radius)
                                        color: tones.frame
                                        borderWidth: 1
                                        borderColor: skinCard.modelData.border
                                        clip: true
                                        // sidebar entries
                                        Column {
                                            x: 7
                                            y: 12
                                            spacing: 5
                                            Repeater {
                                                model: 4
                                                Rectangle {
                                                    required property int index
                                                    width: Math.max(8, miniature.side - 14 - index * 3)
                                                    height: 3
                                                    radius: 1.5
                                                    color: index === 0 ? skinCard.modelData.accent : skinCard.modelData.textFaint
                                                }
                                            }
                                        }
                                        // the list's panel
                                        Rectangle {
                                            x: miniature.side
                                            y: 7
                                            width: miniature.width - x - 1
                                            height: miniature.height - y - 16
                                            topLeftRadius: 5
                                            color: miniature.tones.pane
                                            Column {
                                                x: 8
                                                y: 7
                                                spacing: 7
                                                Repeater {
                                                    model: 3
                                                    Row {
                                                        id: previewRow
                                                        required property int index
                                                        spacing: 5
                                                        Rectangle { width: 10; height: 10; radius: skinCard.modelData.radius / 4; color: skinCard.modelData.surfaceHi }
                                                        Rectangle {
                                                            width: Math.max(12, miniature.width - miniature.side - 34 - previewRow.index * 12)
                                                            height: 4
                                                            radius: 2
                                                            y: 3
                                                            color: previewRow.index === 0 ? skinCard.modelData.accent : skinCard.modelData.textDim
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        // player bar, a hairline over it
                                        Rectangle {
                                            anchors.bottom: parent.bottom
                                            width: parent.width
                                            height: 16
                                            color: miniature.tones.frame
                                            Rectangle { width: parent.width; height: 1; color: skinCard.modelData.border }
                                            RoundRect {
                                                anchors.centerIn: parent
                                                width: 8; height: 8; radius: 4
                                                color: skinCard.modelData.accent
                                            }
                                        }
                                    }
                                    // Swap the frame's and the panel's tones, for this skin.
                                    AbstractButton {
                                        id: invertButton
                                        readonly property bool on: Theme.store.invertedSkins.length >= 0 && Theme.isInverted(skinCard.modelData)
                                        anchors.right: parent.right
                                        anchors.rightMargin: 8
                                        anchors.top: miniature.bottom
                                        anchors.topMargin: 8
                                        width: 26
                                        height: 26
                                        hoverEnabled: true
                                        focusPolicy: Qt.NoFocus
                                        onClicked: Theme.toggleInverted(skinCard.modelData)
                                        ToolTip.visible: hovered
                                        ToolTip.delay: 400
                                        ToolTip.text: qsTr("Invert panel colours")
                                        background: Rectangle {
                                            radius: 6
                                            color: invertButton.on ? Theme.accentSoft : invertButton.hovered ? Theme.hover : "transparent"
                                        }
                                        contentItem: Item {
                                            Icon {
                                                anchors.centerIn: parent
                                                glyph: "contrast"
                                                size: 14
                                                color: invertButton.on ? Theme.accent : invertButton.hovered ? Theme.text : Theme.textFaint
                                            }
                                        }
                                    }
                                    Column {
                                        anchors.left: parent.left
                                        anchors.right: check.left
                                        anchors.leftMargin: 12
                                        anchors.rightMargin: 6
                                        anchors.top: miniature.bottom
                                        anchors.topMargin: 8
                                        spacing: 1
                                        Label {
                                            width: parent.width
                                            text: skinCard.modelData.name
                                            elide: Text.ElideRight
                                            color: Theme.text
                                            font.weight: Font.DemiBold
                                        }
                                        Label {
                                            width: parent.width
                                            text: skinCard.modelData.hint
                                            elide: Text.ElideRight
                                            color: Theme.textFaint
                                            font.pixelSize: Theme.fontSmall
                                        }
                                    }
                                    Icon {
                                        id: check
                                        anchors.right: invertButton.left
                                        anchors.rightMargin: 4
                                        anchors.top: miniature.bottom
                                        anchors.topMargin: 12
                                        visible: skinCard.selected
                                        glyph: "check"
                                        size: 16
                                        color: Theme.accent
                                    }
                                }
                            }
                        }
                    }
                }

                InheritNote { key: "accentOverride" }
                SettingRow {
                    title: qsTr("Accent colour")
                    Row {
                        spacing: 6
                        Repeater {
                            model: Theme.accentChoices
                            delegate: AbstractButton {
                                id: swatch
                                required property string modelData
                                readonly property bool active: Theme.edited("accentOverride") === modelData
                                width: 24
                                height: 24
                                onClicked: Theme.edit("accentOverride", modelData)
                                ToolTip.visible: hovered && modelData.length === 0
                                ToolTip.text: qsTr("Theme default")
                                background: RoundRect {
                                    radius: 12
                                    color: swatch.modelData.length > 0 ? swatch.modelData : Theme.skin.accent
                                    borderWidth: swatch.active ? 3 : 1
                                    borderColor: swatch.active ? Theme.text : Theme.border
                                    Label {
                                        anchors.centerIn: parent
                                        visible: swatch.modelData.length === 0
                                        text: "A"
                                        color: Theme.skin.onAccent
                                        font.pixelSize: 10
                                        font.weight: Font.Bold
                                    }
                                }
                            }
                        }
                    }
                }

                VisualPicker {
                    title: qsTr("Icon style")
                    options: [{ value: "solid", text: qsTr("Solid") }, { value: "line", text: qsTr("Line") },
                              { value: "tile", text: qsTr("Tile") }]
                    key: "iconStyle"
                    current: ["line", "tile"].indexOf(Theme.edited("iconStyle")) >= 0 ? Theme.edited("iconStyle") : "solid"
                    preview: Component { IconStylePreview {} }
                }
                VisualPicker {
                    title: qsTr("Player buttons")
                    options: [{ value: "sonkore", text: "Sonkore" }, { value: "round", text: qsTr("Round") },
                              { value: "outline", text: qsTr("Outline") }]
                    key: "transportStyle"
                    current: Theme.transportStyles.indexOf(Theme.edited("transportStyle")) >= 0 ? Theme.edited("transportStyle") : "sonkore"
                    preview: Component { TransportStylePreview {} }
                }
                VisualPicker {
                    title: qsTr("Progress bar")
                    options: [{ value: "line", text: qsTr("Line") }, { value: "pill", text: qsTr("Pill") },
                              { value: "thick", text: qsTr("Thick") }, { value: "classic", text: qsTr("Classic") },
                              { value: "glow", text: qsTr("Glow") }, { value: "led", text: "LED" },
                              { value: "ring", text: qsTr("Ring") }, { value: "wave", text: qsTr("Waveform") }]
                    key: "sliderStyle"
                    preview: Component { SliderStylePreview {} }
                }
                VisualPicker {
                    title: qsTr("Volume bar")
                    options: [{ value: "line", text: qsTr("Line") }, { value: "pill", text: qsTr("Pill") },
                              { value: "thick", text: qsTr("Thick") }, { value: "classic", text: qsTr("Classic") },
                              { value: "glow", text: qsTr("Glow") }, { value: "led", text: "LED" },
                              { value: "ring", text: qsTr("Ring") }]
                    key: "volumeStyle"
                    preview: Component { VolumeStylePreview {} }
                }
                SectionLabel { text: qsTr("Window"); Layout.topMargin: 8 }
                VisualPicker {
                    title: qsTr("Window buttons")
                    options: [{ value: "windows", text: "Windows" }, { value: "mac", text: "macOS" },
                              { value: "gnome", text: "GNOME" }, { value: "breeze", text: "KDE Breeze" },
                              { value: "cinnamon", text: "Cinnamon" },
                              { value: "minimal", text: qsTr("Minimal") }]
                    key: "windowButtons"
                    preview: Component { WindowButtonsPreview {} }
                }
                VisualPicker {
                    title: qsTr("Window corners")
                    options: [{ value: 0, text: qsTr("Square") }, { value: 4, text: qsTr("Small") },
                              { value: 8, text: qsTr("Rounded") }, { value: 12, text: qsTr("Large") },
                              { value: 16, text: qsTr("Extra large") }]
                    key: "windowRadius"
                    preview: Component { CornerPreview {} }
                }
                RestartButton { needed: Theme.edited("windowRadius") > 0 && !App.alphaWindow }
                VisualPicker {
                    title: qsTr("Window outline")
                    options: [{ value: "off", text: qsTr("Off") }, { value: "line", text: qsTr("Line") },
                              { value: "bold", text: qsTr("Bold") }, { value: "highlight", text: qsTr("Highlight") },
                              { value: "accent", text: qsTr("Accent") }, { value: "glow", text: qsTr("Glow") }]
                    key: "windowOutline"
                    preview: Component { OutlinePreview {} }
                }

                SectionLabel { text: qsTr("Background and visualizer"); Layout.topMargin: 8 }
                VisualPicker {
                    title: qsTr("Background")
                    options: [{ value: "plain", text: qsTr("Plain") }, { value: "aurora", text: qsTr("Aurora") },
                              { value: "gradient", text: qsTr("Gradient") }, { value: "glow", text: qsTr("Glow") }]
                    key: "backdrop"
                    preview: Component { BackdropPreview {} }
                }
                VisualPicker {
                    title: qsTr("Visualizer")
                    options: [{ value: "off", text: qsTr("Off") }, { value: "bars", text: qsTr("Bars") },
                              { value: "mirror", text: qsTr("Mirror") }, { value: "line", text: qsTr("Line") },
                              { value: "dots", text: qsTr("Dots") }]
                    key: "visualizer"
                    preview: Component { VisualizerPreview {} }
                }

                SectionLabel { text: qsTr("Layout"); Layout.topMargin: 8 }
                VisualPicker {
                    title: qsTr("Interface density")
                    options: [{ value: "spacious", text: qsTr("Spacious") }, { value: "comfortable", text: qsTr("Comfortable") },
                              { value: "compact", text: qsTr("Compact") }]
                    // Never chosen: the skin's own density.
                    key: "density"
                    current: Theme.edited("density") === "auto" ? (Theme.skin.compact ? "compact" : "comfortable") : Theme.edited("density")
                    preview: Component { DensityPreview {} }
                }
                VisualPicker {
                    title: qsTr("List density")
                    options: [{ value: "spacious", text: qsTr("Spacious") }, { value: "comfortable", text: qsTr("Comfortable") },
                              { value: "compact", text: qsTr("Compact") }, { value: "list", text: qsTr("List") }]
                    // Never chosen: the one the interface density implies.
                    key: "listDensity"
                    current: Theme.editedListDensity
                    preview: Component { ListDensityPreview {} }
                }
                VisualPicker {
                    title: qsTr("Cover art in track lists")
                    // Compact and List rows have no room for covers: off and locked.
                    description: Theme.listCompact ? qsTr("Only with the Spacious or Comfortable list density.") : ""
                    enabled: !Theme.listCompact
                    options: [{ value: false, text: qsTr("Without covers") }, { value: true, text: qsTr("With covers") }]
                    key: "listCovers"
                    preview: Component { ListCoversPreview {} }
                }
                VisualPicker {
                    title: qsTr("Alternating row colours")
                    options: [{ value: false, text: qsTr("Uniform") }, { value: true, text: qsTr("Alternating") }]
                    key: "alternateRows"
                    preview: Component { AlternateRowsPreview {} }
                }
            }

            // --- Output -------------------------------------------------------
            SettingsCard {
                tab: "audio"
                title: qsTr("Audio output")
                iconName: "speaker"

                RowLayout {
                    Layout.fillWidth: true
                    Rectangle {
                        implicitWidth: 8
                        implicitHeight: 8
                        radius: 4
                        color: !DspController.loaded ? Theme.textFaint : (DspController.bitPerfect ? Theme.good : Theme.warn)
                    }
                    Label {
                        Layout.fillWidth: true
                        text: DspController.loaded ? DspController.outputDescription
                                                   : qsTr("Nothing playing — the output opens when playback starts")
                        color: Theme.textDim
                        elide: Text.ElideRight
                    }
                    PillButton {
                        iconName: "refresh"
                        text: qsTr("Rescan devices")
                        onClicked: DspController.refreshDevices()
                    }
                }

                RowLayout {
                    Layout.fillWidth: true
                    visible: page.savedDeviceMissing
                    spacing: 8
                    Icon {
                        glyph: "info"
                        size: 16
                        color: Theme.warn
                    }
                    Label {
                        Layout.fillWidth: true
                        wrapMode: Text.Wrap
                        font.pixelSize: Theme.fontSmall
                        color: Theme.warn
                        text: qsTr("The chosen output is not connected, so the system default is used. Reconnect it and it is used again the next time playback starts.")
                    }
                }

                Repeater {
                    model: page.deviceGroups
                    delegate: ColumnLayout {
                        id: group
                        required property var modelData
                        Layout.fillWidth: true
                        Layout.topMargin: 6
                        spacing: 2
                        RowLayout {
                            SectionLabel { text: group.modelData.backend }
                            Label {
                                Layout.fillWidth: true
                                text: page.backendHint(group.modelData.backend)
                                color: Theme.textFaint
                                font.pixelSize: Theme.fontSmall
                                elide: Text.ElideRight
                            }
                        }
                        Repeater {
                            model: group.modelData.devices
                            delegate: Rectangle {
                                id: deviceRow
                                required property var modelData
                                readonly property bool selected: modelData.key === page.effectiveDeviceKey
                                Layout.fillWidth: true
                                implicitHeight: Theme.compact ? 32 : 44
                                radius: Theme.radiusSmall
                                color: selected ? Theme.accentSoft : (deviceHover.hovered ? Theme.hover : "transparent")
                                HoverHandler { id: deviceHover }
                                TapHandler { onTapped: DspController.selectOutputDevice(deviceRow.modelData.key) }
                                RowLayout {
                                    anchors.fill: parent
                                    anchors.leftMargin: 10
                                    anchors.rightMargin: 10
                                    spacing: 10
                                    RoundRect {
                                        implicitWidth: 16
                                        implicitHeight: 16
                                        radius: 8
                                        color: "transparent"
                                        borderWidth: 2
                                        borderColor: deviceRow.selected ? Theme.accent : Theme.textFaint
                                        Rectangle {
                                            anchors.centerIn: parent
                                            width: 8
                                            height: 8
                                            radius: 4
                                            color: Theme.accent
                                            visible: deviceRow.selected
                                        }
                                    }
                                    ColumnLayout {
                                        Layout.fillWidth: true
                                        spacing: 0
                                        Label {
                                            Layout.fillWidth: true
                                            text: deviceRow.modelData.name
                                            elide: Text.ElideRight
                                            color: Theme.text
                                            font.weight: deviceRow.selected ? Font.DemiBold : Font.Normal
                                        }
                                        Label {
                                            Layout.fillWidth: true
                                            visible: deviceRow.modelData.detail.length > 0 && !Theme.compact
                                            text: deviceRow.modelData.detail
                                            elide: Text.ElideMiddle
                                            color: Theme.textFaint
                                            font.pixelSize: Theme.fontSmall
                                        }
                                    }
                                    Badge {
                                        visible: deviceRow.modelData.direct
                                        text: qsTr("DIRECT · BIT-PERFECT")
                                        tone: Theme.good
                                    }
                                    Badge {
                                        visible: deviceRow.modelData.isDefault && deviceRow.modelData.key.length > 0
                                        text: qsTr("DEFAULT")
                                        tone: Theme.textDim
                                    }
                                }
                            }
                        }
                    }
                }

                // A direct card (hw:, ASIO) or WASAPI in exclusive mode: the
                // system has no sound from it while Sonkore holds it.
                SettingRow {
                    visible: page.directSelected || DspController.outputExclusiveCapable
                    title: qsTr("DAC control")
                    description: DspController.outputReserved ? qsTr("In use by Sonkore only.") : qsTr("Taken when playback starts.")
                    details: DspController.outputReserved
                        ? qsTr("Sonkore has the DAC to itself: other apps are silent or move to another output until it is released.")
                        : qsTr("Sonkore takes the DAC from the system when playback starts, even if other apps are using it.")
                    PillButton {
                        text: qsTr("Release DAC")
                        enabled: DspController.outputReserved
                        onClicked: DspController.releaseOutput()
                    }
                }
                SettingRow {
                    visible: page.directSelected || DspController.outputExclusiveCapable
                    title: qsTr("Give the DAC back when paused")
                    details: qsTr("After a minute paused or stopped, so other apps can play again. Pressing play takes it back.")
                    LSwitch { checked: DspController.releaseWhenIdle; onToggled: DspController.releaseWhenIdle = checked }
                }
                SettingRow {
                    visible: DspController.hardwareVolumeSupported
                    title: qsTr("Volume control")
                    description: DspController.volumeMode !== 0
                        ? DspController.hardwareVolumeInfo
                        : qsTr("DAC keeps bit-perfect; Digital does not below 100 %.")
                    details: DspController.volumeMode !== 0 ? ""
                        : qsTr("Digital: Sonkore scales the samples, so any volume below 100% leaves bit-perfect. DAC: the volume moves the DAC's own control and the audio stays bit-perfect.")
                    LComboBox {
                        // Listed Automatic, Digital, DAC; stored as the VolumeMode values 2, 0, 1.
                        readonly property var modes: [2, 0, 1]
                        implicitWidth: 140
                        model: [qsTr("Automatic"), qsTr("Digital"), qsTr("DAC")]
                        currentIndex: modes.indexOf(DspController.volumeMode)
                        onActivated: (index) => DspController.volumeMode = modes[index]
                    }
                }
                SettingRow {
                    visible: App.platform === "linux"
                    title: qsTr("Follow the track's rate")
                    description: qsTr("PipeWire switches to each track's sample rate while Sonkore plays.")
                    details: qsTr("Without it, PipeWire runs at the desktop's rate (usually 48 kHz) and Sonkore converts every track to it with SoX VHQ. Other apps keep playing either way; PipeWire converts them instead. After a minute paused or stopped, PipeWire gets its own rate back.")
                    LSwitch { checked: DspController.followTrackRate; onToggled: DspController.followTrackRate = checked }
                }
                SettingRow {
                    visible: App.platform === "windows"
                    title: qsTr("WASAPI exclusive mode")
                    description: qsTr("Bypasses the Windows mixer for bit-perfect output.")
                    LSwitch { checked: DspController.exclusiveMode; onToggled: DspController.exclusiveMode = checked }
                }
                SettingRow {
                    title: qsTr("Play from memory")
                    details: qsTr("Each file is read into RAM before it plays (up to 2 GB), so playback never waits on the disk. Uses as much memory as the current and the next file.")
                    LSwitch { checked: DspController.playFromMemory; onToggled: DspController.playFromMemory = checked }
                }
                SettingRow {
                    title: qsTr("Output buffer")
                    description: qsTr("Larger is safer; smaller reacts faster.")
                    LComboBox {
                        implicitWidth: 140
                        model: [100, 150, 250, 400, 750]
                        currentIndex: Math.max(0, model.indexOf(DspController.bufferMs))
                        displayText: currentValue + " ms"
                        displayFor: (v) => v + " ms"
                        onActivated: DspController.bufferMs = currentValue
                    }
                }
            }

            // --- Sample rate -------------------------------------------------
            SettingsCard {
                tab: "audio"
                id: rateCard
                title: qsTr("Sample rate")
                iconName: "wave"
                subtitle: qsTr("Original keeps every sample untouched.")
                // 0 original, 1 fixed rate, 2 upsampling (fixed rate wins, as in the engine).
                readonly property int mode: DspController.fixedRate !== 0 ? 1 : DspController.upsampling !== 0 ? 2 : 0
                property int lastFixed: 96000

                SettingRow {
                    title: qsTr("Mode")
                    description: rateCard.mode === 0
                        ? qsTr("Bit-perfect; the DAC switches rate between files.")
                        : rateCard.mode === 1
                            ? qsTr("One rate for everything; the DAC never switches.")
                            : qsTr("A multiple of each file's rate.")
                    details: rateCard.mode === 0
                        ? qsTr("Every file at its own rate, bit for bit (bit-perfect). The DAC switches rate between files, and some DACs click or beep when they do.")
                        : rateCard.mode === 1
                            ? qsTr("Every file is converted to one rate, so the DAC never switches and never clicks. Files already at that rate stay bit-perfect; DSD files become PCM.")
                            : qsTr("Every file goes out at a multiple of its own rate (44.1 → 88.2 / 176.4 kHz), so the DAC's own filter has less to do. The DAC still switches between the 44.1 and 48 kHz families.")
                    Segmented {
                        options: [{ value: 0, text: qsTr("Original") }, { value: 1, text: qsTr("Fixed rate") },
                                  { value: 2, text: qsTr("Upsampling") }]
                        current: rateCard.mode
                        onPicked: (v) => {
                            if (rateCard.mode === 1)
                                rateCard.lastFixed = DspController.fixedRate
                            DspController.setRateMode(v === 1 ? rateCard.lastFixed : 0, v === 2 ? 1 : 0)
                        }
                    }
                }
                SettingRow {
                    visible: rateCard.mode === 1
                    title: qsTr("Output rate")
                    description: qsTr("88.2 kHz for CDs, 96 kHz for 48 kHz files.")
                    details: qsTr("88.2 kHz suits a library of mostly CDs (44.1 kHz); 96 kHz suits one with mostly 48 / 96 kHz files. Either way the conversion is inaudible.")
                    LComboBox {
                        implicitWidth: 140
                        model: [44100, 48000, 88200, 96000, 176400, 192000, 352800, 384000]
                        currentIndex: Math.max(0, model.indexOf(DspController.fixedRate))
                        displayText: displayFor(currentValue)
                        displayFor: (v) => (v / 1000) + " kHz"
                        onActivated: DspController.fixedRate = currentValue
                    }
                }
                SettingRow {
                    visible: rateCard.mode === 2
                    title: qsTr("Upsampling factor")
                    LComboBox {
                        implicitWidth: 160
                        model: ["×2", "×4", qsTr("Highest the DAC takes")]
                        currentIndex: Math.max(0, DspController.upsampling - 1)
                        onActivated: (index) => DspController.upsampling = index + 1
                    }
                }
                SettingRow {
                    visible: rateCard.mode !== 0
                    title: qsTr("Conversion filter")
                    description: qsTr("Linear phase is the usual choice.")
                    details: qsTr("Linear phase: the most faithful timing, the usual choice. Minimum phase: no pre-ringing before attacks, which some prefer on percussion. Intermediate: halfway between. Linear, steep: keeps the treble right up to the top of the band. The differences are subtle.")
                    LComboBox {
                        implicitWidth: 160
                        model: [qsTr("Linear phase"), qsTr("Intermediate phase"), qsTr("Minimum phase"), qsTr("Linear, steep")]
                        currentIndex: DspController.resampleFilter
                        onActivated: (index) => DspController.resampleFilter = index
                    }
                }
                SettingRow {
                    visible: rateCard.mode !== 1
                    title: qsTr("Pause on sample rate change")
                    details: qsTr("Silence sent while the DAC switches to a new rate, so the first notes are not cut. It cannot silence a DAC that beeps by itself while switching: use a fixed rate for that.")
                    LComboBox {
                        implicitWidth: 140
                        model: [0, 250, 500, 1000, 2000]
                        currentIndex: Math.max(0, model.indexOf(DspController.resyncDelayMs))
                        displayText: displayFor(currentValue)
                        displayFor: (v) => v === 0 ? qsTr("Off") : v + " ms"
                        onActivated: DspController.resyncDelayMs = currentValue
                    }
                }
            }

            // --- DSD -------------------------------------------------------------
            SettingsCard {
                tab: "audio"
                title: qsTr("DSD")
                iconName: "chip"
                SettingRow {
                    title: qsTr("DSD playback")
                    description: DspController.dsdMode === 0
                        ? qsTr("DSD files are converted to PCM: works with every DAC.")
                        : DspController.dsdMode === 1
                            ? qsTr("Native DSD on direct (hw:) outputs.")
                            : qsTr("Only for DACs with DoP: others play loud noise.")
                    details: DspController.dsdMode === 1
                        ? qsTr("Native DSD to DACs that offer it on a direct (hw:) output; otherwise the file is converted to PCM. EQ, pitch, ReplayGain and digital volume do not apply to DSD: use the DAC's volume.")
                        : DspController.dsdMode === 2
                            ? qsTr("DoP (DSD over PCM) on a direct or exclusive output. Only for DACs that support DoP: others play loud noise. EQ, pitch, ReplayGain and digital volume do not apply to DSD.")
                            : qsTr("Sonkore's own converter, in double precision: DSD64 becomes 88.2 kHz PCM flat to 30 kHz, DSD128 and up 176.4 kHz flat to 50-70 kHz. The ultrasonic noise DSD carries above that is filtered out (about 70 dB lower than with FFmpeg's converter), so it never reaches the amplifier. DST files too.")
                    LComboBox {
                        implicitWidth: 140
                        model: [qsTr("Convert to PCM"), qsTr("Native"), qsTr("DoP")]
                        currentIndex: DspController.dsdMode
                        onActivated: (index) => DspController.dsdMode = index
                    }
                }
                SettingRow {
                    visible: DspController.dsdMode !== 0
                    title: qsTr("PCM to DSD")
                    details: qsTr("Converts PCM tracks to DSD with an 8th-order sigma-delta modulator and sends them through the DSD output above. EQ, convolution, ReplayGain and the digital volume are applied before the conversion. DSD512 runs the channels on separate cores.")
                    LComboBox {
                        // Listed in rate order; stored as the PcmToDsd values (DSD512 came last).
                        readonly property var modes: [0, 1, 2, 3, 5, 4]
                        implicitWidth: 160
                        model: [qsTr("Off"), "DSD64", "DSD128", "DSD256", "DSD512", qsTr("Highest the DAC takes (up to DSD256)")]
                        currentIndex: Math.max(0, modes.indexOf(DspController.pcmToDsd))
                        onActivated: (index) => DspController.pcmToDsd = modes[index]
                    }
                }
            }

            // --- Test signals ------------------------------------------------------
            SettingsCard {
                tab: "audio"
                title: qsTr("Test signals")
                iconName: "wave"
                subtitle: qsTr("Tones made on the spot, played through the same path as the music to check the DAC and the speakers. What was playing resumes afterwards.")
                id: testCard
                property string running // which row's signal plays
                readonly property bool anyRunning: DspController.testSignalPlaying
                onAnyRunningChanged: if (!anyRunning) running = ""

                SettingRow {
                    title: qsTr("DAC sample rate")
                    description: qsTr("A 1 kHz tone at −20 dBFS at the chosen rate: a DAC with a display should show that rate.")
                    details: qsTr("If the display shows another rate, something between Sonkore and the DAC converts it: a fixed output rate above, a shared output (PipeWire or WASAPI shared) or the DAC's own limit. The signal path (▣ in the player bar) tells what Sonkore sends.")
                    RowLayout {
                        spacing: 8
                        LComboBox {
                            id: rateBox
                            implicitWidth: 130
                            readonly property var choices: [
                                { kind: "tone", rate: 44100, text: "44.1 kHz" }, { kind: "tone", rate: 48000, text: "48 kHz" },
                                { kind: "tone", rate: 88200, text: "88.2 kHz" }, { kind: "tone", rate: 96000, text: "96 kHz" },
                                { kind: "tone", rate: 176400, text: "176.4 kHz" }, { kind: "tone", rate: 192000, text: "192 kHz" },
                                { kind: "tone", rate: 352800, text: "352.8 kHz" }, { kind: "tone", rate: 384000, text: "384 kHz" },
                                { kind: "dsd", rate: 64, text: "DSD64" }, { kind: "dsd", rate: 128, text: "DSD128" }, { kind: "dsd", rate: 256, text: "DSD256" }]
                            model: choices.map(c => c.text)
                            currentIndex: 0
                        }
                        TestButton {
                            row: "rate"
                            onClicked: {
                                if (on) {
                                    DspController.stopTestSignal()
                                } else {
                                    testCard.running = row
                                    const c = rateBox.choices[rateBox.currentIndex]
                                    DspController.playTestSignal(c.kind, c.rate, 2)
                                }
                            }
                        }
                    }
                }
                SettingRow {
                    title: qsTr("Speakers")
                    description: qsTr("The tone on each channel in turn: front left, front right, then centre, LFE, back left, back right, side left, side right.")
                    details: qsTr("Multichannel files reach the DAC in this order (the DAC or the receiver places them). A channel that sounds from the wrong speaker is wired or configured the other way round.")
                    RowLayout {
                        spacing: 8
                        LComboBox {
                            id: channelsBox
                            implicitWidth: 130
                            readonly property var counts: [2, 6, 8]
                            model: [qsTr("Stereo"), "5.1", "7.1"]
                            currentIndex: 0
                        }
                        TestButton {
                            row: "channels"
                            onClicked: {
                                if (on) {
                                    DspController.stopTestSignal()
                                } else {
                                    testCard.running = row
                                    DspController.playTestSignal("channels", 48000, channelsBox.counts[channelsBox.currentIndex])
                                }
                            }
                        }
                    }
                }
                SettingRow {
                    visible: DspController.dtsTestAvailable
                    title: qsTr("Bit-perfect (DTS)")
                    description: qsTr("A DTS stream inside a CD-quality WAV: a receiver that decodes DTS plays a tone only if every bit arrives untouched, and noise otherwise.")
                    details: qsTr("Turn the amplifier down first: anything that changes the samples on the way (a volume below 100 %, EQ, resampling, dither, a shared output) turns the stream into loud noise. For an AV receiver on S/PDIF or HDMI; a plain DAC plays noise either way.")
                    TestButton {
                        row: "dts"
                        onClicked: {
                            if (on) {
                                DspController.stopTestSignal()
                            } else {
                                testCard.running = row
                                DspController.playTestSignal("dts", 44100, 2)
                            }
                        }
                    }
                }
            }

            // --- Processing ------------------------------------------------------
            SettingsCard {
                tab: "audio"
                title: qsTr("Sound processing")
                iconName: "eq"
                subtitle: qsTr("Each one leaves bit-perfect while on.")
                SettingRow {
                    title: qsTr("Convolution (FIR)")
                    description: DspController.convolutionFile.length === 0
                        ? qsTr("Room or headphone correction (WAV or FLAC).")
                        : DspController.convolutionFile
                    details: DspController.convolutionFile.length === 0
                        ? qsTr("Room or headphone correction with an impulse response from REW, Dirac, Acourate… (WAV or FLAC: mono, stereo or true stereo with 4 channels). It is resampled to each track's rate.")
                        : ""
                    RowLayout {
                        spacing: 8
                        LSwitch {
                            visible: DspController.convolutionFile.length > 0
                            checked: DspController.convolutionEnabled
                            onToggled: DspController.convolutionEnabled = checked
                        }
                        PillButton { text: DspController.convolutionFile.length > 0 ? qsTr("Change…") : qsTr("Load…"); onClicked: dialogs.open("openImpulse") }
                        PillButton { visible: DspController.convolutionFile.length > 0; text: qsTr("Remove"); onClicked: DspController.clearConvolution() }
                    }
                }
                SettingRow {
                    title: qsTr("Headphone crossfeed")
                    description: qsTr("Less fatigue on headphones. Stereo only.")
                    details: qsTr("Lets each ear hear a little of the other channel, low-passed, as with loudspeakers: less fatigue on long headphone sessions with hard-panned recordings. Stereo tracks only.")
                    LComboBox {
                        implicitWidth: 160
                        model: [qsTr("Off"), qsTr("Default"), "Chu Moy", "Jan Meier"]
                        currentIndex: DspController.crossfeed
                        onActivated: (index) => DspController.crossfeed = index
                    }
                }
                SettingRow {
                    title: qsTr("DSP headroom")
                    description: DspController.clippedSamples > 0
                        ? qsTr("This track is clipping: lower the headroom.")
                        : DspController.headroomAuto
                            ? qsTr("Automatic: now %1 dB.").arg(DspController.effectiveHeadroomDb.toFixed(1))
                            : qsTr("Keeps the DSP from clipping.")
                    details: DspController.headroomAuto
                        ? qsTr("Automatic: 3 dB for inter-sample peaks plus whatever the EQ boosts (now %1 dB). Applied only while some DSP runs; a bit-perfect path is never touched.").arg(DspController.effectiveHeadroomDb.toFixed(1))
                        : qsTr("Lowers the level ahead of the DSP (EQ, pitch, resampling) so boosts and inter-sample peaks never clip. Not applied while the path is bit-perfect.")
                    RowLayout {
                        spacing: 8
                        LComboBox {
                            implicitWidth: 140
                            model: [qsTr("Automatic"), qsTr("Manual")]
                            currentIndex: DspController.headroomAuto ? 0 : 1
                            onActivated: (index) => DspController.headroomAuto = index === 0
                        }
                        NumField {
                            visible: !DspController.headroomAuto
                            value: DspController.headroomDb
                            from: -24
                            to: 0
                            decimals: 1
                            suffix: "dB"
                            onEdited: (v) => DspController.headroomDb = v
                        }
                    }
                }
                SettingRow {
                    title: qsTr("Dither")
                    description: qsTr("Only while DSP is active.")
                    details: qsTr("When DSP is active, the result is requantised to the DAC's bit depth. TPDF adds inaudible noise so that step stays linear; noise shaping also moves that noise away from the frequencies the ear hears best (most useful with 16-bit DACs).")
                    LComboBox {
                        implicitWidth: 160
                        model: [qsTr("Off"), qsTr("TPDF"), qsTr("Noise shaped")]
                        currentIndex: DspController.ditherMode
                        onActivated: (index) => DspController.ditherMode = index
                    }
                }
                SettingRow {
                    title: qsTr("HDCD decoding")
                    details: qsTr("CDs mastered with HDCD carry hidden codes for 20-bit dynamics. They are detected in the first seconds of each 16-bit track and decoded; other tracks stay bit-perfect.")
                    LSwitch { checked: DspController.hdcd; onToggled: DspController.hdcd = checked }
                }
            }

            // --- ReplayGain ----------------------------------------------------
            SettingsCard {
                tab: "audio"
                title: qsTr("ReplayGain")
                iconName: "volume"
                SettingRow {
                    title: qsTr("Volume levelling")
                    details: qsTr("Plays every track, or every album as a whole, at the same loudness (ReplayGain 2.0, EBU R128). Any gain other than 0 dB leaves the bit-perfect path.")
                    LComboBox {
                        implicitWidth: 140
                        model: [qsTr("Off"), qsTr("Track"), qsTr("Album")]
                        currentIndex: DspController.replayGainMode
                        onActivated: (index) => DspController.replayGainMode = index
                    }
                }
                SettingRow {
                    visible: DspController.replayGainMode !== 0
                    title: qsTr("Pre-amplification")
                    NumField {
                        value: DspController.replayGainPreamp
                        from: -15
                        to: 15
                        decimals: 1
                        suffix: "dB"
                        onEdited: (v) => DspController.replayGainPreamp = v
                    }
                }
                SettingRow {
                    visible: DspController.replayGainMode !== 0
                    title: qsTr("Prevent clipping")
                    details: qsTr("Lowers the gain when it would push the track's true peak above full scale.")
                    LSwitch { checked: DspController.replayGainPreventClipping; onToggled: DspController.replayGainPreventClipping = checked }
                }
                SettingRow {
                    visible: DspController.replayGainMode !== 0
                    title: qsTr("Loudness analysis")
                    description: Library.analyzingLoudness
                        ? qsTr("Measuring tracks without ReplayGain tags… %1 of %2. Files are never modified.").arg(Library.loudnessDone).arg(Library.loudnessTotal)
                        : Library.tracksWithoutLoudness > 0
                            ? qsTr("%1 tracks have no loudness data yet. They are measured in the background; files are never modified.").arg(Library.tracksWithoutLoudness)
                            : qsTr("Every track in the library has loudness data, from its tags or measured by Sonkore.")
                    Label {
                        text: isNaN(DspController.currentGainDb) ? qsTr("Current track: no data")
                              : qsTr("Current track: %1 dB").arg((DspController.currentGainDb >= 0 ? "+" : "") + DspController.currentGainDb.toFixed(1))
                        color: Theme.textDim
                    }
                }
            }

            // --- Library ------------------------------------------------------
            SettingsCard {
                tab: "library"
                title: qsTr("Library folders")
                iconName: "folder"

                Repeater {
                    model: Library.folders
                    delegate: RowLayout {
                        id: folderRow
                        required property string modelData
                        Layout.fillWidth: true
                        spacing: 10
                        Icon { glyph: "folder"; size: 16; color: Theme.textDim }
                        Label {
                            Layout.fillWidth: true
                            text: folderRow.modelData
                            elide: Text.ElideMiddle
                            color: Theme.text
                        }
                        IconButton {
                            iconName: "trash"
                            iconSize: 15
                            tip: qsTr("Stop watching this folder")
                            onClicked: AppActions.removeFolderRequested(folderRow.modelData) // asks first
                        }
                    }
                }
                RowLayout {
                    spacing: 8
                    PillButton { primary: true; iconName: "folder-plus"; text: qsTr("Add folder"); onClicked: AppActions.addFolderRequested() }
                    PillButton { iconName: "refresh"; text: qsTr("Rescan"); enabled: !Library.scanning; onClicked: Library.rescan() }
                    Label {
                        text: Library.scanning ? qsTr("Scanning… %1 files").arg(Library.scannedFiles)
                                               : qsTr("%1 tracks indexed").arg(Library.count)
                        color: Theme.textDim
                    }
                }
                SettingRow {
                    visible: Library.hiddenCount > 0
                    title: qsTr("Removed tracks")
                    description: qsTr("%1 tracks removed by hand.").arg(Library.hiddenCount)
                    details: qsTr("%1 tracks were removed from the library by hand (the files are still on disk). Adding their folder again with + also brings them back.").arg(Library.hiddenCount)
                    PillButton { iconName: "refresh"; text: qsTr("Restore"); enabled: !Library.scanning; onClicked: Library.restoreHidden() }
                }
                SectionLabel { text: qsTr("Skipped folders"); Layout.topMargin: 4 }
                Label {
                    Layout.fillWidth: true
                    visible: Library.excludedFolders.length === 0
                    text: qsTr("None. Skip folders inside your music folders that are not music (audiobooks, samples, ringtones).")
                    wrapMode: Text.Wrap
                    color: Theme.textDim
                    font.pixelSize: Theme.fontSmall + 1
                }
                Repeater {
                    model: Library.excludedFolders
                    delegate: RowLayout {
                        id: skippedRow
                        required property string modelData
                        Layout.fillWidth: true
                        spacing: 10
                        Icon { glyph: "folder"; size: 16; color: Theme.textFaint }
                        Label {
                            Layout.fillWidth: true
                            text: skippedRow.modelData
                            elide: Text.ElideMiddle
                            color: Theme.textDim
                        }
                        IconButton {
                            iconName: "close"
                            iconSize: 14
                            tip: qsTr("Scan this folder again")
                            onClicked: Library.removeExcludedFolder(skippedRow.modelData)
                        }
                    }
                }
                PillButton {
                    iconName: "plus"
                    text: qsTr("Skip a folder…")
                    onClicked: dialogs.open("openSkipFolder")
                }
                SettingRow {
                    title: qsTr("Skip short files")
                    LComboBox {
                        implicitWidth: 140
                        model: [0, 5, 10, 30, 60]
                        currentIndex: Math.max(0, model.indexOf(Library.minDuration))
                        displayText: displayFor(currentValue)
                        displayFor: (v) => v === 0 ? qsTr("Off") : qsTr("Under %1 s").arg(v)
                        onActivated: Library.minDuration = currentValue
                    }
                }
                SettingRow {
                    title: qsTr("Watch folders for changes")
                    LSwitch { checked: Library.watchFolders; onToggled: Library.watchFolders = checked }
                }
                SettingRow {
                    title: qsTr("Rescan on startup")
                    LSwitch { checked: Library.rescanOnStartup; onToggled: Library.rescanOnStartup = checked }
                }
            }

            // --- Columns ---------------------------------------------------------
            SettingsCard {
                tab: "library"
                title: qsTr("Track list columns")
                iconName: "sort"
                Label {
                    Layout.fillWidth: true
                    text: qsTr("Tick the columns to show and use the arrows to order them. In any list you can also drag a header to move its column, drag the dividers to resize (double-click resets), click a header to sort and right-click it for this menu.")
                    wrapMode: Text.Wrap
                    color: Theme.textDim
                }
                GridLayout {
                    Layout.fillWidth: true
                    columns: Math.max(1, Math.floor(width / 300))
                    columnSpacing: 16
                    rowSpacing: 2
                    Repeater {
                        model: TrackColumns.catalog
                        delegate: RowLayout {
                            id: colRow
                            required property var modelData
                            readonly property int position: TrackColumns.keys.indexOf(modelData.key)
                            Layout.fillWidth: true
                            LCheckBox {
                                Layout.fillWidth: true
                                text: colRow.modelData.title + (colRow.position >= 0 ? "  · " + (colRow.position + 1) : "")
                                checked: colRow.position >= 0
                                enabled: !(checked && TrackColumns.keys.length === 1)
                                onToggled: TrackColumns.setVisible(colRow.modelData.key, checked)
                            }
                            IconButton {
                                visible: colRow.position >= 0
                                iconName: "chevron-down"
                                iconSize: 12
                                rotation: 180
                                enabled: colRow.position > 0
                                tip: qsTr("Move left")
                                onClicked: TrackColumns.move(colRow.modelData.key, -1)
                            }
                            IconButton {
                                visible: colRow.position >= 0
                                iconName: "chevron-down"
                                iconSize: 12
                                enabled: colRow.position >= 0 && colRow.position < TrackColumns.keys.length - 1
                                tip: qsTr("Move right")
                                onClicked: TrackColumns.move(colRow.modelData.key, 1)
                            }
                        }
                    }
                }
                PillButton { iconName: "refresh"; text: qsTr("Reset columns"); onClicked: TrackColumns.reset() }
            }
        }
    }
}
