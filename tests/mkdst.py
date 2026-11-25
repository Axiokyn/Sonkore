#!/usr/bin/env python3
"""DST (Direct Stream Transfer, ISO/IEC 14496-3 subpart 10) encoder for the
test corpus: writes a DSDIFF file whose DST frames carry the DSD bits of a DSF
or DSDIFF file, losslessly, with the bitstream features real SACD rips use
(several filter and probability tables, coded tables, half-probability
frames) so that DsdDecoder's DST decoder is exercised. Not a good compressor:
the predictors are short, the point is the syntax.

    tests/mkdst.py <in.dsf|in.dff> <out.dff> [plain|mixed]

plain: the same audio as an uncompressed DSDIFF (for comparisons).
mixed: the second frame carries uncompressed DSD inside the DST stream.
Needs numpy.
"""
import struct, sys
import numpy as np

# --- DSD input ----------------------------------------------------------------

def read_dsf(data):
    assert data[:4] == b"DSD " and data[28:32] == b"fmt "
    (fmt_size, ver, fid, ctype, ch, rate, bps, nsamples, block, _) = struct.unpack_from("<QIIIIIIQII", data, 32)
    assert fid == 0 and bps in (1, 8)
    pos = 28 + fmt_size
    assert data[pos:pos + 4] == b"data"
    size = struct.unpack_from("<Q", data, pos + 4)[0]
    body = data[pos + 12:pos + size]
    nbytes = (nsamples + 7) // 8
    chans = [bytearray() for _ in range(ch)]
    off = 0
    while off + block * ch <= len(body) and len(chans[0]) < nbytes:
        for c in range(ch):
            chans[c] += body[off:off + block]
            off += block
    rev = bytes(int(f"{i:08b}"[::-1], 2) for i in range(256)) if bps == 1 else None
    out = []
    for c in range(ch):
        b = bytes(chans[c][:nbytes])
        out.append(b.translate(rev) if rev else b)  # DSF: LSB first per byte; DSDIFF: MSB first
    return rate, out

def read_dff(data):
    assert data[:4] == b"FRM8" and data[12:16] == b"DSD "
    end = 12 + struct.unpack_from(">Q", data, 4)[0]
    pos, rate, ch = 16, 0, 0
    while pos + 12 <= end:
        cid = data[pos:pos + 4]; size = struct.unpack_from(">Q", data, pos + 4)[0]; body = pos + 12
        if cid == b"PROP":
            sub = body + 4
            while sub + 12 <= body + size:
                sid = data[sub:sub + 4]; ssize = struct.unpack_from(">Q", data, sub + 4)[0]
                if sid == b"FS  ": rate = struct.unpack_from(">I", data, sub + 12)[0]
                if sid == b"CHNL": ch = struct.unpack_from(">H", data, sub + 12)[0]
                if sid == b"CMPR": assert data[sub + 12:sub + 16] == b"DSD ", "input must be uncompressed"
                sub += 12 + ssize + (ssize & 1)
        elif cid == b"DSD ":
            raw = data[body:body + size]
            return rate, [raw[c::ch] for c in range(ch)]
        pos = body + size + (size & 1)
    raise SystemExit("no DSD chunk")

# --- bit writer and arithmetic coder --------------------------------------------

class Bits:
    def __init__(self):
        self.bits = []
    def put(self, value, n):
        for i in range(n - 1, -1, -1):
            self.bits.append((value >> i) & 1)
    def sbits(self, value, n):
        self.put(value & ((1 << n) - 1), n)
    def rice(self, c, k):  # get_sr_golomb_dst: zeros, a one, k bits, then the sign if non-zero
        v = abs(c)
        q, r = v >> k, v & ((1 << k) - 1)
        self.bits.extend([0] * q + [1])
        self.put(r, k)
        if v:
            self.bits.append(1 if c < 0 else 0)
    def tobytes(self):
        b = self.bits + [0] * (-len(self.bits) % 8)
        return bytes(int("".join(map(str, b[i:i + 8])), 2) for i in range(0, len(b), 8))

