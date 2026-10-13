#!/usr/bin/env bash
# Runs a command against a private PipeWire daemon (with WirePlumber) that has
# one virtual DAC: "test-dac", stereo, S32, and the stock rate policy of a
# desktop (48 kHz only), so anything else has to be asked for. engine_test's
# "pipewire" test plays through it and records what it receives.
#   tests/pipewire-session.sh <command...>
set -euo pipefail
dir="$(mktemp -d)"
export XDG_RUNTIME_DIR="$dir/run" XDG_CONFIG_HOME="$dir/config" XDG_STATE_HOME="$dir/state" XDG_DATA_HOME="$dir/data"
mkdir -p "$XDG_RUNTIME_DIR" "$XDG_CONFIG_HOME/pipewire/pipewire.conf.d"
chmod 700 "$XDG_RUNTIME_DIR"
cat > "$XDG_CONFIG_HOME/pipewire/pipewire.conf.d/test-dac.conf" <<'CONF'
context.properties = {
    default.clock.rate = 48000
    default.clock.allowed-rates = [ 48000 ]
}
context.objects = [
    { factory = adapter
      args = {
        factory.name = support.null-audio-sink
        node.name = "test-dac"
        node.description = "Test DAC"
        media.class = Audio/Sink
        audio.position = [ FL FR ]
        audio.format = S32
        monitor.channel-volumes = true
      }
    }
]
CONF
pids=()
cleanup() {
    for p in "${pids[@]}"; do kill "$p" 2>/dev/null || true; done
    rm -rf "$dir"
}
trap cleanup EXIT
dbus-daemon --session --address="unix:path=$XDG_RUNTIME_DIR/bus" --nofork --nopidfile >/dev/null 2>&1 &
pids+=($!)
export DBUS_SESSION_BUS_ADDRESS="unix:path=$XDG_RUNTIME_DIR/bus"
sleep 0.5
pipewire >"$dir/pipewire.log" 2>&1 &
pids+=($!)
sleep 1
wireplumber >"$dir/wireplumber.log" 2>&1 &
pids+=($!)
for _ in $(seq 50); do
    pw-cli ls Node 2>/dev/null | grep -q '"test-dac"' && break
    sleep 0.2
done
"$@"
