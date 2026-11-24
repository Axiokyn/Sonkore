//! Raw streams: AC-3 / E-AC-3, DTS, ADTS AAC and MPEG audio (.mp2). Their
//! FFmpeg demuxers know nothing from a header, so avformat_find_stream_info
//! parses frames and the length is estimated from the bit rate
//! (estimate_timings_from_bit_rate), in each demuxer's time base.

use super::{Codec, Demuxed, Dict, Src, read_id3v1, rescale};

fn finish_estimate(d: &mut Demuxed, src: &Src, data_offset: u64, tb_den: i64) {
    d.time_base = (1, tb_den);
    if d.stream_bit_rate > 0 && src.size > data_offset {
        d.duration_ts = Some(rescale((src.size - data_offset) as i64, 8 * tb_den, d.stream_bit_rate));
    }
}

const AC3_BITRATES: [i64; 19] =
    [32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384, 448, 512, 576, 640];
const AC3_CHANNELS: [u32; 8] = [2, 1, 2, 3, 3, 4, 4, 5];

fn ac3(h: &[u8]) -> Option<Demuxed> {
    if h.len() < 8 || h[0] != 0x0B || h[1] != 0x77 {
        return None;
    }
    let bsid = h[5] >> 3;
    let mut bits = super::bits_be(&h[5..]);
    if bsid <= 10 {
        let fscod = h[4] >> 6;
        let frmsizecod = (h[4] & 0x3f) as usize;
        let rate = [48000u32, 44100, 32000].get(fscod as usize).copied()?;
        let shift = bsid.max(8) - 8;
        let kbps = *AC3_BITRATES.get(frmsizecod >> 1)?;
        bits.skip(8); // bsid, bsmod
        let acmod = bits.get(3) as usize;
        if acmod & 1 != 0 && acmod != 1 {
            bits.skip(2);
        }
        if acmod & 4 != 0 {
            bits.skip(2);
        }
        if acmod == 2 {
            bits.skip(2);
        }
        let lfe = bits.get(1) as u32;
        let mut d = Demuxed::new(Codec::Ac3, "ac3");
        d.sample_rate = rate >> shift;
        d.channels = AC3_CHANNELS[acmod] + lfe;
        d.stream_bit_rate = (kbps * 1000) >> shift;
        Some(d)
    } else if bsid <= 16 {
        // E-AC-3: frame size, rate, blocks, mode.
        let frame_size = ((((h[2] & 7) as i64) << 8) | h[3] as i64) + 1;
        let fscod = h[4] >> 6;
        let (rate, blocks) = if fscod == 3 {
            ([24000u32, 22050, 16000].get(((h[4] >> 4) & 3) as usize).copied()?, 6)
        } else {
            ([48000u32, 44100, 32000][fscod as usize], [1i64, 2, 3, 6][((h[4] >> 4) & 3) as usize])
        };
        let acmod = ((h[4] >> 1) & 7) as usize;
        let lfe = (h[4] & 1) as u32;
        let mut d = Demuxed::new(Codec::Eac3, "eac3");
        d.sample_rate = rate;
        d.channels = AC3_CHANNELS[acmod] + lfe;
        d.stream_bit_rate = 8 * frame_size * 2 * rate as i64 / (blocks * 256);
        Some(d)
    } else {
        None
    }
}

const DTS_RATES: [u32; 16] = [0, 8000, 16000, 32000, 0, 0, 11025, 22050, 44100, 0, 0, 12000, 24000, 48000, 0, 0];
const DTS_BITRATES: [i64; 32] = [
    32000, 56000, 64000, 96000, 112000, 128000, 192000, 224000, 256000, 320000, 384000, 448000, 512000, 576000, 640000,
    768000, 960000, 1024000, 1152000, 1280000, 1344000, 1408000, 1411200, 1472000, 1536000, 1920000, 2048000, 3072000,
    3840000, 0, 0, 0,
];
const DTS_CHANNELS: [u32; 16] = [1, 2, 2, 2, 2, 3, 3, 4, 4, 5, 6, 6, 6, 7, 8, 8];

fn dts(h: &[u8]) -> Option<Demuxed> {
    if h.len() < 16 || h[..4] != [0x7F, 0xFE, 0x80, 0x01] {
        return None;
    }
    let mut b = super::bits_be(&h[4..]);
    b.skip(1 + 5 + 1 + 7 + 14); // frame type, deficit, CRC, blocks, frame size
    let amode = b.get(6) as usize;
    let sfreq = b.get(4) as usize;
    let rate_index = b.get(5) as usize;
    b.skip(10); // down mix … ext audio id
    let lfe = b.get(2);
    let mut d = Demuxed::new(Codec::Dts, "dts");
    d.sample_rate = DTS_RATES[sfreq];
    d.channels = DTS_CHANNELS.get(amode).copied().unwrap_or(2) + u32::from(lfe != 0);
    d.stream_bit_rate = DTS_BITRATES[rate_index];
    if d.sample_rate == 0 {
        return None;
    }
    Some(d)
}