class ArithEncoder:
    """The inverse of the decoder's ac_get: a 12-bit interval [low, low + a),
    the probability p (1..128, times 1/256) being that of the symbol e = 0,
    which takes the top of the interval. The bits already written take a
    carry when `low` overflows."""
    def __init__(self, bits):
        self.bits = bits      # the output list, appended to
        self.a = 4095
        self.low = 0
    def carry(self):
        i = len(self.bits) - 1
        while self.bits[i] == 1:
            self.bits[i] = 0
            i -= 1
        self.bits[i] = 1
    def encode(self, e, p):
        k = (self.a >> 8) | ((self.a >> 7) & 1)
        q = k * p
        if e:
            self.a -= q             # [low, low + a - q)
        else:
            self.low += self.a - q  # [low + a - q, low + a)
            self.a = q
            if self.low >= 4096:
                self.low -= 4096
                self.carry()
        while self.a < 2048:
            self.a <<= 1
            self.bits.append(self.low >> 11)
            self.low = (self.low << 1) & 0xFFF
    def finish(self):
        # Any value in [low, low + a) decodes the same: the one with the most
        # trailing zeros, so what follows (zero padding) does not matter.
        for t in range(12, -1, -1):
            cand = (self.low + (1 << t) - 1) >> t << t
            if cand < self.low + self.a:
                break
        if cand >= 4096:
            cand -= 4096
            self.carry()
        for i in range(11, -1, -1):
            self.bits.append((cand >> i) & 1)

# --- the frame -------------------------------------------------------------------

FSETS_PRED = [[-8], [-16, 8], [-9, -5, 6]]
PROBS_PRED = [[-8], [-16, 8], [-24, 24, -8]]

def c_div(a, b):  # C integer division (towards zero), b > 0
    q = abs(a) // b
    return q if a >= 0 else -q

def write_table(w, tables, pred, length_bits, coeff_bits, signed, offset, coded):
    """read_table in reverse: each table either uncoded or predicted with
    `coded[i]` (None = uncoded, else the method 0..2 and the Rice k)."""
    for i, coeffs in enumerate(tables):
        w.put(len(coeffs) - 1, length_bits)
        method = coded[i]
        if method is None:
            w.put(0, 1)
            for c in coeffs:
                w.sbits(c, coeff_bits) if signed else w.put(c - offset, coeff_bits)
            continue
        m, k = method
        w.put(1, 1)
        w.put(m, 2)
        for c in coeffs[:m + 1]:
            w.sbits(c, coeff_bits) if signed else w.put(c - offset, coeff_bits)
        w.put(k, 3)
        for j in range(m + 1, len(coeffs)):
            x = sum(pred[m][t] * coeffs[j - t - 1] for t in range(m + 1))
            adj = -c_div(x + 4, 8) if x >= 0 else c_div(-x + 3, 8)
            w.rice(coeffs[j] - adj, k)

def reverse7(c):
    return int(f"{c & 127:07b}"[::-1], 2)

# The decoder's history starts as 0xAA in every byte: before the frame, the
# virtual samples alternate (the newest one, bit 0, is a 0).
PRE = np.array([1 if ((127 - i) & 1) else -1 for i in range(128)], dtype=np.int32)

def predictions(bits, coeffs):
    """predict[t] = sum_m coeffs[m] * x[t - 1 - m] with x = 2 bits - 1, the
    history before the frame being the decoder's initial pattern; wrapped to
    int16 as the decoder keeps it."""
    x = np.concatenate([PRE, 2 * bits.astype(np.int32) - 1])
    y = np.convolve(x, np.array(coeffs, dtype=np.int64))
    p = y[127:127 + len(bits)]
    return ((p + 32768) & 0xFFFF) - 32768

def design_filter(bits, length, scale):
    """Integer predictor of `length` taps fitted to the data (least squares on
    the first samples), scaled to 9-bit coefficients."""
    n = min(len(bits), 60000)
    x = (2 * bits[:n].astype(np.float64) - 1)
    rows = np.stack([x[length - 1 - m:n - 1 - m] for m in range(length)], axis=1)
    target = x[length:n]
    w = np.linalg.lstsq(rows, target, rcond=None)[0]
    coeffs = [int(max(-256, min(255, round(c * scale)))) for c in w]
    if all(c == 0 for c in coeffs):
        coeffs[0] = 1
    return coeffs

