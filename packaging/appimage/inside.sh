#!/usr/bin/env bash
# Runs inside the AlmaLinux 9 container started by build.sh:
#   /src  project (read-only)   /qt   Qt kit (read-only)
#   /work work folder (ext4)     /out  dist/
set -euo pipefail

source /src/packaging/ffmpeg/options.sh # FFMPEG_VERSION, FFMPEG_SHA256, ffmpeg_options

source /opt/rh/gcc-toolset-13/enable
export HOME=/work/home APPIMAGE_EXTRACT_AND_RUN=1
mkdir -p "$HOME" /work/tools
jobs="$(nproc)"
version="$(sed -n 's/^ *VERSION \([0-9.]*\)$/\1/p' /src/CMakeLists.txt | head -1)"
[ -n "$version" ] || { echo "project version not found in CMakeLists.txt" >&2; exit 1; }

fetch() { # fetch <file> <url>: downloads once into /work/tools
    [ -f "/work/tools/$1" ] && return
    curl -fL --retry 3 -o "/work/tools/$1.part" "$2"
    mv "/work/tools/$1.part" "/work/tools/$1"
}

# --- linuxdeploy + Qt and AppImage plugins --------------------------------------
gh=https://github.com/linuxdeploy
fetch linuxdeploy-x86_64.AppImage                 $gh/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage
fetch linuxdeploy-plugin-qt-x86_64.AppImage       $gh/linuxdeploy-plugin-qt/releases/download/continuous/linuxdeploy-plugin-qt-x86_64.AppImage
fetch linuxdeploy-plugin-appimage-x86_64.AppImage $gh/linuxdeploy-plugin-appimage/releases/download/continuous/linuxdeploy-plugin-appimage-x86_64.AppImage
chmod +x /work/tools/*.AppImage
export PATH="/work/tools:$PATH"

# --- FFmpeg: LGPL, shared, audio decoders and containers only -------------------
# (packaging/ffmpeg/options.sh; zlib for compressed ID3 frames and MP4 headers)
ffmpeg=/work/ffmpeg-$FFMPEG_VERSION
ffmpeg_options+=(--enable-zlib)
# Rebuilt whenever the version or the options change.
if [ "$(cat "$ffmpeg/.done" 2>/dev/null)" != "${ffmpeg_options[*]}" ]; then
    fetch "ffmpeg-$FFMPEG_VERSION.tar.xz" "https://ffmpeg.org/releases/ffmpeg-$FFMPEG_VERSION.tar.xz"
    echo "$FFMPEG_SHA256  /work/tools/ffmpeg-$FFMPEG_VERSION.tar.xz" | sha256sum -c -
    rm -rf "$ffmpeg" /work/ffmpeg-src && mkdir -p /work/ffmpeg-src
    tar -xf "/work/tools/ffmpeg-$FFMPEG_VERSION.tar.xz" -C /work/ffmpeg-src --strip-components=1
    (
        cd /work/ffmpeg-src
        ./configure --prefix="$ffmpeg" "${ffmpeg_options[@]}"
        make -j"$jobs" && make install
        cp COPYING.LGPLv2.1 "$ffmpeg/COPYING.LGPLv2.1"
    )
    echo "${ffmpeg_options[*]}" > "$ffmpeg/.done"
fi

# --- Sonkore ---------------------------------------------------------------------
PKG_CONFIG_PATH="$ffmpeg/lib/pkgconfig" cmake -S /src -B /work/build -G Ninja \
    -DCMAKE_BUILD_TYPE=Release -DCMAKE_PREFIX_PATH=/qt -DSONKORE_WITH_FFMPEG=ON
cmake --build /work/build

# --- AppDir ----------------------------------------------------------------------
appdir=/work/AppDir
rm -rf "$appdir" /work/icons
install -Dm755 /work/build/bin/Sonkore "$appdir/usr/bin/Sonkore"
install -Dm644 /src/packaging/linux/sonkore.metainfo.xml "$appdir/usr/share/metainfo/sonkore.appdata.xml"
for size in 16 22 24 32 48 64 128 256 512; do
    install -Dm644 "/src/packaging/icons/png/$size.png" "/work/icons/${size}x${size}/sonkore.png"
done
install -Dm644 /src/packaging/icons/sonkore-symbolic.svg "$appdir/usr/share/icons/hicolor/symbolic/apps/sonkore-symbolic.svg"

docs="$appdir/usr/share/doc/sonkore"
install -Dm644 /src/LICENSE "$docs/LICENSE"
install -Dm644 /src/THIRD-PARTY.md "$docs/THIRD-PARTY.md"
install -Dm644 /src/licenses/LGPL-2.1.txt "$docs/licenses/LGPL-2.1.txt"
install -Dm644 /src/licenses/LGPL-3.0.txt "$docs/licenses/LGPL-3.0.txt"
for font in /src/fonts/*/; do install -Dm644 "$font/OFL.txt" "$docs/fonts/$(basename "$font")/OFL.txt"; done
cat > "$docs/BUNDLED.txt" <<EOF
Sonkore $version AppImage - bundled libraries

