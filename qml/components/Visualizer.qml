import QtQuick
import Sonkore

// Live spectrum of what is playing (24 bands from DspController.spectrum),
// moved on every Clock tick and drawn by SpectrumView in one anti-aliased
// picture: fast attack, gentle fall, peak caps on the bars. Styles: bars,
// mirror, line, dots. Costs nothing while hidden or silent.
SpectrumView {
    id: viz

    color: Theme.accent
    peaks: style === "bars"
    target: DspController.spectrum
    implicitWidth: 96
    implicitHeight: 36

    // Asks the engine for the spectrum only while something can be seen. Each
    // visualiser (the full view's, the compact player's) counts on its own.
    readonly property bool inView: viz.visible && viz.width > 0
    onInViewChanged: DspController.watchSpectrum(viz, inView)
    Component.onCompleted: DspController.watchSpectrum(viz, inView)

    // Moved by the shared Clock, on the same frames as everything else that
    // moves, until it is at rest.
    readonly property bool animating: viz.visible && (DspController.playing || !viz.resting)
    Connections {
        target: Clock
        enabled: viz.animating
        function onTick(dt) { viz.advance(dt) }
    }
}
