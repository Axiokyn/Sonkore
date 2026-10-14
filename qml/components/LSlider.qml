import QtQuick
import QtQuick.Controls
import Sonkore

// Themed slider (seek, volume and the rest), in the style chosen in
// Settings › Appearance › Progress bar (seek bars, `seekBar`) or Volume bar
// (every other slider) (`sliderStyle`, the Theme's):
//   line      a thin rounded line, the knob on hover
//   pill      a rounded bar with a knob always shown (MusicBee)
//   thick     a thick rounded bar, no knob
//   classic   a square-edged inset groove with a block thumb (Winamp)
//   glow      a line filled with a fading accent, a glowing dot
//   led       lit segments, like a level meter
//   ring      a hairline with a hollow ring knob
//   wave      the track's waveform, played part in the accent (foobar2000)
// The waveform needs `seekBar`: other sliders
// (volume…) draw them as "line".
Slider {
    id: control

    property color fillColor: Theme.accent
    property bool alwaysShowHandle: false
    // A seek bar: the progress bar's style, and the playing track's waveform
    // (WaveformController) for the "wave" style, a flat line while it is
    // being read.
    property bool seekBar: false
    property string sliderStyle: seekBar ? Theme.sliderStyle : Theme.volumeStyle // Settings previews pass their own
    property var peaks: seekBar ? Waveform.peaks : []
    readonly property bool waveStyle: sliderStyle === "wave"
    readonly property string look: waveStyle && !seekBar ? "line" : sliderStyle
    readonly property bool waveLook: look === "wave"

    readonly property bool active: hovered || pressed
    // Height of the bar at rest (NowPlayingBar lines the visualizer up on it).
    readonly property real restHeight: ({ line: 4, pill: 6, thick: 10, classic: 8, glow: 4, led: 10, ring: 3, wave: 20 })[look] || 4
    readonly property real trackHeight: look === "line" && active ? 5 : restHeight
    readonly property color fill: active ? fillColor : Qt.darker(fillColor, Theme.dark ? 1.05 : 1.0)
    // The empty part: a faint tint of the text, so it sits back on any surface.
    readonly property color groove: Qt.rgba(Theme.text.r, Theme.text.g, Theme.text.b, Theme.dark ? 0.08 : 0.13)
    // Styles with a knob at rest; the others show one on hover (line) or
    // only when asked (alwaysShowHandle).
    readonly property bool restingKnob: look === "pill" || look === "classic"
                                        || look === "glow" || look === "ring"

    hoverEnabled: true
    focusPolicy: Qt.NoFocus
    opacity: enabled ? 1 : 0.45
    // Every style the same height (the waveform fills it: no knob to make
    // room for), so changing it moves nothing around it.
    implicitHeight: waveLook ? 20 : Math.max(20, restHeight + 10)

    background: Item {
        x: control.leftPadding
        y: control.topPadding + control.availableHeight / 2 - height / 2
        width: control.availableWidth
        height: control.trackHeight

        // groove and fill (every style but led and the waveforms)
        RoundRect {
            visible: control.look !== "led" && !control.waveLook
            anchors.fill: parent
            radius: control.look === "classic" ? 2 : height / 2
            color: control.groove
            borderWidth: control.look === "classic" ? 1 : 0
            borderColor: Qt.rgba(0, 0, 0, Theme.dark ? 0.45 : 0.12)
            RoundRect {
                visible: control.look !== "glow"
                width: control.visualPosition * parent.width
                height: parent.height
                radius: parent.radius
                color: control.fill
            }
            Rectangle {
                visible: control.look === "glow"
                width: control.visualPosition * parent.width
                height: parent.height
                radius: parent.radius
                gradient: Gradient {
                    orientation: Gradient.Horizontal
                    GradientStop { position: 0; color: Qt.rgba(control.fill.r, control.fill.g, control.fill.b, 0.12) }
                    GradientStop { position: 1; color: control.fill }
                }
            }
        }
        // wave
        Item {
            visible: control.waveLook
            anchors.fill: parent
            WaveCanvas {
                anchors.fill: parent
                peaks: control.peaks
                tint: Qt.rgba(Theme.text.r, Theme.text.g, Theme.text.b, Theme.dark ? 0.22 : 0.28)
            }
            Item {
                width: control.visualPosition * parent.width
                height: parent.height
                clip: true
                WaveCanvas {
                    width: control.availableWidth
                    peaks: control.peaks
                        height: parent.height
                    tint: control.fill
                }
            }
        }
        // led: segments spread over the whole bar, lit continuously up to
        // the value (the segment at the edge lights in part)
        Item {
            id: segments
            visible: control.look === "led"
            anchors.fill: parent
            readonly property real gap: 2
            readonly property int count: Math.max(1, Math.floor((width + gap) / 6))
            readonly property real segment: (width - gap * (count - 1)) / count
            readonly property real litTo: control.visualPosition * width
            Repeater {
                model: segments.visible ? segments.count : 0
                RoundRect {
                    required property int index
                    x: index * (segments.segment + segments.gap)
                    width: segments.segment
                    height: segments.height
                    radius: 1
                    color: control.groove
                    RoundRect {
                        width: Math.max(0, Math.min(parent.width, segments.litTo - parent.x))
                        height: parent.height
                        radius: parent.radius
                        color: control.fill
                    }
                }
            }
        }
    }

    // The waveform, twice: the whole of it in the groove's colour, and over
    // it, cut at the position, in the fill's. Each is painted once (when the
    // track, size or colours change), not as the position moves.
    component WaveCanvas: Canvas {
        id: canvas
        property color tint
        property var peaks: []
        renderStrategy: Canvas.Cooperative
        antialiasing: true // fractional peak heights end in a soft pixel, not a step
        onPeaksChanged: requestPaint()
        onTintChanged: requestPaint()
        onWidthChanged: requestPaint()
        onHeightChanged: requestPaint()
        onPaint: {
            const ctx = getContext("2d")
            ctx.reset()
            ctx.fillStyle = tint
            const n = peaks.length
            const mid = height / 2
            // a 1 px line every 2 px, both ways from the middle
            const step = 2
            for (let x = 0; x + 1 <= width; x += step) {
                let v = 0
                if (n > 0) {
                    // the loudest of the buckets under this column
                    const a = Math.floor(x / width * n)
                    const b = Math.max(a + 1, Math.floor((x + step) / width * n))
                    for (let i = a; i < b && i < n; ++i)
                        v = Math.max(v, peaks[i])
                }
                const half = Math.max(0.5, v * mid)
                ctx.fillRect(x, mid - half, 1, half * 2)
            }
        }
    }

    handle: Item {
        readonly property string style: control.look
        property real size: style === "glow" ? 10 : style === "ring" ? 12 : control.pressed ? 15 : 13
        x: control.leftPadding + control.visualPosition * (control.availableWidth - width)
        y: control.topPadding + control.availableHeight / 2 - height / 2
        // the waveforms: a thin cursor across the whole wave
        readonly property bool cursor: control.waveLook
        width: cursor ? 2 : style === "classic" ? 8 : size
        height: cursor ? control.trackHeight + 4 : style === "classic" ? 16 : size
        visible: control.alwaysShowHandle || control.restingKnob || ((style === "line" || cursor) && control.active)
        Behavior on size { NumberAnimation { duration: 80 } }

        // glow: a soft halo around the dot
        RoundRect {
            visible: parent.style === "glow"
            anchors.centerIn: parent
            width: parent.width + (control.active ? 10 : 6)
            height: width
            radius: width / 2
            color: Qt.rgba(control.fill.r, control.fill.g, control.fill.b, 0.25)
        }
        RoundRect {
            anchors.fill: parent
            radius: parent.style === "classic" ? 2 : width / 2
            color: parent.cursor ? Theme.text
                 : parent.style === "ring" ? Theme.solidBg
                 : parent.style === "glow" ? control.fill
                 : Theme.dark ? "#ffffff" : Theme.surface
            borderWidth: parent.style === "ring" ? 2 : parent.cursor ? 0 : 1
            borderColor: parent.style === "ring" ? control.fill
                       : parent.style === "glow" ? "transparent"
                       : parent.style === "classic" ? Qt.rgba(0, 0, 0, Theme.dark ? 0.5 : 0.2)
                       : Theme.dark ? "transparent" : Theme.border
        }
    }
}
