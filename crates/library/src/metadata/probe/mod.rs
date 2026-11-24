//! The formats earlier Sonkore versions read through FFmpeg:
//! what avformat_open_input and, when the header lacks the rate or the
//! length, avformat_find_stream_info leave in the format and stream
//! contexts, rebuilt here from the container headers without FFmpeg.
//!
//! Each container module fills a `Demuxed` the way its FFmpeg demuxer fills
//! AVFormatContext / AVStream (tag keys and their conversions, the codec,
//! the rate, bits_per_raw/coded_sample, durations, bit rates, attached
//! pictures); `finish` then applies the FFmpeg probe's rules: its codec
//! labels, bitsOf(), streamFrames() and bit rate rule.

pub(crate) mod apev2;
mod asf;
mod caf;
mod dsd;
pub(crate) mod ffid3;
mod lossless;
mod mkv;
pub(crate) mod mp4;
mod ogg;
mod pcm;
mod raw;
#[cfg(test)]
mod tests;

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// What probe() returns.
#[derive(Clone, Debug, Default)]
#[allow(dead_code)]
pub(crate) struct Probe {
    pub codec: String,
    pub sample_rate: u32,
    pub channels: u32,
    /// bitsOf(): 0 for lossy codecs.
    pub bits: u32,
    pub lossless: bool,
    pub total_frames: u64,
    pub duration: f64,
    pub bitrate_kbps: i32,
    /// UTF-8, keys as FFmpeg leaves them: the format's, then the stream's.
    pub tags: Vec<(String, String)>,
    pub has_cover: bool,
    pub cover: Vec<u8>,
}

/// The codecs, as far as their labels and properties go.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Codec {
    Alac,
    Aac,
    Opus,
    Vorbis,
    Flac,
    Mp3,
    Mp2,
    Ape,
    WavPack,
    Tta,
    Tak,
    Musepack,
    WmaLossless,
    WmaPro,
    Wma,
    Ac3,
    Eac3,
    Dts,
    Speex,
    /// DSD, decoded by FFmpeg at 1/8 of the 1-bit rate.
    Dsd,
    /// PCM (integer or float): lossless in FFmpeg's codec table.
    Pcm,
    /// A-law / µ-law and the like: PCM-coded but lossy.
    PcmLossy,
    TrueHd,
    /// Anything else, by FFmpeg's codec name ("dst", "mp1"…).
    Other(String),
}

impl Codec {
    /// AV_CODEC_PROP_LOSSLESS in FFmpeg's codec descriptors.
    fn lossless(&self) -> bool {
        matches!(
            self,
            Codec::Alac
                | Codec::Flac
                | Codec::Ape
                | Codec::WavPack
                | Codec::Tta
                | Codec::Tak
                | Codec::WmaLossless
                | Codec::Dts
                | Codec::Pcm
                | Codec::TrueHd
        )
    }
}

/// An AVDictionary: keys compared without case, and av_dict_set's order
/// (overwriting a key moves the last entry into its place and appends).
#[derive(Clone, Debug, Default)]
pub(crate) struct Dict(pub Vec<(String, String)>);

