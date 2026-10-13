#include "FolderTreeModel.hpp"

#include "LibraryModel.hpp"

#include <QDir>

#include <algorithm>
#include <functional>

FolderTreeModel::FolderTreeModel(QObject* parent)
    : QAbstractItemModel(parent)
    , m_root(std::make_unique<Node>())
{
    LibraryModel* lib = LibraryModel::instance();
    connect(lib, &QAbstractItemModel::modelReset, this, &FolderTreeModel::rebuild);
    connect(lib, &LibraryModel::foldersChanged, this, &FolderTreeModel::rebuild);
    rebuild();
}

FolderTreeModel::~FolderTreeModel() = default;

int FolderTreeModel::Node::row() const
{
    if (!parent)
        return 0;
    for (std::size_t i = 0; i < parent->children.size(); ++i)
        if (parent->children[i].get() == this)
            return static_cast<int>(i);
    return 0;
}

QModelIndex FolderTreeModel::index(int row, int column, const QModelIndex& parent) const
{
    const Node* p = parent.isValid() ? static_cast<Node*>(parent.internalPointer()) : m_root.get();
    if (column != 0 || row < 0 || row >= static_cast<int>(p->children.size()))
        return {};
    return createIndex(row, 0, p->children[static_cast<std::size_t>(row)].get());
}

QModelIndex FolderTreeModel::indexOfPath(const QString& path) const
{
    // Down from the roots, through the child that contains `path`.
    const Node* node = m_root.get();
    while (node) {
        const Node* next = nullptr;
        for (const auto& child : node->children) {
            if (child->path == path)
                return createIndex(child->row(), 0, child.get());
            if (path.startsWith(child->path + u'/')) {
                next = child.get();
                break;
            }
        }
        node = next;
    }
    return {};
}

QModelIndex FolderTreeModel::parent(const QModelIndex& child) const
{
    if (!child.isValid())
        return {};
    const Node* n = static_cast<Node*>(child.internalPointer());
    Node* p = n->parent;
    if (!p || p == m_root.get())
        return {};
    return createIndex(p->row(), 0, p);
}

int FolderTreeModel::rowCount(const QModelIndex& parent) const
{
    if (parent.column() > 0)
        return 0;
    const Node* p = parent.isValid() ? static_cast<Node*>(parent.internalPointer()) : m_root.get();
    return static_cast<int>(p->children.size());
}

int FolderTreeModel::columnCount(const QModelIndex&) const
{
    return 1;
}

QVariant FolderTreeModel::data(const QModelIndex& index, int role) const
{
    if (!index.isValid())
        return {};
    const Node* n = static_cast<Node*>(index.internalPointer());
    switch (role) {
    case Qt::DisplayRole:
    case NameRole: return n->name;
    case PathRole: return n->path;
    case CountRole: return n->count;
    default: return {};
    }
}

QHash<int, QByteArray> FolderTreeModel::roleNames() const
{
    return {{Qt::DisplayRole, "display"}, {NameRole, "name"}, {PathRole, "path"}, {CountRole, "count"}};
}

QString FolderTreeModel::signature(const Node& node)
{
    QString sig = node.path + '\n';
    for (const auto& c : node.children)
        sig += signature(*c);
    return sig;
}

void FolderTreeModel::refreshCounts(Node& fresh, Node& current, const QModelIndex& index)
{
    if (fresh.count != current.count) {
        current.count = fresh.count;
        if (index.isValid())
            emit dataChanged(index, index, {CountRole});
    }
    for (std::size_t i = 0; i < fresh.children.size(); ++i)
        refreshCounts(*fresh.children[i], *current.children[i], this->index(static_cast<int>(i), 0, index));
}

void FolderTreeModel::rebuild()
{
    LibraryModel* lib = LibraryModel::instance();
    auto root = std::make_unique<Node>();

    QStringList roots = lib->folders();
    std::sort(roots.begin(), roots.end(), [](const QString& a, const QString& b) { return a.length() > b.length(); });
    for (const QString& folder : lib->folders()) {
        auto n = std::make_unique<Node>();
        n->name = QDir(folder).dirName().isEmpty() ? folder : QDir(folder).dirName();
        n->path = folder;
        n->parent = root.get();
        root->children.push_back(std::move(n));
    }

    for (const sonkore::TrackMeta& t : lib->tracks()) {
        // Deepest watched folder containing the track.
        Node* node = nullptr;
        QString relative;
        for (const QString& r : roots) {
            if (t.path.startsWith(r + '/')) {
                for (auto& c : root->children)
                    if (c->path == r)
                        node = c.get();
                relative = t.path.mid(r.length() + 1);
                break;
            }
        }
        if (!node)
            continue; // individually added file outside watched folders
        node->count++;
        const QStringList parts = relative.split('/', Qt::SkipEmptyParts);
        for (int i = 0; i + 1 < parts.size(); ++i) { // last part is the file
            Node* next = nullptr;
            for (auto& c : node->children)
                if (c->name == parts[i])
                    next = c.get();
            if (!next) {
                auto n = std::make_unique<Node>();
                n->name = parts[i];
                n->path = node->path + '/' + parts[i];
                n->parent = node;
                next = n.get();
                node->children.push_back(std::move(n));
            }
            next->count++;
            node = next;
        }
    }

    std::function<void(Node&)> sortTree = [&](Node& n) {
        std::sort(n.children.begin(), n.children.end(), [](const auto& a, const auto& b) {
            return QString::compare(a->name, b->name, Qt::CaseInsensitive) < 0;
        });
        for (auto& c : n.children)
            sortTree(*c);
    };
    sortTree(*root);

    // Same folders as before (typical after a rescan): only update counts so
    // the view keeps its expanded/selected state.
    if (signature(*root) == signature(*m_root)) {
        refreshCounts(*root, *m_root, {});
        emit foldersChanged();
        return;
    }
    beginResetModel();
    m_root = std::move(root);
    endResetModel();
    emit foldersChanged();
}

QVariantList FolderTreeModel::childrenOf(const QString& path) const
{
    std::function<const Node*(const Node&)> find = [&](const Node& n) -> const Node* {
        if (n.path == path)
            return &n;
        for (const auto& c : n.children)
            if (path.startsWith(c->path))
                if (const Node* hit = find(*c))
                    return hit;
        return nullptr;
    };
    const Node* node = path.isEmpty() ? m_root.get() : find(*m_root);
    QVariantList out;
    if (node)
        for (const auto& c : node->children)
            out << QVariantMap{{"name", c->name}, {"path", c->path}, {"count", c->count}};
    return out;
}
