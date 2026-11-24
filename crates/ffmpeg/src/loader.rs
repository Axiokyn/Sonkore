//! Opening libavutil, libavcodec and libavformat at run time, the first
//! time a file needs them: dlopen on Linux (libavformat.so.62…), LoadLibrary
//! on Windows (avformat-62.dll… next to the executable or on the PATH).
//! Each FFmpeg release is tried newest first, all three libraries of the
//! same release, and kept only if their version functions say the majors
//! abi.rs knows. A version or layout it does not know is refused, never
//! guessed.

use std::ffi::{CStr, c_char, c_int, c_uint, c_void};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use libloading::Library;

use crate::abi::{self, CodecLayout, FormatLayout, UtilLayout};

pub(crate) type P = *mut c_void;
pub(crate) type ReadFn = unsafe extern "C" fn(P, *mut u8, c_int) -> c_int;
pub(crate) type SeekFn = unsafe extern "C" fn(P, i64, c_int) -> i64;

/// The functions Sonkore calls, all with the same signature in every
/// supported version.
pub(crate) struct Api {
    // libavutil
    pub av_dict_set: unsafe extern "C" fn(*mut P, *const c_char, *const c_char, c_int) -> c_int,
    pub av_dict_free: unsafe extern "C" fn(*mut P),
    pub av_frame_alloc: unsafe extern "C" fn() -> P,
    pub av_frame_free: unsafe extern "C" fn(*mut P),
    pub av_frame_unref: unsafe extern "C" fn(P),
    pub av_malloc: unsafe extern "C" fn(usize) -> P,
    pub av_free: unsafe extern "C" fn(P),
    pub av_opt_set_int: unsafe extern "C" fn(P, *const c_char, i64, c_int) -> c_int,
    pub av_opt_set: unsafe extern "C" fn(P, *const c_char, *const c_char, c_int) -> c_int,
    // libavcodec
    pub avcodec_alloc_context3: unsafe extern "C" fn(*const c_void) -> P,
    pub avcodec_free_context: unsafe extern "C" fn(*mut P),
    pub avcodec_parameters_to_context: unsafe extern "C" fn(P, *const c_void) -> c_int,
    pub avcodec_open2: unsafe extern "C" fn(P, *const c_void, *mut P) -> c_int,
    pub avcodec_send_packet: unsafe extern "C" fn(P, *const c_void) -> c_int,
    pub avcodec_receive_frame: unsafe extern "C" fn(P, P) -> c_int,
    pub avcodec_flush_buffers: unsafe extern "C" fn(P),
    pub av_packet_alloc: unsafe extern "C" fn() -> P,
    pub av_packet_free: unsafe extern "C" fn(*mut P),
    pub av_packet_unref: unsafe extern "C" fn(P),
    pub avcodec_get_name: unsafe extern "C" fn(c_int) -> *const c_char,
    // libavformat
    pub avformat_alloc_context: unsafe extern "C" fn() -> P,
    pub avformat_open_input: unsafe extern "C" fn(*mut P, *const c_char, *const c_void, *mut P) -> c_int,
    pub avformat_find_stream_info: unsafe extern "C" fn(P, *mut P) -> c_int,
    pub av_find_best_stream: unsafe extern "C" fn(P, c_int, c_int, c_int, *mut *const c_void, c_int) -> c_int,
    pub av_read_frame: unsafe extern "C" fn(P, P) -> c_int,
    pub av_seek_frame: unsafe extern "C" fn(P, c_int, i64, c_int) -> c_int,
    pub avformat_close_input: unsafe extern "C" fn(*mut P),
    pub avio_alloc_context:
        unsafe extern "C" fn(*mut u8, c_int, c_int, P, Option<ReadFn>, Option<ReadFn>, Option<SeekFn>) -> P,
    pub avio_context_free: unsafe extern "C" fn(*mut P),

    pub format: &'static FormatLayout,
    pub codec: &'static CodecLayout,
    pub util: &'static UtilLayout,
    /// "libavformat 62.12 · libavcodec 62.28 · libavutil 60.26".
    pub versions: String,
    // Kept loaded for the life of the process.
    _libs: [Library; 3],
}

static API: OnceLock<Result<Api, String>> = OnceLock::new();
static DISABLED: AtomicBool = AtomicBool::new(false);

