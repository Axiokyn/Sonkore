#!/usr/bin/env python3
"""Builds Sonkore Sans, the app's interface font, from Inter 4 (SIL OFL 1.1,
no Reserved Font Name).

Inter with San Francisco's habits as defaults:
  * the t's stem cut on a slant at the top (all weights: the cut drops 0.55
    of the stem's width on the left);
  * compact f and t (Inter's cv12, cv13) and round quotes and commas (ss03);
    Inter's shapes stay reachable through those same features;
  * plain l and closed digits, as in Inter;
  * the Sonkore mark at U+E000; own names; fsType 0.
Output, in <outdir>: variable roman and italic (TTF + WOFF2); static/ with
hinted statics (ttfautohint), text cut (opsz 14) and display cut (opsz 32),
Light to ExtraBold; app/ with fonts/sonkore-sans/<weight>.ttf for the app
(400-700; Latin with Vietnamese, Greek, Cyrillic).

  pip install fonttools brotli ttfautohint-py
  build-sonkore-sans.py InterVariable.ttf InterVariable-Italic.ttf <outdir>
"""
import copy, io, math, os, subprocess, sys
from fontTools.ttLib import TTFont
from fontTools.pens.ttGlyphPen import TTGlyphPen
from fontTools.varLib import instancer

FAMILY = "Sonkore Sans"
VERSION = "3.000"
COPYRIGHT = ("Copyright 2026 Axiokyn (https://github.com/Axiokyn/Sonkore). "
             "Derived from Inter: Copyright 2016 The Inter Project Authors (https://github.com/rsms/inter)")
SLANT = 0.55   # drop of the t's top-left corner, in stem widths
APP_UNICODES = ("U+0000-024F,U+0259,U+02B9-02DD,U+0300-036F,U+0370-03FF,U+0400-052F,U+1E00-1EFF,"
                "U+2000-206F,U+20A0-20CF,U+2100-214F,U+2190-2199,U+2212,U+2215,U+2219,U+221E,U+2248,"
                "U+2260,U+2264,U+2265,U+25A0-25CF,U+E000")


def swap_glyphs(font, a, b):
    """Intercambia contorno, métricas y variaciones de dos glifos (los nombres no cambian,
    así que GSUB/GPOS siguen siendo válidos)."""
    glyf = font["glyf"]; hmtx = font["hmtx"]
    va = vb = None
    if "gvar" in font:
        gv = font["gvar"].variations
        va = gv[a] if a in gv else None   # decodificar ANTES de cambiar los contornos
        vb = gv[b] if b in gv else None
    glyf.glyphs[a], glyf.glyphs[b] = glyf.glyphs[b], glyf.glyphs[a]
    hmtx.metrics[a], hmtx.metrics[b] = hmtx.metrics[b], hmtx.metrics[a]
    if "gvar" in font:
        if va is not None: gv[b] = va
        else: gv.pop(b, None)
        if vb is not None: gv[a] = vb
        else: gv.pop(a, None)
    if "HVAR" in font:
        hv = font["HVAR"].table
        if hv.AdvWidthMap is not None:
            m = hv.AdvWidthMap.mapping
            m[a], m[b] = m.get(b, 0), m.get(a, 0)
        else:
            # mapeo implícito por id de glifo: intercambiamos las filas
            gid = font.getGlyphID
            vd = hv.VarStore.VarData[0]
            ia, ib = gid(a), gid(b)
            vd.Item[ia], vd.Item[ib] = vd.Item[ib], vd.Item[ia]


def lookup_pairs(font, feature_tag):
    gsub = font["GSUB"].table
    pairs = []
    for fr in gsub.FeatureList.FeatureRecord:
        if fr.FeatureTag != feature_tag: continue
        for li in fr.Feature.LookupListIndex:
            lk = gsub.LookupList.Lookup[li]
            for st in lk.SubTable:
                if lk.LookupType == 7: st = st.ExtSubTable
                if hasattr(st, "mapping"): pairs += list(st.mapping.items())
    # sin duplicados, conservando orden
    seen = set(); out = []
    for p in pairs:
        if p not in seen: seen.add(p); out.append(p)
    return out


