#include "LibraryModel.hpp"
#include "AppPaths.hpp"
#include "Loudness.hpp"
#include "CueSheet.hpp"

#include <QCollator>
#include <QDateTime>
#include <QDir>
#include <QDirIterator>
#include <QFile>
#include <QFileInfo>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QSet>
#include <QSettings>
#include <QStandardPaths>

#include <algorithm>

using sonkore::TrackMeta;

namespace {

// 2: ReplayGain / R128 tags are read.
// 3: DSF / DSDIFF lengths (read as 0 through FFmpeg 8); unreadable files dropped.
constexpr int kCacheVersion = 3;

// What a CD image track's entry was read from: the newer of its sheet and
// image file (Metadata::read stores the same value in `modified`).
qint64 cueStamp(const QString& trackPath)
{
    std::filesystem::path sheetPath;
    int number = 0;
    sonkore::CueSheet sheet;
    if (!sonkore::CueSheet::splitTrackPath(trackPath.toStdString(), sheetPath, number) || !sonkore::CueSheet::parse(sheetPath, sheet))
        return -1;
    const auto* track = sheet.track(number);
    const QFileInfo image(QFile::decodeName(QByteArray::fromStdString(track ? track->file.string() : std::string())));
    const QFileInfo cue(QFile::decodeName(QByteArray::fromStdString(sheetPath.string())));
    return std::max(image.lastModified().toMSecsSinceEpoch(), cue.lastModified().toMSecsSinceEpoch());
}

constexpr int kMaxWatchedDirs = 8000;

QStringList stringList(const QJsonValue& value)
{
    QStringList list;
    for (const QJsonValue& v : value.toArray())
        list << v.toString();
    return list;
}

QString cachePath()
{
    return sonkore::paths::dataFile(QStringLiteral("library.json"));
}

QString favoritesPath()
{
    return sonkore::paths::dataFile(QStringLiteral("favorites.json"));
}

QString searchKey(const TrackMeta& t)
{
    return (t.title + ' ' + t.artist + ' ' + t.albumArtist + ' ' + t.album + ' ' + t.genre + ' ' + t.codec).toLower();
}

// Tracks of one album share artist, album, genre and codec: keep a single
// copy of each distinct string (QString is implicitly shared).
void shareRepeatedStrings(QVector<TrackMeta>& tracks)
{
    QHash<QString, QString> pool;
    auto intern = [&pool](QString& s) {
        if (s.isEmpty())
            return;
        const auto it = pool.constFind(s);
        if (it != pool.cend())
            s = *it;
        else
            pool.insert(s, s);
    };
    for (TrackMeta& t : tracks) {
        intern(t.artist);
        intern(t.albumArtist);
        intern(t.album);
        intern(t.genre);
        intern(t.codec);
    }
}

} // namespace

LibraryModel::LibraryModel(QObject* parent)
    : QAbstractListModel(parent)
{
    m_rescanDebounce.setSingleShot(true);
    m_rescanDebounce.setInterval(2000);
    connect(&m_rescanDebounce, &QTimer::timeout, this, &LibraryModel::rescan);
    connect(&m_watcher, &QFileSystemWatcher::directoryChanged, this, [this] { m_rescanDebounce.start(); });

    QSettings settings;
    m_watchFolders = settings.value("library/watchFolders", true).toBool();
    m_rescanOnStartup = settings.value("library/rescanOnStartup", true).toBool();
    m_excludedFolders = settings.value("library/excludedFolders").toStringList();
    m_minDuration = std::clamp(settings.value("library/minDuration", 0).toInt(), 0, 600);

    loadCache();
    loadFavorites();
    if (m_rescanOnStartup && (!m_folders.isEmpty() || !m_extraFiles.isEmpty()))
        QTimer::singleShot(500, this, &LibraryModel::rescan); // pick up changes made while closed
}

LibraryModel::~LibraryModel()
{
    m_cancel->store(true);
    if (m_scanThread)
        m_scanThread->wait();
    if (m_loudnessThread)
        m_loudnessThread->wait();
}

// --- QAbstractListModel ------------------------------------------------------

int LibraryModel::rowCount(const QModelIndex& parent) const
{
    return parent.isValid() ? 0 : static_cast<int>(m_tracks.size());
}

QVariant LibraryModel::data(const QModelIndex& index, int role) const
{
    if (!index.isValid() || index.row() >= m_tracks.size())
        return {};
    if (role == SearchTextRole)
        return m_searchText[index.row()];
    if (role == FavoriteRole)
        return m_favorites.contains(m_tracks[index.row()].path);
    return roleData(m_tracks[index.row()], role);
}

