#!/bin/bash
# A PipeWire of its own for trying the engine's PipeWire output without
# touching the desktop's sound: builds testd.c (a daemon that grants access
# and links streams to a null sink, a session manager's job), starts it in a
# fresh runtime dir and runs the command given against it, with ALSA's
# "default" made silent so a fallback never reaches a real card.
#
#   tests/pipewire/run.sh target/release/examples/play --device "pipewire|test-sink" FILE
#
# Needs gcc and libpipewire-0.3's headers (pkg-config libpipewire-0.3).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
work="$(mktemp -d /tmp/sonkore-pwXXXX)" # short: a socket path has 108 bytes
gcc -O1 -o "$work/testd" "$here/testd.c" $(pkg-config --cflags --libs libpipewire-0.3)
mkdir -p "$work/conf" "$work/run"
cp "$here/test.conf" "$work/conf/"
XDG_RUNTIME_DIR="$work/run" PIPEWIRE_CONFIG_DIR="$work/conf" "$work/testd" test.conf 2> "$work/daemon.log" &
daemon=$!
trap 'kill $daemon 2>/dev/null; echo "daemon log: $work/daemon.log"' EXIT
sleep 0.5
XDG_RUNTIME_DIR="$work/run" ALSA_CONFIG_PATH="$here/alsa-null.conf" "$@"
