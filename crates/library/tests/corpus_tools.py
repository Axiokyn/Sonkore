#!/usr/bin/env python3
"""Hand-made tag layouts for the library's test corpus (make_library_corpus.sh):
the cases ffmpeg / flac / lame do not write. Standard library only.

  corpus_tools.py id3 <out.bin> <version 2|3|4> [ext] KEY=VALUE...   an ID3v2 tag
  corpus_tools.py prepend <tag> <file>                              tag + file -> file
  corpus_tools.py id3v1 <file> title artist album year track genre  ID3v1 at the end
  corpus_tools.py mp4free <file.m4a> name=value...                  iTunes ---- atoms
  corpus_tools.py oggpic <file.ogg|.opus> <image>                   METADATA_BLOCK_PICTURE
  corpus_tools.py dsf <out.dsf> <rate> <channels> <seconds> [tag]   DSF (DSD64...) with ID3
  corpus_tools.py dff <out.dff> <rate> <channels> <seconds> [tag]   DSDIFF with an ID3 chunk
"""
import base64, struct, sys, zlib


def syncsafe(n):
    return bytes([(n >> 21) & 0x7F, (n >> 14) & 0x7F, (n >> 7) & 0x7F, n & 0x7F])


def id3(version, pairs, extended=False):
    """KEY=VALUE pairs: frame id = text; TXXX:desc=value; APIC=<image file>;
    encodings: v2 Latin-1 (UTF-16 with BOM when needed), v3 UTF-16, v4 UTF-8."""
    frames = b""
    for kv in pairs:
        key, value = kv.split("=", 1)
        if version == 2:
            if key == "PIC":
                data = open(value, "rb").read()
                body = b"\x00JPG\x03\x00" + data
            else:
                body = b"\x00" + value.encode("latin-1")
            frames += key.encode()[:3] + struct.pack(">I", len(body))[1:] + body
            continue
        if key == "APIC" or key.startswith("APIC:"):
            ptype = int(key.split(":")[1]) if ":" in key else 3
            data = open(value, "rb").read()
            body = b"\x00image/jpeg\x00" + bytes([ptype]) + b"desc\x00" + data
            fid = "APIC"
        elif key.startswith("TXXX:"):
            desc = key[5:]
            if version == 4:
                body = b"\x03" + desc.encode() + b"\x00" + value.encode()
            else:
                body = b"\x01" + "﻿".encode("utf-16-le") + desc.encode("utf-16-le") + b"\x00\x00" \
                    + "﻿".encode("utf-16-le") + value.encode("utf-16-le")
            fid = "TXXX"
        else:
            fid = key
            if version == 4:
                body = b"\x03" + value.replace("|", "\x00").encode()
            elif value.startswith("be:"):
                body = b"\x02" + value[3:].encode("utf-16-be")
            else:
                body = b"\x01" + b"\xff\xfe" + value.encode("utf-16-le")
        size = syncsafe(len(body)) if version == 4 else struct.pack(">I", len(body))
        frames += fid.encode() + size + b"\x00\x00" + body
    flags = 0
    ext = b""
    if extended and version >= 3:
        flags |= 0x40
        ext = syncsafe(6) + b"\x01\x00" if version == 4 else struct.pack(">I", 6) + b"\x00\x00" + b"\x00\x00\x00\x00"
    body = ext + frames + b"\x00" * 64  # padding
    return b"ID3" + bytes([version, 0, flags]) + syncsafe(len(body)) + body


def prepend(tag, path):
    data = open(path, "rb").read()
    open(path, "wb").write(open(tag, "rb").read() + data)


def id3v1(path, title, artist, album, year, track, genre):
    def field(s, n):
        return s.encode("latin-1")[:n].ljust(n, b"\x00")
    tag = b"TAG" + field(title, 30) + field(artist, 30) + field(album, 30) + field(year, 4) \
        + field("", 28) + b"\x00" + bytes([int(track)]) + bytes([int(genre)])
    open(path, "ab").write(tag)


# --- MP4: iTunes free-form atoms (----:com.apple.iTunes:<name>) -------------

def atoms(data, start, end):
    pos = start
    while pos + 8 <= end:
        size, kind = struct.unpack(">I4s", data[pos:pos + 8])
        if size < 8:
            break
        yield pos, size, kind
        pos += size


