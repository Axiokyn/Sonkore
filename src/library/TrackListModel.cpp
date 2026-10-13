#include "TrackListModel.hpp"

#include "LibraryModel.hpp"

#include <QFileInfo>
#include <QSet>

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
    return parent.isValid() ? 0 : static_cast<int>(m_paths.size());
}

QVariant TrackListModel::data(const QModelIndex& index, int role) const
{
    if (!index.isValid() || index.row() >= m_paths.size())
        return {};
    const QString& path = m_paths[index.row()];
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
    endResetModel();
    emit pathsChanged();
}

QString TrackListModel::pathAt(int row) const
{
    return row >= 0 && row < m_paths.size() ? m_paths[row] : QString();
}

QStringList TrackListModel::pathsOf(const QList<int>& rows) const
{
    QStringList result;
    for (int row : rows)
        if (row >= 0 && row < m_paths.size())
            result << m_paths[row];
    return result;
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
    repaint();
}

void TrackListModel::repaint()
{
    if (!m_paths.isEmpty())
        emit dataChanged(index(0), index(static_cast<int>(m_paths.size()) - 1));
}
