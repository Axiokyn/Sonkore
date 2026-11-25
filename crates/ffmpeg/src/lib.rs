//! FFmpeg as a fallback decoder for the formats symphonia does not read
//! (Opus, Monkey's Audio, WavPack, TTA, TAK, Musepack, WMA, Wave64, AC-3,
//! DTS), loaded at run time when it is installed (`loader`): Sonkore builds
//! and runs without it, and those files then just do not play.
//!
//! `Stream` gives what the engine's decoders give: interleaved samples, a
//! lossless integer codec's as left-justified s32 (16-bit << 16, 24-bit
//! << 8: bit for bit what the file holds), float and lossy ones as f64,
//! never clipped; the exact frame count (Opus pre-skip and end trimming
//! applied by FFmpeg, as the container says); seeks that land on the exact
//! frame (from the nearest seek point before it, decoding and dropping up
//! to it). The file is read through Sonkore's own reader (custom I/O), so
//! FFmpeg opens no file, URL or protocol of its own.

mod abi;
mod loader;

use std::ffi::{CString, c_int, c_void};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::ptr::null_mut;

use abi::sample_fmt;
use loader::{Api, P, read, text, write};

pub use abi::offsets;
pub use loader::set_disabled;

/// The extensions only FFmpeg decodes, lower case.
pub const EXTENSIONS: [&str; 11] = ["opus", "ape", "wv", "tta", "tak", "mpc", "wma", "w64", "ac3", "dts", "eac3"];

/// An extension (any case) only FFmpeg decodes.
pub fn handles_extension(ext: &str) -> bool {
    EXTENSIONS.iter().any(|e| e.eq_ignore_ascii_case(ext))
}

/// The library versions loaded ("libavformat 62.12 · libavcodec 62.28 ·
/// libavutil 60.26"), loading them if they were not; None without FFmpeg.
pub fn versions() -> Option<String> {
    loader::api().ok().map(|a| a.versions.clone())
}

/// FFmpeg is installed in a version Sonkore knows (loads it if needed).
pub fn available() -> bool {
    loader::api().is_ok()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// FFmpeg is not installed (or not in a version Sonkore knows): why.
    Missing(String),
    /// The file could not be opened or decoded.
    Failed(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Missing(why) => write!(f, "FFmpeg is not available ({why})"),
            Error::Failed(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// A file to read: on disk, or in memory.
pub trait Source: Read + Seek + Send {}
impl<T: Read + Seek + Send> Source for T {}

/// What a stream is (the engine's StreamInfo).
#[derive(Clone, Debug, PartialEq)]
pub struct Info {
    pub rate: u32,
    pub channels: usize,
    /// A lossless integer codec's bits (16, 24…); 0 for float and lossy ones.
    pub bits: u32,
    /// Samples are given as floating point (lossy codecs, float WavPack…).
    pub float: bool,
    /// Integer and lossless: the samples are the music, bit for bit.
    pub lossless: bool,
    /// From the container: exact for Ogg, APE, WavPack, TTA, TAK, Wave64;
    /// estimated from the bit rate for raw AC-3 / DTS.
    pub total_frames: Option<u64>,
    /// The codec's short name ("Opus", "WavPack", "WMA"…).
    pub codec: String,
    /// FFmpeg's name of the codec ("opus", "wavpack"…).
    pub codec_id_name: String,
}

/// The label Sonkore shows for an FFmpeg codec (as the library's).
fn codec_label(name: &str) -> String {
    match name {
        "opus" => "Opus",
        "vorbis" => "Vorbis",
        "ape" => "APE",
        "wavpack" => "WavPack",
        "tta" => "TTA",
        "tak" => "TAK",
        "musepack7" | "musepack8" => "MPC",
        "wmav1" | "wmav2" => "WMA",
        "wmapro" => "WMA Pro",
        "wmalossless" => "WMA Lossless",
        "wmavoice" => "WMA Voice",
        "ac3" => "AC-3",
        "eac3" => "E-AC-3",
        "dts" => "DTS",
        "truehd" => "TrueHD",
        "mlp" => "MLP",
        "speex" => "Speex",
        "alac" => "ALAC",
        "flac" => "FLAC",
        "aac" => "AAC",
        "mp3" => "MP3",
        "mp2" => "MP2",
        n if n.starts_with("pcm_") => "PCM",
        n => return n.to_uppercase(),
    }
    .to_string()
}

/// Codecs whose integer output is the source bit for bit (WavPack and DTS
/// can be lossy too: hybrid WavPack without its correction file, a DTS core;
/// a DTS core decodes to float, so integer DTS is DTS-HD Master Audio).
fn lossless_codec(name: &str) -> bool {
    matches!(name, "ape" | "wavpack" | "tta" | "tak" | "wmalossless" | "alac" | "flac" | "mlp" | "truehd" | "dts")
        || (name.starts_with("pcm_") && !matches!(name, "pcm_alaw" | "pcm_mulaw" | "pcm_vidc"))
}

/// The reader behind FFmpeg's custom I/O.
struct Io {
    source: Box<dyn Source>,
    len: u64,
}

unsafe extern "C" fn io_read(opaque: P, buf: *mut u8, size: c_int) -> c_int {
    if size <= 0 {
        return 0;
    }
    // SAFETY: `opaque` is the stream's Io, `buf` holds `size` bytes (FFmpeg's contract).
    let (io, out) = unsafe { (&mut *opaque.cast::<Io>(), std::slice::from_raw_parts_mut(buf, size as usize)) };
    let got = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        loop {
            match io.source.read(out) {
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                r => return r,
            }
        }
    }));
    match got {
        Ok(Ok(0)) => abi::AVERROR_EOF,
        Ok(Ok(n)) => n as c_int,
        _ => -5, // AVERROR(EIO)
    }
}

