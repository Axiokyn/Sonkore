#!/usr/bin/env bash
# The interface's views, rendered offscreen (no window, no display, no
# permission prompt) and compared with the last accepted ones:
#   tools/ui-tour.sh                  build (dev) + every view
#   tools/ui-tour.sh pistas ajustes   only those views
#   tools/ui-tour.sh --base           accept the current captures as the base
#   tools/ui-tour.sh --lista          list the views
#   tools/ui-tour.sh --hoja           every capture on one 1500 px sheet
# Captures: target/ui-tour/capturas/<view>.png. What changed against
# target/ui-tour/base: crops (before above, now below) in target/ui-tour/cambios/
# — look at those, not at the full captures.
# The profile (settings, a tagged test library, covers) lives in
# target/ui-tour/perfil and is made once (ffmpeg); delete it to remake it.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/.." && pwd)"
out="$root/target/ui-tour"
perfil="$out/perfil"
bin="$root/target/debug/sonkore"

# name | environment (SONKORE_* hooks: see screenshot.rs, controller.rs, pages.rs)
views=(
    "pistas|SONKORE_START_PAGE=tracks"
    "reproduciendo|SONKORE_START_PAGE=playing"
    "favoritos|SONKORE_START_PAGE=favorites"
    "albumes|SONKORE_START_PAGE=albums"
    "album|SONKORE_START_PAGE=albums SONKORE_OPEN_ALBUM=0"
    "artistas|SONKORE_START_PAGE=artists SONKORE_BROWSE=0"
    "generos|SONKORE_START_PAGE=genres"
    "carpetas|SONKORE_START_PAGE=folders"
    "cola|SONKORE_START_PAGE=queue"
    "menu-pista|SONKORE_START_PAGE=tracks SONKORE_SHOW=track"
    "menu-orden|SONKORE_START_PAGE=tracks SONKORE_SHOW=sort"
    "menu-columnas|SONKORE_START_PAGE=tracks SONKORE_SHOW=columns"
    "salidas|SONKORE_START_PAGE=tracks SONKORE_SHOW=outputs"
    "volumen|SONKORE_START_PAGE=tracks SONKORE_SHOW=volume"
    "pistas-150|SONKORE_START_PAGE=tracks SONKORE_SCREENSHOT_SCALE=1.5"
    "pistas-chica|SONKORE_START_PAGE=tracks SONKORE_SCREENSHOT_SIZE=900x560"
    "ajustes|SONKORE_SCREENSHOT_WINDOW=settings"
    "ecualizador|SONKORE_SCREENSHOT_WINDOW=equalizer"
    "pitch|SONKORE_SCREENSHOT_WINDOW=pitch"
    "mini|SONKORE_START_PAGE=tracks XDG_CONFIG_HOME=$perfil/config-mini SONKORE_SCREENSHOT_SIZE=480x160"
)

case "${1:-}" in
--hoja)
    python3 "$here/ui-diff.py" --hoja "$out/capturas" "$out/hoja.png"
    exit 0 ;;
--lista) for v in "${views[@]}"; do echo "${v%%|*}"; done; exit 0 ;;
--base)
    rm -rf "$out/base" && mkdir -p "$out/base" && cp "$out"/capturas/*.png "$out/base/"
    echo "base: $(ls "$out/base" | wc -l) capturas"
    exit 0 ;;
esac
wanted=("$@")

make_profile() {
    local music="$perfil/musica" tmp="$perfil/tmp"
    rm -rf "$perfil" && mkdir -p "$music" "$tmp" "$perfil/config/sonkore" "$perfil/data" "$perfil/cache"
    ff() { ffmpeg -nostdin -loglevel error -y "$@"; }
    local colors=(3a6ea5 a5523a 4f8a3a 8a3a7d) albums=("Luz de Norte" "Calle Ruido" "Verde Agua" "Noche Larga")
    local artists=("Aurora Sur" "Los Ruidos" "Aurora Sur" "Marea") genres=(Pop Rock Ambient Jazz)
    for a in 0 1 2 3; do
        ff -f lavfi -i "color=c=0x${colors[$a]}:s=300x300" -frames:v 1 "$tmp/cover$a.png"
        for t in 1 2 3; do
            local f="$music/${albums[$a]}/0$t.flac"
            mkdir -p "$(dirname "$f")"
            ff -f lavfi -i "sine=frequency=$((220 * t + 110 * a)):sample_rate=44100:duration=$((2 + t))" -ac 2 \
                -i "$tmp/cover$a.png" -map 0:a -map 1:v -c:a flac -c:v png -disposition:v attached_pic \
                -metadata title="Tema $((a * 3 + t)) de ${albums[$a]}" -metadata artist="${artists[$a]}" \
                -metadata album="${albums[$a]}" -metadata genre="${genres[$a]}" -metadata track="$t" \
                -metadata date="$((2019 + a))" "$f"
        done
    done
    rm -rf "$tmp"
    cat > "$perfil/config/sonkore/sonkore.conf" <<EOF
[library]
onlineCovers=false
rescanOnStartup=false
watchFolders=false

[ui]
startPage=tracks
trayIcon=false
EOF
    XDG_CONFIG_HOME="$perfil/config" XDG_DATA_HOME="$perfil/data" XDG_CACHE_HOME="$perfil/cache" \
        "$root/target/debug/examples/scan" "$perfil/data/sonkore" "$perfil/config/sonkore/sonkore.conf" "$music" >/dev/null
    mkdir -p "$perfil/config-mini/sonkore"
    sed 's/^\[ui\]$/[ui]\ncompactMode=true/' "$perfil/config/sonkore/sonkore.conf" > "$perfil/config-mini/sonkore/sonkore.conf"
}

start=$SECONDS
cargo build --quiet --manifest-path "$root/Cargo.toml" -p sonkore
[ -d "$perfil/data/sonkore" ] || {
    cargo build --quiet --manifest-path "$root/Cargo.toml" -p sonkore-library --example scan
    make_profile
}
built=$SECONDS

mkdir -p "$out/capturas" "$perfil/cache/fontconfig"
rm -rf "$out/cambios" && mkdir -p "$out/cambios"
jobs=()
for v in "${views[@]}"; do
    name="${v%%|*}"
    if [ ${#wanted[@]} -gt 0 ] && [[ ! " ${wanted[*]} " == *" $name "* ]]; then continue; fi
    jobs+=("$name|${v#*|}")
done
[ ${#jobs[@]} -gt 0 ] || { echo "ninguna vista con ese nombre (--lista)"; exit 1; }
printf '%s\n' "${jobs[@]}" | xargs -P "$(nproc)" -I{} bash -c '
    name="${1%%|*}"; envs="${1#*|}"
    env -i HOME="$HOME" PATH="$PATH" LANG=es_AR.UTF-8 \
        XDG_CONFIG_HOME="$2/config" XDG_DATA_HOME="$2/data" XDG_CACHE_HOME="$2/cache" \
        ALSA_CONFIG_PATH=/dev/null SONKORE_ASOUND_ROOT="$5/tests/asound" $envs SONKORE_SCREENSHOT="$3/capturas/$name.png" "$4" \
        >"$3/capturas/$name.log" 2>&1 || echo "FALLÓ $name (ver $3/capturas/$name.log)"
' _ {} "$perfil" "$out" "$bin" "$root"
python3 "$here/ui-diff.py" "$out/base" "$out/capturas" "$out/cambios" "${jobs[@]%%|*}"
echo "build $((built - start)) s, capturas $((SECONDS - built)) s"
