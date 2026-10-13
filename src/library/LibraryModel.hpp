#pragma once

#include "Metadata.hpp"
#include "QmlSingleton.hpp"

#include <QAbstractListModel>
#include <QFileSystemWatcher>
#include <QHash>
#include <QSet>
#include <QPointer>
#include <QThread>
#include <QTimer>
#include <QUrl>
#include <QVector>
#include <QtQml/qqmlregistration.h>

#include <atomic>
#include <memory>

// The music library: every audio file under the watched folders plus files
// added individually. Scans run on a background thread, are incremental
// (unchanged files keep their cached tags) and are re-triggered by
// filesystem changes. The result is cached as JSON in the app data folder.
class LibraryModel : public QAbstractListModel {
    Q_OBJECT
    QML_NAMED_ELEMENT(Library)
    QML_SINGLETON
    SONKORE_QML_SINGLETON_INSTANCE(LibraryModel)

    Q_PROPERTY(QStringList folders READ folders NOTIFY foldersChanged)
    Q_PROPERTY(int count READ count NOTIFY countChanged)
    Q_PROPERTY(double totalDuration READ totalDuration NOTIFY countChanged)
    Q_PROPERTY(bool scanning READ scanning NOTIFY scanningChanged)
    Q_PROPERTY(int scannedFiles READ scannedFiles NOTIFY scanProgress)
    Q_PROPERTY(bool watchFolders READ watchFolders WRITE setWatchFolders NOTIFY preferencesChanged)
    Q_PROPERTY(bool rescanOnStartup READ rescanOnStartup WRITE setRescanOnStartup NOTIFY preferencesChanged)
    // Folders inside the music folders that the scan skips (audiobooks, samples...).
    Q_PROPERTY(QStringList excludedFolders READ excludedFolders NOTIFY preferencesChanged)
    // Files shorter than this (seconds) are left out: jingles, sound effects. 0 = all.
    Q_PROPERTY(int minDuration READ minDuration WRITE setMinDuration NOTIFY preferencesChanged)
    Q_PROPERTY(QString unknownKey READ unknownKey CONSTANT)
    // "*.flac *.wav *.mp3 ..." for file dialogs: what this build can play.
    Q_PROPERTY(QString audioNameFilter READ audioNameFilter CONSTANT)
    // Tracks removed from the library by hand (hidden even though their
    // folder is watched).
    Q_PROPERTY(int hiddenCount READ hiddenCount NOTIFY countChanged)
    // ReplayGain analysis of the tracks whose files carry no loudness tags.
    Q_PROPERTY(bool analyzingLoudness READ analyzingLoudness NOTIFY loudnessProgress)
    Q_PROPERTY(int loudnessDone READ loudnessDone NOTIFY loudnessProgress)
    Q_PROPERTY(int loudnessTotal READ loudnessTotal NOTIFY loudnessProgress)
    Q_PROPERTY(int tracksWithoutLoudness READ tracksWithoutLoudness NOTIFY loudnessProgress)
    // Bumped on every favourite change, so QML bindings on isFavorite() re-run.
    Q_PROPERTY(int favoritesRevision READ favoritesRevision NOTIFY favoritesChanged)
    Q_PROPERTY(int favoriteCount READ favoriteCount NOTIFY favoritesChanged)

public:
    enum Roles {
        PathRole = Qt::UserRole + 1,
        TitleRole,
        ArtistRole,
        AlbumRole,
        AlbumArtistRole,
        GenreRole,
        TrackNoRole,
        DiscNoRole,
        YearRole,
        DurationRole,
        DurationTextRole,
        FormatRole,
        AlbumKeyRole,
        SearchTextRole,
        CodecRole,
        SampleRateRole,
        BitsRole,
        ChannelsRole,
        BitrateRole,
        FileSizeRole,
        FileNameRole,
        FolderRole,
        ModifiedRole,
        AddedRole,
        FavoriteRole, // library rows only (TrackListModel has no favourites)
    };

    // No default argument on purpose: a default-constructible QML_SINGLETON
    // would make QML build its own instance instead of calling create().
    explicit LibraryModel(QObject* parent);
    // Browser value standing for an empty tag (no genre, no artist...).
    static inline const QString kUnknown = QStringLiteral("\x01");
    static QString unknownKey() { return kUnknown; }
    ~LibraryModel() override;

    int rowCount(const QModelIndex& parent = {}) const override;
    QVariant data(const QModelIndex& index, int role) const override;
    QHash<int, QByteArray> roleNames() const override;
    // Shared with TrackListModel: one track's value for a role (all roles
    // but SearchTextRole), and the role names.
    static QVariant roleData(const sonkore::TrackMeta& t, int role);
    static QHash<int, QByteArray> trackRoleNames();

    QString audioNameFilter() const;
    int hiddenCount() const { return static_cast<int>(m_excluded.size()); }
    // Brings every hand-removed track back (on the next scan).
    Q_INVOKABLE void restoreHidden();
    QStringList folders() const { return m_folders; }
    int count() const { return static_cast<int>(m_tracks.size()); }
    double totalDuration() const;
    bool scanning() const { return m_scanning; }
    int scannedFiles() const { return m_scannedFiles; }
    bool watchFolders() const { return m_watchFolders; }
    void setWatchFolders(bool on);
    bool rescanOnStartup() const { return m_rescanOnStartup; }
    void setRescanOnStartup(bool on);
    QStringList excludedFolders() const { return m_excludedFolders; }
    Q_INVOKABLE void addExcludedFolder(const QUrl& folder);
    Q_INVOKABLE void excludeFolder(const QString& folder); // by path
    Q_INVOKABLE void removeExcludedFolder(const QString& folder);
    int minDuration() const { return m_minDuration; }
    void setMinDuration(int seconds);

