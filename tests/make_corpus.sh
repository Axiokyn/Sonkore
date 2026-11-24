#!/usr/bin/env bash
# Test files for the Rust engine's tests (ffmpeg and flac only):
#   tests/make_corpus.sh <output dir>
# then: SONKORE_TEST_CORPUS=<output dir> cargo test -p sonkore-engine
set -euo pipefail
out="${1:?output dir}"
mkdir -p "$out/album" "$out/formats"
ff() { ffmpeg -nostdin -loglevel error -y "$@"; }
two_tones() { # rate seconds -> stereo, 440 Hz left and 660 Hz right
    echo "sine=frequency=440:sample_rate=$1:duration=$2[l];sine=frequency=660:sample_rate=$1:duration=$2[r];[l][r]amerge=inputs=2"
}

# 12 s cut into three tracks at sample positions inside FLAC blocks: played
# one after the other they must give the long file back, bit for bit.
ff -filter_complex "$(two_tones 44100 12)" -c:a pcm_s16le "$out/album/long.wav"
cuts=(0 177777 355313 529200)
for i in 0 1 2; do
    ff -i "$out/album/long.wav" -af "atrim=start_sample=${cuts[$i]}:end_sample=${cuts[$((i+1))]}" -c:a pcm_s16le "$out/album/part$((i+1)).wav"
    flac -s -f -o "$out/album/0$((i+1)).flac" "$out/album/part$((i+1)).wav"
done

ff -f lavfi -i "anoisesrc=color=pink:sample_rate=96000:duration=4:amplitude=0.3" -ac 2 -c:a pcm_s24le "$out/formats/hires-24-96.wav"
flac -s -f -o "$out/formats/hires-24-96.flac" "$out/formats/hires-24-96.wav"
ff -filter_complex "$(two_tones 192000 3)" -c:a pcm_s24le "$out/formats/hires-24-192.wav"
ff -filter_complex "$(two_tones 44100 3)" -c:a pcm_f32le "$out/formats/float32.wav"
ff -f lavfi -i "sine=frequency=500:sample_rate=44100:duration=3" -c:a pcm_s16le "$out/formats/mono16.wav"
ff -i "$out/album/part1.wav" -c:a libmp3lame -b:a 192k "$out/formats/mp3.mp3"
ff -i "$out/album/part1.wav" -c:a aac -b:a 192k "$out/formats/aac.m4a"
ff -i "$out/album/part1.wav" -c:a alac "$out/formats/alac.m4a"
ff -i "$out/album/part1.wav" -c:a libvorbis "$out/formats/vorbis.ogg" 2>/dev/null || true
ff -i "$out/album/part1.wav" -c:a pcm_s16be "$out/formats/aiff16.aiff"
echo "corpus in $out"

# A CD image and its sheet (the sheet names a .wav, the image is a .flac).
mkdir -p "$out/cue"
flac -s -f -o "$out/cue/imagen.flac" "$out/album/long.wav"
cat > "$out/cue/imagen.cue" <<'CUE'
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
