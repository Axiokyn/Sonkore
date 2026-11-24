//! Decoding: every format to interleaved
//! samples as the engine takes them — integer sources as left-justified
//! s32 (16-bit << 16, 24-bit << 8), so a lossless file reaches the device
//! untouched; float and lossy sources as f64, never clipped. Seeks land on
//! the exact sample. MP3 / AAC encoder delay and padding are trimmed (gapless).

use std::fs::File;
use std::path::Path;

use symphonia::core::audio::{AudioBufferRef, Signal};
use symphonia::core::codecs::{self, CODEC_TYPE_NULL, CodecType, DecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::TimeBase;

use crate::quantize::{S32_FULL_SCALE, S32_INV_FULL_SCALE};

#[derive(Debug)]
pub struct DecodeError(pub String);

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for DecodeError {}

impl From<SymphoniaError> for DecodeError {
    fn from(e: SymphoniaError) -> Self {
        DecodeError(e.to_string())
    }
}

impl From<std::io::Error> for DecodeError {
    fn from(e: std::io::Error) -> Self {
        DecodeError(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, DecodeError>;

/// What a stream is.
#[derive(Clone, Debug, PartialEq)]
pub struct StreamInfo {
    pub rate: u32,
    pub channels: usize,
    /// The source's integer bits (16, 24…); 0 for float and lossy sources.
    pub bits: u32,
    /// Samples are floating point (float files, lossy codecs).
    pub float: bool,
    /// Integer and lossless: the samples are the music, bit for bit.
    pub lossless: bool,
    pub total_frames: Option<u64>,
    /// The codec's short name ("FLAC", "MP3", "AAC"…).
    pub codec: String,
}

impl StreamInfo {
    /// The bits the bit-perfect check compares with the device's: the source's,
    /// else 32 for float and lossy ones.
    pub fn source_bits(&self) -> u32 {
        if self.bits > 0 && !self.float { self.bits } else { 32 }
    }

    pub fn duration_seconds(&self) -> Option<f64> {
        self.total_frames.map(|f| f as f64 / self.rate.max(1) as f64)
    }
}

/// A decoded stream.
pub trait Decoder: Send {
    fn info(&self) -> &StreamInfo;
    /// Up to `out.len() / channels` frames as left-justified s32; 0 at the end.
    fn read_i32(&mut self, out: &mut [i32]) -> Result<usize>;
    /// Up to `out.len() / channels` frames as f64 (s32 / 2^31 for integer
    /// sources: exact); 0 at the end.
    fn read_f64(&mut self, out: &mut [f64]) -> Result<usize>;
    /// Goes to `frame` (clamped to the stream); returns where it is.
    fn seek(&mut self, frame: u64) -> Result<u64>;
}

static FROM_MEMORY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The largest file read whole into memory (playback/playFromMemory).
const MEMORY_LIMIT: u64 = 2 << 30;

/// Settings › Audio › Play from memory: each file opened from now on is read
/// whole into RAM first (up to 2 GB), so playback never waits on the disk.
pub fn set_from_memory(on: bool) {
    FROM_MEMORY.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// The file to decode: in memory when asked and it fits, else on disk.
enum FileSource {
    Disk(File),
    Memory(Prefetch),
}

fn open_source(path: &Path) -> std::io::Result<FileSource> {
    let file = File::open(path)?;
    if !FROM_MEMORY.load(std::sync::atomic::Ordering::Relaxed) {
        return Ok(FileSource::Disk(file));
    }
    let size = file.metadata()?.len();
    if size > MEMORY_LIMIT {
        return Ok(FileSource::Disk(file));
    }
    Ok(FileSource::Memory(Prefetch::start(file, size)))
}

/// A file read whole into memory by a thread of its own, then closed: the
/// decoder never waits for it. What is loaded is read from memory; anything
/// past that (the headers at first, or a seek far into a big file on a slow
/// disk) is read from the file where it is. The loading stops when the
/// decoder is dropped (a track skipped before it was read whole).
struct Prefetch {
    shared: std::sync::Arc<PrefetchShared>,
    pos: u64,
    len: u64,
}

struct PrefetchShared {
    loaded: std::sync::Mutex<Loaded>,
}

struct Loaded {
    /// What the thread has read so far, from the start of the file.
    data: Vec<u8>,
    /// None once all of it is in memory (the file is closed then).
    file: Option<std::sync::Arc<File>>,
}

/// Reads at `offset` without moving a position the other side relies on:
/// both the loading thread and the decoder read this way.
fn read_at(file: &File, buf: &mut [u8], offset: u64) -> std::io::Result<usize> {
    #[cfg(unix)]
    {
        std::os::unix::fs::FileExt::read_at(file, buf, offset)
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::FileExt::seek_read(file, buf, offset)
    }
}

impl Prefetch {
    fn start(file: File, len: u64) -> Prefetch {
        Self::start_slow(file, len, std::time::Duration::ZERO)
    }

    /// The loading thread's side, to see when it has ended (tests).
    #[cfg(test)]
    fn watch(&self) -> std::sync::Weak<PrefetchShared> {
        std::sync::Arc::downgrade(&self.shared)
    }

    /// `delay` before each chunk: a slow disk, for the tests.
    fn start_slow(file: File, len: u64, delay: std::time::Duration) -> Prefetch {
        let file = std::sync::Arc::new(file);
        let shared = std::sync::Arc::new(PrefetchShared {
            loaded: std::sync::Mutex::new(Loaded { data: Vec::with_capacity(len as usize), file: Some(file.clone()) }),
        });
        let filling = shared.clone();
        std::thread::spawn(move || {
            let mut chunk = vec![0u8; 1 << 20];
            let mut at = 0u64;
            // Only this thread holds it: the decoder is gone, stop.
            while std::sync::Arc::strong_count(&filling) > 1 {
                if !delay.is_zero() {
                    std::thread::sleep(delay);
                }
                let n = match read_at(&file, &mut chunk, at) {
                    Ok(n) => n,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break, // the rest is read from the file as asked
                };
                let mut loaded = filling.loaded.lock().unwrap_or_else(|e| e.into_inner());
                if n == 0 {
                    // All of it in memory: the file closes (with this
                    // thread's handle, or a read from it that is under way).
                    loaded.file = None;
                    break;
                }
                loaded.data.extend_from_slice(&chunk[..n]);
                at += n as u64;
            }
        });
        Prefetch { shared, pos: 0, len }
    }
}

impl std::io::Read for Prefetch {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let file = {
            let loaded = self.shared.loaded.lock().unwrap_or_else(|e| e.into_inner());
            if self.pos < loaded.data.len() as u64 {
                let start = self.pos as usize;
                let n = buf.len().min(loaded.data.len() - start);
                buf[..n].copy_from_slice(&loaded.data[start..start + n]);
                self.pos += n as u64;
                return Ok(n);
            }
            match &loaded.file {
                Some(file) => file.clone(),
                None => return Ok(0), // all loaded: the end
            }
        };
        // Not loaded yet: from the file, without waiting for the rest.
        let n = read_at(&file, buf, self.pos)?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl std::io::Seek for Prefetch {
    fn seek(&mut self, to: std::io::SeekFrom) -> std::io::Result<u64> {
        let pos = match to {
            std::io::SeekFrom::Start(p) => p as i128,
            std::io::SeekFrom::End(d) => self.len as i128 + d as i128,
            std::io::SeekFrom::Current(d) => self.pos as i128 + d as i128,
        };
        if pos < 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "seek before the start"));
        }
        self.pos = pos as u64;
        Ok(self.pos)
    }
}

impl symphonia::core::io::MediaSource for Prefetch {
    fn is_seekable(&self) -> bool {
        true
    }
    fn byte_len(&self) -> Option<u64> {
        Some(self.len)
    }
}

/// Opens a file with the decoder for its format; "<sheet>.cue#<n>" opens
/// that track of the CD image the sheet describes.
pub fn open(path: &Path) -> Result<Box<dyn Decoder>> {
    if let Some(track) = sonkore_hdcd::cue::open_track(path) {
        let track = track.map_err(DecodeError)?;
        let image = SymphoniaDecoder::open(&track.file)?;
        let rate = image.info().rate;
        let range = track.range(rate, image.info().total_frames);
        return Ok(Box::new(RangeDecoder::new(Box::new(image), range)?));
    }
    if path.extension().and_then(|e| e.to_str()).is_some_and(|_| sonkore_dsd::is_dsd_extension(path)) {
        return Ok(Box::new(DsdPcm::open(path)?));
    }
    Ok(Box::new(SymphoniaDecoder::open(path)?))
}

/// DSD (DSF, DSDIFF, DST) converted to PCM: 352.8 or
/// 176.4 kHz floating point, for outputs that do not take DSD.
pub struct DsdPcm {
    inner: sonkore_dsd::DsdToPcm,
    info: StreamInfo,
}

impl DsdPcm {
    pub fn open(path: &Path) -> Result<Self> {
        use sonkore_dsd::Decoder as _;
        let source: Box<dyn sonkore_dsd::Source> = match open_source(path).map_err(|e| DecodeError(e.to_string()))? {
            FileSource::Disk(f) => Box::new(f),
            FileSource::Memory(m) => Box::new(m),
        };
        let inner = sonkore_dsd::DsdToPcm::from_source(source).map_err(|e| DecodeError(e.to_string()))?;
        let i = inner.info();
        let info = StreamInfo {
            rate: i.rate,
            channels: i.channels,
            bits: 0,
            float: true,
            lossless: false,
            total_frames: Some(i.total_frames),
            codec: i.codec.clone(),
        };
        Ok(DsdPcm { inner, info })
    }
}

impl Decoder for DsdPcm {
    fn info(&self) -> &StreamInfo {
        &self.info
    }
    fn read_i32(&mut self, out: &mut [i32]) -> Result<usize> {
        use sonkore_dsd::Decoder as _;
        self.inner.read_i32(out).map_err(|e| DecodeError(e.to_string()))
    }
    fn read_f64(&mut self, out: &mut [f64]) -> Result<usize> {
        use sonkore_dsd::Decoder as _;
        self.inner.read_f64(out).map_err(|e| DecodeError(e.to_string()))
    }
    fn seek(&mut self, frame: u64) -> Result<u64> {
        use sonkore_dsd::Decoder as _;
        self.inner.seek(frame).map_err(|e| DecodeError(e.to_string()))
    }
}

/// DSD as packed words for a device that takes DSD (in
/// DoP or native packing): the 1-bit stream untouched, at the frame rate of
/// the packing (DSD rate / 16 or / 32).
pub struct DsdRaw {
    inner: sonkore_dsd::DsdDecoder,
    info: StreamInfo,
}

impl DsdRaw {
    pub fn open(path: &Path, packing: sonkore_dsd::DsdPacking) -> Result<Self> {
        use sonkore_dsd::Decoder as _;
        let source: Box<dyn sonkore_dsd::Source> = match open_source(path).map_err(|e| DecodeError(e.to_string()))? {
            FileSource::Disk(f) => Box::new(f),
            FileSource::Memory(m) => Box::new(m),
        };
        let inner = sonkore_dsd::DsdDecoder::from_source(source, packing).map_err(|e| DecodeError(e.to_string()))?;
        let i = inner.info();
        let info = StreamInfo {
            rate: i.rate,
            channels: i.channels,
            bits: i.bits,
            float: false,
            lossless: true,
            total_frames: Some(i.total_frames),
            codec: i.codec.clone(),
        };
        Ok(DsdRaw { inner, info })
    }

    pub fn packing(&self) -> sonkore_dsd::DsdPacking {
        self.inner.packing()
    }
}

impl Decoder for DsdRaw {
    fn info(&self) -> &StreamInfo {
        &self.info
    }
    fn read_i32(&mut self, out: &mut [i32]) -> Result<usize> {
        use sonkore_dsd::Decoder as _;
        self.inner.read_i32(out).map_err(|e| DecodeError(e.to_string()))
    }
    fn read_f64(&mut self, out: &mut [f64]) -> Result<usize> {
        use sonkore_dsd::Decoder as _;
        self.inner.read_f64(out).map_err(|e| DecodeError(e.to_string()))
    }
    fn seek(&mut self, frame: u64) -> Result<u64> {
        use sonkore_dsd::Decoder as _;
        self.inner.seek(frame).map_err(|e| DecodeError(e.to_string()))
    }
}

/// HDCD decoding (libhdcd) over a 16-bit stereo source that carries HDCD
/// codes: 32-bit output at -6 dB, so peak extension has room above the
/// original full scale. Reset per track and per seek.
pub struct HdcdDecoder {
    inner: Box<dyn Decoder>,
    hdcd: sonkore_hdcd::hdcd::Hdcd,
    info: StreamInfo,
    ints: Vec<i32>,
}

impl HdcdDecoder {
    /// `inner` when it is eligible and its first 5 s carry HDCD codes.
    pub fn wrap(mut inner: Box<dyn Decoder>) -> Box<dyn Decoder> {
        let i = inner.info().clone();
        if !sonkore_hdcd::hdcd::eligible(i.channels, i.bits) || i.float {
            return inner;
        }
        let found = sonkore_hdcd::hdcd::detect(i.rate, |buf| inner.read_i32(buf).unwrap_or(0));
        if inner.seek(0).is_err() || !found {
            return inner;
        }
        let mut hdcd = sonkore_hdcd::hdcd::Hdcd::new();
        hdcd.reset_ext(i.rate, 16);
        let info = StreamInfo { bits: 24, codec: format!("{} HDCD", i.codec), ..i };
        Box::new(HdcdDecoder { inner, hdcd, info, ints: Vec::new() })
    }
}

impl Decoder for HdcdDecoder {
    fn info(&self) -> &StreamInfo {
        &self.info
    }
    fn read_i32(&mut self, out: &mut [i32]) -> Result<usize> {
        let n = self.inner.read_i32(out)?;
        self.hdcd.process_s32(&mut out[..n * 2]);
        Ok(n)
    }
    fn read_f64(&mut self, out: &mut [f64]) -> Result<usize> {
        self.ints.resize(out.len(), 0);
        let mut ints = std::mem::take(&mut self.ints);
        let n = self.read_i32(&mut ints)?;
        for (o, &s) in out.iter_mut().zip(&ints[..n * 2]) {
            *o = s as f64 * S32_INV_FULL_SCALE;
        }
        self.ints = ints;
        Ok(n)
    }
    fn seek(&mut self, frame: u64) -> Result<u64> {
        let at = self.inner.seek(frame)?;
        self.hdcd.reset_ext(self.info.rate, 16);
        Ok(at)
    }
}

/// One track of a CD image: the image's
/// frames from the track's start, for its length; seeks inside it.
pub struct RangeDecoder {
    inner: Box<dyn Decoder>,
    range: sonkore_hdcd::cue::TrackRange,
    info: StreamInfo,
    position: u64,
}

impl RangeDecoder {
    pub fn new(mut inner: Box<dyn Decoder>, range: sonkore_hdcd::cue::TrackRange) -> Result<Self> {
        let mut info = inner.info().clone();
        info.total_frames = Some(range.frames);
        inner.seek(range.start)?;
        Ok(RangeDecoder { inner, range, info, position: 0 })
    }
}

impl Decoder for RangeDecoder {
    fn info(&self) -> &StreamInfo {
        &self.info
    }
    fn read_i32(&mut self, out: &mut [i32]) -> Result<usize> {
        let ch = self.info.channels;
        let n = self.range.readable(self.position, (out.len() / ch) as u64) as usize;
        let got = self.inner.read_i32(&mut out[..n * ch])?;
        self.position += got as u64;
        Ok(got)
    }
    fn read_f64(&mut self, out: &mut [f64]) -> Result<usize> {
        let ch = self.info.channels;
        let n = self.range.readable(self.position, (out.len() / ch) as u64) as usize;
        let got = self.inner.read_f64(&mut out[..n * ch])?;
        self.position += got as u64;
        Ok(got)
    }
    fn seek(&mut self, frame: u64) -> Result<u64> {
        let (position, image) = self.range.seek(frame);
        self.inner.seek(image)?;
        self.position = position;
        Ok(position)
    }
}

/// The extensions symphonia plays.
pub fn supported_extension(ext: &str) -> bool {
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "flac"
            | "wav"
            | "wave"
            | "mp3"
            | "mp2"
            | "mp1"
            | "m4a"
            | "m4b"
            | "mp4"
            | "aac"
            | "caf"
            | "ogg"
            | "oga"
            | "aif"
            | "aiff"
            | "aifc"
            | "mka"
            | "webm"
    )
}

// --- symphonia -------------------------------------------------------------------

enum Samples {
    Int(Vec<i32>),
    Float(Vec<f64>),
}

pub struct SymphoniaDecoder {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn codecs::Decoder>,
    track_id: u32,
    time_base: Option<TimeBase>,
    info: StreamInfo,
    pending: Samples,   // interleaved, decoded and not read yet
    pending_pos: usize, // samples of `pending` already read
    skip: u64,          // frames still to drop after a seek
    position: u64,      // frames handed out (or skipped to)
    finished: bool,
    lead: u64,           // MP4: the encoder's priming, hidden
    length: Option<u64>, // MP4: the frames the edit list plays
    sample_bits: u32,    // the decoded samples' integer width
}

fn codec_name(codec: CodecType) -> &'static str {
    let short = symphonia::default::get_codecs().get_codec(codec).map(|d| d.short_name).unwrap_or("");
    match short {
        "flac" => "FLAC",
        "alac" => "ALAC",
        "mp3" => "MP3",
        "mp2" => "MP2",
        "mp1" => "MP1",
        "aac" => "AAC",
        "vorbis" => "Vorbis",
        s if s.starts_with("pcm") => "PCM",
        s if s.starts_with("adpcm") => "ADPCM",
        _ => "",
    }
}

fn is_float_pcm(codec: CodecType) -> bool {
    matches!(
        codec,
        codecs::CODEC_TYPE_PCM_F32LE
            | codecs::CODEC_TYPE_PCM_F32BE
            | codecs::CODEC_TYPE_PCM_F64LE
            | codecs::CODEC_TYPE_PCM_F64BE
    )
}

impl SymphoniaDecoder {
    pub fn open(path: &Path) -> Result<Self> {
        let source: Box<dyn symphonia::core::io::MediaSource> = match open_source(path)? {
            FileSource::Disk(f) => Box::new(f),
            FileSource::Memory(m) => Box::new(m),
        };
        let stream = MediaSourceStream::new(source, Default::default());
        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }
        let options = FormatOptions { enable_gapless: true, ..Default::default() };
        let probed = symphonia::default::get_probe().format(&hint, stream, &options, &MetadataOptions::default())?;
        let format = probed.format;
        let track = format
            .tracks()
            .iter()
            .find(|t| t.codec_params.codec != CODEC_TYPE_NULL && t.codec_params.sample_rate.is_some())
            .ok_or_else(|| DecodeError("no audio track".into()))?;
        let params = track.codec_params.clone();
        let decoder = symphonia::default::get_codecs().make(&params, &DecoderOptions::default())?;
        let name = codec_name(params.codec);
        let lossy = matches!(name, "MP3" | "MP2" | "MP1" | "AAC" | "Vorbis" | "ADPCM") || name.is_empty();
        let float = lossy || is_float_pcm(params.codec);
        let mut bits = if float { 0 } else { params.bits_per_sample.or(params.bits_per_coded_sample).unwrap_or(0) };
        if bits == 0 && name == "ALAC" {
            bits = params.extra_data.as_deref().and_then(alac_bits).unwrap_or(0);
        }
        let channels = params.channels.map(|c| c.count()).unwrap_or(2).max(1);
        let info = StreamInfo {
            rate: params.sample_rate.unwrap_or(44100),
            channels,
            bits,
            float,
            lossless: !float && bits > 0,
            total_frames: params.n_frames,
            codec: if name.is_empty() { "audio".into() } else { name.into() },
        };
        let mut this = SymphoniaDecoder {
            track_id: track.id,
            time_base: params.time_base,
            format,
            decoder,
            pending: if float { Samples::Float(Vec::new()) } else { Samples::Int(Vec::new()) },
            pending_pos: 0,
            skip: 0,
            position: 0,
            finished: false,
            lead: 0,
            length: None,
            sample_bits: 0,
            info,
        };
        // MP4 (AAC, ALAC): the edit list says what to skip and how long it is.
        let mp4 = matches!(
            path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref(),
            Some("m4a" | "m4b" | "mp4" | "m4p" | "mov")
        );
        if mp4 {
            if let Some(edit) = mp4_edit(path, this.info.rate) {
                this.lead = edit.skip;
                this.skip = edit.skip;
                this.length = Some(edit.length);
                this.info.total_frames = Some(edit.length);
            }
        }
        // AIFF: the frame count its COMM chunk states (symphonia's is off).
        let aiff = matches!(
            path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref(),
            Some("aif" | "aiff" | "aifc")
        );
        if aiff {
            if let Some(frames) = aiff_frames(path) {
                this.length = Some(frames);
                this.info.total_frames = Some(frames);
            }
        }
        // Lossless without a stated depth (ALAC): the first samples tell it.
        if !this.info.float && this.info.bits == 0 && this.refill()? {
            this.info.bits = this.sample_bits;
            this.info.lossless = this.sample_bits > 0;
        }
        Ok(this)
    }

    /// A timestamp of the track in frames.
    fn ts_to_frame(&self, ts: u64) -> u64 {
        match self.time_base {
            Some(tb) if tb.denom != 0 => {
                (ts as u128 * tb.numer as u128 * self.info.rate as u128 / tb.denom as u128) as u64
            }
            _ => ts,
        }
    }

    fn frame_to_ts(&self, frame: u64) -> u64 {
        match self.time_base {
            Some(tb) if tb.numer != 0 && self.info.rate != 0 => {
                (frame as u128 * tb.denom as u128 / (tb.numer as u128 * self.info.rate as u128)) as u64
            }
            _ => frame,
        }
    }

    /// Decodes packets until some samples are pending; false at the end.
    fn refill(&mut self) -> Result<bool> {
        let ch = self.info.channels;
        loop {
            if self.finished {
                return Ok(false);
            }
            let packet = match self.format.next_packet() {
                Ok(p) => p,
                Err(SymphoniaError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    self.finished = true;
                    return Ok(false);
                }
                Err(SymphoniaError::ResetRequired) => {
                    self.decoder.reset();
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            if packet.track_id() != self.track_id {
                continue;
            }
            let decoded = match self.decoder.decode(&packet) {
                Ok(d) => d,
                Err(SymphoniaError::DecodeError(_)) => continue, // a damaged packet: skip it
                Err(e) => return Err(e.into()),
            };
            // (symphonia has already dropped the encoder's delay and padding.)
            let first = 0;
            let last = decoded.frames();
            if last == first {
                continue;
            }
            // A seek lands before the wanted frame: drop the difference.
            let mut start = first;
            if self.skip > 0 {
                let drop = (self.skip as usize).min(last - start);
                start += drop;
                self.skip -= drop as u64;
                if start == last {
                    continue;
                }
            }
            self.pending_pos = 0;
            self.sample_bits = match &decoded {
                AudioBufferRef::U8(_) | AudioBufferRef::S8(_) => 8,
                AudioBufferRef::U16(_) | AudioBufferRef::S16(_) => 16,
                AudioBufferRef::U24(_) | AudioBufferRef::S24(_) => 24,
                AudioBufferRef::U32(_) | AudioBufferRef::S32(_) => 32,
                _ => 0,
            };
            match &mut self.pending {
                Samples::Int(v) => {
                    v.clear();
                    v.reserve((last - start) * ch);
                    interleave_i32(&decoded, start, last, ch, v);
                }
                Samples::Float(v) => {
                    v.clear();
                    v.reserve((last - start) * ch);
                    interleave_f64(&decoded, start, last, ch, v);
                }
            }
            return Ok(true);
        }
    }

    fn pending_len(&self) -> usize {
        match &self.pending {
            Samples::Int(v) => v.len(),
            Samples::Float(v) => v.len(),
        }
    }
}

impl Decoder for SymphoniaDecoder {
    fn info(&self) -> &StreamInfo {
        &self.info
    }

    fn read_i32(&mut self, out: &mut [i32]) -> Result<usize> {
        let ch = self.info.channels;
        let mut want = out.len() / ch * ch;
        if let Some(length) = self.length {
            want = want.min(length.saturating_sub(self.position) as usize * ch);
        }
        let mut done = 0;
        while done < want {
            if self.pending_pos >= self.pending_len() && !self.refill()? {
                break;
            }
            let n = (self.pending_len() - self.pending_pos).min(want - done);
            match &self.pending {
                Samples::Int(v) => out[done..done + n].copy_from_slice(&v[self.pending_pos..self.pending_pos + n]),
                Samples::Float(v) => {
                    for (o, &x) in out[done..done + n].iter_mut().zip(&v[self.pending_pos..self.pending_pos + n]) {
                        *o = (x * S32_FULL_SCALE).round_ties_even().clamp(-S32_FULL_SCALE, S32_FULL_SCALE - 1.0) as i32;
                    }
                }
            }
            self.pending_pos += n;
            done += n;
        }
        self.position += (done / ch) as u64;
        Ok(done / ch)
    }

    fn read_f64(&mut self, out: &mut [f64]) -> Result<usize> {
        let ch = self.info.channels;
        let mut want = out.len() / ch * ch;
        if let Some(length) = self.length {
            want = want.min(length.saturating_sub(self.position) as usize * ch);
        }
        let mut done = 0;
        while done < want {
            if self.pending_pos >= self.pending_len() && !self.refill()? {
                break;
            }
            let n = (self.pending_len() - self.pending_pos).min(want - done);
            match &self.pending {
                Samples::Int(v) => {
                    for (o, &x) in out[done..done + n].iter_mut().zip(&v[self.pending_pos..self.pending_pos + n]) {
                        *o = x as f64 * S32_INV_FULL_SCALE;
                    }
                }
                Samples::Float(v) => out[done..done + n].copy_from_slice(&v[self.pending_pos..self.pending_pos + n]),
            }
            self.pending_pos += n;
            done += n;
        }
        self.position += (done / ch) as u64;
        Ok(done / ch)
    }

    fn seek(&mut self, frame: u64) -> Result<u64> {
        let frame = match self.info.total_frames {
            Some(total) => frame.min(total),
            None => frame,
        };
        let media = frame + self.lead;
        let ts = self.frame_to_ts(media);
        let seeked = self.format.seek(SeekMode::Accurate, SeekTo::TimeStamp { ts, track_id: self.track_id });
        self.decoder.reset();
        self.finished = false;
        self.pending_pos = self.pending_len();
        match seeked {
            Ok(to) => {
                let landed = self.ts_to_frame(to.actual_ts);
                self.skip = media.saturating_sub(landed);
                self.position = frame;
                Ok(frame)
            }
            Err(_) if frame == 0 => Err(DecodeError("cannot seek".into())),
            Err(_) => {
                // Not seekable there: back to the start.
                self.seek(0)
            }
        }
    }
}

// --- Sample conversion -------------------------------------------------------------

/// Integer samples, left-justified to 32 bits.
fn interleave_i32(buf: &AudioBufferRef<'_>, start: usize, end: usize, ch: usize, out: &mut Vec<i32>) {
    macro_rules! copy {
        ($b:expr, $conv:expr) => {{
            let b = $b;
            for f in start..end {
                for c in 0..ch.min(b.spec().channels.count()) {
                    out.push($conv(b.chan(c)[f]));
                }
            }
        }};
    }
    match buf {
        AudioBufferRef::U8(b) => copy!(b, |v: u8| (v as i32 - 128) << 24),
        AudioBufferRef::S8(b) => copy!(b, |v: i8| (v as i32) << 24),
        AudioBufferRef::U16(b) => copy!(b, |v: u16| (v as i32 - 32768) << 16),
        AudioBufferRef::S16(b) => copy!(b, |v: i16| (v as i32) << 16),
        AudioBufferRef::U24(b) => copy!(b, |v: symphonia::core::sample::u24| (v.inner() as i32 - 0x800000) << 8),
        AudioBufferRef::S24(b) => copy!(b, |v: symphonia::core::sample::i24| v.inner() << 8),
        AudioBufferRef::U32(b) => copy!(b, |v: u32| (v ^ 0x8000_0000) as i32),
        AudioBufferRef::S32(b) => copy!(b, |v: i32| v),
        AudioBufferRef::F32(b) => copy!(b, |v: f32| (v as f64 * S32_FULL_SCALE)
            .round_ties_even()
            .clamp(-S32_FULL_SCALE, S32_FULL_SCALE - 1.0) as i32),
        AudioBufferRef::F64(b) => copy!(b, |v: f64| (v * S32_FULL_SCALE)
            .round_ties_even()
            .clamp(-S32_FULL_SCALE, S32_FULL_SCALE - 1.0) as i32),
    }
}

/// Any samples as f64 (integers exactly, floats unclipped).
fn interleave_f64(buf: &AudioBufferRef<'_>, start: usize, end: usize, ch: usize, out: &mut Vec<f64>) {
    macro_rules! copy {
        ($b:expr, $conv:expr) => {{
            let b = $b;
            for f in start..end {
                for c in 0..ch.min(b.spec().channels.count()) {
                    out.push($conv(b.chan(c)[f]));
                }
            }
        }};
    }
    let s = S32_INV_FULL_SCALE;
    match buf {
        AudioBufferRef::U8(b) => copy!(b, |v: u8| ((v as i32 - 128) << 24) as f64 * s),
        AudioBufferRef::S8(b) => copy!(b, |v: i8| ((v as i32) << 24) as f64 * s),
        AudioBufferRef::U16(b) => copy!(b, |v: u16| ((v as i32 - 32768) << 16) as f64 * s),
        AudioBufferRef::S16(b) => copy!(b, |v: i16| ((v as i32) << 16) as f64 * s),
        AudioBufferRef::U24(b) => {
            copy!(b, |v: symphonia::core::sample::u24| ((v.inner() as i32 - 0x800000) << 8) as f64 * s)
        }
        AudioBufferRef::S24(b) => copy!(b, |v: symphonia::core::sample::i24| (v.inner() << 8) as f64 * s),
        AudioBufferRef::U32(b) => copy!(b, |v: u32| ((v ^ 0x8000_0000) as i32) as f64 * s),
        AudioBufferRef::S32(b) => copy!(b, |v: i32| v as f64 * s),
        // A NaN or an infinity would latch into the filters' state (silence
        // or noise until the next seek): it is silence.
        AudioBufferRef::F32(b) => copy!(b, |v: f32| if v.is_finite() { v as f64 } else { 0.0 }),
        AudioBufferRef::F64(b) => copy!(b, |v: f64| if v.is_finite() { v } else { 0.0 }),
    }
}

/// The bit depth in an ALAC magic cookie (ALACSpecificConfig, maybe inside
/// its 'alac' atom): frameLength u32, compatibleVersion u8, bitDepth u8.
fn alac_bits(cookie: &[u8]) -> Option<u32> {
    let config = if cookie.len() >= 12 && &cookie[4..8] == b"alac" { &cookie[12..] } else { cookie };
    let bits = *config.get(5)? as u32;
    matches!(bits, 16 | 20 | 24 | 32).then_some(bits)
}

/// numSampleFrames of an AIFF / AIFC file's COMM chunk.
fn aiff_frames(path: &Path) -> Option<u64> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = File::open(path).ok()?;
    let mut head = [0u8; 12];
    file.read_exact(&mut head).ok()?;
    if &head[..4] != b"FORM" || !(&head[8..12] == b"AIFF" || &head[8..12] == b"AIFC") {
        return None;
    }
    loop {
        let mut chunk = [0u8; 8];
        file.read_exact(&mut chunk).ok()?;
        let size = u32::from_be_bytes(chunk[4..8].try_into().ok()?) as u64;
        if &chunk[..4] == b"COMM" {
            let mut comm = [0u8; 6];
            file.read_exact(&mut comm).ok()?;
            return Some(u32::from_be_bytes(comm[2..6].try_into().ok()?) as u64);
        }
        file.seek(SeekFrom::Current((size + (size & 1)) as i64)).ok()?;
    }
}

// --- MP4 edit lists (gapless AAC / ALAC) -----------------------------------------------

/// An MP4 audio track's edit list, in frames: the encoder's priming to skip
/// and the length to play (FFmpeg applies it; symphonia does not).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mp4Edit {
    pub skip: u64,
    pub length: u64,
}