QVariant LibraryModel::roleData(const TrackMeta& t, int role)
{
    switch (role) {
    case Qt::DisplayRole:
    case TitleRole: return t.title;
    case PathRole: return t.path;
    case ArtistRole: return t.displayArtist();
    case AlbumRole: return t.album;
    case AlbumArtistRole: return t.albumArtist;
    case GenreRole: return t.genre;
    case TrackNoRole: return t.trackNo;
    case DiscNoRole: return t.discNo;
    case YearRole: return t.year;
    case DurationRole: return t.duration;
    case DurationTextRole: {
        const int s = static_cast<int>(t.duration + 0.5);
        return QStringLiteral("%1:%2").arg(s / 60).arg(s % 60, 2, 10, QChar('0'));
    }
    case FormatRole: return t.formatLabel();
    case AlbumKeyRole: return t.albumKey();
    case CodecRole: return t.codec;
    case SampleRateRole: return t.sampleRate;
    case BitsRole: return t.bits;
    case ChannelsRole: return t.channels;
    case BitrateRole: return t.averageBitrate();
    case FileSizeRole: return t.fileSize;
    case FileNameRole: return t.fileName();
    case FolderRole: return t.folder();
    case ModifiedRole: return t.modified;
    case AddedRole: return t.added;
    default: return {};
    }
}

QHash<int, QByteArray> LibraryModel::roleNames() const
{
    return trackRoleNames();
}

QHash<int, QByteArray> LibraryModel::trackRoleNames()
{
    return {
        {PathRole, "path"},
        {TitleRole, "title"},
        {ArtistRole, "artist"},
        {AlbumRole, "album"},
        {AlbumArtistRole, "albumArtist"},
        {GenreRole, "genre"},
        {TrackNoRole, "trackNo"},
        {DiscNoRole, "discNo"},
        {YearRole, "year"},
        {DurationRole, "duration"},
        {DurationTextRole, "durationText"},
        {FormatRole, "format"},
        {AlbumKeyRole, "albumKey"},
        {SearchTextRole, "searchText"},
        {CodecRole, "codec"},
        {SampleRateRole, "sampleRate"},
        {BitsRole, "bits"},
        {ChannelsRole, "channels"},
        {BitrateRole, "bitrate"},
        {FileSizeRole, "fileSize"},
        {FileNameRole, "fileName"},
        {FolderRole, "folder"},
        {ModifiedRole, "modified"},
        {AddedRole, "added"},
        {FavoriteRole, "favorite"},
    };
}

// --- Queries -----------------------------------------------------------------

QString LibraryModel::audioNameFilter() const
{
    QStringList patterns;
    for (const QString& ext : sonkore::Metadata::audioExtensions())
        patterns << QStringLiteral("*.") + ext;
    return patterns.join(u' ');
}

double LibraryModel::totalDuration() const
{
    double total = 0.0;
    for (const TrackMeta& t : m_tracks)
        total += t.duration;
    return total;
}

const TrackMeta* LibraryModel::find(const QString& path) const
{
    const auto it = m_index.constFind(path);
    return it == m_index.cend() ? nullptr : &m_tracks[it.value()];
}

QString LibraryModel::searchText(const QString& path) const
{
    const auto it = m_index.constFind(path);
    return it == m_index.cend() ? QString() : m_searchText.value(it.value());
}

QString LibraryModel::searchText(const TrackMeta& t)
{
    return searchKey(t);
}

QVariantMap LibraryModel::track(const QString& path) const
{
    if (const TrackMeta* t = find(path))
        return t->toVariantMap();
    TrackMeta t;
    sonkore::Metadata::read(path, t);
    return t.toVariantMap();
}

QStringList LibraryModel::albumPaths(const QString& albumKey) const
{
    QStringList paths;
    for (const TrackMeta& t : m_tracks) // already sorted by disc/track
        if (t.albumKey() == albumKey)
            paths << t.path;
    return paths;
}

