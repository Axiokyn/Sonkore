import QtQuick
import Sonkore

// Mouse-wheel scrolling for a Flickable: a fixed, configurable distance per
// notch (Settings › Appearance) with a short ease, instead of Qt's ~24 px.
//
// Wayland reports every pointer as a "touchpad" device, so devices are told
// apart by the event itself: wheel notches carry only an angle delta, while
// touchpads (and other smooth sources) also carry a pixel delta, which is
// followed 1:1 so they keep their natural feel.
WheelHandler {
    id: wheel

    property Flickable flickable
    // false: a horizontal strip, where the plain wheel scrolls sideways.
    property bool vertical: true
    readonly property real step: Prefs.store.wheelStep
    property real goal: 0

    target: null
    acceptedDevices: PointerDevice.Mouse | PointerDevice.TouchPad
    onWheel: (event) => {
        const f = wheel.flickable
        if (!f)
            return
        const smooth = event.pixelDelta.x !== 0 || event.pixelDelta.y !== 0
        const delta = smooth ? event.pixelDelta : event.angleDelta
        // Shift + wheel, or a tilting wheel, scrolls sideways.
        let sideways = (event.modifiers & Qt.ShiftModifier) || Math.abs(delta.x) > Math.abs(delta.y)
        let amount = sideways && delta.x !== 0 ? delta.x : delta.y
        if (!wheel.vertical)
            sideways = true
        if (amount === 0)
            return
        const prop = sideways ? "contentX" : "contentY"
        const origin = sideways ? f.originX : f.originY
        const extent = sideways ? f.contentWidth - f.width : f.contentHeight - f.height
        if (extent <= 0)
            return
        const clamp = (v) => Math.max(origin, Math.min(origin + extent, v))
        if (smooth) {
            scroll.stop()
            f[prop] = clamp(f[prop] - amount)
            return
        }
        if (scroll.property !== prop) {
            scroll.stop()
            scroll.property = prop
        }
        const from = scroll.running ? wheel.goal : f[prop]
        wheel.goal = clamp(from - amount / 120 * wheel.step)
        scroll.to = wheel.goal
        scroll.restart()
    }

    property NumberAnimation scroll: NumberAnimation {
        target: wheel.flickable
        property: "contentY"
        duration: 140
        easing.type: Easing.OutCubic
    }
}
