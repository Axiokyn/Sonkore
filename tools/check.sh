#!/usr/bin/env bash
# Everything that can catch a bug before the user does, in one go:
#   tools/check.sh            rustfmt, build (no warnings), clippy, tests (with the
#                                  engine's and the library's corpus), a type
#                                  check for Windows, offscreen views, live
#                                  scenarios at the user's scale (150 %)
#   tools/check.sh --todo     the same, with the live scenarios at six
#                                  screen scales and at Interface scale 110 %
# Exit status 0 only when every step passed; a summary at the end.
# No step plays sound on a real device (null ALSA, a PipeWire of its own).
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/.." && pwd)"
cd "$root"
all=0
[ "${1:-}" = --todo ] && all=1
export ALSA_CONFIG_PATH="$root/tests/pipewire/alsa-null.conf"
results=()
step() { # name command...
    local name="$1" start=$SECONDS
    shift
    echo "== $name"
    if "$@"; then
        results+=("OK     $name ($((SECONDS - start)) s)")
    else
        results+=("FALLÓ  $name ($((SECONDS - start)) s)")
    fi
}

corpus="$root/target/test-corpus"
[ -d "$corpus/album" ] || tests/make_corpus.sh "$corpus" >/dev/null
library_corpus="$root/target/library-corpus"
[ -d "$library_corpus/Music" ] || crates/library/tests/make_library_corpus.sh "$library_corpus" >/dev/null

# Warnings, also the ones Slint's compiler prints (a deprecated property…):
# cargo replays them on every build, so a warning stays until it is fixed.
no_warnings() { # cargo subcommand, arguments... (--message-format goes
    local out     # right after the subcommand: after "--" it reaches rustc)
    local sub="$1"
    shift
    out=$(cargo "$sub" --message-format=short "$@" 2>&1)
    local status=$?
    echo "$out" | grep -E '^(crates|tools|vendor)/|^(warning|error)' | grep -v '^warning: build failed' | sort -u
    [ $status = 0 ] && ! echo "$out" | grep -qE '^(crates|tools)/[^ ]*: (warning|error)|^warning: sonkore'
}
step "formato (cargo fmt)" cargo fmt --all --check
step "build" no_warnings build -p sonkore
# Clippy: which lints count is in Cargo.toml ([workspace.lints]).
step "clippy" no_warnings clippy --keep-going --workspace --all-targets
# DSD/DST against FFmpeg's decoder, when its corpus was made
# (crates/dsd/tests/reference/make_reference.sh).
dsd_corpus="$root/target/dsd-corpus"
[ -d "$dsd_corpus" ] || dsd_corpus=
step "tests" env SONKORE_TEST_CORPUS="$corpus" SONKORE_LIBRARY_CORPUS="$library_corpus" \
    ${dsd_corpus:+SONKORE_DSD_CORPUS="$dsd_corpus"} cargo test --quiet --workspace
step "Windows (cargo check)" "$here/check-windows.sh" --quiet
step "textos sin traducción" bash -c 'for l in es de fr it pt_BR; do "'"$here"'/textos-sin-traducir.py" $l || exit 1; done'
step "vistas sin pantalla (ui-tour)" bash -c '
    out=$("'"$here"'/ui-tour.sh" 2>&1); echo "$out" | tail -n +1 | grep -v "^build"
    echo "$out" | grep -q "^[0-9]* iguales, 0 cambiaron, 0 sin captura" || {
        echo "   (si el cambio era a propósito: tools/ui-tour.sh --base)"; exit 1; }'
# Settings options no scenario clicks and checks with `guardado` (only shown).
coverage=$("$here/cobertura-ajustes.py" | tail -1)
if [ $all = 1 ]; then
    step "escenarios en vivo, 6 escalas" "$here/ui-live.py" --escalas 100,110,125,150,175,200
    step "escenarios en vivo, interfaz 110 %" "$here/ui-live.py" --interfaz 1.1
else
    step "escenarios en vivo (150 %)" "$here/ui-live.py"
fi

echo
printf '%s\n' "${results[@]}"
echo "INFO   $coverage (tools/cobertura-ajustes.py)"
! printf '%s\n' "${results[@]}" | grep -q '^FALLÓ'
