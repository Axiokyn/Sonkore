//! DSF and DSDIFF reading, DST-compressed DSDIFF included.
//!
//! The bitstream goes to the engine untouched, packed for the DAC: native
//! `DSD_U32_BE/LE` words (frame rate = DSD rate / 32) or DoP (DSD rate / 16,
//! 0x05/0xFA markers alternating, the phase carried across gapless joins
//! with [`DsdDecoder::dop_phase`] / [`DsdDecoder::set_dop_phase`]). Past the
//! end of the file the last word is completed with DSD silence (0x69).

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::dst::{DstDecoder, MAX_CHANNELS as DST_MAX_CHANNELS};
use crate::{DOP_MARKERS, Decoder, DsdError, DsdMode, DsdPacking, Result, SILENCE_BYTE, Source, StreamInfo};

/// What a DSD file holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DsdFormat {
    /// 2822400 for DSD64.
    pub dsd_rate: u32,
    pub channels: u32,
    /// DSD bytes (8 bits each) per channel.
    pub bytes_per_channel: u64,
}

impl DsdFormat {
    /// 64, 128, 256, 512 (or 48 kHz-family multiples) times 44.1 kHz, rounded.
    pub fn multiple(&self) -> u32 {
        ((self.dsd_rate as u64 + 22050) / 44100) as u32
    }
}

/// `.dsf` / `.dff`, any case.
pub fn is_dsd_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("dsf") || e.eq_ignore_ascii_case("dff"))
        .unwrap_or(false)
}

/// Header only. `None` when the file is not DSD (or DST with more channels
/// than the decoder takes).
pub fn probe(path: &Path) -> Option<DsdFormat> {
    let mut f = File::open(path).ok()?;
    parse(&mut f).map(|l| l.format)
}

/// Frame rate of the packed stream: DSD rate / 16 (DoP) or / 32 (native).
pub fn frame_rate(dsd_rate: u32, packing: DsdPacking) -> u32 {
    if packing == DsdPacking::Dop { dsd_rate / 16 } else { dsd_rate / 32 }
}

/// One sample of DSD silence (the 0x69 idle pattern) in this packing; for
/// DoP, `phase` picks the marker (false = 0x05, true = 0xFA).
pub fn silence(packing: DsdPacking, phase: bool) -> i32 {
    match packing {
        DsdPacking::Dop => {
            ((DOP_MARKERS[phase as usize] as u32) << 24 | (SILENCE_BYTE as u32) << 16 | (SILENCE_BYTE as u32) << 8)
                as i32
        }
        _ => i32::from_ne_bytes([SILENCE_BYTE; 4]),
    }
}

/// "DSD64", "DSD128"…
pub fn rate_label(dsd_rate: u32) -> String {
    format!("DSD{}", (dsd_rate as u64 + 22050) / 44100)
}

/// The highest rate a file may claim: DSD1024 of the 48 kHz family, twice
/// over. Anything above is damage, not a stream to play.
const MAX_DSD_RATE: u32 = 2048 * 48000;

fn sane_rate(rate: u32) -> bool {
    (2_822_400..=MAX_DSD_RATE).contains(&rate)
}

// --- parsing -----------------------------------------------------------------

#[derive(Default)]
struct Layout {
    format: DsdFormat,
    dsf: bool,
    lsb_first: bool,
    data_start: u64,
    block_size: u32,
    // DST-compressed DSDIFF: the frame chunks and, when present, the index.
    dst: bool,
    dst_start: u64,
    dst_end: u64,
    frame_count: u32,
    index_offset: u64,
    index_size: u64,
}

fn read_at(f: &mut dyn Source, offset: u64, buf: &mut [u8]) -> bool {
    f.seek(SeekFrom::Start(offset)).is_ok() && f.read_exact(buf).is_ok()
}

fn le(p: &[u8]) -> u64 {
    p.iter().rev().fold(0, |v, &b| v << 8 | b as u64)
}

fn be(p: &[u8]) -> u64 {
    p.iter().fold(0, |v, &b| v << 8 | b as u64)
}

