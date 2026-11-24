#!/usr/bin/env python3
"""Every text the app translates (@tr in the .slint files, tr/trf in the Rust
code) must have a translation in i18n/<lang>.po under the same context: a
text worded differently, or under another context, shows in English.

    tools/textos-sin-traducir.py [es]     (exit 1 if any is missing)
"""
import importlib.util
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location("update", ROOT / "i18n" / "update.py")
po = importlib.util.module_from_spec(spec)
spec.loader.exec_module(po)


def main():
    lang = sys.argv[1] if len(sys.argv) > 1 else "es"
    have = {(c, m) for c, m, t in po.read_po(ROOT / "i18n" / f"{lang}.po") if t}
    missing = [pair for pair in po.used() if pair not in have]
    for ctx, text in missing:
        print(f"  {ctx} «{text}»")
    print(f"{len(missing)} textos sin traducción en {lang}")
    return 1 if missing else 0


if __name__ == "__main__":
    sys.exit(main())