QVariantList LibraryModel::trackFolders() const
{
    QHash<QString, int> counts;
    for (const TrackMeta& t : m_tracks) {
        const int slash = t.path.lastIndexOf(u'/');
        if (slash > 0)
            ++counts[t.path.left(slash)];
    }
    QStringList dirs = counts.keys();
    QCollator collator;
    collator.setNumericMode(true);
    collator.setCaseSensitivity(Qt::CaseInsensitive);
    std::sort(dirs.begin(), dirs.end(), [&](const QString& a, const QString& b) { return collator.compare(a, b) < 0; });
    QVariantList list;
    list.reserve(dirs.size());
    for (const QString& dir : dirs) {
        // Named from the library folder that holds it (that folder itself:
        // its own name); loose files keep their folder's name.
        QString name = QFileInfo(dir).fileName();
        for (const QString& root : m_folders) {
            if (dir == root)
                break;
            if (dir.startsWith(root + u'/')) {
                name = dir.mid(root.size() + 1);
                break;
            }
        }
        list.append(QVariantMap{{"path", dir}, {"name", name}, {"count", counts.value(dir)}});
    }
    return list;
}

QStringList LibraryModel::folderPaths(const QString& folder) const
{
    const QString prefix = folder.endsWith('/') ? folder : folder + '/';
    QStringList paths;
    for (const TrackMeta& t : m_tracks)
        if (t.path.startsWith(prefix))
            paths << t.path;
    return paths;
}

void LibraryModel::setWatchFolders(bool on)
{
    if (on == m_watchFolders)
        return;
    m_watchFolders = on;
    QSettings().setValue("library/watchFolders", on);
    watchDirectories(m_scannedDirs);
    emit preferencesChanged();
}

void LibraryModel::setRescanOnStartup(bool on)
{
    if (on == m_rescanOnStartup)
        return;
    m_rescanOnStartup = on;
    QSettings().setValue("library/rescanOnStartup", on);
    emit preferencesChanged();
}

void LibraryModel::addExcludedFolder(const QUrl& folder)
{
    excludeFolder(folder.toLocalFile());
}

void LibraryModel::excludeFolder(const QString& folder)
{
    QString path = folder;
    while (path.size() > 1 && path.endsWith(u'/'))
        path.chop(1);
    if (path.isEmpty() || m_excludedFolders.contains(path))
        return;
    m_excludedFolders << path;
    m_excludedFolders.sort();
    QSettings().setValue("library/excludedFolders", m_excludedFolders);
    emit preferencesChanged();
    rescan();
}

void LibraryModel::removeExcludedFolder(const QString& folder)
{
    if (!m_excludedFolders.removeOne(folder))
        return;
    QSettings().setValue("library/excludedFolders", m_excludedFolders);
    emit preferencesChanged();
    rescan();
}

void LibraryModel::setMinDuration(int seconds)
{
    seconds = std::clamp(seconds, 0, 600);
    if (seconds == m_minDuration)
        return;
    m_minDuration = seconds;
    QSettings().setValue("library/minDuration", seconds);
    emit preferencesChanged();
    rescan();
}

QVariantList LibraryModel::browse(const QString& field, const QString& genre, const QString& artist) const
{
    struct Entry {
        QString name;
        int count = 0;
        QSet<QString> albums;
        QString albumKey;
    };
    const auto keyOf = [](const QString& value) { return value.isEmpty() ? kUnknown : value; };
    QHash<QString, Entry> entries;
    for (const TrackMeta& t : m_tracks) {
        const QString trackArtist = t.albumArtist.isEmpty() ? t.artist : t.albumArtist;
        if (!genre.isEmpty() && keyOf(t.genre) != genre)
            continue;
        if (!artist.isEmpty() && keyOf(trackArtist) != artist && t.artist != artist)
            continue;
        const QString value = field == u"genre" ? t.genre : field == u"album" ? t.album : trackArtist;
        Entry& e = entries[keyOf(value)];
        e.name = value;
        ++e.count;
        e.albums.insert(t.albumKey());
        if (e.albumKey.isEmpty())
            e.albumKey = t.albumKey();
    }
    QList<QString> keys = entries.keys();
    std::sort(keys.begin(), keys.end(), [&](const QString& a, const QString& b) {
        // The unknown entry goes last.
        if ((a == kUnknown) != (b == kUnknown))
            return b == kUnknown;
        return QString::localeAwareCompare(entries[a].name, entries[b].name) < 0;
    });
    QVariantList list;
    list.reserve(keys.size());
    for (const QString& k : keys) {
        const Entry& e = entries[k];
        list << QVariantMap{{"key", k}, {"name", e.name}, {"count", e.count},
                            {"albums", static_cast<int>(e.albums.size())}, {"albumKey", e.albumKey}};
    }
    return list;
}