def mp4free(path, pairs):
    data = bytearray(open(path, "rb").read())
    chain = []  # (offset, size) of moov, udta, meta, ilst

    def find(start, end, names, header):
        for pos, size, kind in atoms(data, start, end):
            if kind == names[0]:
                chain.append(pos)
                if len(names) == 1:
                    return pos
                inner = pos + 8 + (4 if kind == b"meta" else 0)
                return find(inner, pos + size, names[1:], header)
        return None

    ilst = find(0, len(data), [b"moov", b"udta", b"meta", b"ilst"], None)
    if ilst is None:
        raise SystemExit("no ilst in " + path)
    blob = b""
    for kv in pairs:
        name, value = kv.split("=", 1)
        mean = b"com.apple.iTunes"
        m = struct.pack(">I4sI", 12 + len(mean), b"mean", 0) + mean
        n = struct.pack(">I4sI", 12 + len(name), b"name", 0) + name.encode()
        d = struct.pack(">I4sII", 16 + len(value.encode()), b"data", 1, 0) + value.encode()
        body = m + n + d
        blob += struct.pack(">I4s", 8 + len(body), b"----") + body
    end = ilst + struct.unpack(">I", data[ilst:ilst + 4])[0]
    data[end:end] = blob
    for pos in chain:
        size = struct.unpack(">I", data[pos:pos + 4])[0]
        data[pos:pos + 4] = struct.pack(">I", size + len(blob))
    open(path, "wb").write(data)


# --- Ogg: add a METADATA_BLOCK_PICTURE comment --------------------------------

CRC_TABLE = []
for i in range(256):
    r = i << 24
    for _ in range(8):
        r = ((r << 1) ^ 0x04C11DB7) if r & 0x80000000 else (r << 1)
    CRC_TABLE.append(r & 0xFFFFFFFF)


def ogg_crc(page):
    crc = 0
    for b in page:
        crc = ((crc << 8) ^ CRC_TABLE[((crc >> 24) & 0xFF) ^ b]) & 0xFFFFFFFF
    return crc


def ogg_pages(data):
    pos = 0
    while pos + 27 <= len(data) and data[pos:pos + 4] == b"OggS":
        nseg = data[pos + 26]
        lacing = data[pos + 27:pos + 27 + nseg]
        body_len = sum(lacing)
        yield data[pos:pos + 27 + nseg + body_len]
        pos += 27 + nseg + body_len