unsafe extern "C" fn io_seek(opaque: P, offset: i64, whence: c_int) -> i64 {
    // SAFETY: `opaque` is the stream's Io.
    let io = unsafe { &mut *opaque.cast::<Io>() };
    if whence & abi::AVSEEK_SIZE != 0 {
        return io.len as i64;
    }
    let to = match whence & 0xffff {
        0 if offset >= 0 => SeekFrom::Start(offset as u64),
        1 => SeekFrom::Current(offset),
        2 => SeekFrom::End(offset),
        _ => return -22, // AVERROR(EINVAL)
    };
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| io.source.seek(to))) {
        Ok(Ok(at)) => at as i64,
        _ => -5,
    }
}

const IO_BUFFER_SIZE: usize = 64 * 1024;

enum Pending {
    Int(Vec<i32>),
    Float(Vec<f64>),
}

/// A decoded audio stream.
pub struct Stream {
    api: &'static Api,
    format: P,
    io: P,
    io_state: *mut Io,
    codec: P,
    packet: P,
    frame: P,
    stream: c_int,
    /// AVStream.time_base.
    time_base: (i64, i64),
    /// The file's name, as opened (to open it again).
    name: String,
    info: Info,
    pending: Pending, // interleaved, decoded and not read yet
    pending_pos: usize,
    /// A frame received and not taken into `pending` yet.
    frame_ready: bool,
    input_done: bool,
    finished: bool,
    /// The first frame's position on the stream's clock, in frames: frame 0.
    origin: i64,
    /// After a seek: the frame wanted, on the stream's clock.
    skip_to: Option<i64>,
    /// Where the next decoded frame starts, for one without a timestamp.
    next_at: Option<i64>,
    /// Frames handed out (or sought to).
    position: u64,
    /// Lossy codecs: decoded before a seek target so the decoder settles.
    preroll: i64,
    /// Where decoded frames start when the time base is coarser than a
    /// frame (Matroska and ASF count milliseconds, Musepack packets): the
    /// first frame after the first one, and the codec's frame length. A
    /// timestamp after a seek is rounded to this grid.
    grid: Option<(i64, i64)>,
}

// SAFETY: the FFmpeg contexts belong to this stream alone and are used by
// one thread at a time (&mut self); the source is Send.
unsafe impl Send for Stream {}

