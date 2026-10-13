#!/usr/bin/env bash
# Cross-compiles the audio-only FFmpeg of packaging/ffmpeg/options.sh for
# Windows x64 with MinGW-w64, on Linux (CI: the ffmpeg-windows job). Sonkore
# (built with MSVC) opens the DLLs with LoadLibrary: plain C, no C runtime
# objects cross the boundary. The DLLs link nothing but Windows' own.
#   packaging/windows/build-ffmpeg.sh <output dir>
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
out="$(realpath -m "${1:?output dir}")"
source "$here/../ffmpeg/options.sh"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

curl -fL --retry 3 -o "$work/ffmpeg.tar.xz" "https://ffmpeg.org/releases/ffmpeg-$FFMPEG_VERSION.tar.xz"
echo "$FFMPEG_SHA256  $work/ffmpeg.tar.xz" | sha256sum -c -
mkdir -p "$work/src"
tar -xf "$work/ffmpeg.tar.xz" -C "$work/src" --strip-components=1
cd "$work/src"
./configure --prefix="$out" --arch=x86_64 --target-os=mingw32 --cross-prefix=x86_64-w64-mingw32- \
    --enable-w32threads --disable-pthreads \
    --extra-ldflags="-static-libgcc -Wl,-Bstatic -lwinpthread -Wl,-Bdynamic" \
    "${ffmpeg_options[@]}"
make -j"$(nproc)"
make install
cp COPYING.LGPLv2.1 "$out/COPYING.LGPLv2.1"
{
    echo "FFmpeg $FFMPEG_VERSION (libavformat, libavcodec, libavutil)  LGPL-2.1-or-later"
    echo "  Unmodified source: https://ffmpeg.org/releases/ffmpeg-$FFMPEG_VERSION.tar.xz (sha256 $FFMPEG_SHA256)"
    echo "  Built by packaging/windows/build-ffmpeg.sh (Sonkore sources) with:"
    echo "  ./configure --arch=x86_64 --target-os=mingw32 ${ffmpeg_options[*]}"
} > "$out/BUILD.txt"
# Nothing but system DLLs may be needed.
for dll in "$out"/bin/*.dll; do
    deps="$(x86_64-w64-mingw32-objdump -p "$dll" | sed -n 's/.*DLL Name: //p' | grep -viE '^(kernel32|user32|advapi32|bcrypt|ole32|shell32|msvcrt|api-ms-win-.*|ucrtbase|av[a-z]*-[0-9]+)\.dll$' || true)"
    [ -z "$deps" ] || { echo "$dll needs: $deps" >&2; exit 1; }
done
ls -la "$out/bin"
