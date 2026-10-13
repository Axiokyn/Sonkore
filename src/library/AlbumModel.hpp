#pragma once

#include <QAbstractListModel>
#include <QVariantMap>
#include <QVector>
#include <QtQml/qqmlregistration.h>

// Albums derived from the Library singleton (grouped by album artist + album).
class AlbumModel : public QAbstractListModel {
    Q_OBJECT
    QML_ELEMENT

    Q_PROPERTY(QString search READ search WRITE setSearch NOTIFY searchChanged)
    Q_PROPERTY(int count READ count NOTIFY countChanged)

public:
    enum Roles {
        AlbumKeyRole = Qt::UserRole + 1,
        AlbumRole,
        ArtistRole,
        YearRole,
        CoverPathRole,
        TrackCountRole,
        DurationRole,
        FormatRole,
    };

    explicit AlbumModel(QObject* parent = nullptr);

    int rowCount(const QModelIndex& parent = {}) const override;
    QVariant data(const QModelIndex& index, int role) const override;
    QHash<int, QByteArray> roleNames() const override;

    QString search() const { return m_search; }
    void setSearch(const QString& text);
    int count() const { return static_cast<int>(m_visible.size()); }

    Q_INVOKABLE QVariantMap get(int row) const;
    Q_INVOKABLE QVariantMap byKey(const QString& albumKey) const;

signals:
    void searchChanged();
    void countChanged();

private:
    struct Album {
        QString key, album, artist, coverPath, format, searchText;
        int year = 0;
        int tracks = 0;
        double duration = 0.0;
        bool hasEmbeddedCover = false;
    };
    void rebuild();
    void refilter();
    static QVariantMap toMap(const Album& a);

    QVector<Album> m_all;
    QVector<int> m_visible;
    QString m_search;
};
