#!/usr/bin/env python3
"""Checks a SONKORE_FRAME_DUMP log: the first frame a window draws at a new
size (another monitor's scale, a resize) has to be drawn whole, the whole
window as damage. (Its buffer's age may be 2: the other buffer can already
hold a frame of that size from before.)

softbuffer kept the buffer's age when it changed its size: Sonkore then
repainted only what had changed on top of the old image read with another
width, and on KWin, with one monitor at 100 % and another at 150 %, the window
was left striped, with shifted copies of itself (escenarios/escala-monitor.txt).

    tools/cuadros-enteros.py CARPETA/frames.log
"""
import re
import sys

LINE = re.compile(r'^(\d+) "(.*)" (\d+)x(\d+) scale (\S+) age (\d+) (\S*) ?(\[.*\])$')


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    last = {}
    bad, resized = [], 0
    for line in open(sys.argv[1], encoding="utf-8"):
        m = LINE.match(line.strip())
        if not m:
            continue
        seq, window, w, h, scale, age, _, damage = m.groups()
        size = (int(w), int(h))
        if window in last and last[window] != size:
            resized += 1
            whole = f"[(0, 0, {w}, {h})" in damage
            if not whole:
                bad.append(f"cuadro {seq}, {window}: {last[window][0]}x{last[window][1]} → {w}x{h} "
                           f"(escala {scale}) con edad {age} y daño {damage[:80]}")
        last[window] = size
    if bad:
        print("Cuadros con tamaño nuevo dibujados a medias:\n  " + "\n  ".join(bad), file=sys.stderr)
        return 1
    if resized == 0:
        print("Ninguna ventana cambió de tamaño: la prueba no probó nada", file=sys.stderr)
        return 1
    print(f"{resized} cambios de tamaño, todos dibujados enteros")
    return 0


if __name__ == "__main__":
    sys.exit(main())