def add_logo_glyph(font):
    """Marca de Sonkore: barra vertical y dos arcos ("|))"), en un cuerpo de 1 em.
    Dibujada como contornos rellenos; no varía con los ejes."""
    upm = font["head"].unitsPerEm  # 2048
    cap = font["OS/2"].sCapHeight  # 1490
    # Geometría exacta del logo (SVG de 1024): barra x=300 (ancho 104) de y 312 a 712;
    # arco 1: M522 392 A160 -> 522 632  => centro (416.2, 512), ±48.6°
    # arco 2: M628 290 A310 -> 628 734  => centro (411.6, 512), ±45.7°   (grosor 96)
    # La marca abarca y 242..782 y x 248..770: la llevamos a la altura de mayúsculas.
    Y0, Y1, X0, X1 = 242.0, 782.0, 248.0, 770.0
    s = cap / (Y1 - Y0)
    LSB = 100
    def P(x, y):                   # coordenadas del SVG del logo (y hacia abajo) -> fuente
        return (round((x - X0) * s) + LSB, round((Y1 - y) * s))
    pen = TTGlyphPen(None)
    # barra: de (300,312) a (300,712), ancho 104, extremos redondeados
    def round_bar(x, y1, y2, w):
        r = w / 2
        pen.moveTo(P(x - r, y1)); pen.lineTo(P(x - r, y2))
        pen.qCurveTo(P(x - r, y2 + r * .55), P(x - r * .55, y2 + r), P(x, y2 + r))
        pen.qCurveTo(P(x + r * .55, y2 + r), P(x + r, y2 + r * .55), P(x + r, y2))
        pen.lineTo(P(x + r, y1))
        pen.qCurveTo(P(x + r, y1 - r * .55), P(x + r * .55, y1 - r), P(x, y1 - r))
        pen.qCurveTo(P(x - r * .55, y1 - r), P(x - r, y1 - r * .55), P(x - r, y1))
        pen.closePath()
    def arc(cx, cy, R, w, a0=-90, a1=90, n=14):
        # anillo parcial de radio R, grosor w, con extremos redondeados
        ro, ri = R + w / 2, R - w / 2
        pts_o = [(cx + ro * math.cos(math.radians(a)), cy + ro * math.sin(math.radians(a))) for a in [a0 + (a1 - a0) * k / n for k in range(n + 1)]]
        pts_i = [(cx + ri * math.cos(math.radians(a)), cy + ri * math.sin(math.radians(a))) for a in [a1 - (a1 - a0) * k / n for k in range(n + 1)]]
        pen.moveTo(P(*pts_o[0]))
        for q in pts_o[1:]: pen.lineTo(P(*q))
        # tapa redonda en a1
        ex, ey = cx + R * math.cos(math.radians(a1)), cy + R * math.sin(math.radians(a1))
        for k in range(1, 8):
            t = math.radians(a1) + math.pi * k / 8
            pen.lineTo(P(ex + (w / 2) * math.cos(t), ey + (w / 2) * math.sin(t)))
        for q in pts_i: pen.lineTo(P(*q))
        ex, ey = cx + R * math.cos(math.radians(a0)), cy + R * math.sin(math.radians(a0))
        for k in range(1, 8):
            t = math.radians(a0) + math.pi + math.pi * k / 8
            pen.lineTo(P(ex + (w / 2) * math.cos(t), ey + (w / 2) * math.sin(t)))
        pen.closePath()
    round_bar(300, 312, 712, 104)
    arc(416.2, 512, 160, 96, -48.6, 48.6)
    arc(411.6, 512, 310, 96, -45.7, 45.7)
    glyph = pen.glyph()
    name = "sonkore.logo"
    go = font.getGlyphOrder()
    font["glyf"].glyphs[name] = glyph
    width = round((X1 - X0) * s) + 2 * LSB
    font["hmtx"].metrics[name] = (width, LSB)
    font.setGlyphOrder(go + [name])
    font["glyf"].glyphOrder = font.getGlyphOrder()
    # cmap
    for t in font["cmap"].tables:
        if t.isUnicode(): t.cmap[0xE000] = name
    # HVAR: fila nueva sin variación
    if "HVAR" in font:
        hv = font["HVAR"].table
        if hv.AdvWidthMap is not None:
            hv.AdvWidthMap.mapping[name] = 0
        else:
            vd = hv.VarStore.VarData[0]
            vd.Item.append([0] * len(vd.VarRegionIndex)); vd.ItemCount = len(vd.Item)
    # maxp y post se recalculan al guardar
    if "post" in font and font["post"].formatType == 2.0:
        post = font["post"]
        post.extraNames.append(name)
        if post.mapping is None: post.mapping = {}
    if "GDEF" in font and font["GDEF"].table.GlyphClassDef:
        font["GDEF"].table.GlyphClassDef.classDefs[name] = 1
    return name