impl Drop for Stream {
    fn drop(&mut self) {
        let a = self.api;
        // SAFETY: each pointer is null or what FFmpeg allocated for this stream.
        unsafe {
            if !self.frame.is_null() {
                (a.av_frame_free)(&mut self.frame);
            }
            if !self.packet.is_null() {
                (a.av_packet_free)(&mut self.packet);
            }
            if !self.codec.is_null() {
                (a.avcodec_free_context)(&mut self.codec);
            }
            if !self.format.is_null() {
                (a.avformat_close_input)(&mut self.format);
            }
            if !self.io.is_null() {
                // Custom I/O is the caller's to free, buffer included (FFmpeg may have replaced it).
                (a.av_free)(read::<P>(self.io, abi::IO_BUFFER));
                (a.avio_context_free)(&mut self.io);
            }
            if !self.io_state.is_null() {
                drop(Box::from_raw(self.io_state));
            }
        }
    }
}

fn failed(what: &str) -> Error {
    Error::Failed(what.to_string())
}

impl Stream {
    /// Opens a file on disk.
    pub fn open_path(path: &Path) -> Result<Stream> {
        loader::api().map_err(Error::Missing)?;
        let file = std::fs::File::open(path).map_err(|e| Error::Failed(e.to_string()))?;
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        Stream::open(Box::new(file), &name)
    }

    /// Opens a file through `source`; `name` (the file's name) helps FFmpeg
    /// tell formats with no signature apart by their extension.
    pub fn open(mut source: Box<dyn Source>, name: &str) -> Result<Stream> {
        let api = loader::api().map_err(Error::Missing)?;
        let len = source.seek(SeekFrom::End(0)).and_then(|l| source.seek(SeekFrom::Start(0)).map(|_| l));
        let len = len.map_err(|e| Error::Failed(e.to_string()))?;
        let mut s = Stream {
            api,
            format: null_mut(),
            io: null_mut(),
            io_state: Box::into_raw(Box::new(Io { source, len })),
            codec: null_mut(),
            packet: null_mut(),
            frame: null_mut(),
            stream: -1,
            time_base: (1, 1),
            name: name.to_string(),
            info: Info {
                rate: 0,
                channels: 0,
                bits: 0,
                float: true,
                lossless: false,
                total_frames: None,
                codec: String::new(),
                codec_id_name: String::new(),
            },
            pending: Pending::Float(Vec::new()),
            pending_pos: 0,
            frame_ready: false,
            input_done: false,
            finished: false,
            origin: 0,
            skip_to: None,
            next_at: None,
            position: 0,
            preroll: 0,
            grid: None,
        };
        // SAFETY: FFmpeg's API as documented; struct fields at the offsets of
        // the loaded version (abi.rs), on pointers FFmpeg returned non-null.
        unsafe { s.open_inner(name)? };
        Ok(s)
    }

