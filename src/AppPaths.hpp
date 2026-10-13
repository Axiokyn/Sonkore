#pragma once

#include <QStandardPaths>
#include <QString>

// Where Sonkore keeps its files: one "sonkore" folder per kind, directly in
// the system's locations (no company folder):
//   settings  ~/.config/sonkore/sonkore.conf   (QSettings, see main.cpp)
//   data      ~/.local/share/sonkore             (library, playlists, session, EQ profiles)
//   cache     ~/.cache/sonkore                   (GPU pipelines, MPRIS artwork)
// On Windows the same names live under %APPDATA% / %LOCALAPPDATA%.
namespace sonkore::paths {

inline constexpr char kAppId[] = "sonkore";

inline QString dataDir()
{
    return QStandardPaths::writableLocation(QStandardPaths::GenericDataLocation) + QLatin1Char('/') + QLatin1String(kAppId);
}

inline QString cacheDir()
{
    return QStandardPaths::writableLocation(QStandardPaths::GenericCacheLocation) + QLatin1Char('/') + QLatin1String(kAppId);
}

inline QString dataFile(const QString& name)
{
    return dataDir() + QLatin1Char('/') + name;
}

} // namespace sonkore::paths
