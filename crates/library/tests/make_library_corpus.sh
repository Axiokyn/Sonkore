#!/usr/bin/env bash
# Tagged files for the library's tests (ffmpeg, flac/metaflac, lame, wavpack,
# python3):
#   crates/library/tests/make_library_corpus.sh <output dir> [perf copies]
# then: SONKORE_LIBRARY_CORPUS=<output dir> cargo test -p sonkore-library
# <output dir>/Music is the watched folder, <output dir>/Extra holds files
# added one by one. With [perf copies], <output dir>/Perf gets that many small
# FLAC files (scan speed).
set -euo pipefail
out="${1:?output dir}"
perf="${2:-0}"
here="$(cd "$(dirname "$0")" && pwd)"
tools() { python3 -I "$here/corpus_tools.py" "$@"; }
ff() { ffmpeg -nostdin -loglevel error -y "$@"; }
fl() { flac -s -f "$@" 2>/dev/null; }
rm -rf "$out"
M="$out/Music"
mkdir -p "$out/tmp" "$M/Artist A/2019 - Album One" "$M/Formats" "$M/Cue" "$M/Cue2" "$M/Música/Ñandú" \
    "$M/Skip me/sub" "$M/short" "$M/Empty" "$out/Extra" "$out/Outside/linked album"
T="$out/tmp"

tone() { # rate seconds channels -> wav
    ff -f lavfi -i "sine=frequency=$4:sample_rate=$1:duration=$2" -ac "$3" -c:a pcm_s16le "$5"
}
tone 44100 3 2 440 "$T/a.wav"
tone 44100 3 1 550 "$T/mono.wav"
ff -f lavfi -i "anoisesrc=color=pink:sample_rate=96000:duration=3:amplitude=0.3" -ac 2 -c:a pcm_s24le "$T/hires.wav"
ff -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=3" -ac 2 -c:a pcm_s16le "$T/a48.wav"
ff -f lavfi -i "sine=frequency=440:sample_rate=22050:duration=3" -ac 2 -c:a pcm_s16le "$T/a22.wav"
ff -f lavfi -i "color=c=red:s=600x400:d=1" -frames:v 1 "$T/front.jpg"
ff -f lavfi -i "color=c=blue:s=300x300:d=1" -frames:v 1 "$T/back.jpg"
ff -f lavfi -i "color=c=green:s=200x200:d=1" -frames:v 1 "$T/green.png"

# --- Album One: FLAC with Vorbis comments ---------------------------------------
A="$M/Artist A/2019 - Album One"
for n in 1 2 3; do
    fl -o "$A/0$n - Song $n.flac" "$T/a.wav"
    metaflac --remove-all-tags --set-tag="TITLE=Song $n" --set-tag="ARTIST=Artist A" --set-tag="ALBUM=Album One" \
        --set-tag="TRACKNUMBER=$n/3" --set-tag="DISCNUMBER=1/1" --set-tag="DATE=2019-04-01" --set-tag="GENRE=Rock" \
        "$A/0$n - Song $n.flac"
done
metaflac --set-tag="REPLAYGAIN_TRACK_GAIN=-6.54 dB" --set-tag="REPLAYGAIN_TRACK_PEAK=0.988" \
    --set-tag="REPLAYGAIN_ALBUM_GAIN=-7.10 dB" --set-tag="REPLAYGAIN_ALBUM_PEAK=0.999" "$A/01 - Song 1.flac"
metaflac --import-picture-from="4||back||$T/back.jpg" --import-picture-from="3||front||$T/front.jpg" "$A/02 - Song 2.flac"
metaflac --set-tag="albumartist=Various" --set-tag="ARTIST=Second artist" --set-tag="Year=1999" "$A/03 - Song 3.flac"
cp "$T/front.jpg" "$A/Folder.JPG"
cp "$T/green.png" "$A/zzz.png"

# --- Formats ----------------------------------------------------------------------
F="$M/Formats"
fl -o "$F/hires.flac" "$T/hires.wav"
metaflac --set-tag="TITLE=Hi-res" --set-tag="ARTIST=Noise" --set-tag="TRACKNUMBER=7" "$F/hires.flac"
fl -o "$F/mono.flac" "$T/mono.wav"
fl -o "$F/untagged.flac" "$T/a.wav"
fl -o "$F/id3first.flac" "$T/a.wav"
metaflac --set-tag="TITLE=Vorbis title" "$F/id3first.flac"
tools id3 "$T/tag.id3" 3 "TIT2=ID3 title" "TALB=ID3 album"
tools prepend "$T/tag.id3" "$F/id3first.flac"
cp "$F/untagged.flac" "$F/UPPER CASE.FLAC"