QStringList LibraryModel::audioPaths(const QList<QUrl>& urls) const
{
    QStringList paths;
    for (const QUrl& url : urls) {
        const QString path = QDir::cleanPath(url.isLocalFile() ? url.toLocalFile() : url.toString());
        if (QFileInfo(path).isFile() && sonkore::Metadata::isAudioFile(path))
            paths << path;
        else if (QFileInfo(path).suffix().compare(QLatin1String("cue"), Qt::CaseInsensitive) == 0)
            paths << sonkore::Metadata::cueTracks(path);
    }
    return paths;
}

QStringList LibraryModel::expandAudioPaths(const QList<QUrl>& urls) const
{
    QStringList paths;
    for (const QUrl& url : urls) {
        const QString path = QDir::cleanPath(url.isLocalFile() ? url.toLocalFile() : url.toString());
        const QFileInfo fi(path);
        if (fi.isFile() && sonkore::Metadata::isAudioFile(path)) {
            paths << path;
        } else if (fi.isFile() && fi.suffix().compare(QLatin1String("cue"), Qt::CaseInsensitive) == 0) {
            paths << sonkore::Metadata::cueTracks(path);
        } else if (fi.isDir()) {
            QStringList found, images;
            QDirIterator it(path, QDir::Files | QDir::Readable, QDirIterator::Subdirectories | QDirIterator::FollowSymlinks);
            while (it.hasNext()) {
                const QString file = it.next();
                if (QFileInfo(file).suffix().compare(QLatin1String("cue"), Qt::CaseInsensitive) == 0)
                    found << sonkore::Metadata::cueTracks(file, &images);
                else if (sonkore::Metadata::isAudioFile(file))
                    found << file;
            }
            for (const QString& image : images)
                found.removeAll(image);
            // Natural order: "Track 2" before "Track 10", "disc.cue#2" before "#10".
            QCollator collator;
            collator.setNumericMode(true);
            collator.setCaseSensitivity(Qt::CaseInsensitive);
            std::sort(found.begin(), found.end(), [&collator](const QString& a, const QString& b) { return collator.compare(a, b) < 0; });
            paths << found;
        }
    }
    return paths;
}

// --- Editing -----------------------------------------------------------------

// True when `path` is a watched folder or lies inside one.
static bool insideFolders(const QStringList& folders, const QString& path)
{
    return std::any_of(folders.cbegin(), folders.cend(),
                       [&](const QString& f) { return path == f || path.startsWith(f + u'/'); });
}

// Un-hides the hand-removed tracks under `folder`; true if there were any.
static bool restoreUnder(QStringList& excluded, const QString& folder)
{
    const QString prefix = folder + u'/';
    return excluded.removeIf([&](const QString& p) { return p.startsWith(prefix); }) > 0;
}

void LibraryModel::restoreHidden()
{
    if (m_excluded.isEmpty())
        return;
    m_excluded.clear();
    emit countChanged();
    rescan();
}

// Adds `path` (an existing directory) to the watched folders. Returns true
// when the library has to be scanned again.
bool LibraryModel::watchFolder(const QString& path)
{
    // Adding a folder brings back its skipped sub-folders too (and the
    // folder itself, if it was skipped).
    const QString prefix = path + u'/';
    const bool unskipped = m_excludedFolders.removeIf([&](const QString& f) {
        return f == path || f.startsWith(prefix);
    }) > 0;
    if (unskipped) {
        QSettings().setValue("library/excludedFolders", m_excludedFolders);
        emit preferencesChanged();
    }
    // Already in the library (the folder or a sub-folder of a watched one):
    // adding it again brings back what was removed from it by hand.
    if (insideFolders(m_folders, path))
        return restoreUnder(m_excluded, path) || unskipped;
    // A parent of watched folders takes their place: one root, no overlap.
    m_folders.removeIf([&](const QString& f) { return f.startsWith(prefix); });
    m_folders << path;
    emit foldersChanged();
    return true;
}

void LibraryModel::addFolder(const QUrl& folder)
{
    const QString path = QDir::cleanPath(folder.isLocalFile() ? folder.toLocalFile() : folder.toString());
    if (path.isEmpty() || !QFileInfo(path).isDir())
        return;
    if (watchFolder(path))
        rescan();
}

void LibraryModel::removeFolder(const QString& folder)
{
    if (!m_folders.removeOne(folder))
        return;
    // Its hand-removed tracks go with it: adding the folder again brings
    // everything back.
    restoreUnder(m_excluded, folder);
    emit foldersChanged();
    rescan();
}