const AAC_RATES: [u32; 13] = [96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350];

/// ADTS: the parser's running average of the frames' bit rates over the
/// frames find_stream_info reads: packets without timestamps, so it reads
/// max_ts_probe (50) of them.
fn adts(src: &mut Src, start: u64) -> Option<Demuxed> {
    let mut pos = start;
    let mut first: Option<Demuxed> = None;
    let mut avg: i64 = 0;
    let mut n: i64 = 0;
    let buf = src.read_at(start, 4 << 20);
    let mut p = 0usize;
    while p + 7 <= buf.len() {
        let h = &buf[p..];
        if h[0] != 0xFF || h[1] & 0xF6 != 0xF0 {
            break;
        }
        let sf = ((h[2] >> 2) & 0xF) as usize;
        let rate = *AAC_RATES.get(sf)?;
        let chan = (((h[2] & 1) << 2) | (h[3] >> 6)) as u32;
        let size = ((((h[3] & 3) as usize) << 11) | ((h[4] as usize) << 3) | ((h[5] as usize) >> 5)) as i64;
        let blocks = (h[6] & 3) as i64 + 1;
        if size < 7 {
            break;
        }
        if first.is_none() {
            let mut d = Demuxed::new(Codec::Aac, "aac");
            d.sample_rate = rate;
            d.channels = if chan == 7 { 8 } else { chan };
            first = Some(d);
        }
        let frame_rate = size * 8 * rate as i64 / (1024 * blocks);
        n += 1;
        avg += (frame_rate - avg) / n;
        p += size as usize;
        pos += size as u64;
        if n >= 50 {
            break;
        }
    }
    let _ = pos;
    let mut d = first?;
    d.stream_bit_rate = avg;
    Some(d)
}

/// MPEG audio frame header: layer, rate, channels, bit rate.
fn mpeg_audio(h: &[u8]) -> Option<Demuxed> {
    if h.len() < 4 || h[0] != 0xFF || h[1] & 0xE0 != 0xE0 {
        return None;
    }
    let version = (h[1] >> 3) & 3;
    let layer = 4 - ((h[1] >> 1) & 3);
    let br_index = (h[2] >> 4) as usize;
    let sr_index = ((h[2] >> 2) & 3) as usize;
    if version == 1 || layer == 4 || br_index == 0 || br_index == 15 || sr_index == 3 {
        return None;
    }
    let lsf = version != 3;
    const RATES: [u32; 3] = [44100, 48000, 32000];
    let rate = RATES[sr_index]
        >> if version == 3 {
            0
        } else if version == 2 {
            1
        } else {
            2
        };
    const TABLE: [[[i64; 15]; 3]; 2] = [
        [
            [0, 32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 416, 448],
            [0, 32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384],
            [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320],
        ],
        [
            [0, 32, 48, 56, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256],
            [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160],
            [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160],
        ],
    ];
    let kbps = TABLE[usize::from(lsf)][layer as usize - 1][br_index];
    let codec = match layer {
        1 => Codec::Other("mp1".into()),
        2 => Codec::Mp2,
        _ => Codec::Mp3,
    };
    let mut d = Demuxed::new(codec, "mp3");
    d.sample_rate = rate;
    d.channels = if h[3] >> 6 == 3 { 1 } else { 2 };
    d.stream_bit_rate = kbps * 1000;
    Some(d)
}

pub(super) fn read(src: &mut Src, head: &[u8], start: u64, ext: &str) -> Option<Demuxed> {
    let mut d;
    let den;
    match ext {
        "ac3" | "eac3" => {
            d = ac3(head)?;
            den = 90000;
        }
        "dts" => {
            d = dts(head)?;
            den = 90000;
        }
        "aac" => {
            d = adts(src, start)?;
            den = 28224000;
        }
        _ => {
            // MPEG audio (the mp3 demuxer), after a possible junk prefix.
            let buf = src.read_at(start, 64 * 1024);
            let at = (0..buf.len().saturating_sub(4)).find(|&i| mpeg_audio(&buf[i..]).is_some())?;
            d = mpeg_audio(&buf[at..])?;
            den = 14112000;
            let mut meta = Dict::default();
            read_id3v1(src, &mut meta);
            d.format_tags = meta;
            finish_estimate(&mut d, src, start, den);
            return Some(d);
        }
    }
    finish_estimate(&mut d, src, start, den);
    Some(d)
}
