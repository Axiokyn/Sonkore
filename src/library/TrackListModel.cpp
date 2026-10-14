#include "TrackListModel.hpp"

#include "LibraryModel.hpp"

#include <QFileInfo>
#include <QSet>

#include <algorithm>

TrackListModel::TrackListModel(QObject* parent)
    : QAbstractListModel(parent)
{
    if (LibraryModel* library = LibraryModel::instance()) {
        // Tracks added or removed: a file may have joined or left the library,
        // its tags are read again. A changed row (a favourite) only repaints.
        connect(library, &QAbstractItemModel::modelReset, this, &TrackListModel::libraryChanged);
        connect(library, &QAbstractItemModel::rowsInserted, this, &TrackListModel::libraryChanged);
        connect(library, &QAbstractItemModel::rowsRemoved, this, &TrackListModel::libraryChanged);
        connect(library, &QAbstractItemModel::dataChanged, this, &TrackListModel::repaint);
    }
}

int TrackListModel::rowCount(const QModelIndex& parent) const
{
    return parent.isValid() ? 0 : static_cast<int>(filtered() ? m_rows.size() : m_paths.size());
}

QVariant TrackListModel::data(const QModelIndex& index, int role) const
{
    const int at = index.isValid() ? listIndex(index.row()) : -1;
    if (at < 0 || at >= m_paths.size())
        return {};
    if (role == ListIndexRole)
        return at;
    const QString& path = m_paths[at];
    const LibraryModel* library = LibraryModel::instance();
    if (const sonkore::TrackMeta* t = library ? library->find(path) : nullptr)
        return role == MissingRole ? QVariant(false) : LibraryModel::roleData(*t, role);
    const External& e = external(path);
    return role == MissingRole ? QVariant(e.missing) : LibraryModel::roleData(e.meta, role);
}

QHash<int, QByteArray> TrackListModel::roleNames() const
{
    QHash<int, QByteArray> names = LibraryModel::trackRoleNames();
    names.insert(MissingRole, "missing");
    names.insert(ListIndexRole, "listIndex");
    return names;
}

void TrackListModel::setPaths(const QStringList& paths)
{
    if (paths == m_paths)
        return;
    beginResetModel();
    m_paths = paths;
    // the tags read for files no longer listed go
    if (!m_external.isEmpty()) {
        const QSet<QString> kept(m_paths.cbegin(), m_paths.cend());
        m_external.removeIf([&](const auto& it) { return !kept.contains(it.key()); });
    }
    refilter();
    endResetModel();
    emit pathsChanged();
    emit countChanged();
}

void TrackListModel::setSearch(const QString& text)
{
    if (text == m_search)
        return;
    beginResetModel();
    m_search = text;
    m_words = text.toLower().split(u' ', Qt::SkipEmptyParts);
    refilter();
    endResetModel();
    emit searchChanged();
    emit countChanged();
}

void TrackListModel::refilter()
{
    m_rows.clear();
    if (!filtered())
        return;
    const LibraryModel* library = LibraryModel::instance();
    for (int i = 0; i < m_paths.size(); ++i) {
        QString hay = library ? library->searchText(m_paths[i]) : QString();
        if (hay.isEmpty())
            hay = LibraryModel::searchText(external(m_paths[i]).meta);
        if (std::all_of(m_words.cbegin(), m_words.cend(), [&](const QString& w) { return hay.contains(w); }))
            m_rows.append(i);
    }
}

QString TrackListModel::pathAt(int row) const
{
    const int at = listIndex(row);
    return at >= 0 && at < m_paths.size() ? m_paths[at] : QString();
}

QStringList TrackListModel::pathsOf(const QList<int>& rows) const
{
    QStringList result;
    for (int row : rows) {
        const int at = row >= 0 ? listIndex(row) : -1;
        if (at >= 0 && at < m_paths.size())
            result << m_paths[at];
    }
    return result;
}

int TrackListModel::listIndexAt(int row) const
{
    return row >= 0 && row < rowCount() ? listIndex(row) : -1;
}

int TrackListModel::rowOfListIndex(int listIndex) const
{
    if (listIndex < 0 || listIndex >= m_paths.size())
        return -1;
    if (!filtered())
        return listIndex;
    const auto it = std::lower_bound(m_rows.cbegin(), m_rows.cend(), listIndex);
    return it != m_rows.cend() && *it == listIndex ? static_cast<int>(it - m_rows.cbegin()) : -1;
}

int TrackListModel::rowOfPath(const QString& path) const
{
    return rowOfListIndex(static_cast<int>(m_paths.indexOf(path)));
}

const TrackListModel::External& TrackListModel::external(const QString& path) const
{
    auto it = m_external.find(path);
    if (it == m_external.end()) {
        External e;
        e.missing = !QFileInfo::exists(path);
        if (e.missing || !sonkore::Metadata::read(path, e.meta)) {
            e.meta = sonkore::TrackMeta{};
            e.meta.path = path;
            e.meta.title = QFileInfo(path).completeBaseName();
        }
        it = m_external.insert(path, e);
    }
    return it.value();
}

// Tags were rescanned: refresh the visible rows without a reset, so the
// selection and scroll position survive.
void TrackListModel::libraryChanged()
{
    m_external.clear();
    if (filtered()) {
        // the tags a search matched may have changed
        beginResetModel();
        refilter();
        endResetModel();
        emit countChanged();
        return;
    }
    repaint();
}

void TrackListModel::repaint()
{
    if (rowCount() > 0)
        emit dataChanged(index(0), index(rowCount() - 1));
}