void LibraryModel::addUrls(const QList<QUrl>& urls)
{
    bool changed = false;
    for (const QUrl& url : urls) {
        const QString path = QDir::cleanPath(url.isLocalFile() ? url.toLocalFile() : url.toString());
        const QFileInfo fi(path);
        if (fi.isDir()) {
            changed = watchFolder(path) || changed;
        } else if (fi.isFile() && sonkore::Metadata::isAudioFile(path)) {
            if (insideFolders(m_folders, path)) {
                // In a watched folder: only un-hide it.
                changed = m_excluded.removeAll(path) > 0 || changed;
            } else if (!m_extraFiles.contains(path)) {
                m_extraFiles << path;
                changed = true;
            }
        }
    }
    if (changed)
        rescan();
}

void LibraryModel::removeTracks(const QStringList& paths)
{
    QSet<QString> gone;
    for (const QString& path : paths) {
        if (!m_index.contains(path))
            continue;
        gone.insert(path);
        m_extraFiles.removeAll(path);
        if (insideFolders(m_folders, path) && !m_excluded.contains(path))
            m_excluded << path; // stays hidden even though its folder is watched
    }
    if (gone.isEmpty())
        return;
    QVector<TrackMeta> tracks;
    tracks.reserve(m_tracks.size() - gone.size());
    for (const TrackMeta& t : m_tracks)
        if (!gone.contains(t.path))
            tracks.push_back(t);
    setTracks(std::move(tracks));
    saveCache();
}

// --- Scanning ----------------------------------------------------------------

void LibraryModel::rescan()
{
    if (m_scanning) {
        m_rescanPending = true;
        return;
    }
    m_scanning = true;
    m_scannedFiles = 0;
    emit scanningChanged();
    emit scanProgress();

    const QStringList folders = m_folders;
    const QStringList extras = m_extraFiles;
    const QSet<QString> excluded(m_excluded.cbegin(), m_excluded.cend());
    const QStringList excludedFolders = m_excludedFolders;
    const double minDuration = m_minDuration;
    QHash<QString, TrackMeta> known;
    for (const TrackMeta& t : m_tracks)
        known.insert(t.path, t);
    auto cancel = m_cancel;

    m_scanThread = QThread::create([this, folders, extras, excluded, excludedFolders, minDuration, known, cancel]() {
        ScanResult result;
        QSet<QString> seen;
        int processed = 0;

        auto consider = [&](const QString& path) {
            if (seen.contains(path) || excluded.contains(path))
                return;
            seen.insert(path);
            const QFileInfo fi(path);
            const auto it = known.constFind(path);
            // A CD image track is unchanged while neither its sheet nor the
            // image file is (the newer of the two dates, see Metadata).
            const bool cueTrack = sonkore::CueSheet::isTrackPath(path.toStdString());
            const qint64 stamp = cueTrack ? cueStamp(path) : fi.lastModified().toMSecsSinceEpoch();
            if (it != known.cend() && it->modified == stamp && (cueTrack || it->fileSize == fi.size())) {
                if (it->sampleRate > 0 && it->channels > 0) // unreadable ones were kept by older versions
                    result.tracks.push_back(*it);
            } else {
                TrackMeta t;
                // Files that are not audio after all (empty, damaged, a text
                // file named .wav) would only fail when played: left out.
                if (sonkore::Metadata::read(path, t) && t.sampleRate > 0 && t.channels > 0) {
                    t.added = it != known.cend() ? it->added : QDateTime::currentMSecsSinceEpoch();
                    result.tracks.push_back(std::move(t));
                }
            }
            if (++processed % 64 == 0)
                QMetaObject::invokeMethod(this, [this, processed] {
                    m_scannedFiles = processed;
                    emit scanProgress();
                }, Qt::QueuedConnection);
        };

        QSet<QString> visitedDirs; // canonical paths: each real folder is read once
        for (const QString& folder : folders) {
            if (!QFileInfo(folder).isDir()) {
                // Unplugged drive / unmounted share: keep what was known
                // instead of emptying that part of the library.
                for (auto it = known.cbegin(); it != known.cend(); ++it)
                    if (it.key().startsWith(folder + u'/') && !excluded.contains(it.key()) && !seen.contains(it.key())) {
                        seen.insert(it.key());
                        result.tracks.push_back(it.value());
                    }
                continue;
            }
            // Depth-first walk that follows linked folders, except links
            // back up to the root or above it (they would only repeat it, or
            // pull unrelated music in) and folders already read.
            // Real folders go first, so music reachable both ways is listed
            // under its real path rather than through a link.
            const QString rootCanonical = QFileInfo(folder).canonicalFilePath();
            QStringList pending{folder}, linked;
            while ((!pending.isEmpty() || !linked.isEmpty()) && !cancel->load()) {
                const QString dir = !pending.isEmpty() ? pending.takeLast() : linked.takeLast();
                const QString canonical = QFileInfo(dir).canonicalFilePath();
                if (canonical.isEmpty() || visitedDirs.contains(canonical))
                    continue;
                if (dir != folder && (rootCanonical == canonical || rootCanonical.startsWith(canonical + u'/')))
                    continue;
                if (excludedFolders.contains(dir) || excludedFolders.contains(canonical))
                    continue;
                visitedDirs.insert(canonical);
                result.directories << dir;
                const QFileInfoList entries = QDir(dir).entryInfoList(
                    QDir::AllEntries | QDir::NoDotAndDotDot | QDir::Readable, QDir::Name | QDir::Reversed);
                // CD images: the sheet's tracks stand for the image file.
                QStringList images;
                for (const QFileInfo& fi : entries)
                    if (fi.isFile() && fi.suffix().compare(QLatin1String("cue"), Qt::CaseInsensitive) == 0)
                        for (const QString& track : sonkore::Metadata::cueTracks(fi.filePath(), &images))
                            consider(track);
                for (const QFileInfo& fi : entries) {
                    if (fi.isDir())
                        (fi.isSymLink() ? linked : pending) << fi.filePath();
                    else if (sonkore::Metadata::isAudioFile(fi.filePath()) && !images.contains(fi.filePath()))
                        consider(fi.filePath());
                }
            }
        }
        for (const QString& path : extras)
            if (!cancel->load() && (QFileInfo::exists(path) || sonkore::CueSheet::isTrackPath(path.toStdString())))
                consider(path);

        if (cancel->load())
            return;
        // Short files (jingles, effects): out, unless their length is unknown.
        if (minDuration > 0)
            result.tracks.erase(std::remove_if(result.tracks.begin(), result.tracks.end(),
                                               [&](const TrackMeta& t) { return t.duration > 0 && t.duration < minDuration; }),
                                result.tracks.end());
        sortTracks(result.tracks);
        QMetaObject::invokeMethod(this, [this, r = std::move(result)]() mutable { applyScan(std::move(r)); },
                                  Qt::QueuedConnection);
    });
    connect(m_scanThread, &QThread::finished, m_scanThread, &QObject::deleteLater);
    m_scanThread->start(QThread::LowPriority);
}

