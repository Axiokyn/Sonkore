//! ReplayGain 2.0 analysis: EBU R128 integrated loudness
//! with true peak, by the pure-Rust port of libebur128, over the samples
//! symphonia decodes. Files are never written: results go to the library.
//!
//! As earlier Sonkore versions: histogram mode (a few KB per track, so a
//! whole album is kept for the album value), mono counted as heard on two
//! speakers (DUAL_MONO), the side pair of a 7.1 file weighed like the back
//! pair (Mp090 / Mm090 on channels 6 and 7), samples as doubles so a lossy
//! file's peaks above full scale are measured, not clipped. A CD image
//! track (`<sheet>.cue#<n>`) is its stretch of the image file.
//!
//! Formats symphonia does not decode (Opus, Speex, Monkey's Audio, WavPack,
//! TTA, TAK, Musepack, WMA, AC-3, DTS, DSD) cannot be measured by this
//! crate alone: they come back not measured, as an unreadable file does in
//! earlier versions (which decoded them through FFmpeg), unless the
//! application lends its own decoders with `set_decoder_hook` (the engine's,
//! FFmpeg included), which are then tried first. Only the formats symphonia reads
//! are opened at all: its probe would otherwise find "MP3 frames" in the
//! noise of a file it does not know.

use std::fs::File;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use ebur128::{Channel, EbuR128, Mode};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{CODEC_TYPE_NULL, DecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use crate::cue::{self, CueSheet};

/// Decoded audio from a decoder this crate does not have (the engine's).
pub trait PcmSource: Send {
    fn rate(&self) -> u32;
    fn channels(&self) -> usize;
    /// Interleaved samples, 1.0 = full scale, not clipped; the frames read
    /// (0 at the end).
    fn read(&mut self, out: &mut [f64]) -> usize;
}

/// Opens a path (a file or `<sheet>.cue#<n>`) for measuring; None when it
/// cannot (then this crate's own decoding is tried).
pub type DecoderHook = Arc<dyn Fn(&str) -> Option<Box<dyn PcmSource>> + Send + Sync>;

static DECODER_HOOK: Mutex<Option<DecoderHook>> = Mutex::new(None);

/// Lends the analysis the application's decoders.
pub fn set_decoder_hook(hook: Option<DecoderHook>) {
    *DECODER_HOOK.lock().unwrap() = hook;
}

fn measure_source(mut src: Box<dyn PcmSource>, cancel: &AtomicBool) -> Option<(EbuR128, f64)> {
    let (channels, rate) = (src.channels(), src.rate());
    if channels == 0 || rate == 0 {
        return None;
    }
    let mut st = new_state(channels, rate)?;
    let mut buffer = vec![0.0f64; BLOCK_FRAMES * channels];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let got = src.read(&mut buffer).min(BLOCK_FRAMES);
        if got > 0 {
            st.add_frames_f64(&buffer[..got * channels]).ok()?;
        }
        if got < BLOCK_FRAMES {
            break;
        }
    }
    Some(finish_state(st, channels))
}

/// ReplayGain 2.0's reference level.
pub const REFERENCE_LUFS: f64 = -18.0;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Measurement {
    /// Decoded and not silent.
    pub ok: bool,
    /// EBU R128 integrated loudness.
    pub lufs: f64,
    /// True peak, linear (1.0 = full scale).
    pub peak: f64,
}

impl Measurement {
    pub fn gain_db(&self) -> f64 {
        REFERENCE_LUFS - self.lufs
    }
}

#[derive(Clone, Debug, Default)]
pub struct AlbumMeasurement {
    /// In the order of the paths.
    pub tracks: Vec<Measurement>,
    /// All tracks gated together, the highest peak.
    pub album: Measurement,
}

const BLOCK_FRAMES: usize = 16384;

/// What symphonia decodes (the engine's list).
const SYMPHONIA_FORMATS: [&str; 18] = [
    "flac", "wav", "wave", "mp3", "mp2", "mp1", "m4a", "m4b", "mp4", "aac", "caf", "ogg", "oga", "aif", "aiff", "aifc",
    "mka", "webm",
];

