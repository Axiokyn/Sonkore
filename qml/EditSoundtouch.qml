import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Sonkore

// Floating DSP window: pitch / tempo / rate with micro-decimal precision, the
// engine choice (Rubber Band by default, or SoundTouch) and each engine's
// advanced settings. Every control writes straight to DspController (C++
// double / int / bool); the engine picks the change up at the next
// ~1024-frame block boundary.
AppWindow {
    id: win

    title: qsTr("Pitch & tempo")
    iconName: "wave"
    // A normal window over the main one: a tool window could not always be
    // moved by its integrated bar, and "stay on top" is ignored on Wayland.
    baseFlags: Qt.Window
    maximizable: false
    // Never taller than its content (plus the title bar); on a short screen
    // (large interface scales) it opens smaller and the content scrolls.
    readonly property real fullHeight: Math.ceil(content.implicitHeight) + (header ? header.height : 0)
    readonly property real fitHeight: Math.min(fullHeight, Math.max(360, Screen.desktopAvailableHeight - 60))
    width: 580
    height: 600
    // Sized when shown: the content is laid out by then.
    onVisibleChanged: if (visible) height = fitHeight
    minimumWidth: 460
    minimumHeight: Math.min(360, fullHeight)
    maximumWidth: 760
    maximumHeight: fullHeight

    readonly property bool rubberBand: DspController.pitchEngine === DspController.RubberBand

    readonly property var stepOptions: [0.000001, 0.00001, 0.0001, 0.001, 0.01, 0.1, 1.0]
    readonly property real fineStep: stepOptions[stepBox.currentIndex]
    readonly property int precision: 7

    component Hint: Label {
        Layout.leftMargin: 78
        font.pixelSize: Theme.fontSmall
        color: Theme.textDim
    }

    component SettingSpin: SpinBox {
        property bool autoAtZero: false
        property string unit: "ms"
        editable: true
        Layout.fillWidth: true
        textFromValue: (v, locale) => (autoAtZero && v === 0) ? qsTr("auto") : v + " " + unit
        valueFromText: (t, locale) => {
            const n = parseInt(t)
            return isNaN(n) ? 0 : n
        }
        validator: RegularExpressionValidator {
            regularExpression: /^(auto|\d{1,3})(\s*\w*)?$/
        }
    }

    component Section: Card {
        id: section
        default property alias content: body.data
        property string title
        Layout.fillWidth: true
        Layout.leftMargin: 16
        Layout.rightMargin: 16
        implicitHeight: body.implicitHeight + 24
        ColumnLayout {
            id: body
            anchors.fill: parent
            anchors.margins: 12
            spacing: 6
            SectionLabel { text: section.title; Layout.bottomMargin: 2 }
        }
    }

    // Stops at the edges (no overshoot and bounce back when scrolling fast),
    // like every other list in the app.
    Flickable {
        id: scroller
        anchors.fill: parent
        contentWidth: width
        contentHeight: content.implicitHeight
        boundsBehavior: Flickable.StopAtBounds
        clip: true
        WheelScroll { flickable: scroller }
        ScrollBar.vertical: LScrollBar {}

        ColumnLayout {
            id: content
            width: scroller.width
            spacing: 10

            // --- Output path + on / off -------------------------------------------
            StatusCard {
                Layout.topMargin: 4
                Layout.leftMargin: 16
                Layout.rightMargin: 16
                title: DspController.pitchEngineName
                subtitle: DspController.loaded ? DspController.outputDescription : qsTr("No track loaded")
                checked: DspController.soundTouchEnabled
                onToggled: (on) => DspController.soundTouchEnabled = on
                tip: checked ? qsTr("Turn pitch & tempo off (the values are kept)") : qsTr("Turn pitch & tempo on")
            }

            // --- Engine -----------------------------------------------------------
            Section {
                title: qsTr("Engine")
                RowLayout {
                    Layout.fillWidth: true
                    spacing: 12
                    LComboBox {
                        Layout.fillWidth: true
                        model: ["Rubber Band", "SoundTouch"]
                        currentIndex: DspController.pitchEngine
                        onActivated: (index) => DspController.pitchEngine = index
                    }
                }
                Label {
                    Layout.fillWidth: true
                    wrapMode: Text.Wrap
                    font.pixelSize: Theme.fontSmall
                    color: Theme.textDim
                    text: win.rubberBand
                          ? qsTr("Phase vocoder: clean on complex mixes, vocals and bass; can keep the voice's timbre.")
                          : qsTr("WSOLA time-stretch: very little CPU, fine for small changes.")
                }
            }

            // --- Pitch / Tempo / Rate ---------------------------------------------
            Section {
                title: qsTr("Pitch · Tempo · Rate")
                enabled: DspController.soundTouchEnabled // off: shown dimmed, values kept
                opacity: enabled ? 1 : 0.45

                PrecisionField {
                    Layout.fillWidth: true
                    label: qsTr("Pitch")
                    suffix: qsTr("st")
                    from: -48
                    to: 48
                    decimals: win.precision
                    step: win.fineStep
                    value: DspController.pitchSemitones
                    onValueEdited: v => DspController.pitchSemitones = v
                }
                // Reference A (standard tuning 440 Hz) after the pitch shift.
                Hint {
                    text: qsTr("A = %1 Hz · ratio %2 · %3 cents")
                          .arg(DspController.formatDecimal(440 * DspController.pitchRatio(DspController.pitchSemitones), 3))
                          .arg(DspController.formatDecimal(DspController.pitchRatio(DspController.pitchSemitones), 12))
                          .arg(DspController.formatDecimal(DspController.pitchSemitones * 100.0, 5))
                }

                PrecisionField {
                    Layout.fillWidth: true
                    label: qsTr("Tempo")
                    suffix: "×"
                    from: 0.05
                    to: 20
                    decimals: win.precision
                    step: win.fineStep
                    value: DspController.tempo
                    onValueEdited: v => DspController.tempo = v
                }
                Hint {
                    text: qsTr("%1 % speed, pitch preserved").arg(DspController.formatDecimal((DspController.tempo - 1.0) * 100.0, 5))
                }

                PrecisionField {
                    Layout.fillWidth: true
                    label: qsTr("Rate")
                    suffix: "×"
                    from: 0.05
                    to: 20
                    decimals: win.precision
                    step: win.fineStep
                    value: DspController.rate
                    onValueEdited: v => DspController.rate = v
                }
                Hint {
                    text: qsTr("%1 % speed and pitch (resampling)").arg(DspController.formatDecimal((DspController.rate - 1.0) * 100.0, 5))
                }

                RowLayout {
                    Layout.topMargin: 6
                    Label {
                        text: qsTr("± step")
                        color: Theme.text
                        Layout.preferredWidth: 72
                    }
                    LComboBox {
                        id: stepBox
                        Layout.fillWidth: true
                        model: win.stepOptions.map(s => DspController.formatDecimal(s, 6))
                        currentIndex: 0
                    }
                }
            }

            // --- Advanced: Rubber Band ------------------------------------------------
            Section {
                title: qsTr("Advanced Rubber Band")
                visible: win.rubberBand
                enabled: DspController.soundTouchEnabled // off: shown dimmed, values kept
                opacity: enabled ? 1 : 0.45

                // R2 in real time with crisp or mixed transients resets phase on
                // steady tones too: held notes warble and the stereo smears.
                RowLayout {
                    Layout.fillWidth: true
                    visible: !DspController.rubberBandFiner && DspController.transients !== 2
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
                        text: qsTr("With the R2 engine, crisp and mixed transients make held notes warble. Use the R3 engine, or smooth transients.")
                    }
                }
                LSwitch {
                    text: qsTr("R3 “finer” engine (best quality, more CPU)")
                    checked: DspController.rubberBandFiner
                    onToggled: DspController.rubberBandFiner = checked
                }
                LSwitch {
                    text: qsTr("Preserve formants (natural voices when shifting pitch)")
                    checked: DspController.preserveFormants
                    onToggled: DspController.preserveFormants = checked
                }
                LSwitch {
                    text: qsTr("Centred stereo image (mono-compatible)")
                    checked: DspController.channelsTogether
                    onToggled: DspController.channelsTogether = checked
                }

                GridLayout {
                    Layout.fillWidth: true
                    Layout.topMargin: 4
                    columns: 2
                    columnSpacing: 12
                    rowSpacing: 8

                    Label { text: qsTr("Pitch method"); color: Theme.text }
                    LComboBox {
                        Layout.fillWidth: true
                        model: [qsTr("Quality (fixed shift)"), qsTr("Consistency (no clicks while editing)"), qsTr("Speed (least CPU)")]
                        currentIndex: DspController.pitchMethod
                        onActivated: (index) => DspController.pitchMethod = index
                    }
                    Label { text: qsTr("Window"); color: Theme.text }
                    LComboBox {
                        Layout.fillWidth: true
                        model: [qsTr("Standard"), qsTr("Short (less delay and CPU)"), qsTr("Long (smoother, R2 only)")]
                        currentIndex: DspController.rubberBandWindow
                        onActivated: (index) => DspController.rubberBandWindow = index
                    }
                }

                // R2 only: shown dimmed while R3 is on.
                SectionLabel { text: qsTr("R2 engine only"); Layout.topMargin: 8 }
                GridLayout {
                    Layout.fillWidth: true
                    columns: 2
                    columnSpacing: 12
                    rowSpacing: 8
                    enabled: !DspController.rubberBandFiner
                    opacity: enabled ? 1 : 0.45

                    Label { text: qsTr("Transients"); color: Theme.text }
                    LComboBox {
                        Layout.fillWidth: true
                        model: [qsTr("Crisp"), qsTr("Mixed"), qsTr("Smooth (no warble on steady tones)")]
                        currentIndex: DspController.transients
                        onActivated: (index) => DspController.transients = index
                    }
                    Label { text: qsTr("Transient detector"); color: Theme.text }
                    LComboBox {
                        Layout.fillWidth: true
                        model: [qsTr("Compound (general)"), qsTr("Percussive"), qsTr("Soft (e.g. piano)")]
                        currentIndex: DspController.detector
                        onActivated: (index) => DspController.detector = index
                    }
                    LSwitch {
                        Layout.columnSpan: 2
                        text: qsTr("Independent phase (softer, phasier)")
                        checked: DspController.phaseIndependent
                        onToggled: DspController.phaseIndependent = checked
                    }
                    LSwitch {
                        Layout.columnSpan: 2
                        text: qsTr("Time-domain smoothing (softer, artifacts at attacks)")
                        checked: DspController.rubberBandSmoothing
                        onToggled: DspController.rubberBandSmoothing = checked
                    }
                }
                Hint {
                    Layout.leftMargin: 0
                    Layout.fillWidth: true
                    wrapMode: Text.Wrap
                    visible: DspController.rubberBandFiner
                    text: qsTr("Turn off the R3 engine to use these.")
                }
                PillButton {
                    Layout.topMargin: 4
                    iconName: "refresh"
                    text: qsTr("Recommended engine settings")
                    onClicked: DspController.resetRubberBand()
                }
            }

            // --- Advanced: SoundTouch --------------------------------------------------
            Section {
                title: qsTr("Advanced SoundTouch")
                visible: !win.rubberBand
                enabled: DspController.soundTouchEnabled // off: shown dimmed, values kept
                opacity: enabled ? 1 : 0.45

                GridLayout {
                    Layout.fillWidth: true
                    columns: 2
                    columnSpacing: 12
                    rowSpacing: 8

                    LSwitch {
                        Layout.columnSpan: 2
                        text: qsTr("Use AA Filter (anti-aliasing)")
                        checked: DspController.useAAFilter
                        onToggled: DspController.useAAFilter = checked
                    }
                    Label {
                        text: qsTr("AA Filter Length")
                        color: Theme.text
                        opacity: DspController.useAAFilter ? 1 : 0.4
                    }
                    SettingSpin {
                        unit: qsTr("taps")
                        from: 8
                        to: 128
                        stepSize: 8
                        enabled: DspController.useAAFilter
                        value: DspController.aaFilterLength
                        onValueModified: DspController.aaFilterLength = value
                    }
                    LSwitch {
                        Layout.columnSpan: 2
                        text: qsTr("Use Quickseek (faster, slightly lower quality)")
                        checked: DspController.useQuickSeek
                        onToggled: DspController.useQuickSeek = checked
                    }
                    Label { text: qsTr("Sequence Length"); color: Theme.text }
                    SettingSpin {
                        autoAtZero: true
                        from: 0
                        to: 500
                        value: DspController.sequenceMs
                        onValueModified: DspController.sequenceMs = value
                    }
                    Label { text: qsTr("Seek Window Length"); color: Theme.text }
                    SettingSpin {
                        autoAtZero: true
                        from: 0
                        to: 250
                        value: DspController.seekWindowMs
                        onValueModified: DspController.seekWindowMs = value
                    }
                    Label { text: qsTr("Overlap Length"); color: Theme.text }
                    SettingSpin {
                        from: 1
                        to: 100
                        value: DspController.overlapMs
                        onValueModified: DspController.overlapMs = value
                    }
                }
            }

            RowLayout {
                Layout.fillWidth: true
                Layout.margins: 16
                Layout.topMargin: 0
                Layout.bottomMargin: 12
                PillButton {
                    iconName: "refresh"
                    text: qsTr("Reset to neutral")
                    onClicked: DspController.resetSoundTouch()
                }
                Item { Layout.fillWidth: true }
                PillButton {
                    primary: true
                    text: qsTr("Done")
                    onClicked: win.close()
                }
            }
        }
    }
}
