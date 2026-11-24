#!/usr/bin/env python3
"""Test tones for the sound scenarios (tools/escenarios/sonido-*.txt), made
once in target/ui-live/tonos (flac for the FLACs):

    01-izquierda.flac   1 kHz at -12 dBFS on the left, the right silent (44.1 kHz, 24 bits)
    02-fuerte.flac      1 kHz at -0.5 dBFS on both sides (44.1 kHz, 24 bits)
    03-constante.flac   a constant 0.25 on both sides: without dither, DSP gives
                        a constant back; with it, LSBs of noise (44.1 kHz, 24 bits)
    04-tono-96k.flac    1 kHz at -12 dBFS on both sides (96 kHz, 24 bits)
    05-replaygain.flac  1 kHz at -12 dBFS on both sides; ReplayGain track -6 dB,
                        album -10 dB, peaks 0.25 (44.1 kHz, 24 bits)
    06-tono.dsf         1 kHz at -6 dB (half the modulation), DSD64, 4 s
    08-agudo.flac       21.5 kHz at -12 dBFS on both sides (44.1 kHz, 24 bits): the
                        resampling filters differ there
    09-rg-alto.flac     1 kHz at -12 dBFS on both sides; ReplayGain track +6 dB,
                        peak 0.25: with +9 dB of preamp it would clip
    10-impulso.wav      an impulse response for the convolution: one sample
                        at 0.5 (-6 dB), 48 kHz mono
    07-hdcd.wav         libhdcd's test file, when the test corpus has it
                        (target/test-corpus/libhdcd/hdcd.wav)

All of them are album "Tonos de prueba", artist "Sonkore", 10 s (the DSF 4 s).

    tools/tonos.py                    makes them (if missing); prints the folder
    tools/tonos.py --biblioteca DIR   also adds the folder to the library of the
                                      profile DIR (DIR/data/sonkore/library.json),
                                      for an app started with rescanOnStartup=true
"""
import json
import math
import shutil
import struct
import subprocess
import sys
import wave
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
OUT = REPO / "target" / "ui-live" / "tonos"
VERSION = "5"
SECONDS = 10


def pcm24(path, rate, left, right):
    """A 24-bit stereo WAV of left(n), right(n) in [-1, 1]."""
    frames = bytearray()
    full = (1 << 23) - 1
    for n in range(rate * SECONDS):
        for v in (left(n), right(n)):
            frames += struct.pack("<i", round(v * full))[:3]
    with wave.open(str(path), "wb") as w:
        w.setnchannels(2)
        w.setsampwidth(3)
        w.setframerate(rate)
        w.writeframes(bytes(frames))


def flac(name, rate, left, right, tags=()):
    wav = OUT / (name + ".wav")
    pcm24(wav, rate, left, right)
    title = name.split("-", 1)[1].replace("-", " ").capitalize()
    tags = [f"TITLE=Tono {title}", "ALBUM=Tonos de prueba", "ARTIST=Sonkore", f"TRACKNUMBER={int(name[:2])}", *tags]
    subprocess.run(["flac", "-s", "-f", "-o", str(OUT / (name + ".flac")), *[a for t in tags for a in ("-T", t)], str(wav)], check=True, stderr=subprocess.DEVNULL)
    wav.unlink()


def sine(rate, db, hz=1000):
    a = 10 ** (db / 20)
    w = 2 * math.pi * hz / rate
    return lambda n: a * math.sin(w * n)


def dsf(path, seconds=4):
    """DSD64 stereo, a second-order sigma-delta of a 1 kHz sine at half the
    modulation, bits LSB first in blocks of 4096 bytes per channel."""
    fs = 2822400
    n = fs * seconds
    w = 2 * math.pi * 1000 / fs
    i1 = i2 = 0.0
    bits = bytearray(n // 8)
    sin = math.sin
    for k in range(n):
        y = 1.0 if i2 >= 0 else -1.0
        i1 += 0.5 * sin(w * k) - y
        i2 += i1 - y
        if y > 0:
            bits[k >> 3] |= 1 << (k & 7)
    block = 4096
    blocks = (len(bits) + block - 1) // block
    padded = bytes(bits) + bytes(blocks * block - len(bits))
    data = b"".join(padded[b * block:(b + 1) * block] * 2 for b in range(blocks))
    fmt = b"fmt " + struct.pack("<QIIIIIIQII", 52, 1, 0, 2, 2, fs, 1, n, block, 0)
    body = fmt + b"data" + struct.pack("<Q", 12 + len(data)) + data
    head = b"DSD " + struct.pack("<QQQ", 28, 28 + len(body), 0)
    path.write_bytes(head + body)


def make():
    stamp = OUT / ".version"
    if stamp.exists() and stamp.read_text() == VERSION:
        return
    shutil.rmtree(OUT, ignore_errors=True)
    OUT.mkdir(parents=True)
    r = 44100
    flac("01-izquierda", r, sine(r, -12), lambda n: 0.0)
    flac("02-fuerte", r, sine(r, -0.5), sine(r, -0.5))
    flac("03-constante", r, lambda n: 0.25, lambda n: 0.25)
    flac("04-tono-96k", 96000, sine(96000, -12), sine(96000, -12))
    flac("05-replaygain", r, sine(r, -12), sine(r, -12), [
        "REPLAYGAIN_TRACK_GAIN=-6.00 dB", "REPLAYGAIN_TRACK_PEAK=0.251189",
        "REPLAYGAIN_ALBUM_GAIN=-10.00 dB", "REPLAYGAIN_ALBUM_PEAK=0.251189"])
    flac("08-agudo", r, sine(r, -12, 21500), sine(r, -12, 21500))
    flac("09-rg-alto", r, sine(r, -12), sine(r, -12), [
        "REPLAYGAIN_TRACK_GAIN=+6.00 dB", "REPLAYGAIN_TRACK_PEAK=0.251189"])
    with wave.open(str(OUT / "10-impulso.wav"), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(48000)
        w.writeframes(struct.pack("<h", 16384) + bytes(2 * 255))
    dsf(OUT / "06-tono.dsf")
    hdcd = REPO / "target/test-corpus/libhdcd/hdcd.wav"
    if hdcd.exists():
        shutil.copy(hdcd, OUT / "07-hdcd.wav")
    stamp.write_text(VERSION)


def main():
    make()
    args = sys.argv[1:]
    if args[:1] == ["--biblioteca"]:
        lib = Path(args[1]) / "data/sonkore/library.json"
        data = json.loads(lib.read_text())
        if str(OUT) not in data["folders"]:
            data["folders"].append(str(OUT))
        lib.write_text(json.dumps(data))
    print(OUT)


if __name__ == "__main__":
    main()