    unsafe fn open_inner(&mut self, name: &str) -> Result<()> {
        let a = self.api;
        // SAFETY: see open().
        unsafe {
            let buffer = (a.av_malloc)(IO_BUFFER_SIZE);
            if buffer.is_null() {
                return Err(failed("out of memory"));
            }
            self.io = (a.avio_alloc_context)(
                buffer.cast(),
                IO_BUFFER_SIZE as c_int,
                0,
                self.io_state.cast(),
                Some(io_read),
                None,
                Some(io_seek),
            );
            if self.io.is_null() {
                (a.av_free)(buffer);
                return Err(failed("out of memory"));
            }
            self.format = (a.avformat_alloc_context)();
            if self.format.is_null() {
                return Err(failed("out of memory"));
            }
            write::<P>(self.format, abi::FMT_PB, self.io);
            let url = CString::new(name.replace('\0', "")).unwrap_or_default();
            // On failure FFmpeg frees the context and nulls the pointer.
            if (a.avformat_open_input)(&mut self.format, url.as_ptr(), std::ptr::null(), null_mut()) < 0 {
                return Err(failed("unreadable file"));
            }
            if (a.avformat_find_stream_info)(self.format, null_mut()) < 0 {
                return Err(failed("unreadable file"));
            }
            let mut decoder: *const c_void = std::ptr::null();
            self.stream = (a.av_find_best_stream)(self.format, abi::AVMEDIA_TYPE_AUDIO, -1, -1, &mut decoder, 0);
            if self.stream < 0 {
                return Err(failed("no audio in the file"));
            }
            let count = read::<u32>(self.format, abi::FMT_NB_STREAMS) as usize;
            let streams = read::<*const P>(self.format, abi::FMT_STREAMS);
            if streams.is_null() || self.stream as usize >= count {
                return Err(failed("no audio in the file"));
            }
            let fl = a.format;
            // Only the audio is demuxed (no cover or video packets).
            for i in 0..count {
                if i != self.stream as usize {
                    write::<i32>(*streams.add(i), fl.stream_discard, abi::AVDISCARD_ALL);
                }
            }
            let st = *streams.add(self.stream as usize);
            let par = read::<P>(st, fl.stream_codecpar);
            if par.is_null() || read::<i32>(par, abi::PAR_CODEC_TYPE) != abi::AVMEDIA_TYPE_AUDIO {
                return Err(failed("no audio in the file"));
            }
            let codec_name = text((a.avcodec_get_name)(read::<i32>(par, abi::PAR_CODEC_ID)));
            if decoder.is_null() {
                return Err(Error::Failed(format!("this FFmpeg has no {codec_name} decoder")));
            }
            let tb = (read::<i32>(st, fl.stream_time_base) as i64, read::<i32>(st, fl.stream_time_base + 4) as i64);
            self.time_base = if tb.0 > 0 && tb.1 > 0 { tb } else { (1, 1) };
            let start_time = read::<i64>(st, fl.stream_start_time);
            let duration = read::<i64>(st, fl.stream_duration);
            let cl = a.codec;
            let par_bits = read::<i32>(par, cl.par_bits_raw);
            let par_padding = read::<i32>(par, cl.par_initial_padding);

            self.codec = (a.avcodec_alloc_context3)(decoder);
            if self.codec.is_null() || (a.avcodec_parameters_to_context)(self.codec, par) < 0 {
                return Err(failed("unreadable file"));
            }
            // One thread (a track decodes far faster than it plays), and the
            // packets' time base, so the samples a decoder skips (Opus
            // pre-skip) move the frames' timestamps too.
            let mut options: P = null_mut();
            (a.av_dict_set)(&mut options, c"threads".as_ptr(), c"1".as_ptr(), 0);
            let tb_text = CString::new(format!("{}/{}", self.time_base.0, self.time_base.1)).unwrap_or_default();
            (a.av_dict_set)(&mut options, c"pkt_timebase".as_ptr(), tb_text.as_ptr(), 0);
            let opened = (a.avcodec_open2)(self.codec, decoder, &mut options);
            (a.av_dict_free)(&mut options);
            if opened < 0 {
                return Err(Error::Failed(format!("FFmpeg could not open its {codec_name} decoder")));
            }
            self.packet = (a.av_packet_alloc)();
            self.frame = (a.av_frame_alloc)();
            if self.packet.is_null() || self.frame.is_null() {
                return Err(failed("out of memory"));
            }

            // The first frame tells what the decoder gives (some decoders
            // only know it then) and where frame 0 is on the stream's clock.
            let first = self.receive()?;
            let ctx_rate = read::<i32>(self.codec, cl.ctx_sample_rate);
            let ctx_channels = read::<i32>(self.codec, cl.ctx_channels);
            let (rate, channels, fmt) = if first {
                let u = a.util;
                (
                    read::<i32>(self.frame, u.frame_sample_rate),
                    read::<i32>(self.frame, u.frame_channels),
                    read::<i32>(self.frame, abi::FRAME_FORMAT),
                )
            } else {
                (ctx_rate, ctx_channels, read::<i32>(self.codec, cl.ctx_sample_fmt))
            };
            // What FFmpeg says must make sense, or the layout is not what abi.rs says.
            let rate = if rate > 0 { rate } else { ctx_rate.max(read::<i32>(par, cl.par_sample_rate)) };
            if !(1..=6_144_000).contains(&rate) || !(1..=64).contains(&channels) {
                return Err(failed("unsupported sample rate or channel layout"));
            }
            let Some((packed, _)) = sample_fmt::packed(fmt) else {
                return Err(failed("unsupported sample format"));
            };
            let integer = matches!(packed, sample_fmt::U8 | sample_fmt::S16 | sample_fmt::S32 | sample_fmt::S64);
            let lossless = integer && lossless_codec(&codec_name);
            let bits = if lossless {
                let container = (sample_fmt::bytes(packed) * 8).min(32) as i32;
                let raw = read::<i32>(self.codec, cl.ctx_bits_raw);
                let raw = if raw > 0 { raw } else { par_bits };
                (if raw > 0 { raw.min(container) } else { container }) as u32
            } else {
                0
            };
            self.info.rate = rate as u32;
            self.info.channels = channels as usize;
            self.info.bits = bits;
            self.info.float = !lossless;
            self.info.lossless = lossless;
            self.info.codec = codec_label(&codec_name);
            self.info.codec_id_name = codec_name;
            self.pending = if lossless { Pending::Int(Vec::new()) } else { Pending::Float(Vec::new()) };
            // Lossy decoders need some audio before a seek target to settle
            // (Opus asks for 80 ms).
            self.preroll = if lossless { 0 } else { rate as i64 / 10 };

            self.origin = if first { self.frame_at().unwrap_or(0) } else { 0 };
            self.info.total_frames = self.length(start_time, duration, par_padding);
            if first {
                // The first two frames: the second (kept for reading) gives the grid.
                let n1 = read::<i32>(self.frame, abi::FRAME_NB_SAMPLES) as i64;
                self.take_frame()?;
                let coarse = self.time_base.0 as i128 * self.info.rate as i128 > self.time_base.1 as i128;
                if coarse && self.receive()? {
                    let n2 = read::<i32>(self.frame, abi::FRAME_NB_SAMPLES) as i64;
                    if n2 > 0 {
                        self.grid = Some((self.origin + n1, n2));
                    }
                }
            }
            Ok(())
        }
    }

