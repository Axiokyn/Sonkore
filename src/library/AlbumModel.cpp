#include "AlbumModel.hpp"

#include "LibraryModel.hpp"

#include <QHash>

AlbumModel::AlbumModel(QObject* parent)
    : QAbstractListModel(parent)
{
    connect(LibraryModel::instance(), &QAbstractItemModel::modelReset, this, &AlbumModel::rebuild);
    rebuild();
}

int AlbumModel::rowCount(const QModelIndex& parent) const
{
    return parent.isValid() ? 0 : static_cast<int>(m_visible.size());
}

QVariant AlbumModel::data(const QModelIndex& index, int role) const
{
    if (!index.isValid() || index.row() >= m_visible.size())
        return {};
    const Album& a = m_all[m_visible[index.row()]];
    switch (role) {
    case Qt::DisplayRole:
    case AlbumRole: return a.album;
    case AlbumKeyRole: return a.key;
    case ArtistRole: return a.artist;
    case YearRole: return a.year;
    case CoverPathRole: return a.coverPath;
    case TrackCountRole: return a.tracks;
    case DurationRole: return a.duration;
    case FormatRole: return a.format;
    default: return {};
    }
}

QHash<int, QByteArray> AlbumModel::roleNames() const
{
    return {
        {AlbumKeyRole, "albumKey"},
        {AlbumRole, "album"},
        {ArtistRole, "artist"},
        {YearRole, "year"},
        {CoverPathRole, "coverPath"},
        {TrackCountRole, "trackCount"},
        {DurationRole, "duration"},
        {FormatRole, "format"},
    };
}

void AlbumModel::setSearch(const QString& text)
{
    if (text == m_search)
        return;
    m_search = text;
    refilter();
    emit searchChanged();
}

QVariantMap AlbumModel::toMap(const Album& a)
{
    return {
        {"albumKey", a.key}, {"album", a.album}, {"artist", a.artist}, {"year", a.year},
        {"coverPath", a.coverPath}, {"trackCount", a.tracks}, {"duration", a.duration}, {"format", a.format},
    };
}

QVariantMap AlbumModel::get(int row) const
{
    if (row < 0 || row >= m_visible.size())
        return {};
    return toMap(m_all[m_visible[row]]);
}

QVariantMap AlbumModel::byKey(const QString& albumKey) const
{
    for (const Album& a : m_all)
        if (a.key == albumKey)
            return toMap(a);
    return {};
}

void AlbumModel::rebuild()
{
    QHash<QString, int> index;
    QVector<Album> albums;
    for (const sonkore::TrackMeta& t : LibraryModel::instance()->tracks()) { // sorted by artist/year/album
        const QString key = t.albumKey();
        auto it = index.find(key);
        if (it == index.end()) {
            Album a;
            a.key = key;
            a.album = t.album;
            a.artist = t.albumArtist.isEmpty() ? t.artist : t.albumArtist;
            a.year = t.year;
            a.coverPath = t.path;
            a.format = t.formatLabel();
            a.searchText = (a.album + ' ' + a.artist).toLower();
            it = index.insert(key, static_cast<int>(albums.size()));
            albums.push_back(a);
        }
        Album& a = albums[it.value()];
        a.tracks += 1;
        a.duration += t.duration;
        if (t.hasEmbeddedCover && !a.hasEmbeddedCover) { // prefer a track that carries art
            a.coverPath = t.path;
            a.hasEmbeddedCover = true;
        }
    }
    beginResetModel();
    m_all = std::move(albums);
    m_visible.clear();
    endResetModel();
    refilter();
}

void AlbumModel::refilter()
{
    const QStringList words = m_search.toLower().split(' ', Qt::SkipEmptyParts);
    beginResetModel();
    m_visible.clear();
    for (int i = 0; i < m_all.size(); ++i) {
        bool ok = true;
        for (const QString& w : words)
            if (!m_all[i].searchText.contains(w)) {
                ok = false;
                break;
            }
        if (ok)
            m_visible.push_back(i);
    }
    endResetModel();
    emit countChanged();
}
