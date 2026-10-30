#pragma once

#include <QString>
#include <QStringList>
#include <QUrl>

// In a Flatpak the file chooser may hand out a document portal path
// (/run/user/1000/doc/<id>/Music) instead of the real one. That grant can
// lapse (a new session, the portal restarting) and the folder then looks
// gone although its drive is there. Sonkore reads the whole system anyway
// (--filesystem=host:ro), so such paths are swapped for the real ones.
namespace DocumentPortal {

// The real path behind a document portal path, when Sonkore can read it
// there; any other path comes back as it is.
QString hostPath(const QString& path);

QStringList hostPaths(const QStringList& paths);

// A local file URL as a path, through hostPath(); other URLs as text.
QString localPath(const QUrl& url);

} // namespace DocumentPortal