Qt $(/qt/bin/qmake -query QT_VERSION)  LGPL-3.0 (licenses/LGPL-3.0.txt)
  Unmodified binaries from the Qt online installer. Source:
  https://download.qt.io/official_releases/qt/

FFmpeg $FFMPEG_VERSION (libavformat, libavcodec, libavutil)  LGPL-2.1-or-later (licenses/LGPL-2.1.txt)
  Unmodified source: https://ffmpeg.org/releases/ffmpeg-$FFMPEG_VERSION.tar.xz
  (sha256 $FFMPEG_SHA256)
  Built by packaging/appimage/inside.sh (Sonkore sources) with:
  ./configure ${ffmpeg_options[*]}

Other libraries in usr/lib come unmodified from AlmaLinux 9 packages (source:
https://vault.almalinux.org/); licenses/system/ lists each one with its
package and holds that package's license files (OpenSSL: Apache-2.0).

The libraries can be replaced: extract the AppImage (--appimage-extract) and
swap the files in usr/lib for compatible builds.
EOF

# Extra Qt plugins the Qt plugin of linuxdeploy does not pick on its own:
# native file dialogs through the desktop portal, Wayland shell/decorations.
install -Dm755 /qt/plugins/platformthemes/libqxdgdesktopportal.so "$appdir/usr/plugins/platformthemes/libqxdgdesktopportal.so"
for dir in wayland-shell-integration wayland-decoration-client wayland-graphics-integration-client; do
    cp -r "/qt/plugins/$dir" "$appdir/usr/plugins/$dir"
done

# Libraries opened with dlopen (no linker dependency to follow), copied under the
# soname they are opened by: FFmpeg (by Sonkore) and OpenSSL (by Qt's TLS
# plugin). libssl and libcrypto travel as a pair so a newer system libssl is
# never mixed with the bundled libcrypto that Kerberos (Qt Network) needs.
dlopened=()
for lib in "$ffmpeg"/lib/lib{avutil,avcodec,avformat}.so /usr/lib64/libssl.so.3 /usr/lib64/libcrypto.so.3; do
    soname="$(LC_ALL=C readelf -d "$lib" | sed -n 's/.*(SONAME).*\[\(.*\)\]/\1/p')"
    install -Dm755 "$(realpath "$lib")" "$appdir/usr/lib/$soname"
    dlopened+=(--deploy-deps-only "$appdir/usr/lib/$soname")
done

# Present on every desktop: taken from the system, which also keeps systemd
# (needed by Alma's D-Bus) out of the bundle for distros without it.
excluded=()
for lib in libdbus-1.so.3 libglib-2.0.so.0 libgobject-2.0.so.0 libgio-2.0.so.0 libgmodule-2.0.so.0; do
    excluded+=(--exclude-library "$lib")
done
export LINUXDEPLOY_EXCLUDED_LIBRARIES="libdbus-1.so.3;libglib-2.0.so.0;libgobject-2.0.so.0;libgio-2.0.so.0;libgmodule-2.0.so.0"

export QMAKE=/qt/bin/qmake QML_SOURCES_PATHS=/src/qml EXTRA_PLATFORM_PLUGINS="libqwayland.so"
export LD_LIBRARY_PATH="/qt/lib:$ffmpeg/lib"
export LINUXDEPLOY_OUTPUT_VERSION="$version" LDAI_OUTPUT="/out/Sonkore-$version-x86_64.AppImage"
rm -f "$LDAI_OUTPUT"
icons=()
for f in /work/icons/*/sonkore.png; do icons+=(--icon-file "$f"); done
linuxdeploy-x86_64.AppImage --appdir "$appdir" \
    --executable "$appdir/usr/bin/Sonkore" \
    --desktop-file /src/packaging/linux/sonkore.desktop \
    --icon-file /src/packaging/icons/sonkore.svg "${icons[@]}" \
    "${dlopened[@]}" "${excluded[@]}" \
    --deploy-deps-only "$appdir/usr/plugins/platformthemes" \
    --deploy-deps-only "$appdir/usr/plugins/wayland-shell-integration" \
    --deploy-deps-only "$appdir/usr/plugins/wayland-decoration-client" \
    --deploy-deps-only "$appdir/usr/plugins/wayland-graphics-integration-client" \
    --plugin qt

# Image formats: covers are JPEG, PNG, WebP, GIF or BMP (PNG and BMP are built
# into Qt) and the icons SVG. Qt Quick instantiates every image plugin it finds
# when the first picture loads, so the others (TIFF with libtiff, libLerc and
# libdeflate; ICNS, ICO, TGA, WBMP) stay out of the bundle and out of memory;
# the loop below then drops the libraries only they used.
for f in "$appdir"/usr/plugins/imageformats/*.so; do
    case "$(basename "$f")" in libqsvg.so|libqjpeg.so|libqwebp.so|libqgif.so) ;; *) rm -f "$f" ;; esac
done

# Small, ABI-stable libraries that minimal installs can lack (openSUSE splits
# them into packages of their own) although linuxdeploy counts on them:
# libgthread (a stub over the system's GLib, linked by Qt) is bundled above;
# com_err, needed by the bundled Kerberos of Qt Network, is added here.
install -Dm755 "$(realpath /usr/lib64/libcom_err.so.2)" "$appdir/usr/lib/libcom_err.so.2"

# Drop libraries nothing in the AppDir links to (left behind by plugins whose
# own dependency was taken from the system), except the dlopen()ed ones.
keep=" libavutil.so.60 libavcodec.so.62 libavformat.so.62 libssl.so.3 libcrypto.so.3 "
while :; do
    needed=" $(find "$appdir/usr" -type f \( -name '*.so*' -o -path '*/bin/*' \) -exec sh -c 'LC_ALL=C readelf -d "$@" 2>/dev/null; true' sh {} + \
        | sed -n 's/.*Shared library: \[\(.*\)\]/\1/p' | sort -u | tr '\n' ' ') "
    orphans=()
    for lib in "$appdir"/usr/lib/*.so*; do
        name="$(basename "$lib")"
        [[ "$needed$keep" == *" $name "* ]] || orphans+=("$lib")
    done
    [ ${#orphans[@]} -eq 0 ] && break
    echo "Removing unused: ${orphans[*]##*/}"
    rm -f "${orphans[@]}"
