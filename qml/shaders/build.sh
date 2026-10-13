#!/usr/bin/env bash
# Compiles the shaders to .qsb (all graphics APIs). The .qsb files are
# committed, so building the app does not need Qt Shader Tools.
# Needs qsb (Qt Shader Tools; Fedora: qt6-qtshadertools, binary qsb-qt6).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
qsb="$(command -v qsb || command -v qsb-qt6)"
for src in "$here"/*.frag "$here"/*.vert; do
    [ -e "$src" ] || continue
    "$qsb" --glsl "100 es,120,150" --hlsl 50 --msl 12 -o "$src.qsb" "$src"
    echo "built $(basename "$src").qsb"
done
