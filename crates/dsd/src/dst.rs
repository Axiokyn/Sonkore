//! DST (Direct Stream Transfer, ISO/IEC 14496-3 subpart 10) decoding: the
//! lossless coding of DSD on SACDs and in DSDIFF files (a line-for-line
//! port).
//!
//! Each frame (1/75 s) carries, for every channel, a prediction filter of up
//! to 128 taps over the previous bits and a table of probabilities indexed by
//! the prediction's confidence, and the bits as arithmetic-coded residuals of
//! the filter's guess. The frame comes out as the DSD it was made from, bit for
//! bit. Every SACD rip uses one segment per channel; a frame with more is
//! refused rather than guessed at. One instance per stream.

use crate::SILENCE_BYTE;

pub const MAX_CHANNELS: usize = 6;
/// Tables a frame may define.
pub const MAX_ELEMENTS: usize = 2 * MAX_CHANNELS;

// Predictors for the coded filter coefficients and probability tables (10.12,
// 10.13): each coefficient from the one to three before it.
const FSETS_PRED: [[i32; 3]; 3] = [[-8, 0, 0], [-16, 8, 0], [-9, -5, 6]];
const PROBS_PRED: [[i32; 3]; 3] = [[-8, 0, 0], [-16, 8, 0], [-24, 24, -8]];

fn reverse7(c: u32) -> u32 {
    let mut r = 0;
    for i in 0..7 {
        r |= ((c >> i) & 1) << (6 - i);
    }
    r
}

/// MSB-first bit reader; past the end it returns zeros and remembers it.
struct BitReader<'a> {
    data: &'a [u8],
    bits: usize,
    pos: usize,
    overrun: bool,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        BitReader { data, bits: data.len() * 8, pos: 0, overrun: false }
    }

    #[inline]
    fn bit(&mut self) -> u32 {
        if self.pos >= self.bits {
            self.pos += 1;
            self.overrun = true;
            return 0;
        }
        let v = (self.data[self.pos >> 3] >> (7 - (self.pos & 7))) & 1;
        self.pos += 1;
        v as u32
    }

    fn bits(&mut self, n: u32) -> u32 {
        let mut v = 0u32;
        for _ in 0..n {
            v = v << 1 | self.bit();
        }
        v
    }

    fn sbits(&mut self, n: u32) -> i32 {
        let v = self.bits(n);
        if v & (1 << (n - 1)) != 0 { v as i32 - (1 << n) } else { v as i32 }
    }

    /// Rice code of the standard's tables: zeros, a one, k bits, then a sign
    /// bit when the value is not zero.
    fn rice(&mut self, k: u32) -> i32 {
        let mut q = 0u32;
        while self.bit() == 0 {
            q += 1;
            if q > 4096 || self.overrun {
                return 0;
            }
        }
        let mut v = (q << k | self.bits(k)) as i32;
        if v != 0 && self.bit() != 0 {
            v = -v;
        }
        v
    }
}

/// The binary arithmetic decoder of 10.11: a 12-bit interval `a`, the code
/// value `c` inside it; p (1..128, in 256ths) is the probability of the
/// symbol 0, which takes the top of the interval.
struct ArithDecoder {
    a: u32,
    c: u32,
}

impl ArithDecoder {
    #[inline]
    fn get(&mut self, gb: &mut BitReader, p: u32) -> u32 {
        let k = (self.a >> 8) | ((self.a >> 7) & 1);
        let q = k.wrapping_mul(p);
        let aq = self.a.wrapping_sub(q);
        let e;
        if self.c < aq {
            e = 1;
            self.a = aq;
        } else {
            e = 0;
            self.a = q;
            self.c = self.c.wrapping_sub(aq);
        }
        while self.a < 2048 {
            self.a <<= 1;
            self.c = self.c << 1 | gb.bit();
        }
        e
    }
}

#[derive(Clone)]
struct Table {
    elements: usize,
    length: [u32; MAX_ELEMENTS],
    coeff: [[i32; 128]; MAX_ELEMENTS],
}

impl Default for Table {
    fn default() -> Self {
        Table { elements: 1, length: [0; MAX_ELEMENTS], coeff: [[0; 128]; MAX_ELEMENTS] }
    }
}

/// A DST frame decoder. Not thread-safe; one per stream.
pub struct DstDecoder {
    fsets: Box<Table>,
    probs: Box<Table>,
    // For each filter, history byte and byte value: the sum of the filter's
    // coefficients with the sign of each bit. [element][16][256]
    filter: Vec<i16>,
    status: [[u8; 16]; MAX_CHANNELS],
}

impl Default for DstDecoder {
    fn default() -> Self {
        Self::new()
    }
}

/// Why a frame could not be decoded (its output is then DSD silence).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DstFrameError(pub &'static str);

impl std::fmt::Display for DstFrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for DstFrameError {}

impl DstDecoder {
    pub const MAX_CHANNELS: usize = MAX_CHANNELS;

