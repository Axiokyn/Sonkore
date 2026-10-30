#include "PlaylistManager.hpp"
#include "AppPaths.hpp"
#include "DocumentPortal.hpp"

#include "LibraryModel.hpp"

#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QJsonDocument>
#include <QStandardPaths>
#include <QTextStream>

#include <algorithm>
#include <functional>

namespace {

QString storePath()
{
    return sonkore::paths::dataFile(QStringLiteral("playlists.json"));
}

QString localPath(const QUrl& url)
{
    return url.isLocalFile() ? url.toLocalFile() : url.toString();
}

} // namespace

PlaylistManager::PlaylistManager(QObject* parent)
    : QObject(parent)
{
    load();
}

QStringList PlaylistManager::names() const
{
    QStringList result;
    for (const Playlist& p : m_playlists)
        result << p.name;
    return result;
}

PlaylistManager::Playlist* PlaylistManager::find(const QString& name)
{
    for (Playlist& p : m_playlists)
        if (p.name == name)
            return &p;
    return nullptr;
}

const PlaylistManager::Playlist* PlaylistManager::find(const QString& name) const
{
    for (const Playlist& p : m_playlists)
        if (p.name == name)
            return &p;
    return nullptr;
}

QString PlaylistManager::uniqueName(const QString& wanted) const
{
    const QString base = wanted.trimmed().isEmpty() ? tr("New playlist") : wanted.trimmed();
    QString name = base;
    for (int n = 2; find(name); ++n)
        name = QStringLiteral("%1 %2").arg(base).arg(n);
    return name;
}

QString PlaylistManager::create(const QString& name)
{
    const QString unique = uniqueName(name);
    m_playlists.push_back({unique, {}});
    save();
    emit namesChanged();
    return unique;
}

void PlaylistManager::remove(const QString& name)
{
    for (int i = 0; i < m_playlists.size(); ++i) {
        if (m_playlists[i].name == name) {
            m_playlists.removeAt(i);
            save();
            emit namesChanged();
            return;
        }
    }
}

QString PlaylistManager::rename(const QString& from, const QString& to)
{
    Playlist* p = find(from);
    if (!p || to.trimmed().isEmpty() || to.trimmed() == from)
        return from;
    const QString unique = uniqueName(to);
    p->name = unique;
    save();
    emit namesChanged();
    return unique;
}

void PlaylistManager::addTracks(const QString& name, const QStringList& paths)
{
    Playlist* p = find(name);
    if (!p || paths.isEmpty())
        return;
    p->paths << paths;
    save();
    emit playlistChanged(name);
}

void PlaylistManager::removeAt(const QString& name, int index)
{
    Playlist* p = find(name);
    if (!p || index < 0 || index >= p->paths.size())
        return;
    p->paths.removeAt(index);
    save();
    emit playlistChanged(name);
}

void PlaylistManager::removeRows(const QString& name, const QList<int>& rows)
{
    Playlist* p = find(name);
    if (!p || rows.isEmpty())
        return;
    QList<int> sorted = rows;
    std::sort(sorted.begin(), sorted.end(), std::greater<int>());
    sorted.erase(std::unique(sorted.begin(), sorted.end()), sorted.end());
    for (int row : sorted)
        if (row >= 0 && row < p->paths.size())
            p->paths.removeAt(row);
    save();
    emit playlistChanged(name);
}

void PlaylistManager::insertTracks(const QString& name, int index, const QStringList& paths)
{
    Playlist* p = find(name);
    if (!p || paths.isEmpty())
        return;
    index = std::clamp(index, 0, static_cast<int>(p->paths.size()));
    for (int i = 0; i < paths.size(); ++i)
        p->paths.insert(index + i, paths[i]);
    save();
    emit playlistChanged(name);
}

void PlaylistManager::move(const QString& name, int from, int to)
{
    Playlist* p = find(name);
    if (!p || from < 0 || to < 0 || from >= p->paths.size() || to >= p->paths.size() || from == to)
        return;
    p->paths.move(from, to);
    save();
    emit playlistChanged(name);
}

QStringList PlaylistManager::paths(const QString& name) const
{
    const Playlist* p = find(name);
    return p ? p->paths : QStringList();
}

QVariantList PlaylistManager::tracks(const QString& name) const
{
    QVariantList result;
    const Playlist* p = find(name);
    if (!p)
        return result;
    for (const QString& path : p->paths) {
        QVariantMap m = LibraryModel::instance()->track(path);
        m.insert("missing", !QFileInfo::exists(path));
        result << m;
    }
    return result;
}

double PlaylistManager::duration(const QString& name) const
{
    double total = 0.0;
    const Playlist* p = find(name);
    if (!p)
        return total;
    for (const QString& path : p->paths)
        if (const auto* t = LibraryModel::instance()->find(path))
            total += t->duration;
    return total;
}

QString PlaylistManager::importM3u(const QUrl& file)
{
    // Its real folder: entries are often relative to it.
    const QString path = DocumentPortal::localPath(file);
    QFile f(path);
    if (!f.open(QIODevice::ReadOnly | QIODevice::Text))
        return {};
    const QDir base = QFileInfo(path).dir();
    QStringList entries;
    QTextStream in(&f);
    in.setEncoding(QStringConverter::Utf8);
    while (!in.atEnd()) {
        const QString line = in.readLine().trimmed();
        if (line.isEmpty() || line.startsWith('#'))
            continue;
        const QUrl asUrl(line);
        QString entry = asUrl.isLocalFile() ? asUrl.toLocalFile() : line;
        if (QFileInfo(entry).isRelative())
            entry = base.absoluteFilePath(entry);
        entries << QDir::cleanPath(entry);
    }
    const QString name = create(QFileInfo(path).completeBaseName());
    addTracks(name, entries);
    return name;
}

bool PlaylistManager::exportM3u(const QString& name, const QUrl& file) const
{
    const Playlist* p = find(name);
    if (!p)
        return false;
    QFile f(localPath(file));
    if (!f.open(QIODevice::WriteOnly | QIODevice::Truncate | QIODevice::Text))
        return false;
    QTextStream out(&f);
    out.setEncoding(QStringConverter::Utf8);
    out << "#EXTM3U\n";
    for (const QString& path : p->paths) {
        if (const auto* t = LibraryModel::instance()->find(path))
            out << "#EXTINF:" << static_cast<int>(t->duration) << ',' << t->displayArtist() << " - " << t->title << '\n';
        out << path << '\n';
    }
    return true;
}

void PlaylistManager::load()
{
    QFile f(storePath());
    if (!f.open(QIODevice::ReadOnly))
        return;
    bool moved = false;
    for (const QVariant& v : QJsonDocument::fromJson(f.readAll()).toVariant().toList()) {
        const QVariantMap m = v.toMap();
        const QStringList paths = m.value("tracks").toStringList();
        const QStringList real = DocumentPortal::hostPaths(paths);
        moved = moved || real != paths;
        m_playlists.push_back({m.value("name").toString(), real});
    }
    f.close();
    if (moved) // kept for when the portal's paths are gone
        save();
}

void PlaylistManager::save() const
{
    QVariantList list;
    for (const Playlist& p : m_playlists)
        list << QVariantMap{{"name", p.name}, {"tracks", p.paths}};
    QDir().mkpath(QFileInfo(storePath()).path());
    QFile f(storePath());
    if (f.open(QIODevice::WriteOnly | QIODevice::Truncate))
        f.write(QJsonDocument::fromVariant(list).toJson(QJsonDocument::Indented));
}