void LibraryModel::applyScan(ScanResult result)
{
    // The scan started from a copy of the library: loudness measured while it
    // ran is carried over to the files that did not change.
    for (TrackMeta& t : result.tracks) {
        const auto it = m_index.constFind(t.path);
        if (it == m_index.cend())
            continue;
        const TrackMeta& current = m_tracks[*it];
        if (current.rgAnalyzed && !t.rgAnalyzed && current.modified == t.modified && current.fileSize == t.fileSize) {
            t.rgTrackGain = current.rgTrackGain;
            t.rgTrackPeak = current.rgTrackPeak;
            t.rgAlbumGain = current.rgAlbumGain;
            t.rgAlbumPeak = current.rgAlbumPeak;
            t.rgAnalyzed = true;
        }
    }
    setTracks(std::move(result.tracks));
    watchDirectories(result.directories);
    saveCache();

    m_scanning = false;
    m_scannedFiles = count();
    emit scanProgress();
    emit scanningChanged();
    emit loudnessProgress(); // new files may lack loudness values

    if (m_rescanPending) {
        m_rescanPending = false;
        rescan();
    } else if (m_analyzePending || m_autoAnalyze) {
        m_analyzePending = false;
        analyzeLoudness();
    }
}

void LibraryModel::setAutoAnalyzeLoudness(bool on)
{
    m_autoAnalyze = on;
    if (on)
        analyzeLoudness();
}

// --- ReplayGain analysis -------------------------------------------------------

int LibraryModel::tracksWithoutLoudness() const
{
    return static_cast<int>(std::count_if(m_tracks.cbegin(), m_tracks.cend(), [](const TrackMeta& t) {
        return !t.hasReplayGain() || std::isnan(t.rgAlbumGain);
    }));
}