    pub fn new() -> Self {
        DstDecoder { fsets: Box::default(), probs: Box::default(), filter: Vec::new(), status: [[0; 16]; MAX_CHANNELS] }
    }

    /// Bits per channel in one frame at a DSD rate (588 * rate / 44100).
    pub fn samples_per_frame(dsd_rate: u32) -> usize {
        588 * (dsd_rate / 44100) as usize
    }

    /// Decodes one frame (`data`) into `out`: `channels` runs of `samples / 8`
    /// bytes, MSB first (the oldest bit first). On a malformed frame `out`
    /// holds DSD silence and the error says why.
    pub fn decode(
        &mut self,
        data: &[u8],
        channels: usize,
        samples: usize,
        out: &mut [u8],
    ) -> Result<(), DstFrameError> {
        let bytes_per_channel = samples / 8;
        let total = (bytes_per_channel * channels).min(out.len());
        let fail = |out: &mut [u8], why: &'static str| {
            out[..total].fill(SILENCE_BYTE);
            Err(DstFrameError(why))
        };
        if channels == 0
            || channels > MAX_CHANNELS
            || samples % 8 != 0
            || data.is_empty()
            || out.len() < bytes_per_channel * channels
        {
            return fail(out, "DST frame: unsupported channel count or frame length");
        }
        let mut gb = BitReader::new(data);

        if gb.bit() == 0 {
            // A frame of plain DSD (10.3): the bytes, one per channel in turn.
            gb.bit();
            if gb.bits(6) != 0 || data.len() < 1 + bytes_per_channel * channels {
                return fail(out, "DST frame: bad uncompressed frame");
            }
            let mut p = 1;
            for i in 0..bytes_per_channel {
                for ch in 0..channels {
                    out[ch * bytes_per_channel + i] = data[p];
                    p += 1;
                }
            }
            return Ok(());
        }

        // Segmentation (10.4-10.6): one segment per channel is what every
        // encoder writes; anything else is not decoded.
        if gb.bit() == 0 || gb.bit() == 0 || gb.bit() == 0 {
            return fail(out, "DST frame: segmented frames are not supported");
        }

        // Mapping (10.7-10.9).
        let mut fmap = [0usize; MAX_CHANNELS];
        let mut pmap = [0usize; MAX_CHANNELS];
        let same_map = gb.bit() != 0;
        if !read_map(&mut gb, &mut self.fsets, &mut fmap, channels) {
            return fail(out, "DST frame: bad filter map");
        }
        if same_map {
            self.probs.elements = self.fsets.elements;
            pmap = fmap;
        } else if !read_map(&mut gb, &mut self.probs, &mut pmap, channels) {
            return fail(out, "DST frame: bad probability map");
        }

        // Half probability (10.10): the first bits of a channel, as many as
        // its filter has taps, coded at 50 %.
        let mut half_prob = [false; MAX_CHANNELS];
        for h in half_prob.iter_mut().take(channels) {
            *h = gb.bit() != 0;
        }

        // Filter coefficient sets (10.12) and probability tables (10.13).
        if !read_table(&mut gb, &mut self.fsets, &FSETS_PRED, 7, 9, true, 0) {
            return fail(out, "DST frame: bad filter coefficients");
        }
        if !read_table(&mut gb, &mut self.probs, &PROBS_PRED, 6, 7, false, 1) {
            return fail(out, "DST frame: bad probability table");
        }

        // Arithmetic coded data (10.11).
        if gb.bit() != 0 {
            return fail(out, "DST frame: reserved bit set");
        }
        let mut ac = ArithDecoder { a: 4095, c: 0 };
        ac.c = gb.bits(12);
        if !self.build_filter() {
            return fail(out, "DST frame: filter overflow");
        }

        for s in self.status.iter_mut() {
            s.fill(0xAA);
        }
        out[..bytes_per_channel * channels].fill(0);
        ac.get(&mut gb, reverse7(self.fsets.coeff[0][0] as u32 & 127) + 1); // DST_X_bit

