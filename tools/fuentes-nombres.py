#!/usr/bin/env python3
"""Gives the fonts in fonts/<family>/<weight>.ttf the names Settings › Font
looks them up by (src/fonts.rs): the family name ("Manrope") and the weight.

They were cut from variable fonts and kept the name of the variable font's
first instance ("Manrope ExtraLight", "Outfit Thin", "Nunito Sans 12pt
ExtraLight 12pt"), so Slint never found "Manrope" and drew with Inter. The
names follow Inter's: name 1/2 as the four-style family ("Manrope" Regular,
"Manrope Medium" Regular, "Manrope SemiBold" Regular, "Manrope" Bold),
name 16/17 the whole family and the weight.

    python3 tools/fuentes-nombres.py [--revisar]   (needs fontTools)

--revisar changes nothing: it fails if a font's names are not these.
"""
import sys
from pathlib import Path

from fontTools.ttLib import TTFont

ROOT = Path(__file__).resolve().parent.parent / "fonts"
FAMILIES = {
    "manrope": "Manrope",
    "plus-jakarta-sans": "Plus Jakarta Sans",
    "figtree": "Figtree",
    "outfit": "Outfit",
    "dm-sans": "DM Sans",
    "nunito-sans": "Nunito Sans",
    "rubik": "Rubik",
}
WEIGHTS = {400: "Regular", 500: "Medium", 600: "SemiBold", 700: "Bold"}


def wanted(family, weight):
    style = WEIGHTS[weight]
    ribbi = weight in (400, 700)
    return {
        1: family if ribbi else f"{family} {style}",
        2: style if ribbi else "Regular",
        4: f"{family} {style}" if weight != 400 else family,
        6: f"{family.replace(' ', '')}-{style}",
        16: family,
        17: style,
    }


def main():
    check = "--revisar" in sys.argv[1:]
    wrong = []
    for folder, family in FAMILIES.items():
        for weight in WEIGHTS:
            path = ROOT / folder / f"{weight}.ttf"
            font = TTFont(path)
            table = font["name"]
            names = wanted(family, weight)
            differs = [i for i, text in names.items() if table.getDebugName(i) != text]
            if not differs:
                continue
            if check:
                wrong.append(f"{path.relative_to(ROOT.parent)}: {table.getDebugName(1)!r} {table.getDebugName(2)!r}")
                continue
            # Every platform's record of those ids, then the Windows and Mac ones anew.
            table.names = [n for n in table.names if n.nameID not in names]
            for i, text in names.items():
                table.setName(text, i, 3, 1, 0x409)
                table.setName(text, i, 1, 0, 0)
            os2 = font["OS/2"]
            os2.usWeightClass = weight
            # fsSelection: REGULAR (bit 6) for 400, BOLD (bit 5) for 700.
            os2.fsSelection = (os2.fsSelection & ~0b1100001) | (0b1000000 if weight == 400 else 0b100000 if weight == 700 else 0)
            font["head"].macStyle = 1 if weight == 700 else 0
            font.save(path)
            print(f"{path.relative_to(ROOT.parent)}: {family} {WEIGHTS[weight]}")
    if wrong:
        print("Con nombres que Slint no encuentra (python3 tools/fuentes-nombres.py):")
        print("\n".join("  " + w for w in wrong))
        sys.exit(1)


if __name__ == "__main__":
    main()