/// FFmpeg as if it were not installed (tests).
pub fn set_disabled(off: bool) {
    DISABLED.store(off, Ordering::Relaxed);
}

/// The loaded libraries, loading them the first time.
pub(crate) fn api() -> Result<&'static Api, String> {
    if DISABLED.load(Ordering::Relaxed) {
        return Err("FFmpeg is turned off".into());
    }
    API.get_or_init(|| {
        let loaded = load();
        if let Err(why) = &loaded {
            // Once per run, for a terminal: each file that needs it just fails.
            eprintln!(
                "sonkore: FFmpeg not loaded ({why}): Opus, APE, WavPack, TTA, TAK, Musepack, WMA, AC-3 and DTS files will not play"
            );
        }
        loaded
    })
    .as_ref()
    .map_err(Clone::clone)
}

fn file_name(lib: &str, major: u32) -> String {
    if cfg!(windows) {
        format!("{lib}-{major}.dll")
    } else if cfg!(target_os = "macos") {
        format!("lib{lib}.{major}.dylib")
    } else {
        format!("lib{lib}.so.{major}")
    }
}

fn open(lib: &str, major: u32) -> Result<Library, String> {
    let name = file_name(lib, major);
    // SAFETY: FFmpeg's libraries run no initialisers that depend on the caller.
    unsafe { Library::new(&name) }.map_err(|_| name)
}

/// A function of `lib`, as the type the field it goes to has.
///
/// SAFETY: the caller states the C signature `T` stands for.
unsafe fn get<T: Copy>(lib: &Library, name: &str) -> Result<T, String> {
    let mut c = name.as_bytes().to_vec();
    c.push(0);
    // SAFETY: as the caller says.
    let symbol = unsafe { lib.get::<T>(&c) }.map_err(|_| format!("{name} missing"))?;
    Ok(*symbol)
}

fn load() -> Result<Api, String> {
    if cfg!(not(target_pointer_width = "64")) {
        return Err("only 64-bit FFmpeg builds are supported".into());
    }
    let mut missing = Vec::new();
    for &(f, c, u) in &abi::RELEASES {
        match load_release(f, c, u) {
            Ok(api) => return Ok(api),
            Err(why) => missing.push(why),
        }
    }
    Err(missing.join("; "))
}

fn version_text(name: &str, v: c_uint) -> String {
    format!("{name} {}.{}", v >> 16, (v >> 8) & 0xff)
}

fn load_release(f: u32, c: u32, u: u32) -> Result<Api, String> {
    let util = open("avutil", u)?;
    let codec = open("avcodec", c)?;
    let format = open("avformat", f)?;
    // SAFETY: each name is given its C signature (the same in FFmpeg 5 to 8).
    unsafe {
        let util_version: unsafe extern "C" fn() -> c_uint = get(&util, "avutil_version")?;
        let codec_version: unsafe extern "C" fn() -> c_uint = get(&codec, "avcodec_version")?;
        let format_version: unsafe extern "C" fn() -> c_uint = get(&format, "avformat_version")?;
        let (vu, vc, vf) = (util_version(), codec_version(), format_version());
        if (vu >> 16, vc >> 16, vf >> 16) != (u, c, f) {
            return Err(format!("unexpected versions {}, {}, {}", vf >> 16, vc >> 16, vu >> 16));
        }
        let (Some(format_layout), Some(codec_layout), Some(util_layout)) =
            (abi::format_layout(f), abi::codec_layout(c), abi::util_layout(u))
        else {
            return Err(format!("no layout for libavformat {f}"));
        };
        let set_level: unsafe extern "C" fn(c_int) = get(&util, "av_log_set_level")?;
        let api = Api {
            av_dict_set: get(&util, "av_dict_set")?,
            av_dict_free: get(&util, "av_dict_free")?,
            av_frame_alloc: get(&util, "av_frame_alloc")?,
            av_frame_free: get(&util, "av_frame_free")?,
            av_frame_unref: get(&util, "av_frame_unref")?,
            av_malloc: get(&util, "av_malloc")?,
            av_free: get(&util, "av_free")?,
            av_opt_set_int: get(&util, "av_opt_set_int")?,
            av_opt_set: get(&util, "av_opt_set")?,
            avcodec_alloc_context3: get(&codec, "avcodec_alloc_context3")?,
            avcodec_free_context: get(&codec, "avcodec_free_context")?,
            avcodec_parameters_to_context: get(&codec, "avcodec_parameters_to_context")?,
            avcodec_open2: get(&codec, "avcodec_open2")?,
            avcodec_send_packet: get(&codec, "avcodec_send_packet")?,
            avcodec_receive_frame: get(&codec, "avcodec_receive_frame")?,
            avcodec_flush_buffers: get(&codec, "avcodec_flush_buffers")?,
            av_packet_alloc: get(&codec, "av_packet_alloc")?,
            av_packet_free: get(&codec, "av_packet_free")?,
            av_packet_unref: get(&codec, "av_packet_unref")?,
            avcodec_get_name: get(&codec, "avcodec_get_name")?,
            avformat_alloc_context: get(&format, "avformat_alloc_context")?,
            avformat_open_input: get(&format, "avformat_open_input")?,
            avformat_find_stream_info: get(&format, "avformat_find_stream_info")?,
            av_find_best_stream: get(&format, "av_find_best_stream")?,
            av_read_frame: get(&format, "av_read_frame")?,
            av_seek_frame: get(&format, "av_seek_frame")?,
            avformat_close_input: get(&format, "avformat_close_input")?,
            avio_alloc_context: get(&format, "avio_alloc_context")?,
            avio_context_free: get(&format, "avio_context_free")?,
            format: format_layout,
            codec: codec_layout,
            util: util_layout,
            versions: format!(
                "{} · {} · {}",
                version_text("libavformat", vf),
                version_text("libavcodec", vc),
                version_text("libavutil", vu)
            ),
            _libs: [util, codec, format],
        };
        api.check_codec_layout()?;
        // FFmpeg's own messages (a damaged file's) only when asked for.
        let level =
            if std::env::var_os("SONKORE_FFMPEG_LOG").is_some() { abi::AV_LOG_ERROR } else { abi::AV_LOG_QUIET };
        set_level(level);
        Ok(api)
    }
}

