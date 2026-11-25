#!/usr/bin/env bash
# Builds the headless compositor into $1 (default: target/wl-headless).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
out="${1:-$here/../../target/wl-headless}"
mkdir -p "$out"
if [ "$out/headless" -nt "$here/headless.c" ]; then exit 0; fi
p=/usr/share/wayland-protocols
for x in stable/xdg-shell/xdg-shell stable/viewporter/viewporter staging/fractional-scale/fractional-scale-v1; do
    n="$(basename "$x")"
    wayland-scanner server-header "$p/$x.xml" "$out/$n-server.h"
    wayland-scanner private-code "$p/$x.xml" "$out/$n-code.c"
done
gcc -O2 -w -I"$out" -o "$out/headless" "$here/headless.c" "$out"/*-code.c \
    $(pkg-config --cflags --libs wayland-server pixman-1 xkbcommon) -lm
