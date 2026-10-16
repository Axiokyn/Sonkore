#!/usr/bin/env python3
"""Builds the app's interface font, fonts/inter/<weight>.ttf, from Inter 4
(SIL OFL 1.1, no Reserved Font Name; https://github.com/rsms/inter).

Inter's own shapes, unchanged. Each weight (400-700) is the text cut
(opsz 14, which Qt cannot pick by size on its own), hinted with ttfautohint
(a slightly taller x-height and whole-pixel stems at list sizes) and cut down
to Latin with Vietnamese, Greek and Cyrillic.

  pip install fonttools brotli ttfautohint-py
  build-inter.py InterVariable.ttf fonts/inter
"""
import io, os, subprocess, sys, tempfile
from fontTools.ttLib import TTFont
from fontTools.varLib import instancer

APP_UNICODES = ("U+0000-024F,U+0259,U+02B9-02DD,U+0300-036F,U+0370-03FF,U+0400-052F,U+1E00-1EFF,"
                "U+2000-206F,U+20A0-20CF,U+2100-214F,U+2190-2199,U+2212,U+2215,U+2219,U+221E,U+2248,"
                "U+2260,U+2264,U+2265,U+25A0-25CF")
STYLES = [("Regular", 400), ("Medium", 500), ("SemiBold", 600), ("Bold", 700)]


def set_names(font, style):
    """Family "Inter" with the style as subfamily (and a family of its own
    for the legacy IDs 1/2, which hold four styles at most)."""
    name = font["name"]
    full = "Inter" if style == "Regular" else f"Inter {style}"
    legacy = ("Inter", style) if style in ("Regular", "Bold") else (full, "Regular")
    version = name.getDebugName(5)
    name.names = [r for r in name.names if r.nameID in (0, 7, 8, 9, 11, 12, 13, 14)]
    for nid, text in {1: legacy[0], 2: legacy[1], 3: f"{version};Inter-{style}", 4: full,
                      5: version, 6: f"Inter-{style}", 16: "Inter", 17: style}.items():
        name.setName(text, nid, 3, 1, 0x409)
        name.setName(text, nid, 1, 0, 0)


def hint(font):
    import ttfautohint
    buf = io.BytesIO(); font.save(buf)
    out = ttfautohint.ttfautohint(in_buffer=buf.getvalue(), no_info=True, hint_composites=True,
                                  increase_x_height=14, windows_compatibility=True)
    return TTFont(io.BytesIO(out))


def main():
    src, out = sys.argv[1], sys.argv[2]
    os.makedirs(out, exist_ok=True)
    with tempfile.TemporaryDirectory() as tmp:
        for style, weight in STYLES:
            f = instancer.instantiateVariableFont(TTFont(src), {"wght": weight, "opsz": 14})
            for t in ("STAT", "MVAR"):
                if t in f:
                    del f[t]
            set_names(f, style)
            f["OS/2"].usWeightClass = weight
            f["OS/2"].fsSelection = (f["OS/2"].fsSelection & ~0b1100001) | (
                0b1000000 if style == "Regular" else 0b100000 if style == "Bold" else 0)
            f["head"].macStyle = 1 if style == "Bold" else 0
            full = os.path.join(tmp, f"{weight}.ttf")
            hint(f).save(full)
            subprocess.run([sys.executable, "-m", "fontTools.subset", full,
                            "--unicodes=" + APP_UNICODES, "--layout-features=*", "--name-IDs=*",
                            "--name-languages=*", "--hinting", "--output-file=" + os.path.join(out, f"{weight}.ttf")],
                           check=True)
            print(" ", style, weight)


if __name__ == "__main__":
    main()
