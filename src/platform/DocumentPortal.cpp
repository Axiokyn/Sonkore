#include "DocumentPortal.hpp"

#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QHash>
#include <QMutex>
#include <QRegularExpression>

#if defined(SONKORE_HAVE_DBUS)
#include <QDBusArgument>
#include <QDBusConnection>
#include <QDBusMessage>
#include <QMap>
#endif

namespace DocumentPortal {

namespace {

// The real path of a document, asked once per document (a library holds
// thousands of tracks under the same one). Empty when unknown.
QString documentHost(const QString& id)
{
    static QMutex mutex;
    static QHash<QString, QString> known;
    const QMutexLocker lock(&mutex);
    const auto it = known.constFind(id);
    if (it != known.cend())
        return it.value();
    QString host;
#if defined(SONKORE_HAVE_DBUS)
    QDBusMessage call = QDBusMessage::createMethodCall(
        QStringLiteral("org.freedesktop.portal.Documents"), QStringLiteral("/org/freedesktop/portal/documents"),
        QStringLiteral("org.freedesktop.portal.Documents"), QStringLiteral("GetHostPaths"));
    call << QStringList{id};
    const QDBusMessage reply = QDBusConnection::sessionBus().call(call, QDBus::Block, 2000);
    if (reply.type() == QDBusMessage::ReplyMessage && !reply.arguments().isEmpty()) {
        QMap<QString, QByteArray> paths;
        reply.arguments().constFirst().value<QDBusArgument>() >> paths;
        QByteArray bytes = paths.value(id);
        if (bytes.endsWith('\0'))
            bytes.chop(1);
        host = QFile::decodeName(bytes);
    }
#endif
    known.insert(id, host);
    return host;
}

} // namespace

QString hostPath(const QString& path)
{
    if (!path.startsWith(QLatin1String("/run/")))
        return path;
    // <root>/<id>/<name>[/inside]: <name> is the shared file itself, or the
    // shared folder with what was reached through it.
    static const QRegularExpression doc(QStringLiteral("^/run/(?:user/\\d+|flatpak)/doc/([^/]+)/[^/]+(/.*)?$"));
    const QRegularExpressionMatch m = doc.match(path);
    if (!m.hasMatch())
        return path;
    const QString host = documentHost(m.captured(1));
    // The shared file or folder must be visible from here (a CD image track,
    // "sheet.cue#3", is no file of its own).
    if (host.isEmpty() || !QFileInfo::exists(host))
        return path;
    return QDir::cleanPath(host + m.captured(2));
}

QStringList hostPaths(const QStringList& paths)
{
    QStringList result;
    result.reserve(paths.size());
    for (const QString& path : paths)
        result << hostPath(path);
    return result;
}

QString localPath(const QUrl& url)
{
    return url.isLocalFile() ? hostPath(url.toLocalFile()) : url.toString();
}

} // namespace DocumentPortal
