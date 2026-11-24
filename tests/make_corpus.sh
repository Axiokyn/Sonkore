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

# --- The formats only FFmpeg decodes (crates/ffmpeg, its tests and the
# engine's and library's ffmpeg tests). Lengths that are no multiple of any
# block size; each source WAV next to its encodings, for bit-exact checks.
F="$out/ffmpeg"
mkdir -p "$F"
noise() { # rate frames sample_fmt -> stereo pink noise with a tone, exactly <frames> long
    ff -filter_complex "anoisesrc=color=pink:sample_rate=$1:duration=$(( $2 / $1 + 1 )):amplitude=0.4:seed=7,aformat=channel_layouts=stereo[n];sine=frequency=997:sample_rate=$1:duration=$(( $2 / $1 + 1 ))[s];[n][s]amix=inputs=2:normalize=0,atrim=end_sample=$2" \
        -c:a "$3" "$4"
}
noise 44100 220501 pcm_s16le "$F/src-16-44.wav"
noise 96000 300007 pcm_s24le "$F/src-24-96.wav"
noise 48000 240013 pcm_s16le "$F/src-16-48.wav"
tags=(-metadata title="Título FFmpeg" -metadata artist="Artista FF" -metadata album="Álbum FF" -metadata track=3 -metadata date=2021 -metadata genre=Jazz)
ff -i "$F/src-16-44.wav" -c:a wavpack "${tags[@]}" "$F/wavpack-16.wv"
ff -i "$F/src-24-96.wav" -c:a wavpack -bits_per_raw_sample 24 "$F/wavpack-24.wv"
ff -i "$F/src-16-44.wav" -c:a tta "${tags[@]}" "$F/tta-16.tta"
ff -i "$F/src-24-96.wav" -c:a tta "$F/tta-24.tta"
ff -i "$F/src-24-96.wav" -c:a pcm_s24le -f w64 "$F/pcm-24.w64"
ff -i "$F/src-16-48.wav" -c:a libopus -b:a 128k "${tags[@]}" "$F/opus.opus"
ff -i "$F/src-16-44.wav" -c:a wmav2 -b:a 192k "${tags[@]}" "$F/wma.wma"
ff -i "$F/src-16-48.wav" -c:a ac3 -b:a 384k "$F/ac3.ac3"
ff -i "$F/src-16-48.wav" -c:a dca -strict -2 "$F/dts.dts" 2>/dev/null || true
ff -i "$F/src-16-48.wav" -c:a eac3 -b:a 256k "$F/eac3.eac3" 2>/dev/null || true
# Opus in an .ogg (symphonia opens Ogg but has no Opus decoder) and in WebM.
ff -i "$F/src-16-48.wav" -c:a libopus -b:a 128k "$F/opus-in-ogg.ogg"
ff -i "$F/src-16-48.wav" -c:a libopus -b:a 128k "$F/opus.webm"
# A WavPack CD image with its sheet (the sheet names a .wav).
mkdir -p "$F/cue"
ff -i "$out/album/long.wav" -c:a wavpack "$F/cue/imagen.wv"
sed 's/^PERFORMER .*/PERFORMER "Banda WV"/' "$out/cue/imagen.cue" > "$F/cue/imagen.cue"
# The album's three tracks as WavPack: gapless through FFmpeg too.
mkdir -p "$F/album"
for i in 1 2 3; do ff -i "$out/album/part$i.wav" -c:a wavpack "$F/album/0$i.wv"; done