/// An AIFF's COMM frame count: the chunk headers read one by one, not the
/// whole file.
fn aiff_frames(path: &str) -> Option<u64> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let mut h = [0u8; 14];
    let mut p = 12u64;
    while p + 8 <= len {
        f.seek(SeekFrom::Start(p)).ok()?;
        f.read_exact(&mut h[..8]).ok()?;
        let size = u32::from_be_bytes([h[4], h[5], h[6], h[7]]) as u64;
        if &h[..4] == b"COMM" && p + 14 <= len {
            f.read_exact(&mut h[8..]).ok()?;
            return Some(u32::from_be_bytes([h[10], h[11], h[12], h[13]]) as u64);
        }
        p += 8 + size + (size & 1);
    }
    None
}

/// CD frames (1/75 s) to sample frames, rounded.
fn cd_to_frames(cd: u64, rate: u64) -> u64 {
    (cd * rate + 37) / 75
}

/// Feeds one file into a fresh R128 state, with its true peak; None when
/// it cannot be decoded (or `cancel` is set).
fn measure_file(path: &str, cancel: &AtomicBool) -> Option<(EbuR128, f64)> {
    let hook = DECODER_HOOK.lock().unwrap().clone();
    if let Some(src) = hook.and_then(|h| h(path)) {
        return measure_source(src, cancel);
    }
    // A CD image track: the image file, from its start to its end.
    let (file, range) = match cue::split_track_path(path) {
        Some((sheet, number)) => {
            let sheet = CueSheet::parse(Path::new(&sheet))?;
            let t = sheet.track(number)?;
            (t.file.to_string_lossy().into_owned(), Some((t.start, t.end)))
        }
        None => (path.to_string(), None),
    };
    let ext = Path::new(&file).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if ext == "w64" && range.is_none() {
        return measure_w64(&file, cancel);
    }
    if !SYMPHONIA_FORMATS.contains(&ext.as_str()) {
        return None;
    }
    // AIFF: the frames its COMM chunk states (symphonia reads past them).
    let aiff_frames = if matches!(ext.as_str(), "aif" | "aiff" | "aifc") { aiff_frames(&file) } else { None };
    // MP4: the edit list says what is played (the encoder's priming and
    // padding are not), as FFmpeg's decoding has it.
    let mp4_edit = if matches!(ext.as_str(), "m4a" | "m4b" | "mp4") {
        crate::metadata::probe::mp4::edit_range(Path::new(&file))
    } else {
        None
    };
    let stream = MediaSourceStream::new(Box::new(File::open(&file).ok()?), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = Path::new(&file).extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let options = FormatOptions { enable_gapless: true, ..Default::default() };
    let probed = symphonia::default::get_probe().format(&hint, stream, &options, &MetadataOptions::default()).ok()?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL && t.codec_params.sample_rate.is_some())?;
    let track_id = track.id;
    let mut params = track.codec_params.clone();
    // CAF's ALAC cookie comes wrapped in atoms ("frma", "alac", "chan"):
    // symphonia's decoder takes the 24-byte config inside.
    if params.codec == symphonia::core::codecs::CODEC_TYPE_ALAC
        && let Some(extra) = params.extra_data.as_deref()
        && extra.len() > 24
        && extra.get(4..8) == Some(b"frma")
        && let Some(i) = extra.windows(4).skip(12).position(|w| w == b"alac").map(|i| i + 12)
        && let Some(cookie) = extra.get(i + 8..i + 8 + 24)
    {
        params.extra_data = Some(cookie.to_vec().into_boxed_slice());
    }
    let mut decoder = symphonia::default::get_codecs().make(&params, &DecoderOptions::default()).ok()?;
    // The rate and channels as decoded (MP4 does not always state them).
    let mut st: Option<EbuR128> = None;
    let mut channels = 0usize;
    let (mut first, mut last) = (0u64, aiff_frames);
    let mut position: u64 = 0; // frames decoded so far
    let mut pending: Vec<f64> = Vec::new();
    let mut buffer: Option<SampleBuffer<f64>> = None;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymphoniaError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                break;
            }
            Err(SymphoniaError::ResetRequired) => {
                decoder.reset();
                continue;
            }
            Err(_) => break,
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(SymphoniaError::DecodeError(_)) => continue, // a damaged packet: skipped
            Err(_) => break,
        };
        let frames = decoded.frames() as u64;
        if frames == 0 {
            continue;
        }
        let spec = *decoded.spec();
        if st.is_none() {
            channels = spec.channels.count();
            let rate = spec.rate as u64;
            if channels == 0 || rate == 0 {
                return None;
            }
            st = Some(new_state(channels, rate as u32)?);
            if let Some((skip, played, timescale)) = mp4_edit.filter(|e| e.2 == rate) {
                let _ = timescale;
                first = skip;
                last = Some(skip + played);
            }
            if let Some((start, end)) = range {
                first = cd_to_frames(start, rate);
                if end > 0 {
                    last = Some(cd_to_frames(end, rate));
                }
            }
        }
        if spec.channels.count() != channels {
            continue;
        }
        let buf = buffer.get_or_insert_with(|| SampleBuffer::<f64>::new(decoded.capacity() as u64, spec));
        if buf.capacity() < decoded.capacity() {
            *buf = SampleBuffer::<f64>::new(decoded.capacity() as u64, spec);
        }
        buf.copy_interleaved_ref(decoded);
        let samples = buf.samples();
        // The stretch [first, last) of the file.
        let from = first.saturating_sub(position).min(frames);
        let to = match last {
            Some(l) => l.saturating_sub(position).min(frames),
            None => frames,
        };
        if to > from {
            pending.extend_from_slice(&samples[from as usize * channels..to as usize * channels]);
        }
        position += frames;
        if pending.len() >= BLOCK_FRAMES * channels {
            st.as_mut()?.add_frames_f64(&pending).ok()?;
            pending.clear();
        }
        if last.is_some_and(|l| position >= l) {
            break;
        }
    }
    let mut st = st?;
    if !pending.is_empty() {
        st.add_frames_f64(&pending).ok()?;
    }
    Some(finish_state(st, channels))
}

