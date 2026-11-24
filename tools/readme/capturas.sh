#!/usr/bin/env bash
# The README's screenshots, from the real window (tools/ui-live.py at 200 %,
# the made-up library of biblioteca.py): docs/screenshots/*.webp.
#   tools/readme/capturas.sh
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
"$root/tools/ui-live.py" --escalas 200 "$here/capturas.txt"
python3 - "$root/target/ui-live/capturas-200" "$root/docs/screenshots" <<'PY'
import sys
from pathlib import Path
from PIL import Image
src, dst = map(Path, sys.argv[1:])
for name in ("now-playing", "albums", "mini-player"):
    out = dst / f"{name}.webp"
    Image.open(src / f"{name}-1.png").save(out, "WEBP", quality=90, method=6)
    print(f"{out.relative_to(dst.parent.parent)}: {out.stat().st_size // 1024} KB")
PY
