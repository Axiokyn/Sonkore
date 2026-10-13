#pragma once

#include <QAbstractItemModel>
#include <QString>
#include <QVariantList>
#include <QVariantMap>
#include <QtQml/qqmlregistration.h>

#include <memory>
#include <vector>

// Folder tree of the library (folder browser): one root per watched
// folder, children = sub-folders that contain music, with track counts.
class FolderTreeModel : public QAbstractItemModel {
    Q_OBJECT
    QML_ELEMENT

public:
    enum Roles { NameRole = Qt::UserRole + 1, PathRole, CountRole };

    explicit FolderTreeModel(QObject* parent = nullptr);
    ~FolderTreeModel() override;

    QModelIndex index(int row, int column, const QModelIndex& parent = {}) const override;
    QModelIndex parent(const QModelIndex& child) const override;
    int rowCount(const QModelIndex& parent = {}) const override;
    int columnCount(const QModelIndex& parent = {}) const override;
    QVariant data(const QModelIndex& index, int role) const override;
    QHash<int, QByteArray> roleNames() const override;

    // Sub-folders of `path` ("" = the watched roots) as {name, path, count},
    // for the folder page's own navigation.
    Q_INVOKABLE QVariantList childrenOf(const QString& path) const;
    // Index of the folder `path` (invalid when it is not in the tree).
    Q_INVOKABLE QModelIndex indexOfPath(const QString& path) const;

signals:
    void foldersChanged();

private:
    struct Node {
        QString name;
        QString path;
        int count = 0;
        Node* parent = nullptr;
        std::vector<std::unique_ptr<Node>> children;
        int row() const;
    };

    void rebuild();
    static QString signature(const Node& node);
    void refreshCounts(Node& fresh, Node& current, const QModelIndex& index);

    std::unique_ptr<Node> m_root;
};
