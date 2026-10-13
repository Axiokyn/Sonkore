#include "TrackFilterModel.hpp"

#include "LibraryModel.hpp"
#include "TrackSort.hpp"

#include <algorithm>

TrackFilterModel::TrackFilterModel(QObject* parent)
    : QSortFilterProxyModel(parent)
{
    setSourceModel(LibraryModel::instance());
    setDynamicSortFilter(true);
    m_collator = TrackSort::collator();
    connect(this, &QAbstractItemModel::rowsInserted, this, &TrackFilterModel::countChanged);
    connect(this, &QAbstractItemModel::rowsRemoved, this, &TrackFilterModel::countChanged);
    connect(this, &QAbstractItemModel::modelReset, this, &TrackFilterModel::countChanged);
    connect(this, &QAbstractItemModel::layoutChanged, this, &TrackFilterModel::countChanged);
}

void TrackFilterModel::setSearch(const QString& text)
{
    if (text == m_search)
        return;
    changeFilter([&] {
        m_search = text;
        m_words = text.toLower().split(' ', Qt::SkipEmptyParts);
    });
    emit searchChanged();
    emit countChanged();
}

void TrackFilterModel::setAlbumKey(const QString& key)
{
    if (key == m_albumKey)
        return;
    changeFilter([&] { m_albumKey = key; });
    emit albumKeyChanged();
    emit countChanged();
}

void TrackFilterModel::setFolder(const QString& folder)
{
    if (folder == m_folder)
        return;
    changeFilter([&] {
        m_folder = folder;
        m_folderPrefix = folder.isEmpty() ? QString() : (folder.endsWith('/') ? folder : folder + '/');
    });
    emit folderChanged();
    emit countChanged();
}

void TrackFilterModel::setBrowse(QString& field, const QString& value)
{
    if (value == field)
        return;
    changeFilter([&] { field = value; });
    emit browseChanged();
    emit countChanged();
}

void TrackFilterModel::setGenre(const QString& genre) { setBrowse(m_genre, genre); }
void TrackFilterModel::setArtist(const QString& artist) { setBrowse(m_artist, artist); }
void TrackFilterModel::setAlbum(const QString& album) { setBrowse(m_album, album); }

void TrackFilterModel::setFavoritesOnly(bool on)
{
    if (on == m_favoritesOnly)
        return;
    changeFilter([&] { m_favoritesOnly = on; });
    emit favoritesOnlyChanged();
    emit countChanged();
}

void TrackFilterModel::setSortField(const QString& field)
{
    if (field == m_sortField)
        return;
    m_sortField = field;
    m_sortRole = -1;
    m_sortKeys.clear();
    if (!field.isEmpty() && sourceModel()) {
        const auto roles = sourceModel()->roleNames();
        for (const QString& key : TrackSort::keysFor(field)) {
            const int role = roles.key(key.toUtf8(), -1);
            if (role >= 0)
                m_sortKeys.append({key, role});
        }
        if (!m_sortKeys.isEmpty() && m_sortKeys.first().first == field)
            m_sortRole = m_sortKeys.first().second;
        else
            m_sortKeys.clear();
    }
    applySort();
}

void TrackFilterModel::setDescending(bool on)
{
    if (on == m_descending)
        return;
    m_descending = on;
    applySort();
}

void TrackFilterModel::applySort()
{
    // sort() is a no-op when column and order are unchanged, but the role used
    // by lessThan() may have changed: reset first so the new key always applies.
    sort(-1); // library order: artist / year / album / disc / track
    // By a field, lessThan() turns only the field round (its tie-breakers
    // stay in order, blanks last); the library order is simply reversed.
    if (m_sortRole >= 0)
        sort(0, Qt::AscendingOrder);
    else if (m_descending)
        sort(0, Qt::DescendingOrder);
    emit sortChanged();
}

bool TrackFilterModel::filterAcceptsRow(int sourceRow, const QModelIndex& sourceParent) const
{
    const QModelIndex idx = sourceModel()->index(sourceRow, 0, sourceParent);
    if (m_favoritesOnly && !idx.data(LibraryModel::FavoriteRole).toBool())
        return false;
    if (!m_albumKey.isEmpty() && idx.data(LibraryModel::AlbumKeyRole).toString() != m_albumKey)
        return false;
    if (!m_folderPrefix.isEmpty() && !idx.data(LibraryModel::PathRole).toString().startsWith(m_folderPrefix))
        return false;
    // kUnknown stands for an empty tag.
    const auto matches = [](const QString& wanted, const QString& value) {
        return wanted == LibraryModel::kUnknown ? value.isEmpty() : value == wanted;
    };
    if (!m_genre.isEmpty() && !matches(m_genre, idx.data(LibraryModel::GenreRole).toString()))
        return false;
    if (!m_artist.isEmpty()) {
        const QString artist = idx.data(LibraryModel::ArtistRole).toString(); // empty only without both tags
        if (m_artist == LibraryModel::kUnknown ? !artist.isEmpty()
                                                : artist != m_artist && idx.data(LibraryModel::AlbumArtistRole).toString() != m_artist)
            return false;
    }
    if (!m_album.isEmpty() && !matches(m_album, idx.data(LibraryModel::AlbumRole).toString()))
        return false;
    if (m_words.isEmpty())
        return true;
    const QString hay = idx.data(LibraryModel::SearchTextRole).toString();
    for (const QString& w : m_words)
        if (!hay.contains(w))
            return false;
    return true;
}

bool TrackFilterModel::lessThan(const QModelIndex& l, const QModelIndex& r) const
{
    if (m_sortRole < 0)
        return l.row() < r.row();
    for (int i = 0; i < m_sortKeys.size(); ++i) {
        const auto& [key, role] = m_sortKeys[i];
        const bool down = i == 0 && m_descending;
        const int c = TrackSort::compareKey(m_collator, key, l.data(role), r.data(role), down);
        if (c != 0)
            return down ? c > 0 : c < 0;
    }
    return l.row() < r.row(); // stable on ties
}

QVariantMap TrackFilterModel::get(int row) const
{
    const QString path = pathAt(row);
    return path.isEmpty() ? QVariantMap() : LibraryModel::instance()->track(path);
}

QString TrackFilterModel::pathAt(int row) const
{
    return index(row, 0).data(LibraryModel::PathRole).toString();
}

QStringList TrackFilterModel::paths() const
{
    QStringList result;
    result.reserve(rowCount());
    for (int i = 0; i < rowCount(); ++i)
        result << pathAt(i);
    return result;
}

QStringList TrackFilterModel::pathsOf(const QList<int>& rows) const
{
    QStringList result;
    for (int row : rows)
        if (row >= 0 && row < rowCount())
            result << pathAt(row);
    return result;
}

int TrackFilterModel::rowOfPath(const QString& path) const
{
    for (int i = 0; i < rowCount(); ++i)
        if (pathAt(i) == path)
            return i;
    return -1;
}