/// Reads the first audio track's edit list, at `rate` frames per second.
pub fn mp4_edit(path: &Path, rate: u32) -> Option<Mp4Edit> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = File::open(path).ok()?;
    let file_len = file.metadata().ok()?.len();
    // The top-level boxes, to the "moov" one (read whole: it is small).
    let mut pos = 0u64;
    let moov = loop {
        if pos + 8 > file_len {
            return None;
        }
        file.seek(SeekFrom::Start(pos)).ok()?;
        let mut head = [0u8; 16];
        file.read_exact(&mut head[..8]).ok()?;
        let mut size = u32::from_be_bytes(head[..4].try_into().ok()?) as u64;
        let mut header = 8;
        if size == 1 {
            file.read_exact(&mut head[8..16]).ok()?;
            size = u64::from_be_bytes(head[8..16].try_into().ok()?);
            header = 16;
        } else if size == 0 {
            size = file_len - pos;
        }
        if &head[4..8] == b"moov" {
            if size > 64 << 20 || size < header {
                return None;
            }
            let mut body = vec![0u8; (size - header) as usize];
            file.read_exact(&mut body).ok()?;
            break body;
        }
        if size < header {
            return None;
        }
        pos += size;
    };
    let movie_scale = child(&moov, b"mvhd").and_then(|b| full_box_time_scale(b, 12, 20))?;
    let mut rest = &moov[..];
    while let Some((kind, body, after)) = next_box(rest) {
        rest = after;
        if kind != *b"trak" {
            continue;
        }
        let Some(mdia) = child(body, b"mdia") else { continue };
        let is_sound = child(mdia, b"hdlr").is_some_and(|h| h.len() >= 12 && &h[8..12] == b"soun");
        if !is_sound {
            continue;
        }
        let mdhd = child(mdia, b"mdhd")?;
        let media_scale = full_box_time_scale(mdhd, 12, 20)?;
        // The track's own length, in its (sample-accurate) timescale.
        let media_duration = if mdhd.first() == Some(&1) {
            u64::from_be_bytes(mdhd.get(24..32)?.try_into().ok()?)
        } else {
            u32::from_be_bytes(mdhd.get(16..20)?.try_into().ok()?) as u64
        };
        let elst = child(body, b"edts").and_then(|e| child(e, b"elst"))?;
        let version = *elst.first()?;
        let count = u32::from_be_bytes(elst.get(4..8)?.try_into().ok()?);
        // The first entry that plays media (an empty edit, media_time -1, is a delay).
        let mut at = 8usize;
        for _ in 0..count {
            let (duration, media_time, len) = if version == 1 {
                let d = u64::from_be_bytes(elst.get(at..at + 8)?.try_into().ok()?);
                let m = i64::from_be_bytes(elst.get(at + 8..at + 16)?.try_into().ok()?);
                (d, m, 20)
            } else {
                let d = u32::from_be_bytes(elst.get(at..at + 4)?.try_into().ok()?) as u64;
                let m = i32::from_be_bytes(elst.get(at + 4..at + 8)?.try_into().ok()?) as i64;
                (d, m, 12)
            };
            at += len;
            if media_time < 0 {
                continue;
            }
            let skip = (media_time as u128 * rate as u128 / media_scale as u128) as u64;
            let mut length = (duration as u128 * rate as u128 / movie_scale as u128) as u64;
            // The edit's length is often in milliseconds: when the track's own
            // length (minus the priming) agrees within that rounding, it is
            // the exact one.
            let exact = (media_duration as u128 * rate as u128 / media_scale as u128) as u64;
            let exact = exact.saturating_sub(skip);
            let slack = (rate as u64).div_ceil(movie_scale as u64) + 1;
            if exact.abs_diff(length) <= slack {
                length = exact;
            }
            return Some(Mp4Edit { skip, length });
        }
        return None;
    }
    None
}

