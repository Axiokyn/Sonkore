#!/usr/bin/env bash
# Sonkore's FFmpeg tests against FFmpeg 6.1, 7.0 and 7.1 (the system has one,
# often 8; 7.1 is the Flatpak runtime's): PyAV's wheels from PyPI carry each
# release's libraries. FFmpeg 5 is not on PyPI any more (PyAV 10 and 11 are
# gone): only its headers were checked (tools/ffmpeg-offsets.sh).
#   tools/ffmpeg-versiones.sh [cargo test arguments…]   (default: -p sonkore-ffmpeg)
# The libraries go to target/ffmpeg-versiones/<PyAV version>/ (downloaded once) and
# are only loaded into the test processes (SONKORE_FFMPEG_MAJOR picks one).
set -uo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
work="$root/target/ffmpeg-versiones"
corpus="$root/target/test-corpus"
[ -d "$corpus/ffmpeg" ] || "$root/tests/make_corpus.sh" "$corpus" >/dev/null
args=("$@")
[ ${#args[@]} = 0 ] && args=(-p sonkore-ffmpeg)

# libavformat major, PyAV release, Python it has wheels for
releases=("60 12.3.0 3.12" "61 13.1.0 3.12" "61 14.2.0 3.12")
status=0
for r in "${releases[@]}"; do
    set -- $r
    major=$1 version=$2 python=$3
    libs="$work/$version/av.libs"
    if [ ! -d "$libs" ]; then
        mkdir -p "$work/$version"
        pip download -q "av==$version" --no-deps --only-binary=:all: --python-version "$python" \
            --platform manylinux_2_17_x86_64 -d "$work/$version/wheel" || { status=1; continue; }
        python3 -I -m zipfile -e "$work/$version"/wheel/*.whl "$work/$version/x"
        mv "$work/$version/x/av.libs" "$libs" && rm -rf "$work/$version/x" "$work/$version/wheel"
        # The names FFmpeg's libraries have (PyAV's carry a hash).
        for lib in avformat avcodec avutil; do
            f=$(cd "$libs" && ls | grep "^lib$lib-" | head -1)
            ln -sf "$f" "$libs/lib$lib.so.$(echo "$f" | sed -E 's/.*\.so\.([0-9]+).*/\1/')"
        done
    fi
    echo "== libavformat $major (PyAV $version)"
    out=$(LD_LIBRARY_PATH="$libs" SONKORE_FFMPEG_MAJOR=$major SONKORE_TEST_CORPUS="$corpus" \
        ALSA_CONFIG_PATH="$root/tests/pipewire/alsa-null.conf" \
        cargo test -q "${args[@]}" -- --nocapture --test-threads=1 2>&1)
    echo "$out" | grep -oE 'FFmpeg: libavformat.*|FFmpeg not loaded.*|.*FAILED.*|.*panicked.*|.*off by.*' | sort -u
    if echo "$out" | grep -qE 'FAILED|^error' || ! echo "$out" | grep -q "FFmpeg: libavformat $major\."; then
        echo "   FALLÓ (o no cargó esta versión)"
        status=1
    else
        echo "   bien"
    fi
done
exit $status
