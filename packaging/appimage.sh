#!/usr/bin/env bash
# Builds dist/Sonkore-x86_64.AppImage:
#   packaging/appimage.sh [--rapido] [runtime]
# --rapido: the "diario" profile (optimized, no fat LTO): seconds after a
# change instead of ~4 min, for AppImages to try out; publish with release.
# `runtime`: an AppImage type 2 runtime, or any type 2 AppImage to take it
# from (default: target/appimage-runtime-x86_64; get one from
# https://github.com/AppImage/type2-runtime/releases). The squashfs is
# written by tools/mkappimage (no appimagetool needed).
# The binary is built against this system's glibc: it runs on distributions
# with the same glibc or newer (the CI builds on AlmaLinux 9, glibc 2.34).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/.." && pwd)"
profile=release
if [ "${1:-}" = --rapido ]; then profile=diario; shift; fi
runtime="${1:-$root/target/appimage-runtime-x86_64}"
[ -f "$runtime" ] || { echo "no AppImage runtime at $runtime (see the comment at the top)" >&2; exit 1; }
work="${SONKORE_APPIMAGE_WORK:-$root/target/appimage}"
id=com.axiokyn.Sonkore

cargo build --profile "$profile" --manifest-path "$root/Cargo.toml" -p sonkore
CARGO_TARGET_DIR="$root/target" cargo build --release --manifest-path "$root/tools/mkappimage/Cargo.toml"

rm -rf "$work" && mkdir -p "$work"
app="$work/AppDir"
"$root/packaging/linux/install-files.sh" "$root/target/$profile/sonkore" "$app/usr"
install -Dm644 "$root/packaging/linux/$id.desktop" "$app/$id.desktop"
install -Dm644 "$root/packaging/icons/sonkore.svg" "$app/$id.svg"
install -Dm644 "$root/packaging/icons/png/256.png" "$app/.DirIcon"
cat > "$app/AppRun" <<'RUN'
#!/bin/sh
here="$(dirname "$(readlink -f "$0")")"
exec "$here/usr/bin/sonkore" "$@"
RUN
chmod 755 "$app/AppRun"

mkdir -p "$root/dist"
out="$root/dist/Sonkore-x86_64.AppImage"
"$root/target/release/mkappimage" "$app" "$runtime" "$out"
(cd "$root/dist" && sha256sum "$(basename "$out")" > "$(basename "$out").sha256")
echo "$out"