/// DSF (Sony): little-endian chunks "DSD ", "fmt ", "data"; per channel
/// blocks of `blockSize` bytes, LSB-first when the file stores 1 bit per sample.
fn parse_dsf(f: &mut dyn Source, out: &mut Layout) -> bool {
    let mut h = [0u8; 28 + 52 + 12];
    if !read_at(f, 0, &mut h) {
        return false;
    }
    if &h[0..4] != b"DSD " || &h[28..32] != b"fmt " {
        return false;
    }
    let fmt = &h[28 + 12..];
    let fmt_size = le(&h[28 + 4..28 + 12]);
    if le(&fmt[4..8]) != 0 {
        // format id 0: DSD raw
        return false;
    }
    let channels = le(&fmt[12..16]) as u32;
    let rate = le(&fmt[16..20]) as u32;
    let bits = le(&fmt[20..24]) as u32;
    let samples = le(&fmt[24..32]);
    let block = le(&fmt[32..36]) as u32;
    let data_chunk = 28u64.wrapping_add(fmt_size);
    let mut d = [0u8; 12];
    if !read_at(f, data_chunk, &mut d) || &d[0..4] != b"data" {
        return false;
    }
    if channels == 0 || channels > 8 || !sane_rate(rate) || block == 0 || (bits != 1 && bits != 8) {
        return false;
    }
    out.dsf = true;
    out.lsb_first = bits == 1;
    out.data_start = data_chunk + 12;
    out.block_size = block;
    out.format = DsdFormat { dsd_rate: rate, channels, bytes_per_channel: samples / 8 };
    true
}

/// DSDIFF (Philips): big-endian "FRM8"/"DSD " form; "PROP"/"SND " holds the
/// rate ("FS  "), channels ("CHNL") and compression ("CMPR"); the "DSD "
/// chunk holds the samples, one byte per channel in turn, MSB first. A
/// DST-compressed file has a "DST " chunk instead: "FRTE" (frame count and
/// rate) then a "DSTF" chunk per frame, and may carry a "DSTI" index.
fn parse_dff(f: &mut dyn Source, out: &mut Layout) -> bool {
    let mut h = [0u8; 16];
    if !read_at(f, 0, &mut h) || &h[0..4] != b"FRM8" || &h[12..16] != b"DSD " {
        return false;
    }
    let end = 12u64.saturating_add(be(&h[4..12]));
    let (mut rate, mut channels) = (0u32, 0u32);
    let mut compressed = false;
    let mut pos = 16u64;
    while pos.saturating_add(12) <= end {
        if out.dst && pos >= out.dst_end {
            // Past the frames: only the index chunk is of interest.
            let mut c = [0u8; 12];
            if !read_at(f, pos, &mut c) {
                break;
            }
            let size = be(&c[4..12]);
            if &c[0..4] == b"DSTI" {
                out.index_offset = pos + 12;
                out.index_size = size;
                break;
            }
            pos = pos.saturating_add(12).saturating_add(size).saturating_add(size & 1);
            continue;
        }
        let mut c = [0u8; 12];
        if !read_at(f, pos, &mut c) {
            return false;
        }
        let size = be(&c[4..12]);
        let body = pos + 12;
        match &c[0..4] {
            b"PROP" => {
                let mut sub = body + 4; // after "SND "
                while sub.saturating_add(12) <= body.saturating_add(size) {
                    let mut s = [0u8; 12 + 4];
                    if !read_at(f, sub, &mut s) {
                        return false;
                    }
                    let sub_size = be(&s[4..12]);
                    match &s[0..4] {
                        b"FS  " => rate = be(&s[12..16]) as u32,
                        b"CHNL" => channels = be(&s[12..14]) as u32,
                        b"CMPR" => compressed = &s[12..16] != b"DSD ",
                        _ => {}
                    }
                    sub = sub.saturating_add(12).saturating_add(sub_size).saturating_add(sub_size & 1);
                }
            }
            b"DSD " => {
                if channels == 0 || channels > 8 || !sane_rate(rate) || compressed {
                    return false;
                }
                out.dsf = false;
                out.lsb_first = false;
                out.data_start = body;
                out.format = DsdFormat { dsd_rate: rate, channels, bytes_per_channel: size / channels as u64 };
                return true;
            }
            b"DST " => {
                // DST frames: the rate counts in bits, the frame holds
                // 588 * rate / 44100 of them per channel.
                let mut frte = [0u8; 12 + 6];
                if channels == 0
                    || channels as usize > DST_MAX_CHANNELS
                    || !sane_rate(rate)
                    || rate % 44100 != 0
                    || !read_at(f, body, &mut frte)
                    || &frte[0..4] != b"FRTE"
                {
                    return false;
                }
                let frames = be(&frte[12..16]) as u32;
                let frte_size = be(&frte[4..12]);
                out.dsf = false;
                out.lsb_first = false;
                out.dst = true;
                out.dst_start = (body + 12).saturating_add(frte_size).saturating_add(frte_size & 1);
                out.dst_end = body.saturating_add(size);
                out.frame_count = frames;
                out.format = DsdFormat {
                    dsd_rate: rate,
                    channels,
                    bytes_per_channel: frames as u64 * (DstDecoder::samples_per_frame(rate) / 8) as u64,
                };
            }
            _ => {}
        }
        pos = body.saturating_add(size).saturating_add(size & 1);
    }
    out.dst
}

