#!/usr/bin/env bash
# Builds dist/Sonkore-x86_64.flatpak (the same name for every version, like
# the AppImage), a single-file bundle that installs with
#   flatpak install --user Sonkore-x86_64.flatpak
# and fetches the KDE runtime from Flathub. Needs flatpak and flatpak-builder
# (or the org.flatpak.Builder app). Intermediate files go to
# ~/.cache/sonkore-flatpak (reused by the next run; SONKORE_FLATPAK_WORK
# overrides it).
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
work="${SONKORE_FLATPAK_WORK:-${XDG_CACHE_HOME:-$HOME/.cache}/sonkore-flatpak}"
flathub=https://dl.flathub.org/repo/flathub.flatpakrepo

if command -v flatpak-builder >/dev/null; then
    builder=(flatpak-builder)
elif flatpak info org.flatpak.Builder >/dev/null 2>&1; then
    builder=(flatpak run --command=flatpak-builder org.flatpak.Builder)
else
    echo "flatpak-builder is required (a package, or: flatpak install flathub org.flatpak.Builder)" >&2
    exit 1
fi

mkdir -p "$work" "$root/dist"
flatpak remote-add --user --if-not-exists flathub "$flathub"
"${builder[@]}" --user --install-deps-from=flathub --force-clean --disable-rofiles-fuse \
    --state-dir="$work/state" --repo="$work/repo" "$work/build" "$here/com.axiokyn.Sonkore.yml"
flatpak build-bundle "$work/repo" "$root/dist/Sonkore-x86_64.flatpak" com.axiokyn.Sonkore --runtime-repo="$flathub"
( cd "$root/dist" && sha256sum Sonkore-x86_64.flatpak > Sonkore-x86_64.flatpak.sha256 )
echo "Flatpak: dist/Sonkore-x86_64.flatpak"
