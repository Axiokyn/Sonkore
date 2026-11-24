"""Compares captures with their base and saves crops of what changed:
    python3 ui-diff.py <base dir> <captures dir> <changes dir> <view>...
Each changed area is saved as one image, the base above and the capture
below, at most 900 px wide; a view without a base (or with another size)
is saved whole, shrunk. Prints one line per view that changed.
    python3 ui-diff.py --hoja <captures dir> <out.png>: all of them on one sheet.
Before the dirs: --tolerancia N ignores channel differences up to N (two
renderers antialias apart), --minimo N drops areas with fewer than N
differing pixels."""
import sys
from pathlib import Path

from PIL import Image, ImageChops

CELL = 24  # changed pixels are grouped in cells this big
TOLERANCE = 0
MIN_PIXELS = 1
LIMIT = 6  # crops saved per view (--zonas N)
COARSE = 1  # compared at 1/COARSE size: layout and color, not glyph rendering
PAD = 12
MAX_W = 900


def shrink(img):
    if img.width > MAX_W:
        img = img.resize((MAX_W, round(img.height * MAX_W / img.width)), Image.LANCZOS)
    return img


def areas(diff):
    """Bounding boxes of the groups of changed cells (8-connected)."""
    w, h = diff.size
    cells = set()
    bbox = diff.getbbox()
    if not bbox:
        return []
    x0, y0, x1, y1 = bbox
    for cy in range(y0 // CELL, (y1 - 1) // CELL + 1):
        for cx in range(x0 // CELL, (x1 - 1) // CELL + 1):
            box = (cx * CELL, cy * CELL, min(w, (cx + 1) * CELL), min(h, (cy + 1) * CELL))
            if diff.crop(box).getbbox():
                cells.add((cx, cy))
    out = []
    while cells:
        stack = [cells.pop()]
        gx0 = gx1 = stack[0][0]
        gy0 = gy1 = stack[0][1]
        while stack:
            cx, cy = stack.pop()
            gx0, gx1, gy0, gy1 = min(gx0, cx), max(gx1, cx), min(gy0, cy), max(gy1, cy)
            for dx in (-1, 0, 1):
                for dy in (-1, 0, 1):
                    n = (cx + dx, cy + dy)
                    if n in cells:
                        cells.remove(n)
                        stack.append(n)
        box = (gx0 * CELL, gy0 * CELL, min(w, (gx1 + 1) * CELL), min(h, (gy1 + 1) * CELL))
        # Tighten to the changed pixels, then pad.
        b = diff.crop(box).getbbox()
        box = (max(0, box[0] + b[0] - PAD), max(0, box[1] + b[1] - PAD),
               min(w, box[0] + b[2] + PAD), min(h, box[1] + b[3] + PAD))
        if sum(diff.crop(box).histogram()[255:]) >= MIN_PIXELS:
            out.append(box)
    return sorted(out, key=lambda b: (b[1], b[0]))


def sheet(caps, out):
    """Every capture as a 300 px thumbnail with its name, five per row."""
    from PIL import ImageDraw
    thumbs = []
    for f in sorted(caps.glob("*.png")):
        im = Image.open(f).convert("RGB")
        im.thumbnail((300, 300))
        thumbs.append((f.stem, im))
    rows = (len(thumbs) + 4) // 5
    cell_h = max(t.height for _, t in thumbs) + 14
    img = Image.new("RGB", (1500, rows * cell_h), "white")
    draw = ImageDraw.Draw(img)
    for i, (name, t) in enumerate(thumbs):
        x, y = (i % 5) * 300, (i // 5) * cell_h
        img.paste(t, (x, y + 14))
        draw.text((x + 2, y), name, fill="black")
    img.save(out)
    print(out)


def shift_of(a, b, reach=4):
    """The (dx, dy) moving b's content onto a's best, and how much is left
    (mean channel difference) after it, against before."""
    ga, gb = a.convert("L"), b.convert("L")
    w, h = ga.size
    if w <= 2 * reach or h <= 2 * reach:
        return None
    inner = (reach, reach, w - reach, h - reach)
    ref = ga.crop(inner)

    def cost(dx, dy):
        moved = gb.crop((reach + dx, reach + dy, w - reach + dx, h - reach + dy))
        hist = ImageChops.difference(ref, moved).histogram()
        return sum(i * n for i, n in enumerate(hist)) / max(1, sum(hist))

    before = cost(0, 0)
    best = min(((cost(dx, dy), dx, dy) for dx in range(-reach, reach + 1) for dy in range(-reach, reach + 1)))
    return best[1], best[2], before, best[0]


def main():
    global TOLERANCE, MIN_PIXELS
    if sys.argv[1] == "--hoja":
        return sheet(Path(sys.argv[2]), Path(sys.argv[3]))
    global COARSE, LIMIT
    while sys.argv[1] in ("--tolerancia", "--minimo", "--grueso", "--zonas"):
        flag, value = sys.argv.pop(1), int(sys.argv.pop(1))
        if flag == "--tolerancia":
            TOLERANCE = value
        elif flag == "--grueso":
            COARSE = value
        elif flag == "--zonas":
            LIMIT = value
        else:
            MIN_PIXELS = value
    base, caps, changes = map(Path, sys.argv[1:4])
    same, changed, failed = 0, [], []
    for name in sys.argv[4:]:
        cap = caps / f"{name}.png"
        if not cap.exists():
            failed.append(name)
            continue
        now = Image.open(cap).convert("RGBA")
        old_path = base / f"{name}.png"
        old = Image.open(old_path).convert("RGBA") if old_path.exists() else None
        if old is None or old.size != now.size:
            why = "sin base" if old is None else f"tamaño {old.size} → {now.size}"
            shrink(now).save(changes / f"{name}-entera.png")
            changed.append(f"{name}: {why} ({name}-entera.png)")
            continue
        if COARSE > 1:
            size = (old.width // COARSE, old.height // COARSE)
            small = ImageChops.difference(old.resize(size, Image.BOX), now.resize(size, Image.BOX))
            mask = small.convert("L").point(lambda v: 255 if v > TOLERANCE else 0).resize(old.size, Image.NEAREST)
        else:
            mask = ImageChops.difference(old, now).convert("L").point(lambda v: 255 if v > TOLERANCE else 0)
        boxes = areas(mask)
        if not boxes:
            same += 1
            continue
        files = []
        for i, box in enumerate(boxes[:LIMIT], 1):
            a, b = old.crop(box), now.crop(box)
            pair = Image.new("RGBA", (a.width, a.height * 2 + 4), (255, 0, 255, 255))
            pair.paste(a, (0, 0))
            pair.paste(b, (0, a.height + 4))
            f = f"{name}-{i}.png"
            shrink(pair).save(changes / f)
            note = ""
            if COARSE > 1:
                # Rendered by two engines: say how far the content moved.
                found = shift_of(a, b)
                if found and (found[0] or found[1]) and found[3] < found[2] * 0.5:
                    note = f" (Rust corrido {found[0]:+d},{found[1]:+d} px)"
            files.append(f"{f} {box[2] - box[0]}x{box[3] - box[1]} en {box[0]},{box[1]}{note}")
        more = f" (+{len(boxes) - LIMIT} zonas más)" if len(boxes) > LIMIT else ""
        changed.append(f"{name}: " + "; ".join(files) + more)
    print(f"{same} iguales, {len(changed)} cambiaron, {len(failed)} sin captura")
    for line in changed:
        print("  " + line)
    for name in failed:
        print(f"  {name}: no hay captura")


main()