# MP3: lame CBR with ID3v2.3 (ffmpeg), VBR (Xing), MPEG-2, mono, v2.4, v2.2
ff -i "$T/a.wav" -i "$T/front.jpg" -map 0 -map 1 -c:a libmp3lame -b:a 192k -c:v copy -id3v2_version 3 \
    -metadata title="MP3 título" -metadata artist="Ärtist" -metadata album_artist="Album Ärtist" -metadata album="MP3 Album" \
    -metadata genre="(17)Rock" -metadata track="5/9" -metadata disc="2" -metadata date="2004" \
    -metadata REPLAYGAIN_TRACK_GAIN="-3.21 dB" -metadata REPLAYGAIN_TRACK_PEAK="0.5" "$F/cbr-v23.mp3"
lame --quiet -V 2 "$T/a.wav" "$F/vbr.mp3"
lame --quiet -b 64 "$T/a22.wav" "$F/mpeg2.mp3"
lame --quiet -m m -b 96 "$T/mono.wav" "$F/mono.mp3"
lame --quiet -b 128 -t "$T/a48.wav" "$T/noxing.mp3"   # -t: no Xing/Info frame
tools id3 "$T/v24.id3" 4 ext "TIT2=Four|Second value" "TPE1=V4 artist" "TDRC=2011-02-03" "TRCK=11" "TCON=(8)" \
    "TXXX:replaygain_album_gain=+1.5 dB" "TXXX:REPLAYGAIN_ALBUM_PEAK=0.75" "APIC:0=$T/back.jpg" "APIC=$T/front.jpg"
cp "$T/noxing.mp3" "$F/v24.mp3"; tools prepend "$T/v24.id3" "$F/v24.mp3"
tools id3 "$T/v22.id3" 2 "TT2=Two two" "TP1=V2 artist" "TAL=V2 album" "TYE=1987" "TRK=4/10" "PIC=$T/front.jpg"
cp "$T/noxing.mp3" "$F/v22.mp3"; tools prepend "$T/v22.id3" "$F/v22.mp3"
tools id3 "$T/v23be.id3" 3 "TIT2=be:Big endian" "TPE2=Album artist 23" "TPOS=3/4" "TYER=2001" "TDRC=1990"
cp "$T/noxing.mp3" "$F/v23be.mp3"; tools prepend "$T/v23be.id3" "$F/v23be.mp3"
cp "$T/noxing.mp3" "$F/noxing.mp3"

# WAV: LIST/INFO, id3 chunk, float, 24-bit extensible, 5.1
ff -i "$T/a.wav" -c:a pcm_s16le -metadata title="Wave title" -metadata artist="Wave artist" -metadata album="Wave album" \
    -metadata genre="Ambient" -metadata date="2015" -metadata track="2" "$F/info.wav"
ff -i "$T/a.wav" -c:a pcm_s16le -write_id3v2 1 -metadata title="WAV id3" -metadata artist="Id3 artist" "$F/id3.wav"
ff -i "$T/a.wav" -c:a pcm_f32le "$F/float.wav"
ff -i "$T/hires.wav" -c:a pcm_s24le "$F/24bit.wav"
ff -f lavfi -i "sine=frequency=300:sample_rate=48000:duration=2" -af "pan=5.1|c0=c0|c1=c0|c2=c0|c3=c0|c4=c0|c5=c0" \
    -c:a pcm_s16le "$F/surround.wav"
ff -i "$T/a.wav" -c:a pcm_f32le -ac 2 -f wav "$F/float-auto.wave"

# MP4: AAC and ALAC, iTunes atoms, cover, free-form ReplayGain
ff -i "$T/a.wav" -i "$T/front.jpg" -map 0 -map 1 -c:a aac -b:a 160k -c:v copy -disposition:v attached_pic \
    -metadata title="AAC title" -metadata artist="AAC artist" -metadata album_artist="AAC album artist" \
    -metadata album="AAC album" -metadata genre="Jazz" -metadata track="3/10" -metadata disc="1/2" -metadata date="2020-05-05" \
    "$F/aac.m4a"
