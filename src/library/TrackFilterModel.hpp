#pragma once

#include <QCollator>
#include <QSortFilterProxyModel>
#include <QStringList>
#include <QVariantMap>
#include <QtQml/qqmlregistration.h>

// Searchable, sortable view over the Library singleton. Instantiate freely in
// QML; each instance keeps its own search text, album/folder filter and sort.
class TrackFilterModel : public QSortFilterProxyModel {
    Q_OBJECT
    QML_ELEMENT

    Q_PROPERTY(QString search READ search WRITE setSearch NOTIFY searchChanged)
    Q_PROPERTY(QString albumKey READ albumKey WRITE setAlbumKey NOTIFY albumKeyChanged)
    Q_PROPERTY(QString folder READ folder WRITE setFolder NOTIFY folderChanged)
    // Browser filters ("" = any; LibraryModel::kUnknown = the tag is empty).
    // The artist matches the track artist or the album artist.
    Q_PROPERTY(QString genre READ genre WRITE setGenre NOTIFY browseChanged)
    Q_PROPERTY(QString artist READ artist WRITE setArtist NOTIFY browseChanged)
    Q_PROPERTY(QString album READ album WRITE setAlbum NOTIFY browseChanged)
    Q_PROPERTY(bool favoritesOnly READ favoritesOnly WRITE setFavoritesOnly NOTIFY favoritesOnlyChanged)
    Q_PROPERTY(QString sortField READ sortField WRITE setSortField NOTIFY sortChanged)
    Q_PROPERTY(bool descending READ descending WRITE setDescending NOTIFY sortChanged)
    Q_PROPERTY(int count READ count NOTIFY countChanged)

public:
    explicit TrackFilterModel(QObject* parent = nullptr);

    QString search() const { return m_search; }
    void setSearch(const QString& text);
    QString albumKey() const { return m_albumKey; }
    void setAlbumKey(const QString& key);
    QString folder() const { return m_folder; }
    void setFolder(const QString& folder); // tracks inside this folder, recursively
    QString genre() const { return m_genre; }
    void setGenre(const QString& genre);
    QString artist() const { return m_artist; }
    void setArtist(const QString& artist);
    QString album() const { return m_album; }
    void setAlbum(const QString& album);
    bool favoritesOnly() const { return m_favoritesOnly; }
    void setFavoritesOnly(bool on);
    QString sortField() const { return m_sortField; }
    void setSortField(const QString& field); // any library role name, "" = library order
    bool descending() const { return m_descending; }
    void setDescending(bool on);
    int count() const { return rowCount(); }

    Q_INVOKABLE QVariantMap get(int row) const;
    Q_INVOKABLE QString pathAt(int row) const;
    Q_INVOKABLE QStringList paths() const;
    Q_INVOKABLE QStringList pathsOf(const QList<int>& rows) const;
    Q_INVOKABLE int rowOfPath(const QString& path) const;

signals:
    void searchChanged();
    void albumKeyChanged();
    void folderChanged();
    void browseChanged();
    void favoritesOnlyChanged();
    void sortChanged();
    void countChanged();

protected:
    bool filterAcceptsRow(int sourceRow, const QModelIndex& sourceParent) const override;
    bool lessThan(const QModelIndex& left, const QModelIndex& right) const override;

private:
    // Qt 6.10 replaced invalidateFilter() with begin/endFilterChange().
    template <typename Change>
    void changeFilter(Change&& change)
    {
#if QT_VERSION >= QT_VERSION_CHECK(6, 10, 0)
        beginFilterChange();
        change();
        endFilterChange();
#else
        change();
        invalidateFilter();
#endif
    }
    void applySort();
    void setBrowse(QString& field, const QString& value);

    QString m_search;
    QStringList m_words;
    QString m_albumKey;
    QString m_folder;
    QString m_folderPrefix;
    QString m_genre;
    QString m_artist;
    QString m_album;
    bool m_favoritesOnly = false;
    QString m_sortField;
    int m_sortRole = -1;
    QList<QPair<QString, int>> m_sortKeys; // the field and its tie-breakers (TrackSort::keysFor), with their roles
    bool m_descending = false;
    QCollator m_collator; // natural order: "2" before "10", case and accents aside
};