    /// The stream's length in frames, from frame 0.
    fn length(&self, start_time: i64, duration: i64, initial_padding: i32) -> Option<u64> {
        if duration == abi::AV_NOPTS_VALUE || duration <= 0 {
            // The container's length instead (Matroska / WebM), estimated.
            // SAFETY: the open AVFormatContext, at its version's offset.
            let whole = unsafe { read::<i64>(self.format, self.api.format.fmt_duration) };
            if whole == abi::AV_NOPTS_VALUE || whole <= 0 {
                return None;
            }
            let frames = whole as i128 * self.info.rate as i128 / abi::AV_TIME_BASE as i128;
            return u64::try_from(frames).ok().filter(|&f| f > 0);
        }
        let start = if start_time == abi::AV_NOPTS_VALUE { 0 } else { self.ts_to_frames(start_time) };
        let mut end = start.saturating_add(self.ts_to_frames(duration));
        // Ogg's length counts the samples the decoder drops (Opus pre-skip);
        // the first frame's timestamp is after them already.
        if self.origin <= start && initial_padding > 0 {
            end = end.saturating_sub(initial_padding as i64);
        }
        u64::try_from(end.saturating_sub(self.origin)).ok().filter(|&f| f > 0)
    }

    pub fn info(&self) -> &Info {
        &self.info
    }

    /// A timestamp in the stream's time base, in frames (rounded).
    fn ts_to_frames(&self, ts: i64) -> i64 {
        let (num, den) = self.time_base;
        let n = ts as i128 * num as i128 * self.info.rate as i128;
        let d = den as i128;
        let f = if n >= 0 { (n + d / 2) / d } else { -((-n + d / 2) / d) };
        f.clamp(i64::MIN as i128, i64::MAX as i128) as i64
    }

    /// Frames as a timestamp in the stream's time base (rounded down).
    fn frames_to_ts(&self, frames: i64) -> i64 {
        let (num, den) = self.time_base;
        (frames as i128 * den as i128).div_euclid(num as i128 * self.info.rate.max(1) as i128) as i64
    }

    /// The received frame's position on the stream's clock, in frames.
    fn frame_at(&self) -> Option<i64> {
        // SAFETY: self.frame is a received AVFrame of the loaded version.
        let (pts, best) = unsafe {
            (read::<i64>(self.frame, abi::FRAME_PTS), read::<i64>(self.frame, self.api.util.frame_best_effort))
        };
        let ts = if pts != abi::AV_NOPTS_VALUE { pts } else { best };
        (ts != abi::AV_NOPTS_VALUE).then(|| self.ts_to_frames(ts))
    }