tools mp4free "$F/aac.m4a" "replaygain_track_gain=-2.00 dB" "replaygain_track_peak=0.891" "REPLAYGAIN_ALBUM_GAIN=-2.5 dB"
ff -i "$T/a.wav" -c:a alac -metadata title="ALAC 16" -metadata artist="ALAC artist" "$F/alac16.m4a"
ff -i "$T/hires.wav" -c:a alac -metadata title="ALAC 24" "$F/alac24.m4a"
ff -i "$T/a.wav" -c:a aac -b:a 96k -metadata title="Book" -metadata album="Audiobook" "$F/book.m4b"

# Ogg: Vorbis, Opus (R128), FLAC in Ogg, Speex
ff -i "$T/a.wav" -c:a libvorbis -q:a 4 -metadata TITLE="Vorbis" -metadata ARTIST="Ogg artist" -metadata ALBUMARTIST="Ogg AA" \
    -metadata TRACKNUMBER="8" -metadata DISCNUMBER="2/3" -metadata DATE="1977" -metadata ALBUM="Ogg album" "$F/vorbis.ogg"
tools oggpic "$F/vorbis.ogg" "$T/front.jpg"
ff -i "$T/a48.wav" -c:a libopus -b:a 96k -metadata TITLE="Opus" -metadata ARTIST="Opus artist" \
    -metadata R128_TRACK_GAIN="-1280" -metadata R128_ALBUM_GAIN="256" "$F/opus.opus"
ff -i "$T/a.wav" -c:a flac -metadata TITLE="Ogg FLAC" "$F/flac.oga"
ff -i "$T/a22.wav" -ac 1 -ar 16000 -c:a libspeex -metadata TITLE="Speex" "$F/speex.ogg"

# AIFF (ID3 chunk), AIFC, W64, CAF
ff -i "$T/a.wav" -c:a pcm_s16be -write_id3v2 1 -id3v2_version 3 -metadata title="AIFF title" -metadata artist="AIFF artist" \
    -metadata album="AIFF album" -metadata track="1/2" "$F/aiff.aiff"
ff -i "$T/hires.wav" -c:a pcm_s24be "$F/24bit.aif"
ff -i "$T/a.wav" -c:a pcm_f32be "$F/float.aifc" 2>/dev/null || ff -i "$T/a.wav" -c:a pcm_s16be -f aiff "$F/float.aifc"
ff -i "$T/a.wav" -c:a pcm_s16le "$F/wave64.w64"
ff -i "$T/a.wav" -c:a alac -metadata title="CAF title" "$F/caf.caf"

# APEv2-tagged: WavPack, TTA; ID3v1
wavpack -q -y -w "Title=WavPack title" -w "Artist=WV artist" -w "Album Artist=WV AA" -w "Album=WV album" \
    -w "Track=4/12" -w "Year=2008" -w "Genre=Blues" -w "REPLAYGAIN_TRACK_GAIN=+0.42 dB" "$T/a.wav" -o "$F/wavpack.wv"
wavpack -q -y "$T/hires.wav" -o "$F/hires.wv"
ff -i "$T/a.wav" -c:a tta -metadata title="TTA title" -metadata artist="TTA artist" "$F/tta.tta"
ff -i "$T/a.wav" -c:a wavpack "$T/plain.wv"
cp "$T/plain.wv" "$F/id3v1.wv"; tools id3v1 "$F/id3v1.wv" "V1 title" "V1 artist" "V1 album" "1995" 6 17

# DSD
tools id3 "$T/dsd.id3" 3 "TIT2=DSD title" "TPE1=DSD artist" "TALB=DSD album" "TRCK=1"
tools dsf "$F/dsd64.dsf" 2822400 2 2 "$T/dsd.id3"
tools dsf "$F/dsd128.dsf" 5644800 2 1
tools dff "$F/dsd64.dff" 2822400 2 2 "$T/dsd.id3"

# Matroska / WebM, WMA, raw AC-3 / DTS / MP2 / ADTS
ff -i "$T/a48.wav" -c:a flac -metadata title="MKA title" -metadata artist="MKA artist" -metadata ARTIST_SORT="x" "$F/flac.mka"
ff -i "$T/a48.wav" -c:a libopus -b:a 64k -metadata title="WebM title" "$F/opus.webm"
ff -i "$T/a.wav" -c:a wmav2 -b:a 128k -metadata title="WMA title" -metadata artist="WMA artist" \
    -metadata album="WMA album" -metadata track="9" -metadata date="2003" -metadata album_artist="WMA AA" "$F/wma.wma"