def encode_frame(chan_bits, fmap, filters, pmap, probs, half_prob, same_map, coded_f, coded_p):
    """One DST frame; chan_bits[ch] is a numpy array of the frame's bits."""
    channels = len(chan_bits)
    samples = len(chan_bits[0])
    w = Bits()
    w.put(1, 1)          # DST coded
    w.put(1, 1); w.put(1, 1); w.put(1, 1)  # same segmentation, for all channels, one segment
    w.put(1 if same_map else 0, 1)
    def write_map(m, elements):
        if all(v == 0 for v in m) and elements == 1:
            w.put(1, 1)
            return
        w.put(0, 1)
        count = 1
        for ch in range(1, channels):
            w.put(m[ch], count.bit_length())  # av_log2(count) + 1 bits
            if m[ch] == count:
                count += 1
    write_map(fmap, len(filters))
    if not same_map:
        write_map(pmap, len(probs))
    for ch in range(channels):
        w.put(1 if half_prob[ch] else 0, 1)
    write_table(w, filters, FSETS_PRED, 7, 9, True, 0, coded_f)
    write_table(w, probs, PROBS_PRED, 6, 7, False, 1, coded_p)
    w.put(0, 1)          # reserved

    # Per channel: the residual of every sample and its probability.
    residuals, pvalues = [], []
    for ch in range(channels):
        fe, pe = fmap[ch], pmap[ch]
        predict = predictions(chan_bits[ch], filters[fe])
        table = np.array(probs[pe], dtype=np.int32)
        index = np.minimum(np.abs(predict) >> 3, len(table) - 1)
        p = table[index]
        if half_prob[ch]:
            p[:len(filters[fe])] = 128
        residuals.append(chan_bits[ch] ^ (predict < 0).astype(np.int8))
        pvalues.append(p)
    # Interleaved per sample, as the decoder reads them.
    e_all = np.stack(residuals, axis=1).reshape(-1).tolist()
    p_all = np.stack(pvalues, axis=1).reshape(-1).tolist()

    ac = ArithEncoder(w.bits)
    ac.encode(0, reverse7(filters[0][0]) + 1)  # DST_X_bit
    encode = ac.encode
    for e, p in zip(e_all, p_all):
        encode(e, p)
    ac.finish()
    return w.tobytes()

def plain_frame(chan_bytes):
    """A frame that carries uncompressed DSD: a zero byte, then the bytes, one
    per channel in turn."""
    out = bytearray(b"\x00")
    for i in range(len(chan_bytes[0])):
        for c in chan_bytes:
            out.append(c[i])
    return bytes(out)

# --- DSDIFF ----------------------------------------------------------------------

def chunk(cid, body):
    return cid + struct.pack(">Q", len(body)) + body + (b"\x00" if len(body) & 1 else b"")

def pstring(s):
    b = s.encode()
    out = bytes([len(b)]) + b
    return out + (b"\x00" if len(out) & 1 else b"")

def dsdiff(rate, channels, sound_chunks, compressed):
    ids = [b"SLFT", b"SRGT"] if channels == 2 else [b"C%03d" % c for c in range(channels)]
    prop = b"SND " + chunk(b"FS  ", struct.pack(">I", rate)) \
        + chunk(b"CHNL", struct.pack(">H", channels) + b"".join(ids)) \
        + chunk(b"CMPR", (b"DST " + pstring("DST Encoded")) if compressed else (b"DSD " + pstring("not compressed")))
    body = b"DSD " + chunk(b"FVER", struct.pack(">I", 0x01050000)) + chunk(b"PROP", prop) + sound_chunks
    return b"FRM8" + struct.pack(">Q", len(body)) + body