void LibraryModel::analyzeLoudness()
{
    if (m_analyzing)
        return;
    if (m_scanning) { // measure what the scan finds, once it is done
        m_analyzePending = true;
        return;
    }
    // Whole albums are measured whenever one of their tracks lacks a value:
    // the album gain needs all of them. Tracks without an album stand alone.
    QHash<QString, QVector<int>> albums;
    QStringList order;
    for (int i = 0; i < m_tracks.size(); ++i) {
        const TrackMeta& t = m_tracks[i];
        const QString key = t.album.isEmpty() ? t.path : t.albumKey();
        auto it = albums.find(key);
        if (it == albums.end()) {
            it = albums.insert(key, {});
            order << key;
        }
        it->push_back(i);
    }
    QVector<AlbumGains> work;
    int total = 0;
    for (const QString& key : order) {
        const QVector<int>& rows = albums[key];
        const bool needed = std::any_of(rows.cbegin(), rows.cend(), [this](int i) {
            return !m_tracks[i].hasReplayGain() || std::isnan(m_tracks[i].rgAlbumGain);
        });
        if (!needed)
            continue;
        AlbumGains album;
        for (int i : rows) {
            album.paths << m_tracks[i].path;
            album.modified << m_tracks[i].modified;
        }
        total += static_cast<int>(rows.size());
        work.push_back(std::move(album));
    }
    if (work.isEmpty())
        return;

    m_analyzing = true;
    m_loudnessDone = 0;
    m_loudnessTotal = total;
    m_unsavedAlbums = 0;
    emit loudnessProgress();

    const auto cancel = m_cancel;
    m_loudnessThread = QThread::create([this, work = std::move(work), cancel]() mutable {
        for (AlbumGains& album : work) {
            std::vector<std::filesystem::path> files;
            for (const QString& path : album.paths)
                files.emplace_back(QFile::encodeName(path).toStdString());
            const auto m = sonkore::Loudness::measureAlbum(files, *cancel);
            if (cancel->load())
                return;
            const double nan = std::numeric_limits<double>::quiet_NaN();
            for (const auto& track : m.tracks) {
                album.trackGain << (track.ok ? track.gainDb() : nan);
                album.trackPeak << track.peak;
            }
            album.albumOk = m.album.ok;
            album.albumGain = m.album.gainDb();
            album.albumPeak = m.album.peak;
            QMetaObject::invokeMethod(this, [this, album] { applyLoudness(album); });
        }
        QMetaObject::invokeMethod(this, [this] { finishLoudness(); });
    });
    connect(m_loudnessThread, &QThread::finished, m_loudnessThread, &QObject::deleteLater);
    m_loudnessThread->start(QThread::LowestPriority);
}

// Values read from tags stay; only the missing ones are filled in.
void LibraryModel::applyLoudness(const AlbumGains& gains)
{
    QStringList updated;
    for (int k = 0; k < gains.paths.size(); ++k) {
        const auto it = m_index.constFind(gains.paths[k]);
        if (it == m_index.cend())
            continue;
        TrackMeta& t = m_tracks[*it];
        if (t.modified != gains.modified[k])
            continue; // rescanned meanwhile: the measurement may be stale
        bool changed = false;
        if (!t.hasReplayGain() && !std::isnan(gains.trackGain[k])) {
            t.rgTrackGain = gains.trackGain[k];
            t.rgTrackPeak = gains.trackPeak[k];
            changed = true;
        }
        if (std::isnan(t.rgAlbumGain)) {
            // Silent or unreadable album: the track gain stands in for it.
            t.rgAlbumGain = gains.albumOk ? gains.albumGain : t.rgTrackGain;
            t.rgAlbumPeak = gains.albumOk ? gains.albumPeak : t.rgTrackPeak;
            changed = changed || !std::isnan(t.rgAlbumGain);
        }
        if (changed) {
            t.rgAnalyzed = true;
            updated << t.path;
        }
    }
    m_loudnessDone += static_cast<int>(gains.paths.size());
    if (++m_unsavedAlbums >= 25) { // keep the work if the app is closed midway
        saveCache();
        m_unsavedAlbums = 0;
    }
    emit loudnessProgress();
    if (!updated.isEmpty())
        emit loudnessUpdated(updated);
}

void LibraryModel::finishLoudness()
{
    saveCache();
    m_analyzing = false;
    emit loudnessProgress();
}

void LibraryModel::setTracks(QVector<TrackMeta> tracks)
{
    shareRepeatedStrings(tracks);
    beginResetModel();
    m_tracks = std::move(tracks);
    m_index.clear();
    m_searchText.clear();
    m_index.reserve(m_tracks.size());
    m_searchText.reserve(m_tracks.size());
    for (int i = 0; i < m_tracks.size(); ++i) {
        m_index.insert(m_tracks[i].path, i);
        m_searchText.push_back(searchKey(m_tracks[i]));
    }
    endResetModel();
    emit countChanged(); // also refreshes totalDuration
}

