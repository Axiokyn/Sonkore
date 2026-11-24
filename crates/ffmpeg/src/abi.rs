//! The parts of FFmpeg's ABI Sonkore touches: the offsets of the few struct
//! fields it reads or writes, per major version of each library, and the
//! constants it passes. Within a major version FFmpeg only appends to its
//! structs, so a field's offset holds for every minor version of it.
//!
//! The offsets are those of the real headers on a 64-bit target (x86_64 and
//! aarch64 Linux, x86_64 Windows: none of these structs holds a `long`, so
//! LP64 and LLP64 lay them out alike), printed by `tools/ffmpeg-offsets.sh`
//! for FFmpeg 5.0, 5.1, 6.0, 6.1, 7.0, 7.1 and 8.0. The test
//! `offsets_match_the_installed_headers` compiles the same program against
//! the system's headers and compares.
//!
//! Struct by library: AVFormatContext, AVStream, AVIOContext (libavformat);
//! AVCodecParameters, AVCodecContext, AVPacket (libavcodec); AVFrame
//! (libavutil).

/// The library majors of each supported FFmpeg release (libavformat,
/// libavcodec, libavutil), newest first: 8.x, 7.x, 6.x, 5.x.
pub(crate) const RELEASES: [(u32, u32, u32); 4] = [(62, 62, 60), (61, 61, 59), (60, 60, 58), (59, 59, 57)];

// --- Fields at the same offset in every supported version ---------------------------

pub(crate) const FMT_PB: usize = 32; // AVFormatContext.pb
pub(crate) const FMT_NB_STREAMS: usize = 44; // AVFormatContext.nb_streams
pub(crate) const FMT_STREAMS: usize = 48; // AVFormatContext.streams
pub(crate) const IO_BUFFER: usize = 8; // AVIOContext.buffer
pub(crate) const PAR_CODEC_TYPE: usize = 0; // AVCodecParameters.codec_type
pub(crate) const PAR_CODEC_ID: usize = 4; // AVCodecParameters.codec_id
pub(crate) const PKT_STREAM_INDEX: usize = 36; // AVPacket.stream_index
pub(crate) const FRAME_EXTENDED_DATA: usize = 96; // AVFrame.extended_data
pub(crate) const FRAME_NB_SAMPLES: usize = 112; // AVFrame.nb_samples
pub(crate) const FRAME_FORMAT: usize = 116; // AVFrame.format
pub(crate) const FRAME_PTS: usize = 136; // AVFrame.pts
/// AVChannelLayout.nb_channels, after `enum AVChannelOrder order`.
pub(crate) const CH_LAYOUT_NB_CHANNELS: usize = 4;

/// libavformat: AVStream.
#[derive(Debug)]
pub(crate) struct FormatLayout {
    pub major: u32,
    /// AVFormatContext.duration (in AV_TIME_BASE units).
    pub fmt_duration: usize,
    pub stream_codecpar: usize,
    pub stream_time_base: usize,
    pub stream_start_time: usize,
    pub stream_duration: usize,
    pub stream_discard: usize,
}

/// libavcodec: AVCodecParameters and AVCodecContext. The channel count is
/// the old `int channels` up to 6.x (both 5.0, which has no `ch_layout`,
/// and 5.1/6.x, which have both) and `ch_layout.nb_channels` from 7.0 on.
#[derive(Debug)]
pub(crate) struct CodecLayout {
    pub major: u32,
    pub par_bits_raw: usize,
    pub par_sample_rate: usize,
    pub par_initial_padding: usize,
    pub ctx_sample_rate: usize,
    pub ctx_sample_fmt: usize,
    pub ctx_bits_raw: usize,
    /// Where the channel count is (an int either way).
    pub ctx_channels: usize,
    /// It is `ch_layout` (set with the "ch_layout" option), not `channels` ("ac").
    pub ch_layout: bool,
}

/// libavutil: AVFrame.
#[derive(Debug)]
pub(crate) struct UtilLayout {
    pub major: u32,
    pub frame_channels: usize,
    pub frame_sample_rate: usize,
    pub frame_best_effort: usize,
}