impl Api {
    /// The codec context's offsets, checked against the library itself:
    /// fields set through the options API must be read back where abi.rs
    /// says they are.
    fn check_codec_layout(&self) -> Result<(), String> {
        let l = self.codec;
        // SAFETY: a context of this library, read inside it at offsets of its own major.
        unsafe {
            let mut ctx = (self.avcodec_alloc_context3)(std::ptr::null());
            if ctx.is_null() {
                return Err("out of memory".into());
            }
            let mut ok = (self.av_opt_set_int)(ctx, c"ar".as_ptr(), 43_210, 0) >= 0
                && read::<i32>(ctx, l.ctx_sample_rate) == 43_210
                && (self.av_opt_set_int)(ctx, c"bits_per_raw_sample".as_ptr(), 21, 0) >= 0
                && read::<i32>(ctx, l.ctx_bits_raw) == 21;
            if l.ch_layout {
                ok &= (self.av_opt_set)(ctx, c"ch_layout".as_ptr(), c"5.1".as_ptr(), 0) >= 0
                    && read::<i32>(ctx, l.ctx_channels) == 6;
            } else {
                ok &= (self.av_opt_set_int)(ctx, c"ac".as_ptr(), 5, 0) >= 0 && read::<i32>(ctx, l.ctx_channels) == 5;
            }
            (self.avcodec_free_context)(&mut ctx);
            if ok { Ok(()) } else { Err("its codec context is not laid out as expected".into()) }
        }
    }
}

/// A field of an FFmpeg struct.
///
/// SAFETY: `base` points to the struct, `offset` is that field's in this version.
pub(crate) unsafe fn read<T: Copy>(base: *const c_void, offset: usize) -> T {
    // SAFETY: as the caller says.
    unsafe { std::ptr::read_unaligned(base.cast::<u8>().add(offset).cast::<T>()) }
}

/// Sets a field of an FFmpeg struct.
///
/// SAFETY: as `read`, and the struct is writable there.
pub(crate) unsafe fn write<T: Copy>(base: P, offset: usize, value: T) {
    // SAFETY: as the caller says.
    unsafe { std::ptr::write_unaligned(base.cast::<u8>().add(offset).cast::<T>(), value) }
}

/// A C string FFmpeg owns, as text.
///
/// SAFETY: `p` is null or a NUL-terminated string.
pub(crate) unsafe fn text(p: *const c_char) -> String {
    if p.is_null() { String::new() } else { unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned() }
}
