#pragma once

#include "QmlSingleton.hpp"

#include <QObject>
#include <QStringList>
#include <QUrl>
#include <QVariantList>
#include <QVector>
#include <QtQml/qqmlregistration.h>

// User playlists (ordered lists of file paths), persisted as JSON, with
// M3U/M3U8 import and export.
class PlaylistManager : public QObject {
    Q_OBJECT
    QML_NAMED_ELEMENT(Playlists)
    QML_SINGLETON
    SONKORE_QML_SINGLETON_INSTANCE(PlaylistManager)

    Q_PROPERTY(QStringList names READ names NOTIFY namesChanged)

public:
    // No default argument on purpose: a default-constructible QML_SINGLETON
    // would make QML build its own instance instead of calling create().
    explicit PlaylistManager(QObject* parent);

    QStringList names() const;

    Q_INVOKABLE QString create(const QString& name);
    Q_INVOKABLE void remove(const QString& name);
    Q_INVOKABLE QString rename(const QString& from, const QString& to);

    Q_INVOKABLE void addTracks(const QString& name, const QStringList& paths);
    Q_INVOKABLE void removeAt(const QString& name, int index);
    Q_INVOKABLE void removeRows(const QString& name, const QList<int>& rows);
    Q_INVOKABLE void insertTracks(const QString& name, int index, const QStringList& paths);
    Q_INVOKABLE void move(const QString& name, int from, int to);

    Q_INVOKABLE QStringList paths(const QString& name) const;
    Q_INVOKABLE QVariantList tracks(const QString& name) const;
    Q_INVOKABLE double duration(const QString& name) const;

    Q_INVOKABLE QString importM3u(const QUrl& file);
    Q_INVOKABLE bool exportM3u(const QString& name, const QUrl& file) const;

signals:
    void namesChanged();
    void playlistChanged(const QString& name);

private:
    struct Playlist {
        QString name;
        QStringList paths;
    };
    Playlist* find(const QString& name);
    const Playlist* find(const QString& name) const;
    QString uniqueName(const QString& wanted) const;
    void load();
    void save() const;

    QVector<Playlist> m_playlists;
};