fn parse(f: &mut dyn Source) -> Option<Layout> {
    let mut l = Layout::default();
    if !parse_dsf(f, &mut l) {
        l = Layout::default();
        if !parse_dff(f, &mut l) {
            return None;
        }
    }
    // The stream ends with the file at the latest, whatever length its
    // header claims (a damaged one says up to 2^64 bytes, and the offsets a
    // seek then computes overflow).
    if !l.dst {
        let len = f.seek(SeekFrom::End(0)).unwrap_or(u64::MAX);
        let room = len.saturating_sub(l.data_start);
        let ch = l.format.channels.max(1) as u64;
        let most = if l.dsf {
            // Whole blocks per channel, the last one padded.
            let group = l.block_size as u64 * ch;
            room.div_ceil(group) * l.block_size as u64
        } else {
            room / ch
        };
        l.format.bytes_per_channel = l.format.bytes_per_channel.min(most);
    }
    Some(l)
}

// --- the decoder ---------------------------------------------------------------

#[derive(Default)]
struct Cached {
    index: Option<u64>,
    bytes: Vec<u8>, // per channel, channel after channel
}

/// DST: the frames (1/75 s each) are found as they are needed, through the
/// file's index (DSTI) when it has one or by walking the frame chunks, and
/// decoded on demand; the last two decoded frames are kept.
struct DstState {
    frame_bytes: usize, // per channel
    max_frame: u32,     // the longest a frame can be (uncompressed, plus slack)
    dst_end: u64,       // end of the DST chunk's body
    scan_pos: u64,      // the next frame chunk not yet indexed
    frame_offsets: Vec<u64>,
    frame_sizes: Vec<u32>,
    decoder: DstDecoder,
    cache: [Cached; 2],
    cache_next: usize,
    packet: Vec<u8>,
    error: bool, // a frame failed to decode (reported once)
}

/// How much longer than an uncompressed frame a frame may be before its
/// length is taken for garbage: an encoder should store a frame that does not
/// compress uncompressed, but not every one does (tests/mkdst.py without
/// numpy writes frames 2.2 times as long). The bound is only there so a
/// corrupt length cannot make the reader allocate gigabytes.
const FRAME_GROWTH: usize = 16;
const FRAME_SLACK: usize = 64 * 1024;

impl DstState {
    fn new(frame_bytes: usize, channels: usize, dst_start: u64, dst_end: u64) -> Self {
        // An uncompressed frame is a header byte and the bits (under 1 MB
        // for DSD128 in 6 channels, times FRAME_GROWTH).
        let raw = 1 + frame_bytes * channels;
        let max_frame = (raw * FRAME_GROWTH + FRAME_SLACK).min(u32::MAX as usize) as u32;
        DstState {
            frame_bytes,
            max_frame,
            dst_end,
            scan_pos: dst_start,
            frame_offsets: Vec::new(),
            frame_sizes: Vec::new(),
            decoder: DstDecoder::new(),
            cache: Default::default(),
            cache_next: 0,
            packet: Vec::new(),
            error: false,
        }
    }
}