const FORMAT: [FormatLayout; 4] = [
    FormatLayout {
        major: 59,
        fmt_duration: 72,
        stream_codecpar: 208,
        stream_time_base: 16,
        stream_start_time: 24,
        stream_duration: 32,
        stream_discard: 52,
    },
    // 6.0 moved codecpar up after the AVClass that now opens AVStream.
    FormatLayout {
        major: 60,
        fmt_duration: 72,
        stream_codecpar: 16,
        stream_time_base: 32,
        stream_start_time: 40,
        stream_duration: 48,
        stream_discard: 68,
    },
    FormatLayout {
        major: 61,
        fmt_duration: 104,
        stream_codecpar: 16,
        stream_time_base: 32,
        stream_start_time: 40,
        stream_duration: 48,
        stream_discard: 68,
    },
    FormatLayout {
        major: 62,
        fmt_duration: 104,
        stream_codecpar: 16,
        stream_time_base: 32,
        stream_start_time: 40,
        stream_duration: 48,
        stream_discard: 68,
    },
];

const CODEC: [CodecLayout; 4] = [
    CodecLayout {
        major: 59,
        par_bits_raw: 44,
        par_sample_rate: 116,
        par_initial_padding: 128,
        ctx_sample_rate: 352,
        ctx_sample_fmt: 360,
        ctx_bits_raw: 628,
        ctx_channels: 356,
        ch_layout: false,
    },
    CodecLayout {
        major: 60,
        par_bits_raw: 44,
        par_sample_rate: 116,
        par_initial_padding: 128,
        ctx_sample_rate: 352,
        ctx_sample_fmt: 360,
        ctx_bits_raw: 628,
        ctx_channels: 356,
        ch_layout: false,
    },
    // 7.0 dropped the old channel fields: ch_layout at 352 (+4: nb_channels).
    CodecLayout {
        major: 61,
        par_bits_raw: 60,
        par_sample_rate: 152,
        par_initial_padding: 164,
        ctx_sample_rate: 344,
        ctx_sample_fmt: 348,
        ctx_bits_raw: 652,
        ctx_channels: 352 + CH_LAYOUT_NB_CHANNELS,
        ch_layout: true,
    },
    CodecLayout {
        major: 62,
        par_bits_raw: 60,
        par_sample_rate: 152,
        par_initial_padding: 164,
        ctx_sample_rate: 344,
        ctx_sample_fmt: 348,
        ctx_bits_raw: 652,
        ctx_channels: 352 + CH_LAYOUT_NB_CHANNELS,
        ch_layout: true,
    },
];

const UTIL: [UtilLayout; 4] = [
    UtilLayout { major: 57, frame_channels: 380, frame_sample_rate: 208, frame_best_effort: 344 },
    UtilLayout { major: 58, frame_channels: 380, frame_sample_rate: 208, frame_best_effort: 344 },
    // ch_layout at 408 / 384 (+4: nb_channels).
    UtilLayout {
        major: 59,
        frame_channels: 408 + CH_LAYOUT_NB_CHANNELS,
        frame_sample_rate: 192,
        frame_best_effort: 320,
    },
    UtilLayout {
        major: 60,
        frame_channels: 384 + CH_LAYOUT_NB_CHANNELS,
        frame_sample_rate: 180,
        frame_best_effort: 304,
    },
];

pub(crate) fn format_layout(major: u32) -> Option<&'static FormatLayout> {
    FORMAT.iter().find(|l| l.major == major)
}

pub(crate) fn codec_layout(major: u32) -> Option<&'static CodecLayout> {
    CODEC.iter().find(|l| l.major == major)
}

pub(crate) fn util_layout(major: u32) -> Option<&'static UtilLayout> {
    UTIL.iter().find(|l| l.major == major)
}

