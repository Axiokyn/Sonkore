"""Makes fonts/inter/tnum-<weight>.ttf: Inter with its tabular figures
("tnum") in place of the default ones, as a family of its own ("Inter Tnum").
Slint cannot turn OpenType features on, and times, counts and list cells
are drawn with "tnum" (the "1" with a foot, every digit as wide), so the
app uses this family there (Theme.tnum-family).

    python3 tools/tnum-fonts.py [OUT_DIR]   (needs fontTools: pip install fonttools)

Each glyph the "tnum" feature replaces takes the outline and the advance of
its tabular form, under its own name: the code points stay on the default
glyphs, so Inter's contextual alternates ("calt") still see them. Mapping
the code points to the tabular glyphs instead (as this did until
2026-11-22) broke "calt": the clock's colon stayed low between figures,
where Inter with tnum on (calt before it) raises it.

Inter is under the SIL Open Font License, which allows modified versions
under the same license (fonts/inter/OFL.txt); it reserves no font name.
"""
from pathlib import Path

from fontTools.pens.recordingPen import DecomposingRecordingPen
from fontTools.pens.ttGlyphPen import TTGlyphPen
from fontTools.ttLib import TTFont

ROOT = Path(__file__).resolve().parents[1]
WEIGHTS = (400, 500, 600)


def tnum_map(font):
    """glyph name -> tabular glyph name, from the GSUB "tnum" single substitutions."""
    gsub = font["GSUB"].table
    lookups = set()
    for record in gsub.FeatureList.FeatureRecord:
        if record.FeatureTag == "tnum":
            lookups.update(record.Feature.LookupListIndex)
    mapping = {}
    for index in sorted(lookups):
        lookup = gsub.LookupList.Lookup[index]
        for sub in lookup.SubTable:
            if lookup.LookupType == 7:  # extension
                sub = sub.ExtSubTable
            mapping.update(getattr(sub, "mapping", {}) or {})
    return mapping


def main():
    import sys
    out_dir = Path(sys.argv[1]) if len(sys.argv) > 1 else ROOT / "fonts/inter"
    for weight in WEIGHTS:
        src = ROOT / "fonts/inter" / f"{weight}.ttf"
        out = out_dir / f"tnum-{weight}.ttf"
        font = TTFont(src)
        mapping = tnum_map(font)
        glyf, hmtx = font["glyf"], font["hmtx"]
        # Drawn from the source glyphs, before any is replaced, and
        # decomposed: a tabular figure is mostly the default one as a
        # component, which would then be itself.
        source = font.getGlyphSet()
        shapes = {}
        for t in set(mapping.values()):
            flat = DecomposingRecordingPen(source)
            source[t].draw(flat)
            pen = TTGlyphPen(None)
            flat.replay(pen)
            shapes[t] = (pen.glyph(), hmtx[t])
        for glyph, tabular in mapping.items():
            glyf[glyph], hmtx[glyph] = shapes[tabular]
        # The kerning the default figures have no longer fits them: the
        # tabular ones have none (GPOS pairs on them are dropped).
        drop_pair_kerning(font, set(mapping))
        for record in font["name"].names:
            if record.nameID in (1, 4, 16):
                record.string = str(record.toUnicode()).replace("Inter", "Inter Tnum", 1)
            elif record.nameID == 6:
                record.string = str(record.toUnicode()).replace("Inter", "InterTnum", 1)
        font.save(out)
        print(f"{out}: {len(mapping)} glyphs in their tabular form")


def drop_pair_kerning(font, glyphs):
    """Pair adjustments (GPOS type 2, "kern") on any of `glyphs`, removed."""
    gpos = font["GPOS"].table
    kern = set()
    for record in gpos.FeatureList.FeatureRecord:
        if record.FeatureTag == "kern":
            kern.update(record.Feature.LookupListIndex)
    for index in kern:
        lookup = gpos.LookupList.Lookup[index]
        for sub in lookup.SubTable:
            if lookup.LookupType == 9:  # extension
                sub = sub.ExtSubTable
            if getattr(sub, "LookupType", 2) != 2 and lookup.LookupType not in (2, 9):
                continue
            if getattr(sub, "Format", 0) == 1:
                for i, first in enumerate(sub.Coverage.glyphs):
                    pairs = sub.PairSet[i]
                    if first in glyphs:
                        pairs.PairValueRecord = []
                    else:
                        pairs.PairValueRecord = [r for r in pairs.PairValueRecord if r.SecondGlyph not in glyphs]
                    pairs.PairValueCount = len(pairs.PairValueRecord)
            elif getattr(sub, "Format", 0) == 2:
                # Class kerning: the glyphs leave their classes (class 0: none).
                for g in glyphs:
                    sub.ClassDef2.classDefs.pop(g, None)
                    if g in sub.Coverage.glyphs:
                        sub.ClassDef1.classDefs.pop(g, None)
                        # Class 0 of the first glyph still kerns: no way to
                        # leave the coverage without rebuilding it.
                        sub.Coverage.glyphs = [x for x in sub.Coverage.glyphs if x != g]


main()