    /// The received frame's position, on the grid of frame starts when the
    /// time base is coarser than a frame.
    fn landed_at(&self) -> Option<i64> {
        let p = self.frame_at()?;
        Some(match self.grid {
            Some((start, len)) => start
                .saturating_add(p.saturating_sub(start).saturating_add(len / 2).div_euclid(len).saturating_mul(len)),
            None => p,
        })
    }

    /// Decodes until a frame is received into self.frame; false at the end.
    fn receive(&mut self) -> Result<bool> {
        let a = self.api;
        if self.frame_ready {
            return Ok(true);
        }
        // A file whose every packet fails is not decoded forever.
        let mut errors = 0;
        loop {
            if self.finished {
                return Ok(false);
            }
            // SAFETY: FFmpeg's decoding loop on this stream's own contexts.
            unsafe {
                let r = (a.avcodec_receive_frame)(self.codec, self.frame);
                if r >= 0 {
                    self.frame_ready = true;
                    return Ok(true);
                }
                if r != abi::AVERROR_EAGAIN || self.input_done {
                    // The end (AVERROR_EOF once drained), or a decoder that cannot go on.
                    self.finished = true;
                    return Ok(false);
                }
                let r = (a.av_read_frame)(self.format, self.packet);
                if r < 0 {
                    // The end of the file (or a read error): drain the decoder.
                    (a.avcodec_send_packet)(self.codec, std::ptr::null());
                    self.input_done = true;
                    continue;
                }
                if read::<i32>(self.packet, abi::PKT_STREAM_INDEX) == self.stream {
                    let sent = (a.avcodec_send_packet)(self.codec, self.packet);
                    // A damaged packet is skipped.
                    if sent < 0 && sent != abi::AVERROR_EAGAIN {
                        errors += 1;
                        if errors > 1000 {
                            (a.av_packet_unref)(self.packet);
                            return Err(failed("the file cannot be decoded"));
                        }
                    }
                }
                (a.av_packet_unref)(self.packet);
            }
        }
    }

