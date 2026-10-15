#include "Metadata.hpp"

#include "CueSheet.hpp"
#include "Decoder.hpp"

#include <QDateTime>
#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QJsonValue>
#include <QRegularExpression>
#include <QStringDecoder>

#include <algorithm>
#include <filesystem>

namespace sonkore {

// -----------------------------------------------------------------------------
// TrackMeta
// -----------------------------------------------------------------------------

QString TrackMeta::albumKey() const
{
    const QString who = (albumArtist.isEmpty() ? artist : albumArtist).toLower();
    return who + QChar(0x1f) + album.toLower();
}

QString TrackMeta::formatLabel() const
{
    if (bits > 0 && sampleRate > 0) {
        const double khz = sampleRate / 1000.0;
        return QStringLiteral("%1 %2/%3").arg(codec).arg(bits).arg(khz, 0, 'g', 4);
    }
    if (bitrateKbps > 0)
        return QStringLiteral("%1 %2k").arg(codec).arg(bitrateKbps);
    return codec;
}

int TrackMeta::averageBitrate() const
{
    if (bitrateKbps > 0)
        return bitrateKbps;
    return duration > 0.0 ? static_cast<int>(fileSize * 8.0 / duration / 1000.0 + 0.5) : 0;
}

QString TrackMeta::fileName() const
{
    // A CD image track has no file of its own ("disc.cue#3"): its title
    // from the sheet stands in for one.
    if (sonkore::CueSheet::isTrackPath(path.toStdString()) && !title.isEmpty())
        return title;
    return path.section('/', -1);
}

QString TrackMeta::folder() const
{
    return path.section('/', 0, -2);
}

QVariantMap TrackMeta::toVariantMap() const
{
    const int secs = static_cast<int>(duration + 0.5);
    return {
        {"path", path},
        {"title", title},
        {"artist", displayArtist()},
        {"album", album},
        {"albumArtist", albumArtist},
        {"genre", genre},
        {"trackNo", trackNo},
        {"discNo", discNo},
        {"year", year},
        {"duration", duration},
        {"durationText", QStringLiteral("%1:%2").arg(secs / 60).arg(secs % 60, 2, 10, QChar('0'))},
        {"format", formatLabel()},
        {"codec", codec},
        {"sampleRate", sampleRate},
        {"bits", bits},
        {"channels", channels},
        {"albumKey", albumKey()},
        {"bitrate", averageBitrate()},
        {"fileSize", fileSize},
        {"fileName", fileName()},
        {"folder", folder()},
        {"modified", modified},
        {"added", added},
        {"rgTrackGain", rgTrackGain},
        {"rgTrackPeak", rgTrackPeak},
        {"rgAlbumGain", rgAlbumGain},
        {"rgAlbumPeak", rgAlbumPeak},
    };
}

QJsonObject TrackMeta::toJson() const
{
    QJsonObject o{
        {"p", path}, {"t", title}, {"a", artist}, {"al", album}, {"aa", albumArtist},
        {"g", genre}, {"c", codec}, {"tn", trackNo}, {"dn", discNo}, {"y", year},
        {"d", duration}, {"sr", sampleRate}, {"b", bits}, {"ch", channels},
        {"br", bitrateKbps}, {"sz", fileSize}, {"m", modified}, {"ad", added}, {"cv", hasEmbeddedCover},
    };
    if (!std::isnan(rgTrackGain)) {
        o.insert("rtg", rgTrackGain);
        o.insert("rtp", rgTrackPeak);
    }
    if (!std::isnan(rgAlbumGain)) {
        o.insert("rag", rgAlbumGain);
        o.insert("rap", rgAlbumPeak);
    }
    if (rgAnalyzed)
        o.insert("rga", true);
    return o;
}

TrackMeta TrackMeta::fromJson(const QJsonObject& o)
{
    TrackMeta t;
    t.path = o.value("p").toString();
    t.title = o.value("t").toString();
    t.artist = o.value("a").toString();
    if (t.artist == QLatin1String("Unknown artist")) // caches written before 1.3.1 stored the English fallback
        t.artist.clear();
    t.album = o.value("al").toString();
    t.albumArtist = o.value("aa").toString();
    t.genre = o.value("g").toString();
    t.codec = o.value("c").toString();
    t.trackNo = o.value("tn").toInt();
    t.discNo = o.value("dn").toInt();
    t.year = o.value("y").toInt();
    t.duration = o.value("d").toDouble();
    t.sampleRate = o.value("sr").toInt();
    t.bits = o.value("b").toInt();
    t.channels = o.value("ch").toInt();
    t.bitrateKbps = o.value("br").toInt();
    t.fileSize = o.value("sz").toInteger();
    t.modified = o.value("m").toInteger();
    t.added = o.value("ad").toInteger(t.modified);
    t.hasEmbeddedCover = o.value("cv").toBool();
    const double nan = std::numeric_limits<double>::quiet_NaN();
    t.rgTrackGain = o.value("rtg").toDouble(nan);
    t.rgTrackPeak = o.value("rtp").toDouble();
    t.rgAlbumGain = o.value("rag").toDouble(nan);
    t.rgAlbumPeak = o.value("rap").toDouble();
    t.rgAnalyzed = o.value("rga").toBool();
    return t;
}

namespace {

// -----------------------------------------------------------------------------
// Byte helpers
// -----------------------------------------------------------------------------

quint32 be32(const char* p)
{
    const auto* u = reinterpret_cast<const unsigned char*>(p);
    return (quint32(u[0]) << 24) | (quint32(u[1]) << 16) | (quint32(u[2]) << 8) | quint32(u[3]);
}
quint32 be24(const char* p)
{
    const auto* u = reinterpret_cast<const unsigned char*>(p);
    return (quint32(u[0]) << 16) | (quint32(u[1]) << 8) | quint32(u[2]);
}
quint32 le32(const char* p)
{
    const auto* u = reinterpret_cast<const unsigned char*>(p);
    return quint32(u[0]) | (quint32(u[1]) << 8) | (quint32(u[2]) << 16) | (quint32(u[3]) << 24);
}
quint16 le16(const char* p)
{
    const auto* u = reinterpret_cast<const unsigned char*>(p);
    return quint16(u[0] | (u[1] << 8));
}
quint32 syncsafe(const char* p)
{
    const auto* u = reinterpret_cast<const unsigned char*>(p);
    return (quint32(u[0] & 0x7f) << 21) | (quint32(u[1] & 0x7f) << 14) | (quint32(u[2] & 0x7f) << 7) | quint32(u[3] & 0x7f);
}

int leadingInt(const QString& s) // "3/12" -> 3, "2019-04-01" -> 2019
{
    static const QRegularExpression re(QStringLiteral("^\\s*(\\d+)"));
    const auto m = re.match(s);
    return m.hasMatch() ? m.captured(1).toInt() : 0;
}

// "-6.54 dB" / "+0.3 dB" / "0.988" -> number; NaN when there is none.
double leadingNumber(const QString& s)
{
    static const QRegularExpression re(QStringLiteral("^\\s*([+-]?\\d*\\.?\\d+(?:[eE][+-]?\\d+)?)"));
    const auto m = re.match(s);
    return m.hasMatch() ? m.captured(1).toDouble() : std::numeric_limits<double>::quiet_NaN();
}

// ReplayGain (REPLAYGAIN_*: dB towards -18 LUFS) and Opus R128 (R128_*: Q7.8
// dB towards -23 LUFS, so 5 dB is added to bring them to the same reference).
bool applyLoudnessTag(TrackMeta& t, const QString& key, const QString& value)
{
    if (!key.startsWith(QLatin1String("REPLAYGAIN_")) && !key.startsWith(QLatin1String("R128_")))
        return false;
    const double v = leadingNumber(value);
    if (std::isnan(v))
        return true;
    if (key == "REPLAYGAIN_TRACK_GAIN") t.rgTrackGain = v;
    else if (key == "REPLAYGAIN_TRACK_PEAK") t.rgTrackPeak = v;
    else if (key == "REPLAYGAIN_ALBUM_GAIN") t.rgAlbumGain = v;
    else if (key == "REPLAYGAIN_ALBUM_PEAK") t.rgAlbumPeak = v;
    else if (key == "R128_TRACK_GAIN") t.rgTrackGain = v / 256.0 + 5.0;
    else if (key == "R128_ALBUM_GAIN") t.rgAlbumGain = v / 256.0 + 5.0;
    return true;
}

void applyTag(TrackMeta& t, QString key, const QString& value)
{
    key = key.toUpper();
    if (value.isEmpty())
        return;
    if (applyLoudnessTag(t, key, value))
        return;
    if (key == "TITLE") t.title = value;
    else if (key == "ARTIST") t.artist = value;
    else if (key == "ALBUM") t.album = value;
    else if (key == "ALBUMARTIST" || key == "ALBUM ARTIST" || key == "ALBUM_ARTIST") t.albumArtist = value;
    else if (key == "GENRE") t.genre = value;
    else if (key == "TRACKNUMBER" || key == "TRACK") t.trackNo = leadingInt(value);
    else if (key == "DISCNUMBER" || key == "DISC") t.discNo = leadingInt(value);
    else if ((key == "DATE" || key == "YEAR" || key == "ORIGINALDATE") && t.year == 0) t.year = leadingInt(value);
    else if (t.wantLyrics && (key.startsWith("LYRICS") || key == "UNSYNCEDLYRICS" || key == "UNSYNCED LYRICS" || key == "SYNCEDLYRICS")) {
        if (t.lyrics.isEmpty() || (value.contains('[') && !t.lyrics.contains('['))) // timed ones first
            t.lyrics = value;
    }
}

// -----------------------------------------------------------------------------
// ID3v2 (MP3, and occasionally WAV/FLAC)
// -----------------------------------------------------------------------------

QString decodeId3Text(const QByteArray& data, int encoding)
{
    QString s;
    switch (encoding) {
    case 0: s = QString::fromLatin1(data); break;
    case 1: { // UTF-16 with BOM
        auto dec = QStringDecoder(QStringDecoder::Utf16);
        s = dec.decode(data);
        break;
    }
    case 2: {
        auto dec = QStringDecoder(QStringDecoder::Utf16BE);
        s = dec.decode(data);
        break;
    }
    default: s = QString::fromUtf8(data); break;
    }
    const int nul = s.indexOf(QChar(0)); // v2.4 multi-value: keep the first
    if (nul >= 0)
        s.truncate(nul);
    return s.trimmed();
}

// Position after a terminator for the given text encoding.
int skipId3String(const QByteArray& d, int pos, int encoding)
{
    if (encoding == 1 || encoding == 2) {
        for (int i = pos; i + 1 < d.size(); i += 2)
            if (d[i] == 0 && d[i + 1] == 0)
                return i + 2;
        return d.size();
    }
    const int i = d.indexOf('\0', pos);
    return i < 0 ? d.size() : i + 1;
}

QString stripGenreCode(QString g) // "(17)Rock" / "(17)" -> "Rock" / ""
{
    static const QRegularExpression re(QStringLiteral("^\\(\\d+\\)"));
    return g.remove(re).trimmed();
}

// Parses a complete ID3v2 tag (header included). Fills `t` and, when `cover`
// is non-null, the front cover (or the first picture).
void parseId3(const QByteArray& tag, TrackMeta& t, QByteArray* cover)
{
    if (tag.size() < 10 || !tag.startsWith("ID3"))
        return;
    const int ver = static_cast<unsigned char>(tag[3]);
    const int flags = static_cast<unsigned char>(tag[5]);
    int pos = 10;
    if ((flags & 0x40) && ver >= 3 && tag.size() >= 14) // extended header
        pos += (ver == 4) ? static_cast<int>(syncsafe(tag.constData() + 10)) : static_cast<int>(be32(tag.constData() + 10)) + 4;

    const int idLen = (ver == 2) ? 3 : 4;
    const int hdrLen = (ver == 2) ? 6 : 10;
    int pictureType = -1;

    while (pos + hdrLen <= tag.size()) {
        const QByteArray id = tag.mid(pos, idLen);
        if (id.isEmpty() || id[0] == '\0')
            break;
        quint32 size = 0;
        if (ver == 2) size = be24(tag.constData() + pos + 3);
        else if (ver == 4) size = syncsafe(tag.constData() + pos + 4);
        else size = be32(tag.constData() + pos + 4);
        const int dataPos = pos + hdrLen;
        if (size == 0 || dataPos + static_cast<qint64>(size) > tag.size())
            break;
        const QByteArray d = tag.mid(dataPos, static_cast<int>(size));
        pos = dataPos + static_cast<int>(size);

        const bool isPic = id == "APIC" || id == "PIC";
        if (isPic) {
            t.hasEmbeddedCover = true;
            if (!cover || d.size() < 4)
                continue;
            const int enc = static_cast<unsigned char>(d[0]);
            int p = 1;
            if (id == "PIC") p += 3;                    // 3-char image format
            else p = d.indexOf('\0', 1) + 1;            // MIME type
            if (p <= 0 || p >= d.size())
                continue;
            const int type = static_cast<unsigned char>(d[p]);
            p = skipId3String(d, p + 1, enc);           // description
            if (p < d.size() && (pictureType != 3) && (cover->isEmpty() || type == 3)) {
                *cover = d.mid(p);
                pictureType = type;
            }
            continue;
        }
        if (t.wantLyrics && (id == "USLT" || id == "ULT") && d.size() > 4) { // encoding, language, descriptor, text
            const int enc = static_cast<unsigned char>(d[0]);
            const int textPos = skipId3String(d, 4, enc);
            const QString text = decodeId3Text(d.mid(textPos), enc);
            if (t.lyrics.isEmpty() || (text.contains('[') && !t.lyrics.contains('[')))
                t.lyrics = text;
            continue;
        }
        if (id[0] != 'T' || d.isEmpty())
            continue;
        if (id == "TXXX" || id == "TXX") { // user text: "description\0value" (ReplayGain lives here)
            const int enc = static_cast<unsigned char>(d[0]);
            const int valuePos = skipId3String(d, 1, enc);
            applyLoudnessTag(t, decodeId3Text(d.mid(1, valuePos - 1), enc).toUpper(), decodeId3Text(d.mid(valuePos), enc));
            continue;
        }
        const QString text = decodeId3Text(d.mid(1), static_cast<unsigned char>(d[0]));
        if (id == "TIT2" || id == "TT2") t.title = text;
        else if (id == "TPE1" || id == "TP1") t.artist = text;
        else if (id == "TPE2" || id == "TP2") t.albumArtist = text;
        else if (id == "TALB" || id == "TAL") t.album = text;
        else if (id == "TCON" || id == "TCO") t.genre = stripGenreCode(text);
        else if (id == "TRCK" || id == "TRK") t.trackNo = leadingInt(text);
        else if (id == "TPOS" || id == "TPA") t.discNo = leadingInt(text);
        else if ((id == "TDRC" || id == "TYER" || id == "TYE") && t.year == 0) t.year = leadingInt(text);
    }
}

// Reads the ID3v2 tag at the current file position (if any). Returns its
// total size so callers can skip it.
qint64 readId3AtStart(QFile& f, TrackMeta& t, QByteArray* cover)
{
    const QByteArray hdr = f.peek(10);
    if (hdr.size() < 10 || !hdr.startsWith("ID3"))
        return 0;
    qint64 size = 10 + syncsafe(hdr.constData() + 6);
    if (static_cast<unsigned char>(hdr[5]) & 0x10)
        size += 10; // footer
    const QByteArray tag = f.read(std::min<qint64>(size, 64 * 1024 * 1024));
    parseId3(tag, t, cover);
    f.seek(size);
    return size;
}

// -----------------------------------------------------------------------------
// FLAC
// -----------------------------------------------------------------------------

bool readFlac(QFile& f, TrackMeta& t, QByteArray* cover)
{
    readId3AtStart(f, t, nullptr); // some taggers prepend ID3 to FLAC
    if (f.read(4) != "fLaC")
        return false;
    t.codec = QStringLiteral("FLAC");
    int pictureType = -1;

    for (;;) {
        const QByteArray h = f.read(4);
        if (h.size() < 4)
            break;
        const bool last = static_cast<unsigned char>(h[0]) & 0x80;
        const int type = static_cast<unsigned char>(h[0]) & 0x7f;
        const quint32 len = be24(h.constData() + 1);
        const qint64 next = f.pos() + len;

        if (type == 0 && len >= 18) { // STREAMINFO
            const QByteArray d = f.read(len);
            const auto* u = reinterpret_cast<const unsigned char*>(d.constData());
            t.sampleRate = static_cast<int>((u[10] << 12) | (u[11] << 4) | (u[12] >> 4));
            t.channels = ((u[12] >> 1) & 7) + 1;
            t.bits = (((u[12] & 1) << 4) | (u[13] >> 4)) + 1;
            const quint64 total = (quint64(u[13] & 0x0f) << 32) | be32(d.constData() + 14);
            if (t.sampleRate > 0)
                t.duration = static_cast<double>(total) / t.sampleRate;
        } else if (type == 4) { // VORBIS_COMMENT (little endian)
            const QByteArray d = f.read(len);
            if (d.size() >= 8) {
                qint64 p = 4 + le32(d.constData());
                if (p + 4 <= d.size()) {
                    const quint32 count = le32(d.constData() + p);
                    p += 4;
                    for (quint32 i = 0; i < count && p + 4 <= d.size(); ++i) {
                        const quint32 l = le32(d.constData() + p);
                        p += 4;
                        if (p + l > static_cast<quint64>(d.size()))
                            break;
                        const QString kv = QString::fromUtf8(d.constData() + p, static_cast<int>(l));
                        p += l;
                        const int eq = kv.indexOf('=');
                        if (eq > 0)
                            applyTag(t, kv.left(eq), kv.mid(eq + 1).trimmed());
                    }
                }
            }
        } else if (type == 6) { // PICTURE (big endian)
            t.hasEmbeddedCover = true;
            if (cover && pictureType != 3) {
                const QByteArray d = f.read(len);
                if (d.size() >= 32) {
                    const int picType = static_cast<int>(be32(d.constData()));
                    qint64 p = 4;
                    p += 4 + be32(d.constData() + p);          // MIME
                    if (p + 4 <= d.size()) p += 4 + be32(d.constData() + p); // description
                    p += 16;                                   // width/height/depth/colors
                    if (p + 4 <= d.size()) {
                        const quint32 dataLen = be32(d.constData() + p);
                        p += 4;
                        if (p + dataLen <= static_cast<quint64>(d.size()) && (cover->isEmpty() || picType == 3)) {
                            *cover = d.mid(static_cast<int>(p), static_cast<int>(dataLen));
                            pictureType = picType;
                        }
                    }
                }
            }
        }
        if (last || !f.seek(next))
            break;
    }
    return t.sampleRate > 0;
}

// -----------------------------------------------------------------------------
// MP3: ID3v2 + first frame header (+ Xing/Info for VBR duration)
// -----------------------------------------------------------------------------

bool readMp3(QFile& f, TrackMeta& t, QByteArray* cover)
{
    t.codec = QStringLiteral("MP3");
    const qint64 audioStart = readId3AtStart(f, t, cover);
    const QByteArray buf = f.read(64 * 1024);

    static const int kBitrateV1[] = {0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0};
    static const int kBitrateV2[] = {0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0};
    static const int kRates[] = {44100, 48000, 32000, 0};

    for (int i = 0; i + 4 <= buf.size(); ++i) {
        const auto* u = reinterpret_cast<const unsigned char*>(buf.constData() + i);
        if (u[0] != 0xff || (u[1] & 0xe0) != 0xe0)
            continue;
        const int version = (u[1] >> 3) & 3; // 3 = MPEG1, 2 = MPEG2, 0 = MPEG2.5
        const int layer = (u[1] >> 1) & 3;   // 1 = Layer III
        const int brIndex = u[2] >> 4;
        const int srIndex = (u[2] >> 2) & 3;
        if (version == 1 || layer != 1 || brIndex == 0 || brIndex == 15 || srIndex == 3)
            continue;
        const bool mpeg1 = version == 3;
        const int bitrate = mpeg1 ? kBitrateV1[brIndex] : kBitrateV2[brIndex];
        const int rate = kRates[srIndex] >> (mpeg1 ? 0 : (version == 2 ? 1 : 2));
        const bool mono = (u[3] >> 6) == 3;
        t.sampleRate = rate;
        t.channels = mono ? 1 : 2;
        t.bitrateKbps = bitrate;
        t.bits = 0;

        const int samplesPerFrame = mpeg1 ? 1152 : 576;
        const int sideInfo = mpeg1 ? (mono ? 17 : 32) : (mono ? 9 : 17);
        const int xing = i + 4 + sideInfo;
        if (xing + 12 <= buf.size()) {
            const QByteArray magic = buf.mid(xing, 4);
            if (magic == "Xing" || magic == "Info") {
                const quint32 xflags = be32(buf.constData() + xing + 4);
                if (xflags & 1) {
                    const quint32 frames = be32(buf.constData() + xing + 8);
                    t.duration = static_cast<double>(frames) * samplesPerFrame / rate;
                    if (xflags & 2 && xing + 16 <= buf.size() && t.duration > 0)
                        t.bitrateKbps = static_cast<int>(be32(buf.constData() + xing + 12) * 8 / t.duration / 1000);
                }
            }
        }
        if (t.duration <= 0 && bitrate > 0)
            t.duration = static_cast<double>(f.size() - audioStart - i) * 8.0 / (bitrate * 1000.0);
        return true;
    }
    return false;
}

// -----------------------------------------------------------------------------
// WAV: RIFF fmt/data + LIST/INFO + id3 chunk
// -----------------------------------------------------------------------------

bool readWav(QFile& f, TrackMeta& t, QByteArray* cover)
{
    t.codec = QStringLiteral("WAV");
    const QByteArray riff = f.read(12);
    if (riff.size() < 12 || !riff.startsWith("RIFF") || riff.mid(8, 4) != "WAVE")
        return false;
    int blockAlign = 0;
    bool isFloat = false;
    while (!f.atEnd()) {
        const QByteArray ch = f.read(8);
        if (ch.size() < 8)
            break;
        const QByteArray id = ch.left(4);
        const quint32 size = le32(ch.constData() + 4);
        const qint64 next = f.pos() + size + (size & 1);
        if (id == "fmt " && size >= 16) {
            const QByteArray d = f.read(size);
            const int format = le16(d.constData());
            t.channels = le16(d.constData() + 2);
            t.sampleRate = static_cast<int>(le32(d.constData() + 4));
            blockAlign = le16(d.constData() + 12);
            t.bits = le16(d.constData() + 14);
            isFloat = format == 3;
        } else if (id == "data") {
            if (blockAlign > 0 && t.sampleRate > 0)
                t.duration = static_cast<double>(size / blockAlign) / t.sampleRate;
        } else if (id == "LIST") {
            const QByteArray d = f.read(size);
            if (d.startsWith("INFO")) {
                int p = 4;
                while (p + 8 <= d.size()) {
                    const QByteArray sub = d.mid(p, 4);
                    const quint32 l = le32(d.constData() + p + 4);
                    const QString v = QString::fromUtf8(d.mid(p + 8, static_cast<int>(l))).remove(QChar(0)).trimmed();
                    if (sub == "INAM") t.title = v;
                    else if (sub == "IART") t.artist = v;
                    else if (sub == "IPRD") t.album = v;
                    else if (sub == "IGNR") t.genre = v;
                    else if (sub == "ICRD") t.year = leadingInt(v);
                    else if (sub == "ITRK" || sub == "IPRT") t.trackNo = leadingInt(v);
                    p += 8 + static_cast<int>(l + (l & 1));
                }
            }
        } else if (id == "id3 " || id == "ID3 ") {
            parseId3(f.read(size), t, cover);
        }
        if (!f.seek(next))
            break;
    }
    if (isFloat)
        t.bits = 0;
    return t.sampleRate > 0;
}

// -----------------------------------------------------------------------------
// Everything else (ALAC, AAC, Opus, Vorbis, AIFF, APE, WavPack, DSD...): FFmpeg
// reads the container header; no audio is decoded.
// -----------------------------------------------------------------------------

std::filesystem::path toFsPath(const QString& path)
{
#if defined(_WIN32)
    return std::filesystem::path(path.toStdWString());
#else
    return std::filesystem::path(QFile::encodeName(path).toStdString());
#endif
}

bool readProbed(const QString& path, TrackMeta& t, QByteArray* cover)
{
    sonkore::ProbeResult r;
    if (!sonkore::Decoder::probe(toFsPath(path), r, cover != nullptr))
        return false;
    t.codec = QString::fromStdString(r.stream.codec);
    t.sampleRate = static_cast<int>(r.stream.sampleRate);
    t.channels = static_cast<int>(r.stream.channels);
    t.bits = r.stream.lossless ? static_cast<int>(r.stream.bitsPerSample) : 0;
    t.duration = r.durationSeconds;
    t.bitrateKbps = r.bitrateKbps;
    t.hasEmbeddedCover = r.hasCover;
    for (const auto& [key, value] : r.tags)
        applyTag(t, QString::fromUtf8(key.data(), static_cast<qsizetype>(key.size())),
                 QString::fromUtf8(value.data(), static_cast<qsizetype>(value.size())).trimmed());
    if (cover && !r.cover.empty())
        *cover = QByteArray(r.cover.data(), static_cast<qsizetype>(r.cover.size()));
    return true;
}

QString extensionOf(const QString& path)
{
    const int dot = path.lastIndexOf('.');
    return dot < 0 ? QString() : path.mid(dot + 1).toLower();
}

bool readAny(const QString& path, TrackMeta& t, QByteArray* cover)
{
    const QString ext = extensionOf(path);
    if (sonkore::Decoder::usesFfmpeg(ext.toStdString()))
        return readProbed(path, t, cover);
    QFile f(path);
    if (!f.open(QIODevice::ReadOnly))
        return false;
    if (ext == "flac") return readFlac(f, t, cover);
    if (ext == "mp3") return readMp3(f, t, cover);
    if (ext == "wav" || ext == "wave") return readWav(f, t, cover);
    return false;
}

} // namespace

// -----------------------------------------------------------------------------
// Public API
// -----------------------------------------------------------------------------

namespace {

std::filesystem::path fsPath(const QString& path)
{
    return std::filesystem::path(QFile::encodeName(path).toStdString());
}

QString qtPath(const std::filesystem::path& path)
{
    return QFile::decodeName(QByteArray::fromStdString(path.string()));
}

// One track of a CD image: its own tags from the sheet; format, cover and the
// image's ReplayGain (the whole image is the album) from the audio file.
bool readCueTrack(const QString& path, TrackMeta& out)
{
    std::filesystem::path sheetPath;
    int number = 0;
    sonkore::CueSheet sheet;
    if (!sonkore::CueSheet::splitTrackPath(path.toStdString(), sheetPath, number) || !sonkore::CueSheet::parse(sheetPath, sheet))
        return false;
    const sonkore::CueSheet::Track* track = sheet.track(number);
    if (!track)
        return false;
    TrackMeta image;
    const QString audio = qtPath(track->file);
    readAny(audio, image, nullptr);
    const QFileInfo audioInfo(audio), sheetInfo(qtPath(sheetPath));

    out.path = path;
    out.title = QString::fromStdString(track->title);
    if (out.title.isEmpty())
        out.title = QObject::tr("Track %1").arg(number);
    out.artist = QString::fromStdString(track->performer.empty() ? sheet.performer : track->performer);
    out.album = sheet.title.empty() ? image.album : QString::fromStdString(sheet.title);
    out.albumArtist = sheet.performer.empty() ? image.albumArtist : QString::fromStdString(sheet.performer);
    out.genre = sheet.genre.empty() ? image.genre : QString::fromStdString(sheet.genre);
    out.year = sheet.date.empty() ? image.year : leadingInt(QString::fromStdString(sheet.date));
    out.trackNo = number;
    out.discNo = image.discNo;
    out.codec = image.codec;
    out.sampleRate = image.sampleRate;
    out.bits = image.bits;
    out.channels = image.channels;
    out.bitrateKbps = image.bitrateKbps;
    out.hasEmbeddedCover = image.hasEmbeddedCover;
    const double start = track->start / 75.0;
    const double end = track->end ? track->end / 75.0 : image.duration;
    out.duration = std::max(0.0, end - start);
    // Its share of the image, so the average bitrate stays meaningful.
    out.fileSize = image.duration > 0 ? static_cast<qint64>(audioInfo.size() * (out.duration / image.duration)) : 0;
    out.modified = std::max(audioInfo.lastModified().toMSecsSinceEpoch(), sheetInfo.lastModified().toMSecsSinceEpoch());
    if (!std::isnan(track->gain)) {
        out.rgTrackGain = track->gain;
        out.rgTrackPeak = track->peak;
    }
    if (!std::isnan(sheet.albumGain)) {
        out.rgAlbumGain = sheet.albumGain;
        out.rgAlbumPeak = sheet.albumPeak;
    } else if (!std::isnan(image.rgAlbumGain) || !std::isnan(image.rgTrackGain)) {
        out.rgAlbumGain = std::isnan(image.rgAlbumGain) ? image.rgTrackGain : image.rgAlbumGain;
        out.rgAlbumPeak = std::isnan(image.rgAlbumGain) ? image.rgTrackPeak : image.rgAlbumPeak;
    }
    return true;
}

} // namespace

QStringList Metadata::cueTracks(const QString& sheet, QStringList* imageFiles)
{
    sonkore::CueSheet cue;
    QStringList paths;
    if (!sonkore::CueSheet::parse(fsPath(sheet), cue))
        return paths;
    for (const auto& t : cue.tracks) {
        if (!isAudioFile(qtPath(t.file)))
            continue;
        paths << QString::fromStdString(sonkore::CueSheet::trackPath(fsPath(sheet), t.number));
        if (imageFiles && !imageFiles->contains(qtPath(t.file)))
            *imageFiles << qtPath(t.file);
    }
    return paths;
}

bool Metadata::isAudioFile(const QString& path)
{
    if (sonkore::CueSheet::isTrackPath(path.toStdString()))
        return true;
    const QString ext = extensionOf(path);
    const std::string name = ext.toStdString();
    return !name.empty() && sonkore::Decoder::isSupportedExtension(std::string_view(name));
}

QStringList Metadata::audioExtensions()
{
    QStringList list;
    for (const std::string& ext : sonkore::Decoder::supportedExtensions())
        list << QString::fromStdString(ext);
    return list;
}

bool Metadata::read(const QString& path, TrackMeta& out)
{
    out = TrackMeta{};
    if (sonkore::CueSheet::isTrackPath(path.toStdString()))
        return readCueTrack(path, out);
    out.path = path;
    const QFileInfo fi(path);
    out.fileSize = fi.size();
    out.modified = fi.lastModified().toMSecsSinceEpoch();

    const bool ok = readAny(path, out, nullptr);

    if (out.title.isEmpty())
        out.title = fi.completeBaseName();
    if (out.album.isEmpty())
        out.album = fi.dir().dirName();
    if (out.codec.isEmpty())
        out.codec = fi.suffix().toUpper();
    return ok;
}

QByteArray Metadata::embeddedCover(const QString& path)
{
    TrackMeta t;
    QByteArray cover;
    std::filesystem::path sheetPath;
    int number = 0;
    sonkore::CueSheet sheet;
    if (sonkore::CueSheet::splitTrackPath(path.toStdString(), sheetPath, number)) { // the image file's cover
        if (sonkore::CueSheet::parse(sheetPath, sheet))
            if (const auto* track = sheet.track(number))
                readAny(qtPath(track->file), t, &cover);
        return cover;
    }
    readAny(path, t, &cover);
    return cover;
}

QString Metadata::embeddedLyrics(const QString& path)
{
    TrackMeta t;
    t.wantLyrics = true;
    std::filesystem::path sheetPath;
    int number = 0;
    sonkore::CueSheet sheet;
    if (sonkore::CueSheet::splitTrackPath(path.toStdString(), sheetPath, number))
        return {}; // one image file for the whole album: its lyrics would not be this track's
    readAny(path, t, nullptr);
    return t.lyrics;
}

QString Metadata::folderCover(const QString& directory)
{
    static const QStringList kNames = {"cover", "folder", "front", "album", "albumart", "artwork"};
    const QDir dir(directory);
    const QStringList images = dir.entryList({"*.jpg", "*.jpeg", "*.png", "*.webp"}, QDir::Files);
    for (const QString& wanted : kNames)
        for (const QString& file : images)
            if (QFileInfo(file).completeBaseName().compare(wanted, Qt::CaseInsensitive) == 0)
                return dir.filePath(file);
    return images.isEmpty() ? QString() : dir.filePath(images.first());
}

} // namespace sonkore
