#!/usr/bin/env python3
"""Which options of the Settings window no live scenario tests.

Every control in crates/app/ui/*.slint names the sonkore.conf key it writes
(`key: "playback/limiter"`, or `Appearance.edit("backdrop", …)` for
appearance/backdrop), and a scenario proves the option with a real
click plus `guardado playback/limiter=…` (tools/escenarios/*.txt). This lists
the keys that no `guardado` line checks: options that may not even save.

    tools/cobertura-ajustes.py           the untested keys, by window file
    tools/cobertura-ajustes.py --todas   every key, with the scenarios testing it
Exit status 1 when a key is untested (so check.sh can show it).
"""
import re
import sys
from collections import defaultdict
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
UI = REPO / "crates/app/ui"
SCENARIOS = REPO / "tools/escenarios"

KEY_IN_UI = re.compile(r'\bkey:\s*"([a-zA-Z]+/[A-Za-z0-9/]+)"')
# Settings › Appearance: Appearance.edit("backdrop", …) writes appearance/backdrop
# (settings_ui.rs, edit_appearance), the visualizer ui/visualizer.
APPEARANCE_EDIT = re.compile(r'\bAppearance\.edit\(\s*"([A-Za-z0-9]+)"')
APPEARANCE_KEY = {"visualizer": "ui/visualizer"}
# Options no scenario can click here, and why.
EXCEPTIONS = {
    "library/onlineCovers": "saldría a internet (MusicBrainz)",
}
KEY_SAVED = re.compile(r'^\s*guardado\s+([a-zA-Z]+/[A-Za-z0-9/]+)', re.M)


def ui_keys():
    keys = defaultdict(set)  # key -> files
    for f in sorted(UI.glob("*.slint")):
        text = f.read_text(encoding="utf-8")
        for k in KEY_IN_UI.findall(text):
            keys[k].add(f.name)
        for name in APPEARANCE_EDIT.findall(text):
            keys[APPEARANCE_KEY.get(name, f"appearance/{name}")].add(f.name)
    return keys


def tested_keys():
    tested = defaultdict(set)  # key -> scenarios
    for f in sorted(SCENARIOS.glob("*.txt")):
        for k in KEY_SAVED.findall(f.read_text(encoding="utf-8")):
            tested[k].add(f.stem)
    return tested


def main():
    every = "--todas" in sys.argv[1:]
    keys, tested = ui_keys(), tested_keys()
    untested = [k for k in sorted(keys) if k not in tested and k not in EXCEPTIONS]
    if every:
        for k in sorted(keys):
            why = f"(excepción: {EXCEPTIONS[k]})" if k in EXCEPTIONS else "-- sin prueba --"
            print(f"{k:40} {', '.join(sorted(tested[k])) or why}")
    else:
        by_file = defaultdict(list)
        for k in untested:
            by_file[", ".join(sorted(keys[k]))].append(k)
        for f, ks in sorted(by_file.items()):
            print(f"{f}:")
            for k in ks:
                print(f"  {k}")
    skipped = sum(1 for k in keys if k in EXCEPTIONS and k not in tested)
    print(f"{len(keys) - len(untested) - skipped} de {len(keys)} opciones de Ajustes con `guardado` en algún escenario"
          f" ({skipped} excepciones: --todas)")
    return 1 if untested else 0


if __name__ == "__main__":
    sys.exit(main())
