#!/usr/bin/env bash
# Builds the test corpus for engine_test: deterministic signals in every
# format the engine reads, a gapless album, a CD image with its sheet, DSD,
# multichannel and damaged files. Needs sox, flac, ffmpeg and python3.
#   tests/make_corpus.sh <output dir>
set -euo pipefail
export LC_ALL=C.UTF-8
out="${1:?output dir}"
here="$(cd "$(dirname "$0")" && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$out/Álbum Uno" "$out/Formatos" "$out/CUE" "$out/Rotos" "$out/DSD" "$out/Multicanal" "$out/IR"
cd "$out"

# 12 s of two tones, cut at sample positions that fall inside FLAC blocks.
sox -R -n -r 44100 -b 16 -c 2 "$tmp/long.wav" synth 12 sine 440 sine 660 vol 0.4
python3 - "$tmp" <<'PY'
import sys, wave
tmp = sys.argv[1]
w = wave.open(f"{tmp}/long.wav"); n = w.getnframes(); data = w.readframes(n); p = w.getparams()
cuts = [0, 177777, 355313, n]
for i in range(3):
    o = wave.open(f"{tmp}/part{i+1}.wav", "wb"); o.setparams(p)
    o.writeframes(data[cuts[i]*4:cuts[i+1]*4]); o.close()
PY
for i in 1 2 3; do
    flac -s -f -o "Álbum Uno/0$i - Pista número $i.flac" -T "TITLE=Pista $i" -T "ARTIST=Artista Ñ" \
        -T "ALBUM=Álbum Uno" -T TRACKNUMBER=$i -T DATE=2024 -T GENRE=Test "$tmp/part$i.wav"
done

sox -R -n -r 96000 -b 24 -c 2 Formatos/hires-24-96.flac synth 4 pinknoise vol 0.3
sox -R -n -r 192000 -b 24 -c 2 Formatos/hires-24-192.wav synth 3 sine 1000 vol 0.5
sox -R -n -r 44100 -e floating-point -b 32 -c 2 Formatos/float32.wav synth 3 sine 300 vol 0.5
sox -R -n -r 44100 -b 16 -c 1 Formatos/mono16.wav synth 3 sine 500 vol 0.5
sox -R -n -r 88200 -b 24 -c 2 Formatos/r882.flac synth 3 sine 2000 vol 0.5
sox -R -n -r 44100 -b 16 -c 2 Formatos/aiff16.aiff synth 3 sine 700 vol 0.5
ff() { ffmpeg -nostdin -loglevel error -y -i "$tmp/part1.wav" "$@"; }
ff -c:a libmp3lame -b:a 192k Formatos/mp3.mp3
ff -c:a libopus -b:a 128k Formatos/opus.opus
ff -c:a libvorbis Formatos/vorbis.ogg
ff -c:a alac Formatos/alac.m4a
ff -c:a aac -b:a 192k Formatos/aac.m4a
ff -c:a wavpack Formatos/wavpack.wv
ff -c:a tta Formatos/tta.tta

sox -R -n -r 48000 -b 24 -c 6 Multicanal/51.flac synth 3 sine 200 sine 300 sine 400 sine 50 sine 600 sine 700 vol 0.3
sox -R -n -r 48000 -b 24 -c 8 Multicanal/71.wav synth 3 sine 200 sine 300 sine 400 sine 50 sine 600 sine 700 sine 800 sine 900 vol 0.3

# CD image: the sheet names a .wav, the image is a .flac (converted after ripping).
flac -s -f -o CUE/imagen.flac "$tmp/long.wav"
cat > CUE/imagen.cue <<'CUE'
REM GENRE Rock
REM DATE 1999
PERFORMER "Banda CUE"
TITLE "Disco CUE"
FILE "imagen.wav" WAVE
  TRACK 01 AUDIO
    TITLE "Uno"
    INDEX 01 00:00:00
  TRACK 02 AUDIO
    TITLE "Dos"
    INDEX 00 00:03:50
    INDEX 01 00:04:00
  TRACK 03 AUDIO
    TITLE "Tres"
    INDEX 01 00:08:37
CUE

python3 "$here/mkdsf.py" DSD/dsd64.dsf 2 2822400
python3 "$here/mkdsf.py" DSD/dsd128.dsf 1 5644800

: > Rotos/vacio.flac
head -c 80000 "Álbum Uno/01 - Pista número 1.flac" > Rotos/truncado.flac # past the padding: some frames, then nothing
python3 -c "import random,sys; random.seed(7); sys.stdout.buffer.write(bytes(random.getrandbits(8) for _ in range(20000)))" > Rotos/basura.mp3
echo hola > Rotos/texto.wav

# A unit impulse (a filter that changes nothing) for the convolution engine.
python3 - <<'PY'
import struct, wave
w = wave.open("IR/dirac96.wav", "wb"); w.setnchannels(1); w.setsampwidth(3); w.setframerate(96000)
frames = bytearray(3 * 256); frames[0:3] = struct.pack("<i", (1 << 23) - 1)[:3]
w.writeframes(bytes(frames)); w.close()
PY
echo "corpus ready: $out"