ff -i "$T/a48.wav" -c:a ac3 -b:a 192k "$F/ac3.ac3"
ff -i "$T/a48.wav" -c:a dca -strict -2 "$F/dts.dts" 2>/dev/null || true
ff -i "$T/a48.wav" -c:a mp2 -b:a 192k -metadata title="MP2 title" "$F/mp2.mp2"
ff -i "$T/a.wav" -c:a aac -b:a 128k "$F/adts.aac"

# Edge cases: repeated keys, tags at the start of formats with their own,
# pictures in Ogg / APE / Matroska, DSDIFF text chunks, 7.1
X="$M/Edge"
mkdir -p "$X"
fl -o "$X/two-artists.flac" "$T/a.wav"
metaflac --set-tag="ARTIST=First" --set-tag="ARTIST=Second" --set-tag="artist=third" --set-tag="TITLE=Two artists" \
    --set-tag="YEAR=1971" --set-tag="DATE=1999" "$X/two-artists.flac"
ff -i "$T/a.wav" -c:a libvorbis -metadata ARTIST="One" -metadata TITLE="Ogg two" "$X/two-artists.ogg"
python3 -I - "$X/two-artists.ogg" <<'PY'
import sys
p = sys.argv[1]
d = bytearray(open(p, "rb").read())
# Rename the "ENCODER" comment to a second ARTIST (same length): FFmpeg joins them with ";".
i = d.find(b"encoder=")
if i > 0:
    d[i:i + 8] = b"ARTIST=x"[:8]
open(p, "wb").write(bytes(d))
PY
tools oggpic "$X/two-artists.ogg" "$T/back.jpg" || true
ff -i "$T/a48.wav" -c:a libopus -b:a 64k -metadata TITLE="Opus pic" "$X/pic.opus"
tools oggpic "$X/pic.opus" "$T/front.jpg"
ff -i "$T/a.wav" -c:a tta "$X/id3.tta"
tools id3 "$T/tta.id3" 3 "TIT2=TTA id3 title" "TPE1=TTA id3 artist" "TCON=(8)"
tools prepend "$T/tta.id3" "$X/id3.tta"
ff -i "$T/a.wav" -c:a aac -b:a 128k "$X/id3.aac"
tools id3 "$T/aac.id3" 4 "TIT2=ADTS title" "TPE1=ADTS artist" "TCON=17" "TXXX:REPLAYGAIN_TRACK_GAIN=-1.25 dB" "APIC=$T/front.jpg"
tools prepend "$T/aac.id3" "$X/id3.aac"
ff -i "$T/a48.wav" -c:a mp2 -b:a 128k "$X/v1.mp2"
tools id3v1 "$X/v1.mp2" "MP2 v1" "V1 band" "V1 record" "2001" 3 8
wavpack -q -y --write-binary-tag "Cover Art (Front)=@$T/front.jpg" -w "Title=WV cover" "$T/a.wav" -o "$X/cover.wv"
ff -i "$T/a48.wav" -c:a flac -attach "$T/front.jpg" -metadata:s:t mimetype=image/jpeg -metadata title="MKA cover" "$X/cover.mka"
tools dff "$X/diin.dff" 2822400 2 1 "" "DIIN title" "DIIN artist"
ff -f lavfi -i "anoisesrc=color=pink:sample_rate=48000:duration=2:amplitude=0.2" -af "pan=7.1|c0=c0|c1=c0|c2=c0|c3=c0|c4=c0|c5=c0|c6=c0|c7=c0" -c:a flac "$X/seven-one.flac"
ff -i "$T/a.wav" -c:a pcm_s16le -write_id3v2 1 -metadata title="ID3 over INFO" -metadata artist="Both" "$X/both.wav"
fl -o "$X/renamed image.flac" "$T/a.wav"
printf 'TITLE "Converted"\nFILE "Renamed Image.wav" WAVE\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n  TRACK 02 AUDIO\n    INDEX 01 00:01:00\n' > "$X/converted.cue"

# Not audio after all, and empty
echo "not a wave file" > "$F/fake.wav"
: > "$F/empty.flac"
echo "text" > "$F/notes.txt"

