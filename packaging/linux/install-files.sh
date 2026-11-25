#!/usr/bin/env bash
# Installs Sonkore's files under a prefix, the way packages lay them out:
#   packaging/linux/install-files.sh BINARY PREFIX
# bin/sonkore, the desktop entry, the AppStream metainfo, the icons and the
# licenses (share/doc/sonkore). The AppImage (PREFIX = AppDir/usr) and the
# Flatpak (PREFIX = /app) both use it, so they carry the same files.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
binary="$1"
prefix="$2"
id=com.axiokyn.Sonkore

install -Dm755 "$binary" "$prefix/bin/sonkore"
install -Dm644 "$root/packaging/linux/$id.desktop" "$prefix/share/applications/$id.desktop"
install -Dm644 "$root/packaging/linux/$id.metainfo.xml" "$prefix/share/metainfo/$id.metainfo.xml"
install -Dm644 "$root/packaging/icons/sonkore.svg" "$prefix/share/icons/hicolor/scalable/apps/$id.svg"
install -Dm644 "$root/packaging/icons/sonkore-symbolic.svg" "$prefix/share/icons/hicolor/symbolic/apps/$id-symbolic.svg"
for size in 16 22 24 32 48 64 128 256 512; do
    install -Dm644 "$root/packaging/icons/png/$size.png" "$prefix/share/icons/hicolor/${size}x${size}/apps/$id.png"
done
docs="$prefix/share/doc/sonkore"
install -Dm644 "$root/LICENSE" "$docs/LICENSE"
install -Dm644 "$root/THIRD-PARTY.md" "$docs/THIRD-PARTY.md"
for lic in "$root"/licenses/*; do install -Dm644 "$lic" "$docs/licenses/$(basename "$lic")"; done
for font in "$root"/fonts/*/; do
    [ -f "$font/OFL.txt" ] && install -Dm644 "$font/OFL.txt" "$docs/fonts/$(basename "$font")/OFL.txt"
done
true