fn next_box(data: &[u8]) -> Option<([u8; 4], &[u8], &[u8])> {
    if data.len() < 8 {
        return None;
    }
    let size = u32::from_be_bytes(data[..4].try_into().ok()?) as usize;
    let kind: [u8; 4] = data[4..8].try_into().ok()?;
    let (header, size) = if size == 1 {
        (16, u64::from_be_bytes(data.get(8..16)?.try_into().ok()?) as usize)
    } else if size == 0 {
        (8, data.len())
    } else {
        (8, size)
    };
    if size < header || size > data.len() {
        return None;
    }
    Some((kind, &data[header..size], &data[size..]))
}

fn child<'a>(data: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    let mut rest = data;
    while let Some((k, body, after)) = next_box(rest) {
        if &k == kind {
            return Some(body);
        }
        rest = after;
    }
    None
}

/// The timescale of an mvhd / mdhd box (version 0 at `v0`, version 1 at `v1`).
fn full_box_time_scale(b: &[u8], v0: usize, v1: usize) -> Option<u32> {
    let at = if *b.first()? == 1 { v1 } else { v0 };
    let scale = u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?);
    (scale > 0).then_some(scale)
}

/// A WAV file for the tests: `format` 1 (integer PCM) or 3 (IEEE float),
/// `data` the interleaved samples' bytes.
#[cfg(test)]
pub(crate) fn write_test_wav(path: &Path, rate: u32, channels: u16, format: u16, bits: u16, data: &[u8]) {
    let block = channels as u32 * bits as u32 / 8;
    let mut w = Vec::new();
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&format.to_le_bytes());
    w.extend_from_slice(&channels.to_le_bytes());
    w.extend_from_slice(&rate.to_le_bytes());
    w.extend_from_slice(&rate.wrapping_mul(block).to_le_bytes());
    w.extend_from_slice(&(block as u16).to_le_bytes());
    w.extend_from_slice(&bits.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&(data.len() as u32).to_le_bytes());
    w.extend_from_slice(data);
    std::fs::write(path, w).unwrap();
}