/// A fresh state for this layout, with Sonkore's channel weights.
fn new_state(channels: usize, rate: u32) -> Option<EbuR128> {
    let mut s = EbuR128::new(channels as u32, rate, Mode::I | Mode::TRUE_PEAK | Mode::HISTOGRAM).ok()?;
    if channels == 1 {
        s.set_channel(0, Channel::DualMono).ok()?;
    }
    if channels == 8 {
        s.set_channel(6, Channel::Mp090).ok()?;
        s.set_channel(7, Channel::Mm090).ok()?;
    }
    Some(s)
}

fn finish_state(st: EbuR128, channels: usize) -> (EbuR128, f64) {
    let mut peak = 0.0f64;
    for c in 0..channels as u32 {
        if let Ok(p) = st.true_peak(c) {
            peak = peak.max(p);
        }
    }
    (st, peak)
}

/// Sony Wave64 with PCM, which symphonia does not read: its samples as
/// FFmpeg's PCM decoder gives them (integers over 2^(bits-1)).
fn measure_w64(path: &str, cancel: &AtomicBool) -> Option<(EbuR128, f64)> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = File::open(path).ok()?;
    let size = f.metadata().ok()?.len();
    let mut pos = 40u64;
    let (mut tag, mut channels, mut rate, mut align, mut bits) = (0u16, 0usize, 0u32, 0usize, 0u16);
    let mut data: Option<(u64, u64)> = None;
    while pos + 24 <= size {
        let mut h = [0u8; 24];
        f.seek(SeekFrom::Start(pos)).ok()?;
        f.read_exact(&mut h).ok()?;
        let len = u64::from_le_bytes(h[16..24].try_into().ok()?);
        if len < 24 {
            break;
        }
        if &h[..4] == b"fmt " {
            let mut b = [0u8; 40];
            let n = f.read(&mut b).ok()?;
            if n < 16 {
                return None;
            }
            tag = u16::from_le_bytes([b[0], b[1]]);
            channels = u16::from_le_bytes([b[2], b[3]]) as usize;
            rate = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
            align = u16::from_le_bytes([b[12], b[13]]) as usize;
            bits = u16::from_le_bytes([b[14], b[15]]);
            if tag == 0xFFFE && n >= 26 {
                tag = u16::from_le_bytes([b[24], b[25]]);
            }
        } else if &h[..4] == b"data" {
            data = Some((pos + 24, len - 24));
        }
        pos += (len + 7) & !7;
    }
    let (start, len) = data?;
    let width = (bits as usize).div_ceil(8);
    if channels == 0 || rate == 0 || width == 0 || align != width * channels || !(tag == 1 || tag == 3) {
        return None;
    }
    let mut st = new_state(channels, rate)?;
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut left = len - len % align as u64;
    let mut raw = vec![0u8; BLOCK_FRAMES * align];
    let mut samples = Vec::with_capacity(BLOCK_FRAMES * channels);
    while left > 0 {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let n = (left.min(raw.len() as u64)) as usize;
        f.read_exact(&mut raw[..n]).ok()?;
        left -= n as u64;
        samples.clear();
        for c in raw[..n].chunks_exact(width) {
            let v = match (tag, width) {
                (3, 4) => f32::from_le_bytes([c[0], c[1], c[2], c[3]]) as f64,
                (3, 8) => f64::from_le_bytes(c.try_into().ok()?),
                (_, 1) => (c[0] as f64 - 128.0) / 128.0,
                (_, 2) => i16::from_le_bytes([c[0], c[1]]) as f64 / 32768.0,
                (_, 3) => (((c[0] as i32) << 8 | (c[1] as i32) << 16 | (c[2] as i32) << 24) >> 8) as f64 / 8388608.0,
                (_, 4) => i32::from_le_bytes([c[0], c[1], c[2], c[3]]) as f64 / 2147483648.0,
                _ => return None,
            };
            samples.push(v);
        }
        st.add_frames_f64(&samples).ok()?;
    }
    Some(finish_state(st, channels))
}