/// Reads DSF and DSDIFF (DST included) and hands out packed DSD words.
pub struct DsdDecoder {
    file: Box<dyn Source>,
    info: StreamInfo,
    packing: DsdPacking,
    format: DsdFormat,
    dsf: bool,
    lsb_first: bool, // DSF with 1 bit per sample: bit order reversed in each byte
    data_start: u64,
    block_size: u32, // DSF: bytes per channel per block
    position: u64,   // frames
    phase: bool,
    bytes: Vec<u8>, // per channel, channel after channel
    scratch: Vec<u8>,
    words: Vec<i32>, // read_f64's
    dst: Option<Box<DstState>>,
}

fn reverse_bits(b: u8) -> u8 {
    b.reverse_bits()
}

impl DsdDecoder {
    pub fn open(path: &Path, packing: DsdPacking) -> Result<Self> {
        let f = File::open(path).map_err(|_| DsdError("Not a DSF / DSDIFF file".into()))?;
        Self::from_source(Box::new(f), packing)
    }

    /// From any seekable source (a file in memory, for instance).
    pub fn from_source(mut file: Box<dyn Source>, packing: DsdPacking) -> Result<Self> {
        let layout = parse(&mut *file).ok_or_else(|| DsdError("Not a DSF / DSDIFF file".into()))?;
        let mut dst = None;
        if layout.dst {
            let mut state = Box::new(DstState::new(
                DstDecoder::samples_per_frame(layout.format.dsd_rate) / 8,
                layout.format.channels as usize,
                layout.dst_start,
                layout.dst_end,
            ));
            if layout.index_size > 0 {
                load_index(&mut *file, &mut state, layout.index_offset, layout.index_size, layout.frame_count);
            }
            dst = Some(state);
        }
        let bytes_per_frame: u64 = if packing == DsdPacking::Dop { 2 } else { 4 };
        let label = rate_label(layout.format.dsd_rate);
        let info = StreamInfo {
            rate: frame_rate(layout.format.dsd_rate, packing),
            channels: layout.format.channels as usize,
            bits: if packing == DsdPacking::Dop { 24 } else { 32 },
            float: false,
            lossless: true,
            total_frames: layout.format.bytes_per_channel.div_ceil(bytes_per_frame),
            codec: format!(
                "{label}{}{}",
                if layout.dst { " DST" } else { "" },
                if packing == DsdPacking::Dop { " DoP" } else { " native" }
            ),
            dsd: if packing == DsdPacking::Dop { DsdMode::Dop } else { DsdMode::Native },
            dsd_rate: layout.format.dsd_rate,
            decoder: if layout.dst { "DST decoder" } else { "DSD reader" },
        };
        Ok(DsdDecoder {
            file,
            info,
            packing,
            format: layout.format,
            dsf: layout.dsf,
            lsb_first: layout.lsb_first,
            data_start: layout.data_start,
            block_size: layout.block_size,
            position: 0,
            phase: false,
            bytes: Vec::new(),
            scratch: Vec::new(),
            words: Vec::new(),
            dst,
        })
    }

    pub fn packing(&self) -> DsdPacking {
        self.packing
    }

    /// The file's DSD rate, channels and exact length in bytes.
    pub fn format(&self) -> DsdFormat {
        self.format
    }

    pub fn is_dst(&self) -> bool {
        self.dst.is_some()
    }

    /// DoP marker of the next frame: carried across a gapless join so the DAC
    /// never sees two equal markers in a row.
    pub fn dop_phase(&self) -> bool {
        self.phase
    }

    pub fn set_dop_phase(&mut self, phase: bool) {
        self.phase = phase;
    }

