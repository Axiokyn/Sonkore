#!/usr/bin/env bash
# The FFmpeg struct offsets crates/ffmpeg/src/abi.rs uses, from the real
# headers of each release (downloaded once from ffmpeg.org into
# target/ffmpeg-headers), side by side; "sys" is /usr/include. Run it when a
# new FFmpeg major comes out, and copy its column into abi.rs.
#   tools/ffmpeg-offsets.sh [version…]      (default: 5.0.3 5.1.6 6.0.1 6.1.2 7.0.2 7.1.1 8.0)
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cache="$root/target/ffmpeg-headers"
src="$root/crates/ffmpeg/tests/offsets.c"
versions=("$@")
[ ${#versions[@]} -gt 0 ] || versions=(5.0.3 5.1.6 6.0.1 6.1.2 7.0.2 7.1.1 8.0)
mkdir -p "$cache"
cols=()
for v in "${versions[@]}"; do
    d="$cache/$v"
    if [ ! -f "$d/libavformat/avformat.h" ]; then
        mkdir -p "$d"
        curl -sSfL "https://ffmpeg.org/releases/ffmpeg-$v.tar.xz" |
            tar -xJ -C "$d" --strip-components=1 --wildcards '*/libavutil/*.h' '*/libavcodec/*.h' '*/libavformat/*.h'
        # Written by configure; only its two flags matter here.
        printf '#define AV_HAVE_BIGENDIAN 0\n#define AV_HAVE_FAST_UNALIGNED 1\n' > "$d/libavutil/avconfig.h"
    fi
    gcc -w -I"$d" "$src" -o "$cache/offsets-$v"
    "$cache/offsets-$v" > "$cache/$v.txt"
    cols+=("$v")
done
if [ -f /usr/include/libavformat/avformat.h ]; then
    gcc -w "$src" -o "$cache/offsets-sys" && "$cache/offsets-sys" > "$cache/sys.txt" && cols+=(sys)
fi
python3 - "$cache" "${cols[@]}" <<'EOF'
import sys
cache, cols = sys.argv[1], sys.argv[2:]
tables = []
for c in cols:
    rows = {}
    for line in open(f"{cache}/{c}.txt"):
        key, value = line.split(" ", 1)
        rows[key] = value.strip().replace(" ", "/")
    tables.append(rows)
keys = []
for t in tables:
    keys += [k for k in t if k not in keys]
print(f"{'':40}" + "".join(f"{c:>10}" for c in cols))
for k in keys:
    print(f"{k:40}" + "".join(f"{t.get(k, '-'):>10}" for t in tables))
EOF
