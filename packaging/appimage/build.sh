#!/usr/bin/env bash
# Builds dist/Sonkore-<version>-x86_64.AppImage: Qt, the QML modules and an
# audio-only FFmpeg inside, so it runs on any x86_64 distro with glibc 2.34+.
# Everything is compiled in an AlmaLinux 9 container (podman), never against
# the host's newer glibc. Usage:
#   packaging/appimage/build.sh [path/to/Qt/6.x/gcc_64]
# Intermediate files go to ~/.cache/sonkore-appimage (reused by the next run;
# SONKORE_APPIMAGE_WORK overrides it). It must be a Linux filesystem: the
# build sets file permissions, which NTFS/exFAT drives do not keep.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
qt="$(realpath "${1:-$HOME/Qt/6.11.3/gcc_64}")"
image="sonkore-appimage-builder"
work="${SONKORE_APPIMAGE_WORK:-${XDG_CACHE_HOME:-$HOME/.cache}/sonkore-appimage}"

[ -x "$qt/bin/qmake" ] || { echo "Qt not found (no bin/qmake): $qt" >&2; exit 1; }
command -v podman >/dev/null || { echo "podman is required" >&2; exit 1; }

podman build -t "$image" -f "$here/Containerfile" "$here"

mkdir -p "$work" "$root/dist"
# keep-id: the files created in the work folder and dist/ belong to the user.
# label=disable: no SELinux relabelling of the project or Qt folders.
podman run --rm --userns=keep-id --security-opt label=disable \
    -v "$root:/src:ro" \
    -v "$qt:/qt:ro" \
    -v "$work:/work" \
    -v "$root/dist:/out" \
    "$image" bash /src/packaging/appimage/inside.sh