    /// Bytes `[first, first + count)` of every channel into `self.bytes`, in
    /// time order, MSB first; past the end of the stream they are DSD silence.
    fn read_bytes(&mut self, first: u64, count: usize) -> Result<()> {
        let ch = self.format.channels as usize;
        self.bytes.clear();
        self.bytes.resize(ch * count, SILENCE_BYTE);
        let total = self.format.bytes_per_channel;
        let valid = if first >= total { 0 } else { (count as u64).min(total - first) as usize };

        if self.dsf {
            // Channel blocks: [ch0 block][ch1 block]... per group of blockSize bytes.
            let block = self.block_size as u64;
            let mut done = 0usize;
            while done < valid {
                let at = first + done as u64;
                let group = at / block;
                let offset = (at % block) as usize;
                let n = (valid - done).min(block as usize - offset);
                for c in 0..ch {
                    let pos = self.data_start + (group * ch as u64 + c as u64) * block + offset as u64;
                    let dst = &mut self.bytes[c * count + done..c * count + done + n];
                    self.file.seek(SeekFrom::Start(pos))?;
                    self.file.read_exact(dst)?;
                }
                done += n;
            }
            if self.lsb_first {
                for c in 0..ch {
                    for b in &mut self.bytes[c * count..c * count + valid] {
                        *b = reverse_bits(*b);
                    }
                }
            }
            return Ok(());
        }
        if let Some(dst) = self.dst.as_deref_mut() {
            // Decoded frames, each `frame_bytes` per channel.
            let fb = dst.frame_bytes;
            let mut done = 0usize;
            while done < valid {
                let at = first + done as u64;
                let offset = (at % fb as u64) as usize;
                let n = (valid - done).min(fb - offset);
                if let Some(planar) = dst_frame(&mut *self.file, dst, self.format.channels as usize, at / fb as u64) {
                    for c in 0..ch {
                        self.bytes[c * count + done..c * count + done + n]
                            .copy_from_slice(&planar[c * fb + offset..c * fb + offset + n]);
                    }
                }
                done += n;
            }
            return Ok(());
        }
        // DSDIFF: one byte per channel in turn.
        self.scratch.resize(valid * ch, 0);
        if valid > 0 {
            self.file.seek(SeekFrom::Start(self.data_start + first * ch as u64))?;
            self.file.read_exact(&mut self.scratch)?;
        }
        for i in 0..valid {
            for c in 0..ch {
                self.bytes[c * count + i] = self.scratch[i * ch + c];
            }
        }
        Ok(())
    }
}

/// The DSTI chunk: for every frame, its offset in the file (of the frame
/// chunk, or of the frame itself) and its length. Anything that does not add
/// up is ignored: the frames are then found by walking the chunks. An index
/// shorter than the frame count is used for the frames it lists, and the
/// walk for the others starts after the last of them.
fn load_index(file: &mut dyn Source, st: &mut DstState, offset: u64, size: u64, frames: u32) -> bool {
    let count = (size / 12).min(frames as u64);
    if count == 0 {
        return false;
    }
    let Ok(len) = usize::try_from(count * 12) else { return false };
    let mut raw = vec![0u8; len];
    if !read_at(file, offset, &mut raw) {
        return false;
    }
    // Does an entry point at a chunk header or at the frame bytes?
    let mut probe = [0u8; 4];
    let first = be(&raw[0..8]);
    if !read_at(file, first, &mut probe) {
        return false;
    }
    let skip = if &probe == b"DSTF" { 12 } else { 0 };
    let mut offsets = Vec::with_capacity(count as usize);
    let mut sizes = Vec::with_capacity(count as usize);
    let mut end = st.scan_pos;
    for e in raw.chunks_exact(12) {
        let at = be(&e[0..8]).wrapping_add(skip);
        let length = be(&e[8..12]) as u32;
        if at < st.scan_pos || at.saturating_add(length as u64) > st.dst_end || length == 0 || length > st.max_frame {
            return false;
        }
        offsets.push(at);
        sizes.push(length);
        end = end.max(at + length as u64 + (length & 1) as u64);
    }
    if count < frames as u64 {
        // The frames past the index are walked to from the chunk after the
        // last one it lists (walking from the start would find frame 0, 1…
        // again and play them as the later frames). That has to be a chunk.
        let mut id = [0u8; 4];
        if end < st.dst_end && (!read_at(file, end, &mut id) || (&id != b"DSTF" && &id != b"DSTC")) {
            return false;
        }
        st.scan_pos = end;
    }
    st.frame_offsets = offsets;
    st.frame_sizes = sizes;
    true
}

