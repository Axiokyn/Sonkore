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
install -Dm755 "$root/target/$profile/sonkore" "$app/usr/bin/sonkore"
install -Dm644 "$root/packaging/linux/$id.desktop" "$app/usr/share/applications/$id.desktop"
install -Dm644 "$root/packaging/linux/$id.desktop" "$app/$id.desktop"
install -Dm644 "$root/packaging/linux/$id.metainfo.xml" "$app/usr/share/metainfo/$id.metainfo.xml"
install -Dm644 "$root/packaging/icons/sonkore.svg" "$app/usr/share/icons/hicolor/scalable/apps/$id.svg"
install -Dm644 "$root/packaging/icons/sonkore-symbolic.svg" "$app/usr/share/icons/hicolor/symbolic/apps/$id-symbolic.svg"
for size in 16 22 24 32 48 64 128 256 512; do
    install -Dm644 "$root/packaging/icons/png/$size.png" "$app/usr/share/icons/hicolor/${size}x${size}/apps/$id.png"
done
install -Dm644 "$root/packaging/icons/sonkore.svg" "$app/$id.svg"
install -Dm644 "$root/packaging/icons/png/256.png" "$app/.DirIcon"
docs="$app/usr/share/doc/sonkore"
install -Dm644 "$root/LICENSE" "$docs/LICENSE"
install -Dm644 "$root/THIRD-PARTY.md" "$docs/THIRD-PARTY.md"
for lic in "$root"/licenses/*; do install -Dm644 "$lic" "$docs/licenses/$(basename "$lic")"; done
for font in "$root"/fonts/*/; do
    [ -f "$font/OFL.txt" ] && install -Dm644 "$font/OFL.txt" "$docs/fonts/$(basename "$font")/OFL.txt"
done
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
