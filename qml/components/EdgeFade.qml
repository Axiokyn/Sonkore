import QtQuick

// What scrolls up under a head fades out instead of being cut: `tint`
// (the surface behind) fully at the top, clear at the bottom. Eased at both
// ends, so it has no visible start or stop line across the rows under it.
// Keep it short (10-14 px): taller, it dims rows that are still whole.
Rectangle {
    property color tint

    color: "transparent"
    gradient: Gradient {
        GradientStop { position: 0.0;  color: Qt.rgba(tint.r, tint.g, tint.b, tint.a) }
        GradientStop { position: 0.2;  color: Qt.rgba(tint.r, tint.g, tint.b, tint.a * 0.90) }
        GradientStop { position: 0.4;  color: Qt.rgba(tint.r, tint.g, tint.b, tint.a * 0.65) }
        GradientStop { position: 0.6;  color: Qt.rgba(tint.r, tint.g, tint.b, tint.a * 0.35) }
        GradientStop { position: 0.8;  color: Qt.rgba(tint.r, tint.g, tint.b, tint.a * 0.10) }
        GradientStop { position: 1.0;  color: Qt.rgba(tint.r, tint.g, tint.b, 0) }
    }
}
