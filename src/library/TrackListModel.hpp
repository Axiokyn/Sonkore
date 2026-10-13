#pragma once

#include "Metadata.hpp"

#include <QAbstractListModel>
#include <QHash>
#include <QStringList>
#include <QtQml/qqmlregistration.h>

// An ordered list of track paths (the play queue, a playlist) exposed with
// the library's roles. Rows only hold the path and the tags are looked up in
// LibraryModel when a row is shown, so thousands of rows cost a few hundred
// KB instead of one JavaScript object per track.
class TrackListModel : public QAbstractListModel {
    Q_OBJECT
    QML_ELEMENT

    Q_PROPERTY(QStringList paths READ paths WRITE setPaths NOTIFY pathsChanged)
    Q_PROPERTY(int count READ count NOTIFY pathsChanged)

public:
    enum { MissingRole = Qt::UserRole + 100 };

    explicit TrackListModel(QObject* parent = nullptr);

    int rowCount(const QModelIndex& parent = {}) const override;
    QVariant data(const QModelIndex& index, int role) const override;
    QHash<int, QByteArray> roleNames() const override;

    QStringList paths() const { return m_paths; }
    void setPaths(const QStringList& paths);
    int count() const { return static_cast<int>(m_paths.size()); }

    Q_INVOKABLE QString pathAt(int row) const;
    Q_INVOKABLE QStringList pathsOf(const QList<int>& rows) const;

signals:
    void pathsChanged();

private:
    struct External {
        sonkore::TrackMeta meta;
        bool missing = false;
    };
    // Tags of a file that is not in the library (read once, then cached).
    const External& external(const QString& path) const;
    void libraryChanged();
    void repaint();

    QStringList m_paths;
    mutable QHash<QString, External> m_external;
};