def set_names(font, family, subfamily, italic):
    name = font["name"]
    ps_family = family.replace(" ", "")
    full = f"{family} {subfamily}" if subfamily != "Regular" else family
    psname = f"{ps_family}-{subfamily.replace(' ', '')}"
    typo_sub = subfamily
    # familia "legacy" (IDs 1/2) con 4 estilos máximo
    legacy_sub = ("Bold Italic" if ("Bold" == subfamily.replace(" Italic", "") and italic) else
                  "Bold" if subfamily == "Bold" else "Italic" if italic else "Regular")
    legacy_fam = family if subfamily in ("Regular", "Bold", "Italic", "Bold Italic") else f"{family} {subfamily.replace(' Italic', '')}"
    records = {
        0: COPYRIGHT,
        1: legacy_fam, 2: legacy_sub,
        3: f"{VERSION};AXKN;{psname}",
        4: full, 5: f"Version {VERSION}", 6: psname,
        7: "Sonkore and the Sonkore logo are trademarks of Axiokyn.",
        8: "Axiokyn", 9: "Axiokyn, based on Inter by Rasmus Andersson",
        10: ("Sonkore Sans is the typeface of the Sonkore Hi-Fi music player. It is derived from Inter "
             "with the tailed l and open digits as defaults for legibility, and a logo glyph at U+E000."),
        11: "https://github.com/Axiokyn/Sonkore", 12: "https://github.com/rsms/inter",
        13: ("This Font Software is licensed under the SIL Open Font License, Version 1.1. "
             "This license is available with a FAQ at: https://openfontlicense.org"),
        14: "https://openfontlicense.org",
        16: family, 17: typo_sub,
    }
    # conservar los nombres de ejes/instancias/features (IDs >= 256) y sus entradas
    keep = [r for r in name.names if r.nameID >= 256]
    name.names = []
    for nid, txt in records.items():
        name.setName(txt, nid, 3, 1, 0x409)
        name.setName(txt, nid, 1, 0, 0)
    for r in keep:
        t = r.toUnicode()
        if "InterVariable" in t or t.startswith("Inter"):
            r.string = t.replace("InterVariable", ps_family + ("-Italic" if italic and "Italic" not in t else "")).replace("Inter ", family + " ")
    name.names += keep
    # el STAT y fvar siguen apuntando a los IDs >= 256, que no tocamos


def swap_defaults(font):
    loc, moved = {}, set()
    for tag in ("cv12", "cv13", "ss03"):
        for a, b in lookup_pairs(font, tag):
            swap_glyphs(font, a, b)
            moved.update((a, b))
            back = {v: k for k, v in loc.items()}
            oa, ob = back.get(a, a), back.get(b, b)
            loc[oa], loc[ob] = b, a
    glyf = font["glyf"]
    for name in moved:   # moved composites: follow their components
        g = glyf[name]
        if g.isComposite():
            for c in g.components:
                c.glyphName = loc.get(c.glyphName, c.glyphName)


def slant_t(font, name):
    """Lower the top-left corner of the stem: San Francisco's cut t."""
    glyf = font["glyf"]
    g = glyf[name]
    if g.isComposite() or g.numberOfContours < 1:
        return False
    coords = g.coordinates
    ends = g.endPtsOfContours
    start = 0
    best = None
    for end in ends:
        idx = list(range(start, end + 1))
        on = [i for i in idx if g.flags[i] & 1]
        top = max(coords[i][1] for i in on)
        tops = [i for i in on if coords[i][1] == top]
        if len(tops) == 2 and (best is None or top > best[0]):
            best = (top, tops)
        start = end + 1
    if not best:
        return False
    a, b = sorted(best[1], key=lambda i: coords[i][0])   # a: top-left, b: top-right
    if "gvar" in font and name in font["gvar"].variations:
        orig = list(coords) + [(0, 0)] * 4               # phantom points
        for tv in font["gvar"].variations[name]:
            if any(c is None for c in tv.coordinates):  # spell out inferred deltas first
                tv.calcInferredDeltas(orig, list(ends) + [len(coords) + i for i in range(4)])
    w0 = coords[b][0] - coords[a][0]
    x, y = coords[a]
    coords[a] = (x, round(coords[b][1] - SLANT * w0))
    if "gvar" in font and name in font["gvar"].variations:
        for tv in font["gvar"].variations[name]:
            d = tv.coordinates
            if d[a] is None or d[b] is None:
                continue
            dbx, dby = d[b]; dax, day = d[a]
            # keep the drop at SLANT stem widths in every master
            d[a] = (dax, round(dby - SLANT * (dbx - dax)))
    return True