    /// Moves the received frame into `pending` (after what a seek drops).
    fn take_frame(&mut self) -> Result<()> {
        self.frame_ready = false;
        let a = self.api;
        let frame = self.frame;
        // SAFETY: a received AVFrame; its planes hold nb_samples samples of
        // `format` for each of its channels (one plane, interleaved, when packed).
        unsafe {
            let n = read::<i32>(frame, abi::FRAME_NB_SAMPLES).max(0) as usize;
            let channels = read::<i32>(frame, a.util.frame_channels);
            let fmt = read::<i32>(frame, abi::FRAME_FORMAT);
            let data = read::<*const *const u8>(frame, abi::FRAME_EXTENDED_DATA);
            // Counted from the frame before, else from its timestamp.
            let at = self.next_at.or_else(|| self.landed_at());
            let result = (|| {
                let Some((packed, planar)) = sample_fmt::packed(fmt) else {
                    return Err(failed("unsupported sample format"));
                };
                if !(1..=64).contains(&channels) || n > 1 << 20 || data.is_null() {
                    return Err(failed("damaged frame"));
                }
                let mut first = 0usize;
                if let Some(target) = self.skip_to {
                    if let Some(p) = at {
                        if p.saturating_add(n as i64) <= target {
                            self.next_at = Some(p.saturating_add(n as i64));
                            return Ok(());
                        }
                        first = target.saturating_sub(p).clamp(0, n as i64) as usize;
                    }
                    self.skip_to = None;
                }
                // Frames of other lengths: the grid does not hold.
                if let (Some((start, len)), Some(p)) = (self.grid, at)
                    && p.saturating_sub(start).rem_euclid(len) != 0
                {
                    self.grid = None;
                }
                self.next_at = at.map(|p| p.saturating_add(n as i64));
                if first >= n {
                    return Ok(());
                }
                let ch = self.info.channels;
                let fc = channels as usize;
                let used = ch.min(fc);
                let bytes = sample_fmt::bytes(packed);
                // Sample i of channel c, as left-justified s32 or as f64.
                let at_byte = |c: usize, i: usize| -> *const u8 {
                    if planar { (*data.add(c)).add(i * bytes) } else { (*data).add((i * fc + c) * bytes) }
                };
                let int_of = |p: *const u8| -> i32 {
                    match packed {
                        sample_fmt::U8 => ((*p as i32) - 128) << 24,
                        sample_fmt::S16 => (std::ptr::read_unaligned(p.cast::<i16>()) as i32) << 16,
                        sample_fmt::S32 => std::ptr::read_unaligned(p.cast::<i32>()),
                        sample_fmt::S64 => (std::ptr::read_unaligned(p.cast::<i64>()) >> 32) as i32,
                        sample_fmt::FLT => to_s32(std::ptr::read_unaligned(p.cast::<f32>()) as f64),
                        _ => to_s32(std::ptr::read_unaligned(p.cast::<f64>())),
                    }
                };
                let float_of = |p: *const u8| -> f64 {
                    let v = match packed {
                        sample_fmt::FLT => std::ptr::read_unaligned(p.cast::<f32>()) as f64,
                        sample_fmt::DBL => std::ptr::read_unaligned(p.cast::<f64>()),
                        _ => return int_of(p) as f64 * (1.0 / 2_147_483_648.0),
                    };
                    // A NaN or an infinity would latch into the filters: silence.
                    if v.is_finite() { v } else { 0.0 }
                };
                if self.pending_pos >= self.pending_len() {
                    self.pending_pos = 0;
                    match &mut self.pending {
                        Pending::Int(v) => v.clear(),
                        Pending::Float(v) => v.clear(),
                    }
                }
                match &mut self.pending {
                    Pending::Int(v) => {
                        v.reserve((n - first) * ch);
                        for i in first..n {
                            for c in 0..ch {
                                v.push(if c < used { int_of(at_byte(c, i)) } else { 0 });
                            }
                        }
                    }
                    Pending::Float(v) => {
                        v.reserve((n - first) * ch);
                        for i in first..n {
                            for c in 0..ch {
                                v.push(if c < used { float_of(at_byte(c, i)) } else { 0.0 });
                            }
                        }
                    }
                }
                Ok(())
            })();
            (a.av_frame_unref)(frame);
            result
        }
    }

    fn pending_len(&self) -> usize {
        match &self.pending {
            Pending::Int(v) => v.len(),
            Pending::Float(v) => v.len(),
        }
    }

    /// Decodes until samples are pending; false at the end.
    fn refill(&mut self) -> Result<bool> {
        while self.pending_pos >= self.pending_len() {
            if !self.receive()? {
                return Ok(false);
            }
            self.take_frame()?;
        }
        Ok(true)
    }

    /// Up to `out.len() / channels` frames as left-justified s32; 0 at the end.
    pub fn read_i32(&mut self, out: &mut [i32]) -> Result<usize> {
        let ch = self.info.channels;
        let want = out.len() / ch * ch;
        let mut done = 0;
        while done < want {
            if !self.refill()? {
                break;
            }
            let n = (self.pending_len() - self.pending_pos).min(want - done);
            let from = self.pending_pos..self.pending_pos + n;
            match &self.pending {
                Pending::Int(v) => out[done..done + n].copy_from_slice(&v[from]),
                Pending::Float(v) => {
                    for (o, &x) in out[done..done + n].iter_mut().zip(&v[from]) {
                        *o = to_s32(x);
                    }
                }
            }
            self.pending_pos += n;
            done += n;
        }
        self.position = self.position.saturating_add((done / ch) as u64);
        Ok(done / ch)
    }

    /// Up to `out.len() / channels` frames as f64 (integers exactly, over
    /// 2^31; floats unclipped); 0 at the end.
    pub fn read_f64(&mut self, out: &mut [f64]) -> Result<usize> {
        let ch = self.info.channels;
        let want = out.len() / ch * ch;
        let mut done = 0;
        while done < want {
            if !self.refill()? {
                break;
            }
            let n = (self.pending_len() - self.pending_pos).min(want - done);
            let from = self.pending_pos..self.pending_pos + n;
            match &self.pending {
                Pending::Int(v) => {
                    for (o, &x) in out[done..done + n].iter_mut().zip(&v[from]) {
                        *o = x as f64 * (1.0 / 2_147_483_648.0);
                    }
                }
                Pending::Float(v) => out[done..done + n].copy_from_slice(&v[from]),
            }
            self.pending_pos += n;
            done += n;
        }
        self.position = self.position.saturating_add((done / ch) as u64);
        Ok(done / ch)
    }

