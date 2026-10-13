#!/usr/bin/env bash
# Registers a locally built Sonkore in the user's desktop (menu entry +
# icon), so GNOME/KDE show the right icon in the dock/taskbar and the media
# controls can find the app. Usage: packaging/linux/install-user.sh [path/to/Sonkore]
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
binary="$(realpath "${1:-$root/build/bin/Sonkore}")"
data="${XDG_DATA_HOME:-$HOME/.local/share}"

[ -x "$binary" ] || { echo "Sonkore binary not found: $binary" >&2; exit 1; }

mkdir -p "$data/applications" "$data/icons/hicolor/scalable/apps" "$data/icons/hicolor/symbolic/apps"
for size in 16 22 24 32 48 64 128 256 512; do
    mkdir -p "$data/icons/hicolor/${size}x${size}/apps"
    install -m 644 "$root/packaging/icons/png/$size.png" "$data/icons/hicolor/${size}x${size}/apps/sonkore.png"
done
install -m 644 "$root/packaging/icons/sonkore.svg" "$data/icons/hicolor/scalable/apps/sonkore.svg"
install -m 644 "$root/packaging/icons/sonkore-symbolic.svg" "$data/icons/hicolor/symbolic/apps/sonkore-symbolic.svg"

# Exec/TryExec point at the build; the path is quoted because it may contain spaces.
sed -e "s|^Exec=Sonkore|Exec=\"$binary\"|" -e "s|^TryExec=Sonkore|TryExec=$binary|" \
    "$root/packaging/linux/sonkore.desktop" > "$data/applications/sonkore.desktop"
chmod 644 "$data/applications/sonkore.desktop"

command -v update-desktop-database >/dev/null && update-desktop-database "$data/applications" || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$data/icons/hicolor" || true
echo "Installed: $data/applications/sonkore.desktop -> $binary"