def main():
    src, dst = sys.argv[1], sys.argv[2]
    mode = sys.argv[3] if len(sys.argv) > 3 else ""
    data = open(src, "rb").read()
    rate, chans = read_dsf(data) if data[:4] == b"DSD " else read_dff(data)
    channels = len(chans)
    if mode == "plain":
        raw = bytearray()
        for i in range(len(chans[0])):
            for c in chans:
                raw.append(c[i])
        open(dst, "wb").write(dsdiff(rate, channels, chunk(b"DSD ", bytes(raw)), False))
        return

    frame_bytes = 588 * (rate // 44100) // 8  # per channel: 4704 at DSD64 (1/75 s)
    nframes = (len(chans[0]) + frame_bytes - 1) // frame_bytes
    bits = [np.unpackbits(np.frombuffer(c, dtype=np.uint8)).astype(np.int8) for c in chans]

    # Two predictors (a short and a longer one) and two probability tables,
    # the channels mapped to them in turn; the coding method varies per frame.
    f0 = design_filter(bits[0], 12, 1400)
    f1 = design_filter(bits[-1], 40, 600)
    frames = []
    for n in range(nframes):
        lo, hi = n * frame_bytes * 8, (n + 1) * frame_bytes * 8
        chan_bits = []
        for c in range(channels):
            part = bits[c][lo:hi]
            if len(part) < frame_bytes * 8:  # the last frame: completed with the idle pattern 0x69
                pad = np.unpackbits(np.full((frame_bytes * 8 - len(part) + 7) // 8, 0x69, dtype=np.uint8)).astype(np.int8)
                part = np.concatenate([part, pad[:frame_bytes * 8 - len(part)]])
            chan_bits.append(part)
        if mode == "mixed" and n == 1 and nframes > 2:
            frames.append(plain_frame([bytes(chans[c][lo // 8:hi // 8]).ljust(frame_bytes, b"\x69") for c in range(channels)]))
            continue
        variant = n % 4
        import os
        if os.environ.get("DST_VARIANT"):
            variant = int(os.environ["DST_VARIANT"])
        if variant == 0:    # one filter and one table for every channel, uncoded
            filters, fmap, probs, pmap = [f0], [0] * channels, [[64, 48, 32, 20, 12, 8, 5, 3, 2, 1]], [0] * channels
            same_map, coded_f, coded_p = True, [None], [None]
            half = [False] * channels
        elif variant == 1:  # each channel its own filter, tables coded with every method
            filters = [f0, f1][:channels] + [f0] * max(0, channels - 2)
            fmap = list(range(min(channels, 2))) + [0] * max(0, channels - 2)
            probs = [[70, 52, 38, 26, 17, 11, 7, 4, 3, 2, 2, 1], [60, 40, 24, 14, 8, 4, 2, 1]][:len(filters)]
            pmap = fmap
            same_map, coded_f, coded_p = True, [(0, 4), (2, 3)][:len(filters)], [(1, 2), (0, 2)][:len(filters)]
            half = [True] + [False] * (channels - 1)
        elif variant == 2:  # different maps for filters and probabilities
            filters, fmap = [f1], [0] * channels
            probs = [[64, 48, 32, 20, 12, 8, 5, 3, 2, 1], [90, 60, 30, 10, 3, 1]]
            pmap = [c % 2 for c in range(channels)]
            same_map, coded_f, coded_p = False, [(1, 5)], [None, (2, 2)]
            half = [c % 2 == 1 for c in range(channels)]
        elif variant == 3:  # half-probability start on every channel, a 1-tap filter
            filters, fmap, probs, pmap = [[-120]], [0] * channels, [[128]], [0] * channels
            same_map, coded_f, coded_p = True, [None], [None]
            half = [True] * channels
        elif variant == 4:  # debug: one filter, uncoded, half-probability
            filters, fmap, probs, pmap = [f0], [0] * channels, [[64, 48, 32, 20, 12, 8, 5, 3, 2, 1]], [0] * channels
            same_map, coded_f, coded_p = True, [None], [None]
            half = [True] * channels
        elif variant == 5:  # debug: one filter, coded tables
            filters, fmap, probs, pmap = [f0], [0] * channels, [[64, 48, 32, 20, 12, 8, 5, 3, 2, 1]], [0] * channels
            same_map, coded_f, coded_p = True, [(0, 4)], [(1, 2)]
            half = [False] * channels
        elif variant == 6:  # debug: two filters, uncoded
            filters, fmap = [f0, f1][:channels], list(range(min(channels, 2)))
            probs, pmap = [[64, 48, 32, 20, 12, 8, 5, 3, 2, 1], [60, 40, 24, 14, 8, 4, 2, 1]][:len(filters)], fmap
            same_map, coded_f, coded_p = True, [None] * len(filters), [None] * len(filters)
            half = [False] * channels
        else:               # debug: the long filter alone
            filters, fmap, probs, pmap = [f1], [0] * channels, [[64, 48, 32, 20, 12, 8, 5, 3, 2, 1]], [0] * channels
            same_map, coded_f, coded_p = True, [None], [None]
            half = [False] * channels
        frames.append(encode_frame(chan_bits, fmap, filters, pmap, probs, half, same_map, coded_f, coded_p))

    dst_body = chunk(b"FRTE", struct.pack(">IH", nframes, 75)) + b"".join(chunk(b"DSTF", f) for f in frames)
    # DSTI: the file offset and length of every frame's chunk.
    header = dsdiff(rate, channels, b"", True)
    pos = len(header) + 12 + 12 + 6  # the DST chunk header, the FRTE chunk (its body is 6 bytes)
    index = bytearray()
    for f in frames:
        index += struct.pack(">QI", pos, len(f))
        pos += 12 + len(f) + (len(f) & 1)
    open(dst, "wb").write(dsdiff(rate, channels, chunk(b"DST ", dst_body) + chunk(b"DSTI", bytes(index)), True))

if __name__ == "__main__":
    main()