/// Decodes every file once. The album value is measured over all its
/// tracks together, as ReplayGain asks, not averaged. Returns early (with
/// what is not measured left `ok = false`) once `cancel` is set.
pub fn measure_album(paths: &[String], cancel: &AtomicBool) -> AlbumMeasurement {
    let mut result =
        AlbumMeasurement { tracks: vec![Measurement::default(); paths.len()], album: Measurement::default() };
    let mut states: Vec<EbuR128> = Vec::new();
    for (i, path) in paths.iter().enumerate() {
        let measured = measure_file(path, cancel);
        if cancel.load(Ordering::Relaxed) {
            return result;
        }
        let Some((st, peak)) = measured else { continue };
        // Silence (below the absolute gate) has no loudness: no gain for it.
        if let Ok(lufs) = st.loudness_global()
            && lufs.is_finite()
        {
            result.tracks[i] = Measurement { ok: true, lufs, peak };
            result.album.peak = result.album.peak.max(peak);
            states.push(st);
        }
    }
    if !states.is_empty()
        && let Ok(lufs) = EbuR128::loudness_global_multiple(states.iter())
        && lufs.is_finite()
    {
        result.album.ok = true;
        result.album.lufs = lufs;
    }
    result
}

#[cfg(test)]
mod tests {
    #[test]
    fn aiff_frame_count_after_other_chunks() {
        let mut f = b"FORM\0\0\0\0AIFF".to_vec();
        f.extend_from_slice(b"ANNO\0\0\0\x03abc\0"); // odd size: a pad byte
        f.extend_from_slice(b"SSND\0\0\0\x10");
        f.extend_from_slice(&[0; 16]);
        f.extend_from_slice(b"COMM\0\0\0\x12\0\x02");
        f.extend_from_slice(&123_456u32.to_be_bytes());
        f.extend_from_slice(&[0; 12]);
        let path = std::env::temp_dir().join(format!("sonkore-aiff-{}.aiff", std::process::id()));
        std::fs::write(&path, &f).unwrap();
        let p = path.to_str().unwrap();
        assert_eq!(super::aiff_frames(p), Some(123_456));
        std::fs::write(&path, &f[..f.len() - 16]).unwrap(); // COMM cut short
        assert_eq!(super::aiff_frames(p), None);
        let _ = std::fs::remove_file(&path);
    }
}
