/* The offsets of the FFmpeg struct fields Sonkore reads (crates/ffmpeg/src/abi.rs),
 * as the headers it is compiled against lay them out. Built and compared by
 * the test offsets_match_the_installed_headers, and by tools/ffmpeg-offsets.sh
 * against other FFmpeg releases' headers. */
#include <stdio.h>
#include <stddef.h>
#include <libavformat/avformat.h>
#include <libavcodec/avcodec.h>
#include <libavutil/frame.h>

#define O(T, f) printf("%s.%s %zu\n", #T, #f, offsetof(T, f))

int main(void)
{
    printf("versions %d %d %d\n", LIBAVFORMAT_VERSION_MAJOR, LIBAVCODEC_VERSION_MAJOR, LIBAVUTIL_VERSION_MAJOR);
    O(AVFormatContext, pb);
    O(AVFormatContext, nb_streams);
    O(AVFormatContext, streams);
    O(AVFormatContext, duration);
    O(AVIOContext, buffer);
    O(AVStream, codecpar);
    O(AVStream, time_base);
    O(AVStream, start_time);
    O(AVStream, duration);
    O(AVStream, discard);
    O(AVCodecParameters, codec_type);
    O(AVCodecParameters, codec_id);
    O(AVCodecParameters, bits_per_raw_sample);
    O(AVCodecParameters, sample_rate);
    O(AVCodecParameters, initial_padding);
    O(AVCodecContext, sample_rate);
    O(AVCodecContext, sample_fmt);
    O(AVCodecContext, bits_per_raw_sample);
    O(AVPacket, stream_index);
    O(AVFrame, extended_data);
    O(AVFrame, nb_samples);
    O(AVFrame, format);
    O(AVFrame, pts);
    O(AVFrame, sample_rate);
    O(AVFrame, best_effort_timestamp);
#if LIBAVCODEC_VERSION_MAJOR >= 61
    O(AVCodecContext, ch_layout);
#else
    O(AVCodecContext, channels);
#endif
#if LIBAVUTIL_VERSION_MAJOR >= 59
    O(AVFrame, ch_layout);
#else
    O(AVFrame, channels);
#endif
#if LIBAVUTIL_VERSION_MAJOR > 57 || LIBAVUTIL_VERSION_MINOR >= 24
    O(AVChannelLayout, nb_channels);
#else
    printf("AVChannelLayout.nb_channels 4\n"); /* 5.0 has no AVChannelLayout: not used there */
#endif
    printf("AVERROR_EOF %d\n", AVERROR_EOF);
    printf("AVERROR_EAGAIN %d\n", AVERROR(EAGAIN));
    printf("AVDISCARD_ALL %d\n", AVDISCARD_ALL);
    printf("AVSEEK_SIZE %d\n", AVSEEK_SIZE);
    return 0;
}