/// The frame's place in the file, walking the frame chunks from where the
/// last walk stopped (the CRC chunks between them are skipped). A frame
/// chunk that runs past the DST chunk or is longer than any frame can be
/// keeps its place with a length of 0: it plays as silence.
fn locate_frame(file: &mut dyn Source, st: &mut DstState, index: u64) -> Option<(u64, u32)> {
    while index >= st.frame_offsets.len() as u64 {
        let mut c = [0u8; 12];
        if st.scan_pos.saturating_add(12) > st.dst_end || !read_at(file, st.scan_pos, &mut c) {
            return None;
        }
        let chunk_size = be(&c[4..12]);
        if &c[0..4] == b"DSTF" {
            let body = st.scan_pos + 12;
            let fits = body.saturating_add(chunk_size) <= st.dst_end && chunk_size <= st.max_frame as u64;
            st.frame_offsets.push(body);
            st.frame_sizes.push(if fits { chunk_size as u32 } else { 0 });
        }
        st.scan_pos = st.scan_pos.saturating_add(12).saturating_add(chunk_size).saturating_add(chunk_size & 1);
    }
    Some((st.frame_offsets[index as usize], st.frame_sizes[index as usize]))
}

/// A decoded frame, planar; `None`: silence (missing or unreadable).
fn dst_frame<'a>(file: &mut dyn Source, st: &'a mut DstState, channels: usize, index: u64) -> Option<&'a [u8]> {
    if let Some(i) = st.cache.iter().position(|c| c.index == Some(index)) {
        return Some(&st.cache[i].bytes);
    }
    let (offset, size) = locate_frame(file, st, index)?;
    if size == 0 || size > st.max_frame {
        return None;
    }
    st.packet.resize(size as usize, 0);
    if !read_at(file, offset, &mut st.packet) {
        return None;
    }
    let slot_index = st.cache_next;
    st.cache_next = (st.cache_next + 1) % 2;
    let samples = st.frame_bytes * 8;
    let slot = &mut st.cache[slot_index];
    slot.index = None;
    slot.bytes.resize(st.frame_bytes * channels, 0);
    if let Err(e) = st.decoder.decode(&st.packet, channels, samples, &mut slot.bytes) {
        if !st.error {
            eprintln!("Sonkore: frame {index}: {e}");
        }
        st.error = true;
    }
    slot.index = Some(index);
    Some(&st.cache[slot_index].bytes)
}

impl Decoder for DsdDecoder {
    fn info(&self) -> &StreamInfo {
        &self.info
    }

    fn read_i32(&mut self, out: &mut [i32]) -> Result<usize> {
        let ch = self.format.channels as usize;
        let total = self.info.total_frames;
        let frames = ((out.len() / ch) as u64).min(total - total.min(self.position)) as usize;
        if frames == 0 {
            return Ok(0);
        }
        let width = if self.packing == DsdPacking::Dop { 2 } else { 4 };
        let count = frames * width;
        self.read_bytes(self.position * width as u64, count)?;

        for f in 0..frames {
            let marker = (DOP_MARKERS[self.phase as usize] as u32) << 24;
            for c in 0..ch {
                let b = &self.bytes[c * count + f * width..];
                out[f * ch + c] = match self.packing {
                    DsdPacking::Dop => (marker | (b[0] as u32) << 16 | (b[1] as u32) << 8) as i32,
                    DsdPacking::NativeU32Be => i32::from_ne_bytes([b[0], b[1], b[2], b[3]]),
                    DsdPacking::NativeU32Le => i32::from_ne_bytes([b[3], b[2], b[1], b[0]]),
                };
            }
            self.phase = !self.phase;
        }
        self.position += frames as u64;
        Ok(frames)
    }

    /// The words / 2^31 (not PCM).
    fn read_f64(&mut self, out: &mut [f64]) -> Result<usize> {
        let mut words = std::mem::take(&mut self.words);
        words.resize(out.len(), 0);
        let got = self.read_i32(&mut words);
        self.words = words;
        let got = got?;
        let n = got * self.info.channels;
        for (o, w) in out[..n].iter_mut().zip(&self.words) {
            *o = *w as f64 * (1.0 / 2147483648.0);
        }
        Ok(got)
    }

