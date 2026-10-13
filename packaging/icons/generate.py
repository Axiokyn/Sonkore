#!/usr/bin/env python3
"""Regenerates every raster icon from the SVG masters.

Requires ImageMagick (`magick`) with SVG support and Pillow.
  sonkore.svg        - master, used for 64 px and up
  sonkore-small.svg  - thicker strokes, used for 16-48 px; each size is drawn
                       from it with its edges moved onto whole pixels
Every PNG is rasterized at its final size (no downscaling, no palette).
Outputs: png/<size>.png, ../windows/sonkore.ico, ../macos/sonkore.icns (if that folder exists)
"""
import pathlib
import re
import subprocess

from PIL import Image

HERE = pathlib.Path(__file__).resolve().parent
SIZES = [16, 22, 24, 32, 48, 64, 128, 256, 512, 1024]


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
    # The bar: a pill on whole pixels (was a 140-wide stroke from y 290 to 734).
    w = max(2, round(140 / u)) * u
    x = snap(330 - w / 2)
    top, bottom = snap(290 - 70), snap(734 + 70)
    text = text.replace('<path d="M330 290V734" stroke-width="140"/>',
                        f'<rect x="{x:g}" y="{top:g}" width="{w:g}" height="{bottom - top:g}" rx="{w / 2:g}" fill="{mark}" stroke="none"/>')
    wave = max(2, round(130 / u)) * u
    text = text.replace('stroke-width="130"', f'stroke-width="{wave:g}"')
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
    subprocess.run(["magick", "-limit", "memory", "256MiB", "-limit", "disk", "512MiB",
                    "-background", "none", "-density", f"{96 * size / declared:.4f}", str(master),
                    "-define", "png:compression-level=9", f"PNG32:{out}"], check=True)
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

    macos = HERE.parent / "macos"  # only while the macOS port is in the tree
    if macos.is_dir():
        images[1024].save(macos / "sonkore.icns", format="ICNS",
                          append_images=[images[s] for s in (16, 32, 64, 128, 256, 512)])
    (HERE / "png" / "1024.png").unlink()  # only needed for the .icns
    print("icons regenerated")


if __name__ == "__main__":
    main()