impl Dict {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.as_str())
    }
    fn position(&self, key: &str) -> Option<usize> {
        self.0.iter().position(|(k, _)| k.eq_ignore_ascii_case(key))
    }
    /// av_dict_set(m, key, value, 0); an empty value is still set.
    pub fn set(&mut self, key: &str, value: &str) {
        if let Some(i) = self.position(key) {
            self.0.swap_remove(i);
        }
        self.0.push((key.to_string(), value.to_string()));
    }
    /// AV_DICT_DONT_OVERWRITE.
    pub fn set_new(&mut self, key: &str, value: &str) {
        if self.position(key).is_none() {
            self.0.push((key.to_string(), value.to_string()));
        }
    }
    /// av_dict_set(m, key, NULL, 0).
    pub fn remove(&mut self, key: &str) {
        if let Some(i) = self.position(key) {
            self.0.swap_remove(i);
        }
    }
    /// AV_DICT_APPEND after a ";" when the key is there (Vorbis comments).
    pub fn append(&mut self, key: &str, value: &str) {
        match self.position(key) {
            Some(i) => {
                self.0[i].1.push(';');
                self.0[i].1.push_str(value);
            }
            None => self.0.push((key.to_string(), value.to_string())),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    /// ff_metadata_conv(pm, NULL, conv): native names to FFmpeg's.
    pub fn convert(&mut self, conv: &[(&str, &str)]) {
        let old = std::mem::take(&mut self.0);
        for (key, value) in old {
            let key = conv
                .iter()
                .find(|(native, _)| native.eq_ignore_ascii_case(&key))
                .map(|(_, g)| g.to_string())
                .unwrap_or(key);
            self.set(&key, &value);
        }
    }
    /// av_dict_copy(dst, self, 0).
    pub fn copy_into(&self, dst: &mut Dict) {
        for (k, v) in &self.0 {
            dst.set(k, v);
        }
    }
}

/// An attached picture stream.
#[derive(Clone, Debug)]
pub(crate) struct Picture {
    pub data: Vec<u8>,
    /// Its "comment" is "Cover (front)".
    pub front: bool,
}

/// A demuxer's view of the file after avformat_open_input (and the parts
/// of avformat_find_stream_info the probe needs).
#[derive(Clone, Debug)]
pub(crate) struct Demuxed {
    pub codec: Codec,
    /// iformat->name ("aiff", "w64", "caf"…): PCM labels depend on it.
    pub container: &'static str,
    pub sample_rate: u32,
    pub channels: u32,
    pub bits_raw: i32,
    pub bits_coded: i32,
    /// st->duration in `time_base` units (None: AV_NOPTS_VALUE).
    pub duration_ts: Option<i64>,
    pub time_base: (i64, i64),
    /// AVFormatContext.duration in µs (None: unknown).
    pub format_duration_us: Option<i64>,
    pub initial_padding: i64,
    pub stream_bit_rate: i64,
    pub format_bit_rate: i64,
    pub format_tags: Dict,
    pub stream_tags: Dict,
    pub pictures: Vec<Picture>,
}

impl Demuxed {
    pub fn new(codec: Codec, container: &'static str) -> Demuxed {
        Demuxed {
            codec,
            container,
            sample_rate: 0,
            channels: 0,
            bits_raw: 0,
            bits_coded: 0,
            duration_ts: None,
            time_base: (1, 1),
            format_duration_us: None,
            initial_padding: 0,
            stream_bit_rate: 0,
            format_bit_rate: 0,
            format_tags: Dict::default(),
            stream_tags: Dict::default(),
            pictures: Vec::new(),
        }
    }
}

/// av_rescale_rnd(a, b, c, AV_ROUND_NEAR_INF).
pub(crate) fn rescale(a: i64, b: i64, c: i64) -> i64 {
    if c == 0 {
        return 0;
    }
    let (a, b, c) = (a as i128, b as i128, c as i128);
    let num = a * b;
    let q = if num >= 0 { (num + c / 2) / c } else { -((-num + c / 2) / c) };
    q.clamp(i64::MIN as i128, i64::MAX as i128) as i64
}

/// The FFmpeg probe's short labels.
fn codec_label(d: &Demuxed) -> String {
    match &d.codec {
        Codec::Alac => "ALAC".into(),
        Codec::Aac => "AAC".into(),
        Codec::Opus => "Opus".into(),
        Codec::Vorbis => "Vorbis".into(),
        Codec::Flac => "FLAC".into(),
        Codec::Mp3 => "MP3".into(),
        Codec::Mp2 => "MP2".into(),
        Codec::Ape => "APE".into(),
        Codec::WavPack => "WavPack".into(),
        Codec::Tta => "TTA".into(),
        Codec::Tak => "TAK".into(),
        Codec::Musepack => "MPC".into(),
        Codec::WmaLossless => "WMA Lossless".into(),
        Codec::WmaPro => "WMA Pro".into(),
        Codec::Wma => "WMA".into(),
        Codec::Ac3 => "AC-3".into(),
        Codec::Eac3 => "E-AC-3".into(),
        Codec::Dts => "DTS".into(),
        Codec::Speex => "Speex".into(),
        Codec::Dsd => {
            let multiple = (d.sample_rate as f64 * 8.0 / 44100.0).round() as i64;
            if multiple > 0 { format!("DSD{multiple}") } else { "DSD".into() }
        }
        Codec::Pcm | Codec::PcmLossy => {
            if d.container.contains("aiff") {
                "AIFF".into()
            } else if d.container == "w64" {
                "W64".into()
            } else if d.container == "caf" {
                "CAF".into()
            } else {
                "PCM".into()
            }
        }
        Codec::TrueHd => "TRUEHD".into(),
        Codec::Other(name) => name.to_uppercase(),
    }
}

/// probe(): the stream as Sonkore reads it.
fn finish(path: &Path, d: Demuxed, want_cover: bool) -> Option<Probe> {
    if d.sample_rate == 0 {
        return None;
    }
    let rate = d.sample_rate as i64;
    let mut bits = 0u32;
    if d.codec.lossless() {
        let b = if d.bits_raw > 0 { d.bits_raw } else { d.bits_coded };
        bits = b.clamp(0, 32) as u32;
    }
    // streamFrames(): DSF / DSDIFF headers give the exact length.
    let mut frames: u64 = 0;
    let mut have = false;
    if let Some(dsd) = dsd::sonkore_dsd_probe(path)
        && dsd.rate / 8 == d.sample_rate
    {
        frames = dsd.bytes_per_channel;
        have = true;
    }
    if !have {
        if let Some(ts) = d.duration_ts.filter(|&ts| ts > 0) {
            let mut f = rescale(ts, d.time_base.0 * rate, d.time_base.1).max(0) as u64;
            let padding = d.initial_padding.max(0) as u64;
            if d.container == "ogg" && padding < f {
                f -= padding;
            }
            frames = f;
        } else if let Some(us) = d.format_duration_us.filter(|&us| us > 0) {
            frames = rescale(us, rate, 1_000_000).max(0) as u64;
        }
    }
    let lossless = bits > 0;
    let bit_rate = if d.stream_bit_rate > 0 { d.stream_bit_rate } else { d.format_bit_rate };
    let bitrate_kbps = if lossless || bit_rate <= 0 { 0 } else { ((bit_rate + 500) / 1000) as i32 };

    let mut tags = d.format_tags.0.clone();
    tags.extend(d.stream_tags.0.iter().cloned());
    // The front cover, or the first picture.
    let mut cover = Vec::new();
    let has_cover = !d.pictures.is_empty();
    if want_cover {
        let mut have_front = false;
        for p in &d.pictures {
            if p.data.is_empty() {
                continue;
            }
            if cover.is_empty() || (p.front && !have_front) {
                cover = p.data.clone();
                have_front = p.front;
            }
        }
    }
    Some(Probe {
        codec: codec_label(&d),
        sample_rate: d.sample_rate,
        channels: d.channels,
        bits,
        lossless,
        total_frames: frames,
        duration: frames as f64 / rate as f64,
        bitrate_kbps,
        tags,
        has_cover,
        cover,
    })
}

/// Reading helpers over a file.
pub(crate) struct Src {
    pub file: File,
    pub size: u64,
}

impl Src {
    pub fn open(path: &Path) -> Option<Src> {
        let file = File::open(path).ok()?;
        let size = file.metadata().ok()?.len();
        Some(Src { file, size })
    }
    /// Up to `n` bytes at `pos`.
    pub fn read_at(&mut self, pos: u64, n: u64) -> Vec<u8> {
        if pos >= self.size || self.file.seek(SeekFrom::Start(pos)).is_err() {
            return Vec::new();
        }
        let n = n.min(self.size - pos).min(1 << 28);
        let mut buf = Vec::with_capacity(n as usize);
        let _ = (&mut self.file).take(n).read_to_end(&mut buf);
        buf
    }
}

/// The size of an ID3v2 tag at the start of `head` (header, body, footer).
pub(crate) fn id3v2_size(head: &[u8]) -> u64 {
    if head.len() >= 10
        && head.starts_with(b"ID3")
        && head[3] != 0xff
        && head[4] != 0xff
        && head[6..10].iter().all(|&b| b & 0x80 == 0)
    {
        let size = ((head[6] as u64) << 21) | ((head[7] as u64) << 14) | ((head[8] as u64) << 7) | head[9] as u64;
        return 10 + size + if head[5] & 0x10 != 0 { 10 } else { 0 };
    }
    0
}

/// The FFmpeg probe for one file: None when FFmpeg would not open it
/// (or finds no audio).
pub(crate) fn probe(path: &Path, want_cover: bool) -> Option<Probe> {
    let mut src = Src::open(path)?;
    // avformat_open_input reads an ID3v2 tag at the start of any file and
    // skips it before the demuxer looks (several can follow each other).
    let mut id3_end = 0u64;
    let mut id3_tags: Vec<Vec<u8>> = Vec::new();
    loop {
        let head = src.read_at(id3_end, 10);
        let size = id3v2_size(&head);
        if size == 0 {
            break;
        }
        id3_tags.push(src.read_at(id3_end, size));
        id3_end += size;
    }
    let head = src.read_at(id3_end, 64);
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let mut d = sniff_and_read(&mut src, &head, id3_end, &ext)?;

    // The ID3v2 tag at the start: kept only when the demuxer found no tags
    // of its own ("Discarding ID3 tags because more suitable tags were
    // found"); its pictures only for the demuxers that take them.
    if !id3_tags.is_empty() {
        let mut meta = Dict::default();
        let mut pics = Vec::new();
        for tag in &id3_tags {
            ffid3::read_dict(tag, &mut meta, Some(&mut pics));
        }
        if matches!(d.container, "mp3" | "aac" | "tta" | "wav") {
            d.pictures.splice(0..0, pics);
        }
        if d.format_tags.is_empty() {
            d.format_tags = meta;
        }
    }
    finish(path, d, want_cover)
}

fn sniff_and_read(src: &mut Src, head: &[u8], start: u64, ext: &str) -> Option<Demuxed> {
    let h = |at: usize, s: &[u8]| head.len() >= at + s.len() && &head[at..at + s.len()] == s;
    if h(4, b"ftyp") || h(4, b"moov") || h(4, b"mdat") || h(4, b"free") || h(4, b"wide") || h(4, b"skip") {
        return mp4::read(src, start);
    }
    if h(0, b"OggS") {
        return ogg::read(src, start);
    }
    if h(0, b"FORM") && (h(8, b"AIFF") || h(8, b"AIFC")) {
        return pcm::read_aiff(src, start);
    }
    if h(0, b"riff") && head.len() >= 16 && head[..16] == pcm::W64_RIFF {
        return pcm::read_w64(src, start);
    }
    if h(0, b"caff") {
        return caf::read(src, start);
    }
    if h(0, b"MAC ") {
        return lossless::read_ape(src, start);
    }
    if h(0, b"wvpk") {
        return lossless::read_wv(src, start);
    }
    if h(0, b"TTA1") {
        return lossless::read_tta(src, start);
    }
    if h(0, b"MPCK") || h(0, b"MP+") {
        return lossless::read_mpc(src, start);
    }
    if h(0, b"tBaK") {
        return lossless::read_tak(src, start);
    }
    if h(0, b"DSD ") {
        return dsd::read_dsf(src);
    }
    if h(0, b"FRM8") {
        return dsd::read_dff(src);
    }
    if head.len() >= 16 && head[..16] == asf::HEADER_GUID {
        return asf::read(src);
    }
    if h(0, &[0x1A, 0x45, 0xDF, 0xA3]) {
        return mkv::read(src);
    }
    raw::read(src, head, start, ext)
}

/// FFmpeg's ID3v1 genre names (ff_id3v1_genre_str).
pub(crate) const ID3V1_GENRES: [&str; 192] = [
    "Blues",
    "Classic Rock",
    "Country",
    "Dance",
    "Disco",
    "Funk",
    "Grunge",
    "Hip-Hop",
    "Jazz",
    "Metal",
    "New Age",
    "Oldies",
    "Other",
    "Pop",
    "R&B",
    "Rap",
    "Reggae",
    "Rock",
    "Techno",
    "Industrial",
    "Alternative",
    "Ska",
    "Death Metal",
    "Pranks",
    "Soundtrack",
    "Euro-Techno",
    "Ambient",
    "Trip-Hop",
    "Vocal",
    "Jazz+Funk",
    "Fusion",
    "Trance",
    "Classical",
    "Instrumental",
    "Acid",
    "House",
    "Game",
    "Sound Clip",
    "Gospel",
    "Noise",
    "AlternRock",
    "Bass",
    "Soul",
    "Punk",
    "Space",
    "Meditative",
    "Instrumental Pop",
    "Instrumental Rock",
    "Ethnic",
    "Gothic",
    "Darkwave",
    "Techno-Industrial",
    "Electronic",
    "Pop-Folk",
    "Eurodance",
    "Dream",
    "Southern Rock",
    "Comedy",
    "Cult",
    "Gangsta",
    "Top 40",
    "Christian Rap",
    "Pop/Funk",
    "Jungle",
    "Native American",
    "Cabaret",
    "New Wave",
    "Psychedelic",
    "Rave",
    "Showtunes",
    "Trailer",
    "Lo-Fi",
    "Tribal",
    "Acid Punk",
    "Acid Jazz",
    "Polka",
    "Retro",
    "Musical",
    "Rock & Roll",
    "Hard Rock",
    "Folk",
    "Folk-Rock",
    "National Folk",
    "Swing",
    "Fast Fusion",
    "Bebob",
    "Latin",
    "Revival",
    "Celtic",
    "Bluegrass",
    "Avantgarde",
    "Gothic Rock",
    "Progressive Rock",
    "Psychedelic Rock",
    "Symphonic Rock",
    "Slow Rock",
    "Big Band",
    "Chorus",
    "Easy Listening",
    "Acoustic",
    "Humour",
    "Speech",
    "Chanson",
    "Opera",
    "Chamber Music",
    "Sonata",
    "Symphony",
    "Booty Bass",
    "Primus",
    "Porn Groove",
    "Satire",
    "Slow Jam",
    "Club",
    "Tango",
    "Samba",
    "Folklore",
    "Ballad",
    "Power Ballad",
    "Rhythmic Soul",
    "Freestyle",
    "Duet",
    "Punk Rock",
    "Drum Solo",
    "A capella",
    "Euro-House",
    "Dance Hall",
    "Goa",
    "Drum & Bass",
    "Club-House",
    "Hardcore",
    "Terror",
    "Indie",
    "BritPop",
    "Negerpunk",
    "Polsk Punk",
    "Beat",
    "Christian Gangsta",
    "Heavy Metal",
    "Black Metal",
    "Crossover",
    "Contemporary Christian",
    "Christian Rock",
    "Merengue",
    "Salsa",
    "Thrash Metal",
    "Anime",
    "JPop",
    "Synthpop",
    "Abstract",
    "Art Rock",
    "Baroque",
    "Bhangra",
    "Big Beat",
    "Breakbeat",
    "Chillout",
    "Downtempo",
    "Dub",
    "EBM",
    "Eclectic",
    "Electro",
    "Electroclash",
    "Emo",
    "Experimental",
    "Garage",
    "Global",
    "IDM",
    "Illbient",
    "Industro-Goth",
    "Jam Band",
    "Krautrock",
    "Leftfield",
    "Lounge",
    "Math Rock",
    "New Romantic",
    "Nu-Breakz",
    "Post-Punk",
    "Post-Rock",
    "Psytrance",
    "Shoegaze",
    "Space Rock",
    "Trop Rock",
    "World Music",
    "Neoclassical",
    "Audiobook",
    "Audio Theatre",
    "Neue Deutsche Welle",
    "Podcast",
    "Indie Rock",
    "G-Funk",
    "Dubstep",
    "Garage Rock",
    "Psybient",
];

/// ff_id3v1_read: the 128-byte tag at the end of the file.
pub(crate) fn read_id3v1(src: &mut Src, dict: &mut Dict) {
    if src.size <= 128 {
        return;
    }
    let buf = src.read_at(src.size - 128, 128);
    if buf.len() < 128 || !buf.starts_with(b"TAG") {
        return;
    }
    let get = |dict: &mut Dict, key: &str, field: &[u8]| {
        let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
        let s = super::text::latin1(&field[..end]);
        let s = s.trim_end_matches(' ');
        if !s.is_empty() {
            dict.set(key, s);
        }
    };
    get(dict, "title", &buf[3..33]);
    get(dict, "artist", &buf[33..63]);
    get(dict, "album", &buf[63..93]);
    get(dict, "date", &buf[93..97]);
    get(dict, "comment", &buf[97..127]);
    if buf[125] == 0 && buf[126] != 0 {
        dict.set("track", &buf[126].to_string());
    }
    if (buf[127] as usize) < ID3V1_GENRES.len() {
        dict.set("genre", ID3V1_GENRES[buf[127] as usize]);
    }
}

/// Little/big endian readers on slices (0 past the end).
pub(crate) fn rd(d: &[u8], at: usize, n: usize, big: bool) -> u64 {
    if at + n > d.len() {
        return 0;
    }
    let mut v = 0u64;
    for i in 0..n {
        let b = d[if big { at + i } else { at + n - 1 - i }] as u64;
        v = (v << 8) | b;
    }
    v
}
pub(crate) fn be(d: &[u8], at: usize, n: usize) -> u64 {
    rd(d, at, n, true)
}
pub(crate) fn le(d: &[u8], at: usize, n: usize) -> u64 {
    rd(d, at, n, false)
}

/// A big-endian bit reader (0 past the end).
pub(crate) struct BitsBe<'a> {
    d: &'a [u8],
    at: usize,
}

pub(crate) fn bits_be(d: &[u8]) -> BitsBe<'_> {
    BitsBe { d, at: 0 }
}

impl BitsBe<'_> {
    pub fn get(&mut self, n: usize) -> u64 {
        let mut v = 0u64;
        for _ in 0..n {
            let byte = self.d.get(self.at / 8).copied().unwrap_or(0);
            v = (v << 1) | ((byte >> (7 - self.at % 8)) & 1) as u64;
            self.at += 1;
        }
        v
    }
    pub fn skip(&mut self, n: usize) {
        self.at += n;
    }
}
