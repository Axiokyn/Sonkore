#pragma once

#include <QByteArray>
#include <QJsonObject>
#include <QString>
#include <QStringList>
#include <QVariantMap>

#include <cmath>
#include <limits>

namespace sonkore {

struct TrackMeta {
    QString path;
    QString title;
    QString artist;
    QString album;
    QString albumArtist;
    QString genre;
    QString codec;      // "FLAC", "WAV", "MP3", "ALAC", "AAC", "Opus"...
    int trackNo = 0;
    int discNo = 0;
    int year = 0;
    double duration = 0.0; // seconds
    int sampleRate = 0;
    int bits = 0;          // 0 = lossy
    int channels = 0;
    int bitrateKbps = 0;
    qint64 fileSize = 0;
    qint64 modified = 0;   // ms since epoch
    qint64 added = 0;      // ms since epoch, first time the library saw the file
    bool hasEmbeddedCover = false;
    // Lyrics from the file's tags, read only on request (wantLyrics), never
    // kept in the library.
    bool wantLyrics = false;
    QString lyrics;

    // ReplayGain 2.0: gains in dB towards -18 LUFS, peaks as linear sample
    // values (1.0 = full scale). NaN = unknown. Read from the file's tags, or
    // measured by Sonkore (rgAnalyzed) for files without them; files are
    // never written.
    double rgTrackGain = std::numeric_limits<double>::quiet_NaN();
    double rgTrackPeak = 0.0;
    double rgAlbumGain = std::numeric_limits<double>::quiet_NaN();
    double rgAlbumPeak = 0.0;
    bool rgAnalyzed = false;
    bool hasReplayGain() const { return !std::isnan(rgTrackGain); }

    // Normalised album identity used for grouping ("artist\x1falbum").
    QString albumKey() const;
    QString displayArtist() const { return artist.isEmpty() ? albumArtist : artist; }
    QString formatLabel() const; // "FLAC 24/96", "MP3 320k"
    int averageBitrate() const;  // kbps; computed from size/duration for lossless files
    QString fileName() const;
    QString folder() const;
    QVariantMap toVariantMap() const;
    QJsonObject toJson() const;
    static TrackMeta fromJson(const QJsonObject& object);
};

// Lightweight tag readers that only touch the container headers (no audio
// decoding), so scanning thousands of files stays fast.
namespace Metadata {
bool isAudioFile(const QString& path);
QStringList audioExtensions(); // playable in this build, lower case, no dot
bool read(const QString& path, TrackMeta& out);
// The playable tracks of a .cue sheet ("<sheet>#<n>"); `imageFiles` receives the
// audio files it splits (listed through the sheet, not on their own).
QStringList cueTracks(const QString& sheet, QStringList* imageFiles = nullptr);
QByteArray embeddedCover(const QString& path); // raw image bytes or empty
QString embeddedLyrics(const QString& path);   // LYRICS / USLT text, LRC timestamps kept, or empty
QString folderCover(const QString& directory); // cover.jpg / folder.png ... or empty
} // namespace Metadata

} // namespace sonkore
