pragma Singleton
import QtQuick
import QtCore
import Sonkore

// Skins. Every colour, radius and density value used by the UI comes from
// here, so a skin is just one entry in `skins`.
QtObject {
    id: theme

    property Settings store: Settings {
        category: "appearance"
        property int skinIndex: 0          // index into skins; the default is the first, "Sonkore" (defaultSkin)
        property string accentOverride: ""   // "" = the skin's own accent
        property string density: "comfortable"     // interface (menus, bars, buttons): spacious | comfortable | compact ("auto": the skin's)
        property string listDensity: "comfortable" // track lists: spacious | comfortable | compact | list ("auto": as the interface)
        property bool listCovers: true
        property bool alternateRows: false
        property string backdrop: "plain"    // plain | aurora | gradient | glow
        property int windowRadius: 8         // corners of the integrated window, 0 = square
        property string windowOutline: "highlight" // off | line | bold | highlight | accent | glow (WindowOutline)
        property string windowButtons: "windows" // windows | mac | gnome | breeze | cinnamon | minimal (TitleBarButton)
        property string sliderStyle: "line"  // progress bars: line | pill | thick | classic | glow | led | ring | wave (LSlider)
        property string volumeStyle: "line"  // volume and every other slider: the same but wave
        property string iconStyle: "solid"   // line | solid (qml/icons/solid/) | tile (line icons on a soft square); anything else = solid
        property string transportStyle: "sonkore" // sonkore | round | outline: the play button's shape (PlayButton)
        property string invertedSkins: ""  // skins (by name, comma-separated) whose frame and lists' panel swap tones
    }

    // --- The mini player's own appearance ---------------------------------
    // Only what was changed for it (key → value, JSON); everything else
    // follows the full view. Edited in Settings › Appearance with "Mini
    // player" chosen at the top.
    property Settings compactStore: Settings {
        category: "appearanceCompact"
        property string values: ""
    }
    readonly property var compactValues: {
        try {
            const v = JSON.parse(compactStore.values)
            if (v && typeof v === "object")
                return v
        } catch (e) {}
        return {}
    }
    // The settings that can differ (the visualizer's lives in Prefs).
    readonly property var compactKeys: ["skinIndex", "accentOverride", "density", "listDensity", "listCovers",
                                        "alternateRows", "backdrop", "windowRadius", "windowOutline",
                                        "windowButtons", "sliderStyle", "volumeStyle", "iconStyle", "transportStyle", "visualizer"]
    readonly property bool compactActive: Prefs.store.compactMode
    function fullValue(key) { return key === "visualizer" ? Prefs.store.visualizer : store[key] }
    // A setting as the window shows it now.
    function setting(key) {
        return compactActive && compactValues[key] !== undefined ? compactValues[key] : fullValue(key)
    }
    // Settings › Appearance: which one is being edited.
    property bool editingCompact: false
    function isOwn(key) { return compactValues[key] !== undefined }
    function edited(key) {
        return editingCompact && compactValues[key] !== undefined ? compactValues[key] : fullValue(key)
    }
    function edit(key, value) {
        if (editingCompact) {
            const v = Object.assign({}, compactValues)
            v[key] = value
            compactStore.values = JSON.stringify(v)
        } else if (key === "visualizer") {
            Prefs.store.visualizer = value
        } else {
            store[key] = value
        }
    }
    // List density as edited, "auto" resolved as for the window.
    readonly property string editedListDensity: {
        const v = edited("listDensity")
        if (["spacious", "comfortable", "compact", "list"].indexOf(v) >= 0)
            return v
        const d = edited("density")
        return d === "compact" || (d === "auto" && skin.compact) ? "compact" : d === "spacious" ? "spacious" : "comfortable"
    }
    // Back to the full view's value.
    function inherit(key) {
        const v = Object.assign({}, compactValues)
        delete v[key]
        compactStore.values = JSON.stringify(v)
    }

    readonly property var accentChoices: ["", "#33d6c8", "#1ed760", "#0a84ff", "#5e5ce6", "#bd93f9",
                                          "#fa2d48", "#ff6ac1", "#ff9f0a", "#f5c518", "#8e8e93"]

    // Kept in this order: the stored skin is an index into this list.
    readonly property var skins: [
        // The default (defaultSkin), listed first.
        { name: "Sonkore", hint: qsTr("Deep black · green"), dark: true, radius: 8, compact: false,
          bg: "#0a0a0c", sidebar: "#0e0e11", surface: "#141418", surfaceHi: "#1c1c21", hover: "#22222a",
          border: "#25252c", text: "#f4f4f6", textDim: "#9b9ba5", textFaint: "#5d5d67",
          accent: "#1ed760", onAccent: "#000000", good: "#1ed760", warn: "#f5a524" },
        { name: "Verdant", hint: qsTr("Charcoal · green"), dark: true, radius: 8, compact: false,
          bg: "#121212", sidebar: "#000000", surface: "#181818", surfaceHi: "#232323", hover: "#2a2a2a",
          border: "#282828", text: "#ffffff", textDim: "#b3b3b3", textFaint: "#6a6a6a",
          accent: "#1ed760", onAccent: "#000000", good: "#1ed760", warn: "#f59b23" },
        { name: "Porcelain", hint: qsTr("Bright · blue"), dark: false, radius: 12, compact: false,
          bg: "#f5f5f7", sidebar: "#ebebee", surface: "#ffffff", surfaceHi: "#f0f0f3", hover: "#e6e6eb",
          border: "#d8d8dd", text: "#1d1d1f", textDim: "#6e6e73", textFaint: "#a1a1a6",
          accent: "#007aff", onAccent: "#ffffff", good: "#1f9d55", warn: "#c47a00" },
        { name: "Slate", hint: qsTr("Dark grey · blue"), dark: true, radius: 12, compact: false,
          bg: "#1c1c1e", sidebar: "#232325", surface: "#2a2a2c", surfaceHi: "#353537", hover: "#3a3a3c",
          border: "#3a3a3c", text: "#f5f5f7", textDim: "#98989d", textFaint: "#636366",
          accent: "#0a84ff", onAccent: "#ffffff", good: "#30d158", warn: "#ff9f0a" },
        { name: "Crimson", hint: qsTr("Dark · red"), dark: true, radius: 10, compact: false,
          bg: "#141416", sidebar: "#1b1b1e", surface: "#1f1f22", surfaceHi: "#29292d", hover: "#2e2e33",
          border: "#2f2f34", text: "#f6f6f7", textDim: "#a0a0a8", textFaint: "#62626a",
          accent: "#fa2d48", onAccent: "#ffffff", good: "#30d158", warn: "#ff9f0a" },
        { name: "Graphite", hint: qsTr("Soft grey"), dark: false, radius: 10, compact: false,
          bg: "#e8e8ea", sidebar: "#dddde0", surface: "#f6f6f7", surfaceHi: "#ececee", hover: "#e0e0e4",
          border: "#cdcdd2", text: "#232326", textDim: "#66666d", textFaint: "#9c9ca3",
          accent: "#5b6675", onAccent: "#ffffff", good: "#2f8f53", warn: "#b5710b" },
        { name: "Fjord", hint: qsTr("Cool blue-grey"), dark: true, radius: 8, compact: false,
          bg: "#2e3440", sidebar: "#2a2f3a", surface: "#353c4a", surfaceHi: "#3e4555", hover: "#454d5f",
          border: "#4c566a", text: "#eceff4", textDim: "#a8b0be", textFaint: "#6d7689",
          accent: "#88c0d0", onAccent: "#2e3440", good: "#a3be8c", warn: "#ebcb8b" },
        { name: "Nocturne", hint: qsTr("Ink · violet"), dark: true, radius: 8, compact: false,
          bg: "#282a36", sidebar: "#21222c", surface: "#2f3140", surfaceHi: "#383a4a", hover: "#44475a",
          border: "#44475a", text: "#f8f8f2", textDim: "#abadc4", textFaint: "#6272a4",
          accent: "#bd93f9", onAccent: "#21222c", good: "#50fa7b", warn: "#ffb86c" },
        { name: "Parchment", hint: qsTr("Warm light"), dark: false, radius: 6, compact: false,
          bg: "#fdf6e3", sidebar: "#eee8d5", surface: "#fffbef", surfaceHi: "#f5efdc", hover: "#eee8d5",
          border: "#e0d9c3", text: "#073642", textDim: "#586e75", textFaint: "#93a1a1",
          accent: "#268bd2", onAccent: "#ffffff", good: "#859900", warn: "#cb4b16" },
        { name: "Obsidian", hint: qsTr("True black · amber"), dark: true, radius: 10, compact: false,
          bg: "#000000", sidebar: "#000000", surface: "#0a0a0a", surfaceHi: "#121212", hover: "#181818",
          border: "#1c1c1c", text: "#ffffff", textDim: "#8c8c8c", textFaint: "#4d4d4d",
          accent: "#ff9f0a", onAccent: "#000000", good: "#30d158", warn: "#ff9f0a" },
        { name: "Dusk", hint: qsTr("Muted plum · rose"), dark: true, radius: 10, compact: false,
          bg: "#191724", sidebar: "#1f1d2e", surface: "#211f31", surfaceHi: "#26233a", hover: "#2c2944",
          border: "#2e2b42", text: "#e0def4", textDim: "#908caa", textFaint: "#6e6a86",
          accent: "#ebbcba", onAccent: "#191724", good: "#9ccfd8", warn: "#f6c177" },
        { name: "Classic", hint: qsTr("Compact desktop"), dark: false, radius: 2, compact: true,
          bg: "#f0f0f0", sidebar: "#e3e3e3", surface: "#ffffff", surfaceHi: "#f3f3f3", hover: "#e5f1fb",
          border: "#c9c9c9", text: "#111111", textDim: "#4a4a4a", textFaint: "#8a8a8a",
          accent: "#0078d7", onAccent: "#ffffff", good: "#118a3c", warn: "#b86e00" },
        { name: "Ocean", hint: qsTr("Navy · cyan"), dark: true, radius: 10, compact: false,
          bg: "#0b1622", sidebar: "#0d1a28", surface: "#11202f", surfaceHi: "#172a3c", hover: "#1c3247",
          border: "#1f3549", text: "#e6f1fa", textDim: "#8fa9bf", textFaint: "#557089",
          accent: "#3fb6ff", onAccent: "#00223a", good: "#3ddc97", warn: "#ffb547" },
        { name: "Forest", hint: qsTr("Deep green · moss"), dark: true, radius: 10, compact: false,
          bg: "#0f1712", sidebar: "#121c16", surface: "#16231b", surfaceHi: "#1d2d23", hover: "#22352a",
          border: "#263a2e", text: "#e8f2ea", textDim: "#9cb5a3", textFaint: "#5f7a67",
          accent: "#7bd88f", onAccent: "#0b2412", good: "#7bd88f", warn: "#e8b04a" },
        { name: "Ember", hint: qsTr("Warm dark · orange"), dark: true, radius: 10, compact: false,
          bg: "#161210", sidebar: "#1b1613", surface: "#211a16", surfaceHi: "#2a211c", hover: "#322720",
          border: "#352a23", text: "#f6ece6", textDim: "#b6a196", textFaint: "#75645b",
          accent: "#ff7a3d", onAccent: "#2a0e00", good: "#7fd17a", warn: "#ffb547" },
        { name: "Mocha", hint: qsTr("Coffee · cream"), dark: true, radius: 12, compact: false,
          bg: "#1a1411", sidebar: "#1f1814", surface: "#251d18", surfaceHi: "#2e241e", hover: "#362a23",
          border: "#3a2e26", text: "#f3e9df", textDim: "#bba895", textFaint: "#7d6c5d",
          accent: "#d9a066", onAccent: "#2a1806", good: "#9ccf7d", warn: "#e8a33d" },
        { name: "Aurora", hint: qsTr("Night · violet & mint"), dark: true, radius: 12, compact: false,
          bg: "#0d0b1a", sidebar: "#110e22", surface: "#15122b", surfaceHi: "#1c1838", hover: "#231e45",
          border: "#272149", text: "#ece9ff", textDim: "#a39fcf", textFaint: "#625d8f",
          accent: "#5ef2d6", onAccent: "#04201b", good: "#5ef2d6", warn: "#ffc46b" },
        { name: "Neon", hint: qsTr("Synth night · magenta"), dark: true, radius: 8, compact: false,
          bg: "#0c0814", sidebar: "#100a1b", surface: "#160e24", surfaceHi: "#1e132f", hover: "#26183b",
          border: "#2c1c45", text: "#f7ecff", textDim: "#b9a4d6", textFaint: "#6e5a8c",
          accent: "#ff3ea5", onAccent: "#ffffff", good: "#3cf0a0", warn: "#ffb02e" },
        { name: "Sakura", hint: qsTr("Light · pink"), dark: false, radius: 12, compact: false,
          bg: "#fdf7f9", sidebar: "#f7ecf0", surface: "#ffffff", surfaceHi: "#f9eff3", hover: "#f3e2e9",
          border: "#ead5de", text: "#2b1d23", textDim: "#7a6069", textFaint: "#b49aa4",
          accent: "#e0457b", onAccent: "#ffffff", good: "#2e9d62", warn: "#c77700" },
        { name: "Sand", hint: qsTr("Light · warm beige"), dark: false, radius: 10, compact: false,
          bg: "#f4efe6", sidebar: "#ebe4d8", surface: "#fbf8f2", surfaceHi: "#f1ebe1", hover: "#e8e0d2",
          border: "#dcd2c1", text: "#2e2a24", textDim: "#6f665a", textFaint: "#a39886",
          accent: "#b8742a", onAccent: "#ffffff", good: "#4f8a3c", warn: "#b5570b" },
        { name: "Lavender", hint: qsTr("Light · violet"), dark: false, radius: 12, compact: false,
          bg: "#f6f4fb", sidebar: "#ece8f6", surface: "#ffffff", surfaceHi: "#f1eef9", hover: "#e7e2f5",
          border: "#dcd5ef", text: "#221d33", textDim: "#67608a", textFaint: "#a19bbd",
          accent: "#7c5cff", onAccent: "#ffffff", good: "#2f9d6a", warn: "#c27400" },
        { name: "Glacier", hint: qsTr("Light · icy blue"), dark: false, radius: 10, compact: false,
          bg: "#f2f7fb", sidebar: "#e6eff6", surface: "#ffffff", surfaceHi: "#edf4fa", hover: "#e1ecf5",
          border: "#d2e1ec", text: "#14222e", textDim: "#557085", textFaint: "#93a9b9",
          accent: "#0f8fd6", onAccent: "#ffffff", good: "#22955a", warn: "#c27400" },
        { name: "Paper", hint: qsTr("High contrast · light"), dark: false, radius: 4, compact: false,
          bg: "#ffffff", sidebar: "#f4f4f4", surface: "#ffffff", surfaceHi: "#f2f2f2", hover: "#ebebeb",
          border: "#d0d0d0", text: "#000000", textDim: "#444444", textFaint: "#8a8a8a",
          accent: "#111111", onAccent: "#ffffff", good: "#1b7f3b", warn: "#a35a00" },
        { name: "Carbon", hint: qsTr("High contrast · dark"), dark: true, radius: 4, compact: false,
          bg: "#111111", sidebar: "#0b0b0b", surface: "#171717", surfaceHi: "#1f1f1f", hover: "#262626",
          border: "#2c2c2c", text: "#ffffff", textDim: "#b0b0b0", textFaint: "#6b6b6b",
          accent: "#ffffff", onAccent: "#000000", good: "#4ade80", warn: "#fbbf24" },
        // Code-editor style palettes
        { name: "Studio", hint: qsTr("Editor dark · blue"), dark: true, radius: 6, compact: false,
          bg: "#1f1f1f", sidebar: "#181818", surface: "#252526", surfaceHi: "#2d2d30", hover: "#2a2d2e",
          border: "#2b2b2b", text: "#cccccc", textDim: "#9d9d9d", textFaint: "#6e6e6e",
          accent: "#0078d4", onAccent: "#ffffff", good: "#89d185", warn: "#cca700" },
        { name: "Daylight", hint: qsTr("Editor light · blue"), dark: false, radius: 6, compact: false,
          bg: "#ffffff", sidebar: "#f8f8f8", surface: "#ffffff", surfaceHi: "#f3f3f3", hover: "#ebebeb",
          border: "#e5e5e5", text: "#1f1f1f", textDim: "#616161", textFaint: "#9e9e9e",
          accent: "#005fb8", onAccent: "#ffffff", good: "#388a34", warn: "#bf8803" },
        { name: "Ironwood", hint: qsTr("IDE dark · orange"), dark: true, radius: 4, compact: false,
          bg: "#2b2b2b", sidebar: "#3c3f41", surface: "#313335", surfaceHi: "#393b3d", hover: "#4b4d4f",
          border: "#323232", text: "#bbbbbb", textDim: "#8c8c8c", textFaint: "#606366",
          accent: "#cc7832", onAccent: "#1b1b1b", good: "#6a8759", warn: "#bbb529" },
        { name: "Islands", hint: qsTr("IDE dark · cobalt"), dark: true, radius: 10, compact: false,
          bg: "#1e1f22", sidebar: "#191a1c", surface: "#2b2d30", surfaceHi: "#313438", hover: "#2e3033",
          border: "#393b40", text: "#dfe1e5", textDim: "#9da0a8", textFaint: "#6f737a",
          accent: "#3574f0", onAccent: "#ffffff", good: "#5fb865", warn: "#e0a33e" },
        { name: "Polar", hint: qsTr("Arctic night · frost"), dark: true, radius: 8, compact: false,
          bg: "#2e3440", sidebar: "#272c36", surface: "#3b4252", surfaceHi: "#434c5e", hover: "#3b4252",
          border: "#3f4655", text: "#eceff4", textDim: "#a9b1c1", textFaint: "#6c768a",
          accent: "#88c0d0", onAccent: "#2e3440", good: "#a3be8c", warn: "#ebcb8b" },
        { name: "Velvet", hint: qsTr("Night · purple & pink"), dark: true, radius: 8, compact: false,
          bg: "#282a36", sidebar: "#21222c", surface: "#2f3241", surfaceHi: "#383a4a", hover: "#343746",
          border: "#3a3d4e", text: "#f8f8f2", textDim: "#b2b4c4", textFaint: "#6272a4",
          accent: "#bd93f9", onAccent: "#21222c", good: "#50fa7b", warn: "#ffb86c" },
        { name: "Citrus", hint: qsTr("Olive night · lime"), dark: true, radius: 6, compact: false,
          bg: "#272822", sidebar: "#1e1f1c", surface: "#2f3029", surfaceHi: "#3a3b33", hover: "#34352d",
          border: "#3b3c34", text: "#f8f8f2", textDim: "#b3b29f", textFaint: "#75715e",
          accent: "#a6e22e", onAccent: "#1e1f1c", good: "#a6e22e", warn: "#fd971f" },
        { name: "Solstice", hint: qsTr("Deep teal · blue"), dark: true, radius: 6, compact: false,
          bg: "#002b36", sidebar: "#00232c", surface: "#073642", surfaceHi: "#0b3f4d", hover: "#0a3c48",
          border: "#0d4250", text: "#eee8d5", textDim: "#93a1a1", textFaint: "#657b83",
          accent: "#268bd2", onAccent: "#fdf6e3", good: "#859900", warn: "#b58900" },
        { name: "Linen", hint: qsTr("Warm light · blue"), dark: false, radius: 6, compact: false,
          bg: "#fdf6e3", sidebar: "#eee8d5", surface: "#fffbef", surfaceHi: "#f5efdc", hover: "#ece5d0",
          border: "#e0d9c3", text: "#073642", textDim: "#586e75", textFaint: "#93a1a1",
          accent: "#268bd2", onAccent: "#ffffff", good: "#738a05", warn: "#b58900" },
        { name: "Harvest", hint: qsTr("Retro brown · amber"), dark: true, radius: 6, compact: false,
          bg: "#282828", sidebar: "#1d2021", surface: "#32302f", surfaceHi: "#3c3836", hover: "#3a3634",
          border: "#3c3836", text: "#ebdbb2", textDim: "#bdae93", textFaint: "#7c6f64",
          accent: "#fabd2f", onAccent: "#1d2021", good: "#b8bb26", warn: "#fe8019" },
        { name: "Metro", hint: qsTr("City night · periwinkle"), dark: true, radius: 10, compact: false,
          bg: "#1a1b26", sidebar: "#16161e", surface: "#1f2335", surfaceHi: "#24283b", hover: "#222538",
          border: "#292e42", text: "#c0caf5", textDim: "#9aa5ce", textFaint: "#565f89",
          accent: "#7aa2f7", onAccent: "#16161e", good: "#9ece6a", warn: "#e0af68" },
        { name: "Cocoa", hint: qsTr("Soft night · lilac"), dark: true, radius: 12, compact: false,
          bg: "#1e1e2e", sidebar: "#181825", surface: "#262637", surfaceHi: "#313244", hover: "#2a2b3c",
          border: "#313244", text: "#cdd6f4", textDim: "#a6adc8", textFaint: "#6c7086",
          accent: "#cba6f7", onAccent: "#1e1e2e", good: "#a6e3a1", warn: "#fab387" },
        { name: "Harbor", hint: qsTr("Slate night · sky"), dark: true, radius: 8, compact: false,
          bg: "#282c34", sidebar: "#21252b", surface: "#2c313a", surfaceHi: "#333842", hover: "#2f343e",
          border: "#3a3f4b", text: "#d7dae0", textDim: "#abb2bf", textFaint: "#636d83",
          accent: "#61afef", onAccent: "#21252b", good: "#98c379", warn: "#e5c07b" },
        { name: "Abyss", hint: qsTr("Deep navy · bright blue"), dark: true, radius: 8, compact: false,
          bg: "#0d1117", sidebar: "#010409", surface: "#161b22", surfaceHi: "#1c2128", hover: "#1a1f27",
          border: "#30363d", text: "#e6edf3", textDim: "#9198a1", textFaint: "#656c76",
          accent: "#2f81f7", onAccent: "#ffffff", good: "#3fb950", warn: "#d29922" },
        // The app icon's colours: its graphite base and its green. Listed
        // second (see skinOrder), though added last: the stored skin is an
        // index.
        { name: "Basalt", hint: qsTr("Graphite · green"), dark: true, radius: 8, compact: false,
          bg: "#141418", sidebar: "#111215", surface: "#1a1a1f", surfaceHi: "#222228", hover: "#29292f",
          border: "#2b2b32", text: "#f4f4f6", textDim: "#9e9ea8", textFaint: "#60606a",
          accent: "#24e86c", onAccent: "#000000", good: "#24e86c", warn: "#f5a524" }
    ]
    readonly property int defaultSkin: 0
    // Order of the skins in Settings: the default first, the graphite one
    // (the app icon's) next, then the list.
    readonly property int graphiteSkin: skins.length - 1
    readonly property var skinOrder: [defaultSkin, graphiteSkin].concat(skins.map((s, i) => i).filter(i => i !== defaultSkin && i !== graphiteSkin))

    readonly property int skinIndex: Math.max(0, Math.min(setting("skinIndex"), skins.length - 1))
    readonly property var skin: skins[skinIndex]

    readonly property string name: skin.name
    readonly property bool dark: skin.dark
    // Background style: colour fields drawn by the window under the window-level
    // panels (pages, sidebar, player bar), which then let them show through;
    // cards, menus and text stay opaque.
    readonly property string backdrop: setting("backdrop")
    readonly property bool decorated: backdrop.length > 0 && backdrop !== "plain"
    readonly property real panelAlpha: decorated ? 0.55 : 1
    readonly property bool translucent: panelAlpha < 1
    function fade(c, a) { const k = Qt.darker(c, 1.0); return Qt.rgba(k.r, k.g, k.b, a) }
    readonly property color bg: translucent ? fade(skin.bg, panelAlpha) : skin.bg
    // Always opaque: secondary windows (Settings, Equalizer...) and anything
    // drawn on the accent. The background style belongs to the main window.
    readonly property color solidBg: skin.bg
    // The frame (top bar, sidebar, player bar) and the lists' panel in it
    // (the page, the mini's list), the panel darker than the frame: the
    // skin's page colour when it already is, else a step down from the
    // sidebar. A black sidebar cannot go darker: then the page colour, or
    // the surface when that is black too. "Invert" (per skin, Settings)
    // swaps the two.
    function isInverted(s) { return store.invertedSkins.split(",").indexOf(s.name) >= 0 }
    function toggleInverted(s) {
        const names = store.invertedSkins.split(",").filter(n => n.length > 0 && n !== s.name)
        if (!isInverted(s))
            names.push(s.name)
        store.invertedSkins = names.join(",")
    }
    function tones(s) {
        // the skins' colours are strings: as colours, to read their channels
        const bg = Qt.darker(s.bg, 1.0), side = Qt.darker(s.sidebar, 1.0)
        // at least this much apart (about 4 steps of 255), or the panel is lost
        const apart = c => luminance(side) - luminance(c) >= 0.0157
        let pane = luminance(bg) < luminance(side) ? bg : Qt.darker(side, s.dark ? 1.35 : 1.05)
        if (!apart(pane))
            pane = Qt.darker(side, 1.6)
        if (!apart(pane)) // a sidebar about black: the panel a step lighter instead
            pane = luminance(bg) - luminance(side) >= 0.0157 ? bg : Qt.darker(s.surface, 1.0)
        return isInverted(s) ? { frame: pane, pane: side } : { frame: side, pane: pane }
    }
    readonly property var skinTones: tones(skin)
    readonly property color sidebar: translucent ? fade(skinTones.frame, panelAlpha) : skinTones.frame
    readonly property color pane: translucent ? fade(skinTones.pane, panelAlpha) : skinTones.pane
    readonly property color surface: skin.surface
    readonly property color surfaceHi: skin.surfaceHi
    readonly property color hover: skin.hover
    readonly property color border: skin.border
    readonly property color text: skin.text
    readonly property color textDim: skin.textDim
    readonly property color textFaint: skin.textFaint
    readonly property string accentOverride: setting("accentOverride")
    readonly property bool accentOverridden: accentOverride.length > 0
    readonly property color accent: accentOverridden ? accentOverride : skin.accent
    readonly property color onAccent: accentOverridden ? (luminance(accent) > 0.55 ? "#000000" : "#ffffff") : skin.onAccent
    readonly property color good: skin.good
    readonly property color warn: skin.warn
    readonly property color accentSoft: Qt.rgba(accent.r, accent.g, accent.b, dark ? 0.16 : 0.12)

    readonly property int radius: skin.radius
    readonly property int radiusSmall: Math.max(2, Math.round(skin.radius * 0.6))
    // Interface density (menus, bars, buttons) and track-list density are
    // separate settings; the lists follow the interface unless set.
    readonly property string density: setting("density")
    readonly property bool compact: density === "auto" ? skin.compact : density === "compact"
    readonly property bool spacious: density === "spacious" // roomier bars, menus and buttons
    // Square controls (switches, pills, badges): part of a skin's look (the
    // Classic desktop skin), not of the density.
    readonly property bool squareShapes: skin.radius <= 2
    readonly property string listDensity: ["spacious", "comfortable", "compact", "list"].indexOf(setting("listDensity")) >= 0
                                          ? setting("listDensity") : (compact ? "compact" : spacious ? "spacious" : "comfortable")
    readonly property bool listSpacious: listDensity === "spacious" // taller rows, bigger covers
    readonly property bool listCompact: listDensity === "compact" || listDensity === "list" // short rows, no covers
    // List: classic-player track lists (short flat rows, no covers, plain text).
    readonly property bool listMode: listDensity === "list"
    readonly property bool listCovers: setting("listCovers") && !listCompact
    readonly property bool alternateRows: setting("alternateRows")
    readonly property color rowAlt: Qt.rgba(text.r, text.g, text.b, dark ? 0.025 : 0.03)
    readonly property int rowHeight: listMode ? 22 : listCompact ? 30 : listSpacious ? 68 : 56
    // How much of a list row shows while it scrolls up under a head:
    // `hidden` px of it are past the edge, its content (`content` px tall,
    // centred) is what reads. Whole until that content starts being cut,
    // gone once it is half cut, so no sliver of text stays under the head
    // and no row that is still whole looks dimmed (an overlay fade could
    // not avoid both).
    function rowFade(hidden, rowHeight, content) {
        const cut = hidden - (rowHeight - content) / 2
        return Math.max(0, Math.min(1, 1 - cut / (content / 2)))
    }
    readonly property int spacing: compact ? 6 : spacious ? 16 : 12

    readonly property int fontSmall: 11
    readonly property int fontBody: 13
    readonly property int fontTitle: 15

    function setSkin(index) {
        edit("skinIndex", index)
    }

    function luminance(c) {
        return 0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b
    }

    // Icon style (Settings › Appearance): filled glyphs, or the line ones
    // (also used by "tile", which sets them on a soft square, see IconButton).
    readonly property string windowButtons: ["windows", "mac", "gnome", "breeze", "cinnamon", "minimal"].indexOf(setting("windowButtons")) >= 0
                                            ? setting("windowButtons") : "windows"
    readonly property bool iconTiles: setting("iconStyle") === "tile"
    readonly property string sliderStyle: ["line", "pill", "thick", "classic", "glow", "led", "ring", "wave"].indexOf(setting("sliderStyle")) >= 0
                                          ? setting("sliderStyle") : "line"
    readonly property string volumeStyle: ["line", "pill", "thick", "classic", "glow", "led", "ring"].indexOf(setting("volumeStyle")) >= 0
                                          ? setting("volumeStyle") : "line"
    readonly property bool lineIcons: setting("iconStyle") === "line" || iconTiles
    // A tiling window manager lays the window out edge to edge: square
    // corners and no outline of its own (it draws its borders).
    readonly property int windowRadius: App.tilingWindowManager ? 0 : setting("windowRadius")
    readonly property string windowOutline: App.tilingWindowManager ? "off" : setting("windowOutline")
    readonly property string visualizer: setting("visualizer")
    // Player buttons (Settings › Appearance): the play button's shape (the
    // glyphs follow the icon style, like every other button).
    readonly property var transportStyles: ["sonkore", "round", "outline"]
    readonly property string transportStyle: transportStyles.indexOf(setting("transportStyle")) >= 0
                                             ? setting("transportStyle") : "sonkore"
    function icon(name, style) {
        const line = style !== undefined ? style === "line" || style === "tile" : lineIcons
        return Qt.resolvedUrl((line ? "icons/" : "icons/solid/") + name + ".svg")
    }
}