def build(src, italic):
    font = TTFont(src)
    swap_defaults(font)
    done = [n for n in ("t", "t.1") if n in font["glyf"].keys() and slant_t(font, n)]
    add_logo_glyph(font)
    set_names(font, FAMILY, "Italic" if italic else "Regular", italic)
    os2 = font["OS/2"]; os2.fsType = 0; os2.achVendID = b"AXKN"
    font["head"].fontRevision = 3.0
    labels = {"cv12": "Wide f (Inter)", "cv13": "Wide t (Inter)", "ss03": "Angular quotes & commas (Inter)"}
    for fr in font["GSUB"].table.FeatureList.FeatureRecord:
        fp = fr.Feature.FeatureParams
        if fr.FeatureTag in labels and fp is not None and hasattr(fp, "UINameID"):
            for r in font["name"].names:
                if r.nameID == fp.UINameID:
                    r.string = labels[fr.FeatureTag]
    print("  slanted t:", done)
    return font


def hint(font):
    import ttfautohint
    buf = io.BytesIO(); font.save(buf)
    out = ttfautohint.ttfautohint(in_buffer=buf.getvalue(), no_info=True, hint_composites=True,
                                  increase_x_height=14, windows_compatibility=True)
    return TTFont(io.BytesIO(out))


STYLES = [("Light", 300), ("Regular", 400), ("Medium", 500), ("SemiBold", 600), ("Bold", 700), ("ExtraBold", 800)]


def statics(base, italic, outdir):
    for display in (False, True):
        for sub, w in STYLES:
            f = instancer.instantiateVariableFont(copy.deepcopy(base), {"wght": w, "opsz": 32 if display else 14},
                                                  updateFontNames=False)
            fam = FAMILY + (" Display" if display else "")
            subn = sub + (" Italic" if italic else "")
            if subn == "Regular Italic":
                subn = "Italic"
            set_names(f, fam, subn, italic)
            f["OS/2"].usWeightClass = w
            sel = f["OS/2"].fsSelection & ~0b1100001
            if sub == "Regular" and not italic:
                sel |= 0b1000000
            if italic:
                sel |= 1
            if w >= 700 and sub == "Bold":
                sel |= 0b100000
            f["OS/2"].fsSelection = sel
            f["head"].macStyle = (2 if italic else 0) | (1 if sub == "Bold" else 0)
            for t in ("STAT", "MVAR"):
                if t in f:
                    del f[t]
            f = hint(f)
            path = os.path.join(outdir, f"{fam.replace(' ', '')}-{subn.replace(' ', '')}.ttf")
            f.save(path)
    print("  statics:", outdir)


def main():
    src, src_i, out = sys.argv[1], sys.argv[2], sys.argv[3]
    os.makedirs(os.path.join(out, "static"), exist_ok=True)
    for path, italic, name in ((src, False, "SonkoreSans[opsz,wght]"), (src_i, True, "SonkoreSans-Italic[opsz,wght]")):
        print(name)
        f = build(path, italic)
        f.save(os.path.join(out, name + ".ttf"))
        w = TTFont(os.path.join(out, name + ".ttf")); w.flavor = "woff2"; w.save(os.path.join(out, name + ".woff2"))
        statics(TTFont(os.path.join(out, name + ".ttf")), italic, os.path.join(out, "static"))
    app_fonts(out)



def app_fonts(outdir):
    """The four weights the app embeds (fonts/sonkore-sans/<weight>.ttf)."""
    app = os.path.join(outdir, "app"); os.makedirs(app, exist_ok=True)
    for style, weight in (("Regular", 400), ("Medium", 500), ("SemiBold", 600), ("Bold", 700)):
        subprocess.run([sys.executable, "-m", "fontTools.subset", os.path.join(outdir, "static", f"SonkoreSans-{style}.ttf"),
                        "--unicodes=" + APP_UNICODES, "--layout-features=*", "--name-IDs=*", "--name-languages=*",
                        "--hinting", "--output-file=" + os.path.join(app, f"{weight}.ttf")], check=True)


if __name__ == "__main__":
    main()