    /// Frames handed out so far (or sought to).
    pub fn position(&self) -> u64 {
        self.position
    }

    /// Goes to `frame` (clamped to the stream's length); returns where it is.
    /// The demuxer goes to the seek point at or before it (before the
    /// preroll, for a lossy codec), and what is decoded before it is dropped.
    pub fn seek(&mut self, frame: u64) -> Result<u64> {
        let frame = match self.info.total_frames {
            Some(total) => frame.min(total),
            None => frame,
        };
        let a = self.api;
        // (A frame past any length: the end.)
        let frame = frame.min(i64::MAX as u64 / 4);
        let target = self.origin.saturating_add(frame as i64);
        if target.saturating_sub(self.preroll) <= self.origin {
            return self.from_the_start(frame);
        }
        // A demuxer that lands after the target (timestamps rounded to the
        // millisecond, a first packet before zero) is sent further back, up
        // to a few seconds before the start.
        let mut floor = self.origin.saturating_sub(4 * self.info.rate as i64);
        let mut at = target.saturating_sub(self.preroll).max(self.origin);
        for _ in 0..10 {
            let ts = self.frames_to_ts(at);
            // SAFETY: FFmpeg's API on this stream's contexts.
            let r = unsafe {
                let r = (a.av_seek_frame)(self.format, self.stream, ts, abi::AVSEEK_FLAG_BACKWARD);
                (a.avcodec_flush_buffers)(self.codec);
                if self.frame_ready {
                    (a.av_frame_unref)(self.frame);
                }
                r
            };
            self.frame_ready = false;
            self.pending_pos = self.pending_len();
            self.finished = false;
            self.input_done = false;
            self.next_at = None;
            self.skip_to = Some(target);
            self.position = frame;
            if r < 0 {
                if at == self.origin {
                    return Err(failed("cannot seek"));
                }
                // Some demuxers only go back to the start, none before it.
                floor = self.origin;
                at = self.origin;
                continue;
            }
            if !self.receive()? {
                return Ok(frame);
            }
            match self.landed_at() {
                Some(p) if p > target && at > floor => {
                    at = at.saturating_sub(target.saturating_sub(at).max(self.info.rate as i64)).max(floor);
                }
                _ => break,
            }
        }
        Ok(frame)
    }

    /// A seek near the start, with no room before it for the decoder to
    /// settle: the file opened again, as at first, and read up to `frame`.
    /// A flush is not a new decoder: FFmpeg 5 to 7's AC-3 decoder kept the
    /// last block's overlap (the first 224 samples came out wrong), and Opus
    /// drops its pre-skip only in a new one.
    fn from_the_start(&mut self, frame: u64) -> Result<u64> {
        // SAFETY: io_state is this stream's own until it is dropped.
        let source = unsafe {
            std::mem::replace(&mut (*self.io_state).source, Box::new(std::io::Cursor::new(Vec::<u8>::new())))
        };
        let mut fresh = Stream::open(source, &self.name)?;
        std::mem::swap(self, &mut fresh);
        drop(fresh);
        let ch = self.info.channels.max(1);
        let mut scratch = vec![0.0f64; 4096 * ch];
        let mut left = frame;
        while left > 0 {
            let n = (left as usize).min(4096);
            let got = self.read_f64(&mut scratch[..n * ch])?;
            if got == 0 {
                break;
            }
            left -= got as u64;
        }
        self.position = frame;
        Ok(frame)
    }
}

/// A float sample (1.0 = full scale) as left-justified s32, rounded and clipped.
fn to_s32(x: f64) -> i32 {
    const FULL: f64 = 2_147_483_648.0;
    if x.is_finite() { (x * FULL).round_ties_even().clamp(-FULL, FULL - 1.0) as i32 } else { 0 }
}
