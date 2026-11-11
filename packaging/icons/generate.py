#!/usr/bin/env python3
"""Regenerates every raster icon from the SVG masters.

Requires Pillow, and ImageMagick (`magick`) with SVG support or, without it,
librsvg's `rsvg-convert`.
  sonkore.svg        - master, used for 64 px and up
  sonkore-small.svg  - the same mark with thicker strokes, used for 16-48 px;
                       each size is drawn from it with its edges moved onto
                       whole pixels
Every PNG is rasterized at its final size (no downscaling, no palette).
Outputs: png/<size>.png and ../windows/sonkore.ico
"""
import pathlib
import re
import shutil
import subprocess

from PIL import Image

HERE = pathlib.Path(__file__).resolve().parent
SIZES = [16, 22, 24, 32, 48, 64, 128, 256, 512]


def small_svg(size: int) -> str:
    """sonkore-small.svg for one pixel size: the squircle, the bar and the
    wave's stroke snapped to whole pixels, so their edges stay sharp instead
    of falling between two pixels (16-48 px, 1 px = 21-64 units)."""
    u = 1024 / size  # units per pixel
    snap = lambda v: round(v / u) * u
    text = (HERE / "sonkore-small.svg").read_text()
    mark = re.search(r'<g fill="none" stroke="([^"]+)"', text).group(1)  # the bar takes the mark's paint
    inset = snap(16)
    text = text.replace('<rect x="16" y="16" width="992" height="992" rx="240"',
                        f'<rect x="{inset:g}" y="{inset:g}" width="{1024 - 2 * inset:g}" height="{1024 - 2 * inset:g}" rx="240"')
    # The bar: a pill on whole pixels (a vertical stroke with round caps).
    bar = re.search(r'<path d="M(\d+) (\d+)V(\d+)" stroke-width="(\d+)"/>', text)
    cx, y0, y1, width = (int(v) for v in bar.groups())
    w = max(2, round(width / u)) * u
    x = snap(cx - w / 2)
    top, bottom = snap(y0 - width / 2), snap(y1 + width / 2)
    text = text.replace(bar.group(0),
                        f'<rect x="{x:g}" y="{top:g}" width="{w:g}" height="{bottom - top:g}" rx="{w / 2:g}" fill="{mark}" stroke="none"/>')
    # The waves: whole-pixel strokes.
    text = re.sub(r'(<path d="M[^"]*A[^"]*" stroke-width=")(\d+)"',
                  lambda m: f'{m.group(1)}{max(2, round(int(m.group(2)) / u)) * u:g}"', text)
    return text


def render(size: int) -> pathlib.Path:
    out = HERE / "png" / f"{size}.png"
    out.parent.mkdir(exist_ok=True)
    if size <= 48:
        master = HERE / "png" / f".small-{size}.svg"
        master.write_text(small_svg(size))
        declared = 64  # width attribute of sonkore-small.svg
    else:
        master = HERE / "sonkore.svg"
        declared = 1024
    # librsvg draws the vector straight at `size`: sharper than drawing larger
    # and scaling down. PNG32 keeps full 8-bit RGBA, never a palette.
    if shutil.which("magick"):
        subprocess.run(["magick", "-limit", "memory", "256MiB", "-limit", "disk", "512MiB",
                        "-background", "none", "-density", f"{96 * size / declared:.4f}", str(master),
                        "-define", "png:compression-level=9", f"PNG32:{out}"], check=True)
    else:
        subprocess.run(["rsvg-convert", "-w", str(size), "-h", str(size), "-o", str(out), str(master)], check=True)
        Image.open(out).convert("RGBA").save(out, optimize=True)
    if size <= 48:
        master.unlink()
    return out


def main() -> None:
    pngs = {size: render(size) for size in SIZES}
    images = {size: Image.open(path).convert("RGBA") for size, path in pngs.items()}

    ico_sizes = [16, 24, 32, 48, 64, 128, 256]
    images[256].save(HERE.parent / "windows" / "sonkore.ico", format="ICO",
                     sizes=[(s, s) for s in ico_sizes],
                     append_images=[images[s] for s in ico_sizes if s != 256])

    print("icons regenerated")


if __name__ == "__main__":
    main()
