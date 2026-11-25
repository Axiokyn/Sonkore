#!/usr/bin/env python3
"""A made-up library for the README's screenshots: six albums with gradient
covers and short tagged FLAC files (ffmpeg), in target/readme/musica (made once).

    tools/readme/biblioteca.py                  makes it, prints the folder
    tools/readme/biblioteca.py --perfil DIR     also makes it the only folder of
                                                the ui-live profile DIR (library.json)
"""
import json
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
OUT = REPO / "target/readme/musica"
VERSION = "1"

# artist, album, year, genre, cover colours, bits/rate, track titles
ALBUMS = [
    ("Aurora Vale", "Northern Lights", 2021, "Ambient", ("0x1d2b64", "0x1fa2a1"), (24, 96000),
     ["First Snow", "Magnetic North", "Polar Night", "Driftwood"]),
    ("The Quiet Rooms", "Paper Harbour", 2019, "Indie", ("0xf7971e", "0xd9475f"), (16, 44100),
     ["Lanterns", "Paper Harbour", "Tin Roof Rain", "Late Ferry"]),
    ("Mira Kalen", "Static Bloom", 2023, "Electronic", ("0x5f2c82", "0xf2994a"), (24, 48000),
     ["Signal", "Static Bloom", "Overpass", "Afterglow"]),
    ("Sundial Trio", "Low Tide Sessions", 2018, "Jazz", ("0x134e2c", "0xa8e063"), (24, 192000),
     ["Low Tide", "Brass Compass", "Saltwater Waltz", "Coda"]),
    ("Elm & Ivory", "Glass Cathedral", 2020, "Classical", ("0x0f2027", "0x6dd5fa"), (24, 88200),
     ["Prelude in Glass", "Nave", "Clerestory", "Bells at Dusk"]),
    ("Neon Ferns", "Midnight Arcade", 2024, "Synthwave", ("0xc31432", "0x240b36"), (16, 44100),
     ["Insert Coin", "Midnight Arcade", "High Score", "Game Over"]),
]
NOTES = [220.0, 246.9, 261.6, 293.7, 329.6, 349.2]


def ff(*args):
    subprocess.run(["ffmpeg", "-nostdin", "-loglevel", "error", "-y", *args], check=True)


def make():
    stamp = OUT / ".version"
    if stamp.exists() and stamp.read_text() == VERSION:
        return
    OUT.mkdir(parents=True, exist_ok=True)
    for a, (artist, album, year, genre, (c0, c1), (bits, rate), titles) in enumerate(ALBUMS):
        folder = OUT / artist / f"{year} - {album}"
        folder.mkdir(parents=True, exist_ok=True)
        cover = folder / "cover.png"
        ff("-f", "lavfi", "-i", f"gradients=s=600x600:c0={c0}:c1={c1}:x0=0:y0=0:x1=600:y1=600:n=2:d=1",
           "-frames:v", "1", str(cover))
        for t, title in enumerate(titles, 1):
            f0 = NOTES[(a + t) % len(NOTES)]
            # A soft chord, 40 s, fading in and out.
            expr = f"0.12*(sin(2*PI*{f0}*t)+0.6*sin(2*PI*{f0 * 1.25}*t)+0.5*sin(2*PI*{f0 * 1.5}*t))*min(1,t/2)*min(1,(40-t)/2)"
            expr = expr.replace(",", "\\,")  # commas inside a filter's option
            ff("-f", "lavfi", "-i", f"aevalsrc={expr}|{expr}:s={rate}:d=40", "-i", str(cover),
               "-map", "0:a", "-map", "1:v", "-c:a", "flac", "-sample_fmt", "s32" if bits == 24 else "s16",
               *(["-bits_per_raw_sample", "24"] if bits == 24 else []),
               "-c:v", "png", "-disposition:v", "attached_pic",
               "-metadata", f"title={title}", "-metadata", f"artist={artist}", "-metadata", f"album={album}",
               "-metadata", f"album_artist={artist}", "-metadata", f"genre={genre}",
               "-metadata", f"track={t}", "-metadata", f"date={year}",
               str(folder / f"{t:02d} - {title}.flac"))
    stamp.write_text(VERSION)


def main():
    make()
    args = sys.argv[1:]
    if args[:1] == ["--perfil"]:
        lib = Path(args[1]) / "data/sonkore/library.json"
        data = json.loads(lib.read_text())
        data.update(folders=[str(OUT)], tracks=[])
        lib.write_text(json.dumps(data))
    print(OUT)


if __name__ == "__main__":
    main()