        let fsets = &*self.fsets;
        let probs = &*self.probs;
        for i in 0..samples {
            for ch in 0..channels {
                let felem = fmap[ch];
                let filter = &self.filter[felem * 16 * 256..(felem + 1) * 16 * 256];
                let status = &mut self.status[ch];
                let mut sum = 0i32;
                for j in 0..16 {
                    sum += filter[j * 256 + status[j] as usize] as i32;
                }
                let predict = sum as i16;

                let prob = if !half_prob[ch] || i >= fsets.length[felem] as usize {
                    let pelem = pmap[ch];
                    let index = (if predict < 0 { -(predict as i32) } else { predict as i32 }) as u32 >> 3;
                    probs.coeff[pelem][index.min(probs.length[pelem].wrapping_sub(1)) as usize] as u32
                } else {
                    128
                };
                let residual = ac.get(&mut gb, prob);
                let v = ((predict < 0) as u32 ^ residual) & 1;
                out[ch * bytes_per_channel + (i >> 3)] |= (v << (7 - (i & 7))) as u8;

                // 128 bits of history, the newest in bit 0 of byte 0.
                let mut carry = v as u8;
                for b in status.iter_mut() {
                    let next = *b >> 7;
                    *b = *b << 1 | carry;
                    carry = next;
                }
            }
        }
        if gb.overrun {
            return fail(out, "DST frame: truncated");
        }
        Ok(())
    }

    /// The prediction of a filter is the sum over its taps of a coefficient
    /// with the sign of the bit it looks at; tabulated per byte of history, so
    /// a prediction costs 16 lookups.
    fn build_filter(&mut self) -> bool {
        let fsets = &*self.fsets;
        self.filter.clear();
        self.filter.resize(fsets.elements * 16 * 256, 0);
        for i in 0..fsets.elements {
            let length = fsets.length[i] as i32;
            for j in 0..16 {
                let total = (length - j as i32 * 8).clamp(0, 8);
                let row = &mut self.filter[(i * 16 + j) * 256..(i * 16 + j + 1) * 256];
                for (k, slot) in row.iter_mut().enumerate() {
                    let mut v = 0i32;
                    for l in 0..total as usize {
                        v += (((k >> l) & 1) as i32 * 2 - 1) * fsets.coeff[i][j * 8 + l];
                    }
                    if v != v as i16 as i32 {
                        return false;
                    }
                    *slot = v as i16;
                }
            }
        }
        true
    }
}

/// Which filter (or probability table) each channel uses (10.7-10.9): either
/// all the first one, or an index per channel, a new table being announced by
/// the first channel that names it.
fn read_map(gb: &mut BitReader, t: &mut Table, map: &mut [usize; MAX_CHANNELS], channels: usize) -> bool {
    t.elements = 1;
    map[0] = 0;
    if gb.bit() != 0 {
        map.fill(0);
        return true;
    }
    for ch in 1..channels {
        let bits = usize::BITS - t.elements.leading_zeros(); // std::bit_width
        map[ch] = gb.bits(bits) as usize;
        if map[ch] == t.elements {
            t.elements += 1;
            if t.elements >= MAX_ELEMENTS {
                return false;
            }
        } else if map[ch] > t.elements {
            return false;
        }
    }
    true
}

fn read_table(
    gb: &mut BitReader,
    t: &mut Table,
    pred: &[[i32; 3]; 3],
    length_bits: u32,
    coeff_bits: u32,
    signed: bool,
    offset: i32,
) -> bool {
    for i in 0..t.elements {
        t.length[i] = gb.bits(length_bits) + 1;
        let uncoded = |gb: &mut BitReader, t: &mut Table, count: usize| {
            for j in 0..count {
                t.coeff[i][j] = (if signed { gb.sbits(coeff_bits) } else { gb.bits(coeff_bits) as i32 }) + offset;
            }
        };
        if gb.bit() == 0 {
            let n = t.length[i] as usize;
            uncoded(gb, t, n);
            continue;
        }
        let method = gb.bits(2) as usize;
        if method == 3 {
            return false;
        }
        uncoded(gb, t, method + 1);
        let lsb = gb.bits(3);
        for j in method + 1..t.length[i] as usize {
            let mut x = 0i32;
            for k in 0..=method {
                x += pred[method][k] * t.coeff[i][j - k - 1];
            }
            let mut c = gb.rice(lsb);
            c += if x >= 0 { -((x + 4) / 8) } else { (-x + 3) / 8 };
            if !signed && (c < offset || c >= offset + (1 << coeff_bits)) {
                return false;
            }
            t.coeff[i][j] = c;
        }
    }
    !gb.overrun
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_per_frame() {
        assert_eq!(DstDecoder::samples_per_frame(2_822_400), 37632);
        assert_eq!(DstDecoder::samples_per_frame(5_644_800), 75264);
    }

    #[test]
    fn uncompressed_frame() {
        // A zero header byte, then the bytes one per channel in turn.
        let mut frame = vec![0u8];
        for i in 0..4u8 {
            frame.push(i);
            frame.push(0x80 | i);
        }
        let mut out = [0u8; 8];
        DstDecoder::new().decode(&frame, 2, 32, &mut out).unwrap();
        assert_eq!(out, [0, 1, 2, 3, 0x80, 0x81, 0x82, 0x83]);
    }

    #[test]
    fn malformed_frames_give_silence() {
        let mut out = [0u8; 8];
        // Segmented frame (the second bit clear).
        assert!(DstDecoder::new().decode(&[0x80], 2, 32, &mut out).is_err());
        assert_eq!(out, [SILENCE_BYTE; 8]);
        out = [0; 8];
        // A truncated compressed frame.
        assert!(DstDecoder::new().decode(&[0xF0, 0x12], 2, 32, &mut out).is_err());
        assert_eq!(out, [SILENCE_BYTE; 8]);
        // Too many channels.
        let mut big = [0u8; 7 * 4];
        assert!(DstDecoder::new().decode(&[0], 7, 32, &mut big).is_err());
    }
}