    // Values of `field` ("genre", "artist" or "album") among the tracks that
    // match the given genre / artist ("" = any): [{ key, name, count, albums,
    // albumKey }], sorted by name. The artist is the album artist, else the
    // track artist. An empty tag is listed once, with key kUnknown.
    Q_INVOKABLE QVariantList browse(const QString& field, const QString& genre = {}, const QString& artist = {}) const;

    const QVector<sonkore::TrackMeta>& tracks() const { return m_tracks; }
    const sonkore::TrackMeta* find(const QString& path) const;

    // Track info as a QML map; reads tags directly for files not in the library.
    Q_INVOKABLE QVariantMap track(const QString& path) const;
    Q_INVOKABLE QStringList albumPaths(const QString& albumKey) const;
    Q_INVOKABLE QStringList folderPaths(const QString& folder) const; // recursive, library order
    // Every folder that holds tracks directly: [{ path, name, count }], name
    // relative to its library folder ("Rock/Alt"), in natural order.
    Q_INVOKABLE QVariantList trackFolders() const;
    // Local paths of the audio files among `urls` (folders/other files skipped).
    Q_INVOKABLE QStringList audioPaths(const QList<QUrl>& urls) const;
    // Like audioPaths(), but dropped folders are expanded recursively.
    Q_INVOKABLE QStringList expandAudioPaths(const QList<QUrl>& urls) const;

    Q_INVOKABLE void addFolder(const QUrl& folder);
    Q_INVOKABLE void removeFolder(const QString& folder);
    Q_INVOKABLE void addUrls(const QList<QUrl>& urls); // files and/or folders
    // Several tracks at once (one model reset, one cache write).
    Q_INVOKABLE void removeTracks(const QStringList& paths);
    Q_INVOKABLE void rescan();

    bool analyzingLoudness() const { return m_analyzing; }
    int loudnessDone() const { return m_loudnessDone; }
    int loudnessTotal() const { return m_loudnessTotal; }
    int tracksWithoutLoudness() const;
    // Measures, album by album, every track that lacks track or album gain.
    // Results go to the library cache; the audio files are never written.
    Q_INVOKABLE void analyzeLoudness();

    // Favourites, by path: kept in their own file (favorites.json), so they
    // survive rescans and tracks that leave the library for a while.
    int favoritesRevision() const { return m_favoritesRevision; }
    int favoriteCount() const { return static_cast<int>(m_favorites.size()); }
    Q_INVOKABLE bool isFavorite(const QString& path) const { return m_favorites.contains(path); }
    Q_INVOKABLE void setFavorite(const QString& path, bool on);
    Q_INVOKABLE void toggleFavorite(const QString& path) { setFavorite(path, !isFavorite(path)); }
    // While on, new and changed files are measured after every scan.
    void setAutoAnalyzeLoudness(bool on);

signals:
    void foldersChanged();
    void countChanged();
    void scanningChanged();
    void scanProgress();
    void preferencesChanged();
    void loudnessProgress();
    void favoritesChanged();
    // Gains measured for these tracks (the player re-applies them).
    void loudnessUpdated(const QStringList& paths);

private:
    struct ScanResult {
        QVector<sonkore::TrackMeta> tracks;
        QStringList directories;
    };

    bool watchFolder(const QString& path);
    void loadCache();
    void saveCache() const;
    void loadFavorites();
    void saveFavorites() const;
    void applyScan(ScanResult result);
    void setTracks(QVector<sonkore::TrackMeta> tracks);
    void watchDirectories(const QStringList& dirs);
    static void sortTracks(QVector<sonkore::TrackMeta>& tracks);

    QVector<sonkore::TrackMeta> m_tracks;
    QHash<QString, int> m_index;
    QVector<QString> m_searchText;
    QStringList m_folders;
    QStringList m_extraFiles;
    QStringList m_excluded;
    QSet<QString> m_favorites;
    int m_favoritesRevision = 0;

    bool m_scanning = false;
    bool m_rescanPending = false;
    int m_scannedFiles = 0;
    QPointer<QThread> m_scanThread;
    std::shared_ptr<std::atomic<bool>> m_cancel = std::make_shared<std::atomic<bool>>(false);

    struct AlbumGains {
        QStringList paths;
        QVector<qint64> modified; // to skip files changed while being measured
        QVector<double> trackGain, trackPeak; // NaN = not measured
        double albumGain = 0.0, albumPeak = 0.0;
        bool albumOk = false;
    };
    void applyLoudness(const AlbumGains& gains);
    void finishLoudness();
    bool m_analyzing = false;
    bool m_analyzePending = false;
    bool m_autoAnalyze = false;
    int m_loudnessDone = 0;
    int m_loudnessTotal = 0;
    int m_unsavedAlbums = 0;
    QPointer<QThread> m_loudnessThread;

    QFileSystemWatcher m_watcher;
    QTimer m_rescanDebounce;
    QStringList m_scannedDirs;
    bool m_watchFolders = true;
    bool m_rescanOnStartup = true;
    QStringList m_excludedFolders;
    int m_minDuration = 0;
};