    fn seek(&mut self, frame: u64) -> Result<u64> {
        self.position = frame.min(self.info.total_frames);
        self.phase = false; // the engine's pre-roll before it ends on 0xFA
        Ok(self.position)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_and_labels() {
        assert_eq!(frame_rate(2_822_400, DsdPacking::Dop), 176_400);
        assert_eq!(frame_rate(2_822_400, DsdPacking::NativeU32Be), 88_200);
        assert_eq!(rate_label(2_822_400), "DSD64");
        assert_eq!(rate_label(22_579_200), "DSD512");
        assert_eq!(rate_label(3_072_000), "DSD70"); // 48 kHz family: rounded
        assert!(is_dsd_extension(Path::new("a/b.DSF")));
        assert!(is_dsd_extension(Path::new("x.dff")));
        assert!(!is_dsd_extension(Path::new("x.flac")));
    }

    fn chunk(out: &mut Vec<u8>, id: &[u8; 4], body: &[u8]) -> u64 {
        let at = out.len() as u64;
        out.extend_from_slice(id);
        out.extend_from_slice(&(body.len() as u64).to_be_bytes());
        out.extend_from_slice(body);
        if body.len() % 2 == 1 {
            out.push(0);
        }
        at
    }

    #[test]
    fn a_frame_chunk_longer_than_the_file_allocates_nothing() {
        // A DST chunk (DSD64 stereo) holding a frame chunk that claims ~1 TB.
        let mut file = vec![0u8; 16];
        let first = chunk(&mut file, b"DSTF", &[0x80, 1, 2, 3]);
        file.extend_from_slice(b"DSTF");
        file.extend_from_slice(&0xFF_FFFF_FFFFu64.to_be_bytes());
        file.extend_from_slice(&[0; 32]);
        let mut st = DstState::new(4704, 2, 16, file.len() as u64);
        let mut src = std::io::Cursor::new(file);
        assert_eq!(locate_frame(&mut src, &mut st, 0), Some((first + 12, 4)));
        let (_, size) = locate_frame(&mut src, &mut st, 1).unwrap();
        assert!(size <= st.max_frame, "{size} bytes taken from a corrupt chunk");
        assert!(dst_frame(&mut src, &mut st, 2, 1).is_none());
        assert!(st.packet.capacity() < 1 << 20);
        assert!(locate_frame(&mut src, &mut st, 2).is_none());
    }

    #[test]
    fn frames_past_a_short_index_are_not_the_first_ones_again() {
        // Four frame chunks (a CRC chunk between the first two), and an
        // index listing only the first two.
        let mut file = vec![0u8; 16];
        let mut at = Vec::new();
        for n in 0..4u8 {
            at.push(chunk(&mut file, b"DSTF", &[0x80, n, n, n, n][..if n == 1 { 5 } else { 4 }]));
            if n == 0 {
                chunk(&mut file, b"DSTC", &[9, 9, 9, 9]);
            }
        }
        let dst_end = file.len() as u64;
        let mut index = Vec::new();
        for (n, &a) in at.iter().take(2).enumerate() {
            index.extend_from_slice(&a.to_be_bytes());
            index.extend_from_slice(&(if n == 1 { 5u32 } else { 4 }).to_be_bytes());
        }
        let index_at = chunk(&mut file, b"DSTI", &index) + 12;
        let mut st = DstState::new(4704, 1, 16, dst_end);
        let mut src = std::io::Cursor::new(file);
        assert!(load_index(&mut src, &mut st, index_at, index.len() as u64, 4));
        for (n, &a) in at.iter().enumerate() {
            let (offset, _) = locate_frame(&mut src, &mut st, n as u64).unwrap();
            assert_eq!(offset, a + 12, "frame {n}");
        }
        assert!(locate_frame(&mut src, &mut st, 4).is_none());
    }

    #[test]
    fn silence_words() {
        assert_eq!(silence(DsdPacking::Dop, false) as u32, 0x0569_6900);
        assert_eq!(silence(DsdPacking::Dop, true) as u32, 0xFA69_6900);
        assert_eq!(silence(DsdPacking::NativeU32Be, false).to_ne_bytes(), [0x69; 4]);
        assert_eq!(silence(DsdPacking::NativeU32Le, true).to_ne_bytes(), [0x69; 4]);
    }
}