/// A 16-bit PCM WAV of `frames` frames of a quiet sine.
#[cfg(test)]
pub(crate) fn write_sine_wav(path: &Path, rate: u32, channels: u16, frames: usize) {
    let mut data = Vec::with_capacity(frames * channels as usize * 2);
    for f in 0..frames {
        for c in 0..channels as usize {
            let v = ((f as f64 * (0.05 + 0.01 * c as f64)).sin() * 8000.0) as i16;
            data.extend_from_slice(&v.to_le_bytes());
        }
    }
    write_test_wav(path, rate, channels, 1, 16, &data);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Seek, SeekFrom};

    #[test]
    fn float_samples_that_are_not_numbers_are_silence() {
        // One NaN in a float file latched into the filters' state: silence
        // (or noise) until the next seek.
        let path = std::env::temp_dir().join(format!("sonkore-nan-{}.wav", std::process::id()));
        let samples = [0.25f32, -0.25, f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.5, 0.125, -0.125];
        let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        write_test_wav(&path, 44100, 2, 3, 32, &data);
        let mut d = open(&path).unwrap();
        assert!(d.info().float);
        let mut out = vec![1.0f64; 8];
        assert_eq!(d.read_f64(&mut out).unwrap(), 4);
        assert_eq!(out, [0.25, -0.25, 0.0, 0.0, 0.0, 0.5, 0.125, -0.125]);
        let _ = std::fs::remove_file(&path);
    }

    /// A file read into memory slowly (`delay` per chunk), as from a NAS.
    fn slow_prefetch(name: &str, len: usize) -> (std::path::PathBuf, Vec<u8>, Prefetch) {
        let path = std::env::temp_dir().join(format!("sonkore-{name}-{}", std::process::id()));
        let bytes: Vec<u8> = (0..len as u32).map(|i| (i * 7 % 251) as u8).collect();
        std::fs::write(&path, &bytes).unwrap();
        let m = Prefetch::start_slow(File::open(&path).unwrap(), len as u64, std::time::Duration::from_millis(50));
        (path, bytes, m)
    }

    #[test]
    fn a_seek_past_what_is_loaded_reads_the_disk() {
        // Seeking far into a big file waited for the whole file to load.
        let (path, bytes, mut m) = slow_prefetch("prefetch-seek", 20 << 20);
        let t = std::time::Instant::now();
        m.seek(SeekFrom::Start(19 << 20)).unwrap();
        let mut head = [0u8; 4096];
        m.read_exact(&mut head).unwrap();
        assert_eq!(head[..], bytes[19 << 20..(19 << 20) + 4096]);
        assert!(t.elapsed() < std::time::Duration::from_millis(300), "{:?}", t.elapsed());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_dropped_file_stops_loading() {
        let (path, _bytes, m) = slow_prefetch("prefetch-drop", 20 << 20);
        let watch = m.watch();
        std::thread::sleep(std::time::Duration::from_millis(80));
        drop(m);
        let t = std::time::Instant::now();
        while watch.upgrade().is_some() && t.elapsed() < std::time::Duration::from_secs(3) {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(t.elapsed() < std::time::Duration::from_millis(300), "the loading went on: {:?}", t.elapsed());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_file_in_memory_reads_and_seeks_as_on_disk() {
        let path = std::env::temp_dir().join(format!("sonkore-prefetch-{}", std::process::id()));
        let bytes: Vec<u8> = (0..3_000_000u32).map(|i| (i * 7 % 251) as u8).collect();
        std::fs::write(&path, &bytes).unwrap();
        let mut m = Prefetch::start(File::open(&path).unwrap(), bytes.len() as u64);
        let mut head = [0u8; 16];
        m.read_exact(&mut head).unwrap();
        assert_eq!(head, bytes[..16]);
        // Past what the first chunk holds, and from the end.
        assert_eq!(m.seek(SeekFrom::Start(2_500_000)).unwrap(), 2_500_000);
        m.read_exact(&mut head).unwrap();
        assert_eq!(head, bytes[2_500_000..2_500_016]);
        m.seek(SeekFrom::End(-4)).unwrap();
        let mut tail = Vec::new();
        m.read_to_end(&mut tail).unwrap();
        assert_eq!(tail, bytes[bytes.len() - 4..]);
        let _ = std::fs::remove_file(&path);
    }
}