# --- CD images ----------------------------------------------------------------------
ff -f lavfi -i "sine=frequency=330:sample_rate=44100:duration=9" -ac 2 -c:a pcm_s16le "$T/disc.wav"
fl -o "$M/Cue/disc.flac" "$T/disc.wav"
metaflac --set-tag="REPLAYGAIN_ALBUM_GAIN=-4.00 dB" --set-tag="REPLAYGAIN_ALBUM_PEAK=0.9" --set-tag="DISCNUMBER=1" "$M/Cue/disc.flac"
metaflac --import-picture-from="3||||$T/front.jpg" "$M/Cue/disc.flac"
cat > "$M/Cue/disc.cue" <<'EOF'
REM GENRE "Classical"
REM DATE 1985
PERFORMER "Cue Performer"
TITLE "Cue Album"
FILE "disc.wav" WAVE
  TRACK 01 AUDIO
    TITLE "Opening"
    PERFORMER "Soloist"
    REM REPLAYGAIN_TRACK_GAIN -3.50 dB
    REM REPLAYGAIN_TRACK_PEAK 0.80
    INDEX 01 00:00:00
  TRACK 02 AUDIO
    TITLE "Middle"
    INDEX 00 00:02:50
    INDEX 01 00:03:00
  TRACK 03 AUDIO
    INDEX 01 00:06:37
EOF
# Latin-1 sheet, CRLF, two files
ff -f lavfi -i "sine=frequency=500:sample_rate=48000:duration=4" -ac 2 -c:a pcm_s16le "$M/Cue2/side a.wav"
ff -f lavfi -i "sine=frequency=600:sample_rate=48000:duration=4" -ac 2 -c:a pcm_s16le "$M/Cue2/side b.wav"
printf 'PERFORMER "Caf\xe9 Band"\r\nTITLE "D\xe9j\xe0 vu"\r\nFILE "side a.wav" WAVE\r\n  TRACK 1 AUDIO\r\n    TITLE "Un"\r\n    INDEX 01 00:00:00\r\n  TRACK 2 AUDIO\r\n    TITLE "Deux"\r\n    INDEX 01 00:02:00\r\nFILE "side b.wav" WAVE\r\n  TRACK 3 AUDIO\r\n    TITLE "Trois"\r\n    INDEX 01 00:00:00\r\n  TRACK 4 AUDIO\r\n    INDEX 01 00:01:00\r\n' > "$M/Cue2/sides.cue"

# --- Names, skipped folders, links, short files ----------------------------------
fl -o "$M/Música/Ñandú/Ñandú 10.flac" "$T/a.wav"
fl -o "$M/Música/Ñandú/ñandú 2.flac" "$T/a.wav"
fl -o "$M/Música/Ñandú/Éclair.flac" "$T/a.wav"
fl -o "$M/Música/top.flac" "$T/a.wav"
fl -o "$M/Skip me/skipped.flac" "$T/a.wav"
fl -o "$M/Skip me/sub/deep.flac" "$T/a.wav"
ff -f lavfi -i "sine=frequency=440:sample_rate=44100:duration=1" -c:a flac "$M/short/jingle.flac"
fl -o "$out/Outside/linked album/linked.flac" "$T/a.wav"
ln -s "../Outside/linked album" "$M/Linked"
ln -s "." "$M/Formats/loop"
lame --quiet -b 128 "$T/a.wav" "$out/Extra/loose.mp3"
fl -o "$out/Extra/loose.flac" "$T/a.wav"

# --- Scan speed -----------------------------------------------------------------------
if [ "$perf" -gt 0 ]; then
    P="$out/Perf"
    ff -f lavfi -i "sine=frequency=440:sample_rate=44100:duration=1" -ac 2 -c:a pcm_s16le "$T/p.wav"
    fl -o "$T/p.flac" "$T/p.wav"
    metaflac --set-tag="ARTIST=Perf" --set-tag="ALBUM=Perf album" "$T/p.flac"
    lame --quiet -b 128 "$T/p.wav" "$T/p.mp3"
    for ((i = 0; i < perf; i++)); do
        d="$P/album $((i / 20))"
        [ $((i % 20)) -eq 0 ] && mkdir -p "$d"
        if [ $((i % 2)) -eq 0 ]; then cp "$T/p.flac" "$d/track $i.flac"; else cp "$T/p.mp3" "$d/track $i.mp3"; fi
    done
fi
rm -rf "$T"
echo "library corpus in $out"