void LibraryModel::watchDirectories(const QStringList& dirs)
{
    m_scannedDirs = dirs;
    const QStringList current = m_watcher.directories();
    if (!current.isEmpty())
        m_watcher.removePaths(current);
    if (!m_watchFolders)
        return;
    const QStringList limited = dirs.mid(0, kMaxWatchedDirs);
    if (!limited.isEmpty())
        m_watcher.addPaths(limited);
}

void LibraryModel::sortTracks(QVector<TrackMeta>& tracks)
{
    std::sort(tracks.begin(), tracks.end(), [](const TrackMeta& a, const TrackMeta& b) {
        const QString aa = (a.albumArtist.isEmpty() ? a.artist : a.albumArtist).toLower();
        const QString ba = (b.albumArtist.isEmpty() ? b.artist : b.albumArtist).toLower();
        if (aa != ba) return aa < ba;
        if (a.year != b.year) return a.year < b.year;
        const QString al = a.album.toLower(), bl = b.album.toLower();
        if (al != bl) return al < bl;
        if (a.discNo != b.discNo) return a.discNo < b.discNo;
        if (a.trackNo != b.trackNo) return a.trackNo < b.trackNo;
        return a.path < b.path;
    });
}

// --- Persistence -------------------------------------------------------------

void LibraryModel::loadCache()
{
    QFile f(cachePath());
    if (!f.open(QIODevice::ReadOnly))
        return;
    // Straight from the JSON tree: a QVariant copy of it would triple the
    // start-up peak and leave the heap fragmented.
    QVector<TrackMeta> tracks;
    {
        const QJsonObject root = QJsonDocument::fromJson(f.readAll()).object();
        f.close();
        m_folders = stringList(root.value("folders"));
        m_extraFiles = stringList(root.value("files"));
        m_excluded = stringList(root.value("excluded"));
        const QJsonArray list = root.value("tracks").toArray();
        // Version 1 did not read loudness tags: mark its entries as changed
        // so the next scan reads every file again (dates added are kept).
        const int version = root.value("version").toInt();
        tracks.reserve(list.size());
        for (const QJsonValue& v : list) {
            tracks.push_back(TrackMeta::fromJson(v.toObject()));
            TrackMeta& t = tracks.back();
            // Version 2 read DSD without its length and kept files with no
            // audio: only those are read again (measured loudness is kept).
            if (version < 2 || (version < 3 && (t.duration <= 0.0 || t.sampleRate <= 0)))
                t.modified = 0;
        }
    }
    setTracks(std::move(tracks));
}

void LibraryModel::saveCache() const
{
    QJsonArray tracks;
    for (const TrackMeta& t : m_tracks)
        tracks.append(t.toJson());
    const QJsonObject root{
        {"version", kCacheVersion},
        {"folders", QJsonArray::fromStringList(m_folders)},
        {"files", QJsonArray::fromStringList(m_extraFiles)},
        {"excluded", QJsonArray::fromStringList(m_excluded)},
        {"tracks", tracks},
    };
    QDir().mkpath(QFileInfo(cachePath()).path());
    QFile f(cachePath());
    if (f.open(QIODevice::WriteOnly | QIODevice::Truncate))
        f.write(QJsonDocument(root).toJson(QJsonDocument::Compact));
}

void LibraryModel::loadFavorites()
{
    QFile f(favoritesPath());
    if (!f.open(QIODevice::ReadOnly))
        return;
    const QStringList list = stringList(QJsonDocument::fromJson(f.readAll()).object().value("tracks"));
    m_favorites = QSet<QString>(list.cbegin(), list.cend());
}

void LibraryModel::saveFavorites() const
{
    QStringList list(m_favorites.cbegin(), m_favorites.cend());
    list.sort();
    QDir().mkpath(QFileInfo(favoritesPath()).path());
    QFile f(favoritesPath());
    if (f.open(QIODevice::WriteOnly | QIODevice::Truncate))
        f.write(QJsonDocument(QJsonObject{{"tracks", QJsonArray::fromStringList(list)}}).toJson());
}

void LibraryModel::setFavorite(const QString& path, bool on)
{
    if (path.isEmpty() || on == m_favorites.contains(path))
        return;
    if (on)
        m_favorites.insert(path);
    else
        m_favorites.remove(path);
    saveFavorites();
    ++m_favoritesRevision;
    const auto it = m_index.constFind(path);
    if (it != m_index.cend()) {
        const QModelIndex i = index(it.value());
        emit dataChanged(i, i); // every role: proxies re-filter and re-sort the row
    }
    emit favoritesChanged();
}