/// Every offset of one release, named as `tools/ffmpeg-offsets.sh` prints
/// them ("AVStream.codecpar"); channel counts as the field that holds them.
pub fn offsets(format: u32, codec: u32, util: u32) -> Option<Vec<(String, usize)>> {
    let (f, c, u) = (format_layout(format)?, codec_layout(codec)?, util_layout(util)?);
    let mut v: Vec<(&str, usize)> = vec![
        ("AVFormatContext.pb", FMT_PB),
        ("AVFormatContext.nb_streams", FMT_NB_STREAMS),
        ("AVFormatContext.streams", FMT_STREAMS),
        ("AVFormatContext.duration", f.fmt_duration),
        ("AVIOContext.buffer", IO_BUFFER),
        ("AVStream.codecpar", f.stream_codecpar),
        ("AVStream.time_base", f.stream_time_base),
        ("AVStream.start_time", f.stream_start_time),
        ("AVStream.duration", f.stream_duration),
        ("AVStream.discard", f.stream_discard),
        ("AVCodecParameters.codec_type", PAR_CODEC_TYPE),
        ("AVCodecParameters.codec_id", PAR_CODEC_ID),
        ("AVCodecParameters.bits_per_raw_sample", c.par_bits_raw),
        ("AVCodecParameters.sample_rate", c.par_sample_rate),
        ("AVCodecParameters.initial_padding", c.par_initial_padding),
        ("AVCodecContext.sample_rate", c.ctx_sample_rate),
        ("AVCodecContext.sample_fmt", c.ctx_sample_fmt),
        ("AVCodecContext.bits_per_raw_sample", c.ctx_bits_raw),
        ("AVPacket.stream_index", PKT_STREAM_INDEX),
        ("AVFrame.extended_data", FRAME_EXTENDED_DATA),
        ("AVFrame.nb_samples", FRAME_NB_SAMPLES),
        ("AVFrame.format", FRAME_FORMAT),
        ("AVFrame.pts", FRAME_PTS),
        ("AVFrame.sample_rate", u.frame_sample_rate),
        ("AVFrame.best_effort_timestamp", u.frame_best_effort),
        ("AVChannelLayout.nb_channels", CH_LAYOUT_NB_CHANNELS),
    ];
    if c.ch_layout {
        v.push(("AVCodecContext.ch_layout", c.ctx_channels - CH_LAYOUT_NB_CHANNELS));
    } else {
        v.push(("AVCodecContext.channels", c.ctx_channels));
    }
    if u.major >= 59 {
        v.push(("AVFrame.ch_layout", u.frame_channels - CH_LAYOUT_NB_CHANNELS));
    } else {
        v.push(("AVFrame.channels", u.frame_channels));
    }
    Some(v.into_iter().map(|(k, o)| (k.to_string(), o)).collect())
}

// --- Constants -------------------------------------------------------------------

pub(crate) const AV_NOPTS_VALUE: i64 = i64::MIN;
/// AVERROR_EOF: FFERRTAG('E', 'O', 'F', ' ').
pub(crate) const AVERROR_EOF: i32 = -0x2046_4F45;
/// AVERROR(EAGAIN): the C library's EAGAIN, negated.
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd", target_os = "openbsd", target_os = "netbsd"))]
pub(crate) const AVERROR_EAGAIN: i32 = -35;
#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd"
)))]
pub(crate) const AVERROR_EAGAIN: i32 = -11;
pub(crate) const AVMEDIA_TYPE_AUDIO: i32 = 1;
pub(crate) const AVDISCARD_ALL: i32 = 48;
pub(crate) const AVSEEK_FLAG_BACKWARD: i32 = 1;
pub(crate) const AVSEEK_SIZE: i32 = 0x10000;
pub(crate) const AV_LOG_QUIET: i32 = -8;
pub(crate) const AV_LOG_ERROR: i32 = 16;
pub(crate) const AV_TIME_BASE: i64 = 1_000_000;

/// enum AVSampleFormat (the same in every version).
pub(crate) mod sample_fmt {
    pub const U8: i32 = 0;
    pub const S16: i32 = 1;
    pub const S32: i32 = 2;
    pub const FLT: i32 = 3;
    pub const DBL: i32 = 4;
    pub const S64: i32 = 10;
    /// The packed format of a planar one (U8P → U8…), and whether it was planar.
    pub fn packed(f: i32) -> Option<(i32, bool)> {
        match f {
            0..=4 | 10 => Some((f, false)),
            5..=9 => Some((f - 5, true)),
            11 => Some((10, true)),
            _ => None,
        }
    }
    pub fn bytes(packed: i32) -> usize {
        match packed {
            U8 => 1,
            S16 => 2,
            S32 | FLT => 4,
            _ => 8,
        }
    }
}
