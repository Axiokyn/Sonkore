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
// With `search` set only the matching tracks are rows (every word in the
// title, artists, album, genre or codec); `listIndex` is a row's place in
// the whole list.
class TrackListModel : public QAbstractListModel {
    Q_OBJECT
    QML_ELEMENT

    Q_PROPERTY(QStringList paths READ paths WRITE setPaths NOTIFY pathsChanged)
    Q_PROPERTY(QString search READ search WRITE setSearch NOTIFY searchChanged)
    Q_PROPERTY(int count READ count NOTIFY countChanged)

public:
    enum { MissingRole = Qt::UserRole + 100, ListIndexRole };

    explicit TrackListModel(QObject* parent = nullptr);

    int rowCount(const QModelIndex& parent = {}) const override;
    QVariant data(const QModelIndex& index, int role) const override;
    QHash<int, QByteArray> roleNames() const override;

    QStringList paths() const { return m_paths; }
    void setPaths(const QStringList& paths);
    QString search() const { return m_search; }
    void setSearch(const QString& text);
    int count() const { return rowCount(); }

    Q_INVOKABLE QString pathAt(int row) const;
    Q_INVOKABLE QStringList pathsOf(const QList<int>& rows) const;
    // A row's place in the whole list, and back (-1: filtered out).
    Q_INVOKABLE int listIndexAt(int row) const;
    Q_INVOKABLE int rowOfListIndex(int listIndex) const;
    Q_INVOKABLE int rowOfPath(const QString& path) const;

signals:
    void pathsChanged();
    void searchChanged();
    void countChanged();

private:
    struct External {
        sonkore::TrackMeta meta;
        bool missing = false;
    };
    // Tags of a file that is not in the library (read once, then cached).
    const External& external(const QString& path) const;
    void libraryChanged();
    void repaint();
    bool filtered() const { return !m_words.isEmpty(); }
    int listIndex(int row) const { return filtered() ? m_rows.value(row, -1) : row; }
    // The rows that match the search (after the list or the tags changed).
    void refilter();

    QStringList m_paths;
    QString m_search;
    QStringList m_words;
    QList<int> m_rows; // list indices of the matching tracks, while searching
    mutable QHash<QString, External> m_external;
};
