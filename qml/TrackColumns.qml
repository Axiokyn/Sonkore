pragma Singleton
import QtQuick
import QtCore

// Track-list columns: the catalogue, which ones are visible (and in which
// order), their widths, and how each value is displayed. Shared by every
// track list and persisted across sessions.
QtObject {
    id: columns

    readonly property var catalog: [
        { key: "title",       title: qsTr("Title"),        width: 260, flex: true },
        { key: "artist",      title: qsTr("Artist"),       width: 180 },
        { key: "album",       title: qsTr("Album"),        width: 210 },
        { key: "albumArtist", title: qsTr("Album artist"), width: 170 },
        { key: "genre",       title: qsTr("Genre"),        width: 120 },
        { key: "year",        title: qsTr("Year"),         width: 60 },
        { key: "trackNo",     title: qsTr("Track"),        width: 60 },
        { key: "discNo",      title: qsTr("Disc"),         width: 52 },
        { key: "duration",    title: qsTr("Time"),         width: 64 },
        { key: "format",      title: qsTr("Format"),       width: 119, badge: true },
        { key: "codec",       title: qsTr("Codec"),        width: 70 },
        { key: "sampleRate",  title: qsTr("Sample rate"),  width: 96 },
        { key: "bits",        title: qsTr("Bit depth"),    width: 80 },
        { key: "channels",    title: qsTr("Channels"),     width: 80 },
        { key: "bitrate",     title: qsTr("Bitrate"),      width: 121 },
        { key: "fileSize",    title: qsTr("Size"),         width: 84 },
        { key: "fileName",    title: qsTr("File name"),    width: 230 },
        { key: "folder",      title: qsTr("Folder"),       width: 260 },
        { key: "added",       title: qsTr("Date added"),   width: 110 },
        { key: "modified",    title: qsTr("Modified"),     width: 110 },
        // A heart per row; never the column that stretches.
        { key: "favorite",    title: qsTr("Favorite"),     width: 44, fixed: true }
    ]
    // Title, artist and album first (a file without tags shows its name as
    // the title); the format badge; time. Bitrate and file name are a click
    // away in the header's menu.
    readonly property var defaultKeys: ["title", "artist", "album", "format", "duration", "favorite"]
    // Space between a cell's text and the column dividers (header and rows).
    readonly property int cellPadding: 12

    property Settings store: Settings {
        category: "columns"
        property string keys: ""
        property string widths: ""
        // The favourite column is added once to lists saved before it existed.
        property bool favoriteAdded: false
    }

    // Visible columns in display order, and per-column widths (key -> px).
    property var keys: parseKeys()
    property var widths: parseWidths()

    function parseKeys() {
        try {
            const k = JSON.parse(store.keys)
            if (Array.isArray(k) && k.length > 0) {
                const valid = k.filter(x => info(x) !== null)
                if (valid.length > 0)
                    return valid
            }
        } catch (e) {}
        return defaultKeys.slice()
    }
    function parseWidths() {
        try {
            const w = JSON.parse(store.widths)
            if (w && typeof w === "object")
                return w
        } catch (e) {}
        return {}
    }

    // Not inside parseKeys(): writing the store from the `keys` binding would loop.
    Component.onCompleted: {
        if (store.favoriteAdded)
            return
        if (store.keys.length > 0 && !isVisible("favorite"))
            setVisible("favorite", true)
        store.favoriteAdded = true
    }

    function info(key) {
        for (const c of catalog)
            if (c.key === key)
                return c
        return null
    }
    function title(key) { const c = info(key); return c ? c.title : key }
    // The last column fills the space the others leave (its stored width is
    // the least it gets; beyond that the list scrolls sideways). Every other
    // column keeps its own width, so dragging a column's divider resizes just
    // that column and the ones after it move along.
    // A `fixed` column (the heart) keeps its width even when last: the last
    // other column stretches instead.
    readonly property string flexKey: {
        for (let i = keys.length - 1; i >= 0; --i)
            if (!(info(keys[i]) || {}).fixed)
                return keys[i]
        return keys.length > 0 ? keys[keys.length - 1] : ""
    }
    function isFlex(key) { return key.length > 0 && key === flexKey }
    // Never narrower than the column's title, so a drag acts from the first pixel.
    function minWidthOf(key) {
        const c = info(key)
        if (key === "favorite")
            return 40 // its head is a heart, not the word
        return c ? Math.max(48, headerWidth(c.title)) : 48
    }
    // Header text as drawn (SectionLabel): default widths grow to fit it,
    // so a longer translation is never cut off.
    // FontMetrics (not TextMetrics): measuring must not change any property,
    // or every binding that reads a column width loops.
    property FontMetrics headerMetrics: FontMetrics {
        font.pixelSize: 10
        font.weight: Font.Bold
        font.letterSpacing: 1.2
    }
    function headerWidth(text) {
        return Math.ceil(headerMetrics.advanceWidth(text.toUpperCase())) + 2 * cellPadding + 15 // room for the sort arrow
    }
    function widthOf(key) {
        const w = widths[key]
        // A saved width never cuts the header off (e.g. after a language change).
        if (w > 0)
            return Math.max(w, minWidthOf(key))
        const c = info(key)
        const base = c ? Math.max(c.width, minWidthOf(key)) : 100
        // The first column (usually title or file name) starts out wide.
        return keys.length > 1 && key === keys[0] ? Math.max(base, 829) : base
    }
    function isVisible(key) { return keys.indexOf(key) >= 0 }

    function setVisible(key, on) {
        if (!on && keys.length === 1 && keys[0] === key)
            return // keep at least one column
        let k = keys.slice()
        if (on && k.indexOf(key) < 0)
            k.push(key)
        if (!on)
            k = k.filter(x => x !== key)
        keys = k
        store.keys = JSON.stringify(k)
    }
    function move(key, delta) {
        moveTo(key, keys.indexOf(key) + delta)
    }
    // Moves a visible column to position `index` (0 = first).
    function moveTo(key, index) {
        const k = keys.slice()
        const i = k.indexOf(key)
        if (i < 0 || index < 0 || index >= k.length || index === i)
            return
        k.splice(i, 1)
        k.splice(index, 0, key)
        keys = k
        store.keys = JSON.stringify(k)
    }
    function clampWidth(key, w) {
        return Math.max(minWidthOf(key), Math.min(1200, Math.round(w)))
    }
    // Several widths at once (a divider drag resizes two columns); `persist`
    // false keeps it in memory until the drag ends (commitWidths()).
    function setWidths(map, persist) {
        const m = Object.assign({}, widths)
        for (const key in map)
            m[key] = clampWidth(key, map[key])
        widths = m
        if (persist !== false)
            commitWidths()
    }
    function commitWidths() {
        store.widths = JSON.stringify(widths)
    }
    function resetWidth(key) {
        const m = Object.assign({}, widths)
        delete m[key]
        widths = m
        commitWidths()
    }
    function reset() {
        keys = defaultKeys.slice()
        widths = {}
        store.keys = ""
        store.widths = ""
    }

    function humanSize(bytes) {
        if (!(bytes > 0))
            return ""
        const units = ["B", "KB", "MB", "GB"]
        let v = bytes, u = 0
        while (v >= 1024 && u < units.length - 1) { v /= 1024; ++u }
        return (u >= 2 ? v.toFixed(1) : Math.round(v)) + " " + units[u]
    }
    function timeText(seconds) {
        const s = Math.max(0, Math.round(seconds || 0))
        return Math.floor(s / 60) + ":" + String(s % 60).padStart(2, "0")
    }

    // Display text of `key` for one track (a model row or a track map).
    function text(key, d) {
        if (!d)
            return ""
        const v = d[key]
        switch (key) {
        case "duration": return d.durationText !== undefined ? d.durationText : timeText(v)
        case "year":
        case "trackNo":
        case "discNo": return v > 0 ? String(v) : ""
        case "sampleRate": return v > 0 ? (v / 1000) + " kHz" : ""
        case "bits": return v > 0 ? v + "-bit" : "—"
        case "channels": return v === 1 ? qsTr("Mono") : (v === 2 ? qsTr("Stereo") : (v > 0 ? v + " ch" : ""))
        case "bitrate": return v > 0 ? v + " kbps" : ""
        case "fileSize": return humanSize(v)
        // The name to read: without its extension (the format column says
        // it) or a leading track number (the row number does).
        case "fileName": return v ? String(v).replace(/\.[A-Za-z0-9]{2,5}$/, "").replace(/^\d{1,3}\s*[.\-_)]\s*/, "") : ""
        case "favorite": return 
        case "added":
        case "modified": return v > 0 ? new Date(v).toLocaleDateString(Qt.locale(), Locale.ShortFormat) : ""
        default: return v === undefined || v === null ? "" : String(v)
        }
    }
}