def oggpic(path, image):
    data = open(path, "rb").read()
    pages = list(ogg_pages(data))
    # The comment header is the first packet of page 2 (Vorbis puts the setup
    # header after it in the same page, maybe continued on the next one).
    page = pages[1]
    nseg = page[26]
    lacing = list(page[27:27 + nseg])
    body = page[27 + nseg:]
    packets, pos, run, segs = [], 0, 0, []
    for l in lacing:
        run += l
        segs.append(l)
        if l < 255:
            packets.append((body[pos:pos + run], True, segs))
            pos += run
            run, segs = 0, []
    if segs:
        packets.append((body[pos:pos + run], False, segs))
    comment_packet = packets[0][0]
    opus = comment_packet.startswith(b"OpusTags")
    p = 8 if opus else 7
    vlen = struct.unpack("<I", comment_packet[p:p + 4])[0]
    p += 4 + vlen
    count_at = p
    count = struct.unpack("<I", comment_packet[p:p + 4])[0]
    p += 4
    for _ in range(count):
        l = struct.unpack("<I", comment_packet[p:p + 4])[0]
        p += 4 + l
    img = open(image, "rb").read()
    block = struct.pack(">I", 3) + struct.pack(">I", 10) + b"image/jpeg" + struct.pack(">I", 0) \
        + struct.pack(">IIII", 0, 0, 0, 0) + struct.pack(">I", len(img)) + img
    comment = b"METADATA_BLOCK_PICTURE=" + base64.b64encode(block)
    new_comment = comment_packet[:count_at] + struct.pack("<I", count + 1) + comment_packet[count_at + 4:p] \
        + struct.pack("<I", len(comment)) + comment + comment_packet[p:]
    packets[0] = (new_comment, True, None)
    new_lacing, new_body = [], b""
    for pk, complete, segs in packets:
        new_body += pk
        if complete:
            new_lacing += [255] * (len(pk) // 255) + [len(pk) % 255]
        else:
            new_lacing += segs
    if len(new_lacing) > 255:
        raise SystemExit("picture too large for one page")
    header = bytearray(page[:26]) + bytes([len(new_lacing)]) + bytes(new_lacing)
    new_page = bytearray(header + new_body)
    new_page[22:26] = b"\x00\x00\x00\x00"
    new_page[22:26] = struct.pack("<I", ogg_crc(new_page))
    pages[1] = bytes(new_page)
    open(path, "wb").write(b"".join(pages))


# --- DSD ------------------------------------------------------------------------

def dsd_bytes(channels, seconds, rate):
    per_channel = rate // 8 * seconds
    return per_channel, bytes([0x69]) * (per_channel * channels)  # 0x69: DSD silence pattern


def dsf(path, rate, channels, seconds, tag=None):
    per_channel, _ = dsd_bytes(channels, seconds, rate)
    block = 4096
    blocks = -(-per_channel // block)
    data = bytes([0x69]) * (blocks * block * channels)
    tag_bytes = open(tag, "rb").read() if tag else b""
    fmt = struct.pack("<4sQIIIIIIQII", b"fmt ", 52, 1, 0, 2 if channels == 2 else 1, channels, rate, 1,
                      per_channel * 8, block, 0)
    data_chunk = struct.pack("<4sQ", b"data", 12 + len(data)) + data
    total = 28 + len(fmt) + len(data_chunk) + len(tag_bytes)
    meta = 28 + len(fmt) + len(data_chunk) if tag else 0
    head = struct.pack("<4sQQQ", b"DSD ", 28, total, meta)
    open(path, "wb").write(head + fmt + data_chunk + tag_bytes)


def dff(path, rate, channels, seconds, tag=None, title=None, artist=None):
    per_channel, data = dsd_bytes(channels, seconds, rate)

    def chunk(cid, body):
        pad = b"\x00" if len(body) & 1 else b""
        return cid + struct.pack(">Q", len(body)) + body + pad

    names = [b"SLFT", b"SRGT", b"C   ", b"LFE ", b"LS  ", b"RS  "][:channels]
    prop = b"SND " + chunk(b"FS  ", struct.pack(">I", rate)) \
        + chunk(b"CHNL", struct.pack(">H", channels) + b"".join(names)) \
        + chunk(b"CMPR", b"DSD " + b"\x0enot compressed" + b"\x00")
    diin = b""
    if title or artist:
        def text(cid, t):
            raw = t.encode()
            return chunk(cid, struct.pack(">I", len(raw)) + raw)
        diin = chunk(b"DIIN", (text(b"DIAR", artist) if artist else b"") + (text(b"DITI", title) if title else b""))
    body = b"DSD " + chunk(b"FVER", struct.pack(">I", 0x01050000)) + chunk(b"PROP", prop) + diin + chunk(b"DSD ", data)
    if tag:
        body += chunk(b"ID3 ", open(tag, "rb").read())
    open(path, "wb").write(b"FRM8" + struct.pack(">Q", len(body)) + body)


def main():
    cmd, args = sys.argv[1], sys.argv[2:]
    if cmd == "id3":
        out, version = args[0], int(args[1])
        ext = len(args) > 2 and args[2] == "ext"
        pairs = args[3:] if ext else args[2:]
        open(out, "wb").write(id3(version, pairs, ext))
    elif cmd == "prepend":
        prepend(args[0], args[1])
    elif cmd == "id3v1":
        id3v1(*args)
    elif cmd == "mp4free":
        mp4free(args[0], args[1:])
    elif cmd == "oggpic":
        oggpic(args[0], args[1])
    elif cmd == "dsf":
        dsf(args[0], int(args[1]), int(args[2]), int(args[3]), args[4] if len(args) > 4 else None)
    elif cmd == "dff":
        dff(args[0], int(args[1]), int(args[2]), int(args[3]), args[4] if len(args) > 4 and args[4] else None,
            args[5] if len(args) > 5 else None, args[6] if len(args) > 6 else None)
    else:
        raise SystemExit(__doc__)


if __name__ == "__main__":
    main()
