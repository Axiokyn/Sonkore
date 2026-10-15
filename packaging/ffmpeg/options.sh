# FFmpeg build shared by the Linux AppImage (packaging/appimage/inside.sh) and
# the Windows package (packaging/windows/build-ffmpeg.sh): LGPL, shared
# libraries, audio decoders and containers only. Sonkore opens these libraries
# on demand, so they ship next to it instead of depending on whatever FFmpeg
# the system has (or lacks). Sourced by bash.
FFMPEG_VERSION=8.1.3
FFMPEG_SHA256=7138d28c96d9d3e3af4ee3d8cad72741f8ffb40da90c1112235dea3ecd3178a3
ffmpeg_options=(
    --enable-shared --disable-static --enable-pic
    --disable-programs --disable-doc --disable-debug --disable-network
    --disable-autodetect
    # libswresample stays: the Opus decoder resamples SILK with it, and
    # configure silently drops Opus without it.
    --disable-avdevice --disable-avfilter --disable-swscale
    --disable-everything --enable-protocol=file --enable-bsf=null
    --enable-decoder='pcm_*,flac,alac,aac,aac_latm,mp1,mp1float,mp2,mp2float,mp3,mp3float,opus,vorbis,wavpack,ape,tta,tak,mpc7,mpc8,dsd_lsbf,dsd_lsbf_planar,dsd_msbf,dsd_msbf_planar,dst,wmav1,wmav2,wmapro,wmalossless,ac3,eac3,dca,truehd,mlp'
    --enable-demuxer=aac,ac3,aiff,ape,asf,au,caf,dsf,dts,eac3,flac,iff,matroska,mlp,mov,mp3,mpc,mpc8,ogg,tak,truehd,tta,w64,wav,wv
    --enable-encoder=dca # the bit-perfect test signal (a DTS stream in a WAV)
    --enable-parser=aac,aac_latm,ac3,dca,flac,mlp,mpegaudio,opus,tak,vorbis
)

# After ./configure, in the FFmpeg source dir: fails if a decoder or demuxer
# asked for above was dropped (configure does so without a word when one of
# its dependencies is disabled).
ffmpeg_check_components() {
    local opt kind list name missing=()
    for opt in "${ffmpeg_options[@]}"; do
        case "$opt" in
            --enable-decoder=*) kind=DECODER list=${opt#*=} ;;
            --enable-demuxer=*) kind=DEMUXER list=${opt#*=} ;;
            *) continue ;;
        esac
        list=${list//\'/}
        for name in ${list//,/ }; do
            [[ "$name" == *'*'* ]] && continue
            grep -q "^#define CONFIG_${name^^}_${kind} 1" config_components.h || missing+=("$name ${kind,,}")
        done
    done
    if [ ${#missing[@]} -gt 0 ]; then
        printf 'FFmpeg configure dropped: %s\n' "${missing[@]}" >&2
        return 1
    fi
}