done

# License files of the system libraries that ended up in the bundle (OpenSSL,
# ICU, xcb-util, Kerberos...), taken from the packages they came from.
mkdir -p "$docs/licenses/system"
for lib in "$appdir"/usr/lib/*.so*; do
    pkg="$(rpm -qf --qf '%{NAME}\n' "/usr/lib64/$(basename "$lib")" 2>/dev/null)" || continue
    for file in $(rpm -qL "$pkg"); do
        install -Dm644 "$file" "$docs/licenses/system/$pkg/$(basename "$file")"
    done
    echo "$(basename "$lib")  $(rpm -q "$pkg")" >> "$docs/licenses/system/LIBRARIES.txt"
done
sort -o "$docs/licenses/system/LIBRARIES.txt" "$docs/licenses/system/LIBRARIES.txt"

# The AppImage's own icon, read by file managers and thumbnailers: a real
# 256 px PNG (linuxdeploy leaves a link to the SVG, which most do not read).
rm -f "$appdir/.DirIcon"
install -m644 /src/packaging/icons/png/256.png "$appdir/.DirIcon"

linuxdeploy-x86_64.AppImage --appdir "$appdir" --output appimage

( cd /out && sha256sum "$(basename "$LDAI_OUTPUT")" > "$(basename "$LDAI_OUTPUT").sha256" )
echo "AppImage: dist/$(basename "$LDAI_OUTPUT")"
