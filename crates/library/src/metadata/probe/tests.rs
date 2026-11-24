//! Damaged and crafted files: the probe gives up on them (or reads what it
//! can) without panicking or looping forever. A panic leaves the file out of
//! the library, and a file in the library is read on every start.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Duration;

static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn temp_file(ext: &str, bytes: &[u8]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sonkore-probe-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{}.{ext}", COUNTER.fetch_add(1, Ordering::Relaxed)));
    std::fs::write(&path, bytes).unwrap();
    path
}

/// Every reader over the file, on a thread with a small stack (deep
/// recursion shows), within a few seconds (a loop that never ends shows).
fn survives(what: &str, ext: &str, bytes: &[u8]) {
    let path = temp_file(ext, bytes);
    let (tx, rx) = mpsc::channel();
    let p = path.clone();
    std::thread::Builder::new()
        .stack_size(512 << 10)
        .spawn(move || {
            let _ = super::probe(&p, true);
            let _ = super::dsd::sonkore_dsd_probe(&p);
            let _ = super::mp4::edit_range(&p);
            let mut t = crate::meta::TrackMeta::default();
            let _ = crate::metadata::read_into(p.to_str().unwrap(), &mut t);
            let _ = tx.send(());
        })
        .unwrap();
    let r = rx.recv_timeout(Duration::from_secs(10));
    let _ = std::fs::remove_file(&path);
    match r {
        Ok(()) => {}
        Err(mpsc::RecvTimeoutError::Timeout) => panic!("{what}: the probe never finished"),
        Err(mpsc::RecvTimeoutError::Disconnected) => panic!("{what}: the probe panicked"),
    }
}

fn be32(v: u32) -> [u8; 4] {
    v.to_be_bytes()
}
fn be64(v: u64) -> [u8; 8] {
    v.to_be_bytes()
}

/// An MP4 atom.
fn atom(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = be32(8 + body.len() as u32).to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(body);
    out
}

/// A DSDIFF chunk (64-bit size).
fn chunk(id: &[u8; 4], size: u64, body: &[u8]) -> Vec<u8> {
    let mut out = id.to_vec();
    out.extend_from_slice(&be64(size));
    out.extend_from_slice(body);
    out
}

fn dff(chunks: &[Vec<u8>]) -> Vec<u8> {
    let body: Vec<u8> = chunks.concat();
    let mut out = chunk(b"FRM8", 4 + body.len() as u64, b"DSD ");
    out.extend_from_slice(&body);
    out
}

fn path(p: &Path) -> &str {
    p.to_str().unwrap()
}

#[test]
fn asf_objects_too_small_or_too_big() {
    let header = |objects: &[u8]| {
        let mut h = super::asf::HEADER_GUID.to_vec();
        h.extend_from_slice(&(30 + objects.len() as u64).to_le_bytes());
        h.extend_from_slice(&[0; 6]);
        h.extend_from_slice(objects);
        h
    };
    const STREAM_PROPERTIES: [u8; 16] =
        [0x91, 0x07, 0xDC, 0xB7, 0xB7, 0xA9, 0xCF, 0x11, 0x8E, 0xE6, 0x00, 0xC0, 0x0C, 0x20, 0x53, 0x65];
    // A stream properties object of 30 bytes: no room for its stream type.
    let mut small = STREAM_PROPERTIES.to_vec();
    small.extend_from_slice(&30u64.to_le_bytes());
    small.extend_from_slice(&[0; 6]);
    survives("asf: small stream properties", "wma", &header(&small));
    // An object whose size wraps the position around.
    let mut huge = STREAM_PROPERTIES.to_vec();
    huge.extend_from_slice(&(u64::MAX - 9).to_le_bytes());
    huge.extend_from_slice(&[0; 40]);
    survives("asf: huge object", "wma", &header(&huge));
}

#[test]
fn dff_short_and_truncated() {
    survives("dff: 8 bytes", "dff", b"FRM8\0\0\0\0");
    survives("dff: 15 bytes", "dff", b"FRM8\0\0\0\0\0\0\0\x20DSD");
    // PROP's sub-chunks cut short by the end of the file.
    survives("dff: FS cut", "dff", &dff(&[chunk(b"PROP", 100, b"SND FS")]));
    survives("dff: CMPR cut", "dff", &dff(&[chunk(b"PROP", 100, b"SND CMPR\0\0\0\0\0\0\0\x04DS")]));
    survives("dff: DIIN cut", "dff", &dff(&[chunk(b"DIIN", 100, b"DI")]));
    survives("dff: chunk header cut", "dff", &dff(&[b"PROP\0\0".to_vec()]));
}

#[test]
fn dff_sizes_that_wrap_around() {
    let wrap = u64::MAX - 11; // 12 + size wraps to the same position
    let mut prop = b"SND ".to_vec();
    prop.extend_from_slice(&chunk(b"FS  ", wrap, &[0; 8]));
    survives("dff: PROP sub-chunk", "dff", &dff(&[chunk(b"PROP", prop.len() as u64, &prop)]));
    survives("dff: chunk", "dff", &dff(&[chunk(b"COMT", wrap, &[0; 8])]));
    let diin = chunk(b"DIAR", wrap, &[0; 8]);
    survives("dff: DIIN sub-chunk", "dff", &dff(&[chunk(b"DIIN", diin.len() as u64, &diin)]));
    // The whole form's size.
    let mut form = chunk(b"FRM8", u64::MAX - 3, b"DSD ");
    form.extend_from_slice(&chunk(b"COMT", 4, b"abcd"));
    survives("dff: form", "dff", &form);
}

#[test]
fn mp4_sizes_that_wrap_around() {
    let ftyp = atom(b"ftyp", b"M4A \0\0\0\0");
    // A 64-bit top-level size that brings the position back to 0.
    let mut back = ftyp.clone();
    back.extend_from_slice(&be32(1));
    back.extend_from_slice(b"free");
    back.extend_from_slice(&be64(0u64.wrapping_sub(ftyp.len() as u64)));
    survives("mp4: top-level atom", "m4a", &back);
    // Inside moov, a 64-bit size that ends before the atom starts.
    let mut inner = atom(b"mvhd", &[0; 100]);
    inner.extend_from_slice(&be32(1));
    inner.extend_from_slice(b"udta");
    inner.extend_from_slice(&be64(u64::MAX - 7));
    inner.extend_from_slice(&atom(b"meta", &[0; 16]));
    let mut file = ftyp.clone();
    file.extend_from_slice(&atom(b"moov", &inner));
    survives("mp4: atom inside moov", "m4a", &file);
}

#[test]
fn mp4_sample_tables_that_claim_too_much() {
    let stbl = |table: Vec<u8>| {
        let stbl = atom(b"stbl", &table);
        let minf = atom(b"minf", &stbl);
        let mut hdlr = vec![0; 8];
        hdlr.extend_from_slice(b"soun");
        hdlr.extend_from_slice(&[0; 12]);
        let mut mdia = atom(b"hdlr", &hdlr);
        mdia.extend_from_slice(&minf);
        let trak = atom(b"trak", &atom(b"mdia", &mdia));
        let mut file = atom(b"ftyp", b"M4A \0\0\0\0");
        file.extend_from_slice(&atom(b"moov", &trak));
        file
    };
    // stsz: every sample 4 GiB, 4 billion of them.
    let mut stsz = vec![0; 4];
    stsz.extend_from_slice(&be32(u32::MAX));
    stsz.extend_from_slice(&be32(u32::MAX));
    survives("mp4: stsz", "m4a", &stbl(atom(b"stsz", &stsz)));
    // stts: 4 billion samples of 4 billion units, twice.
    let mut stts = vec![0; 4];
    stts.extend_from_slice(&be32(2));
    for _ in 0..4 {
        stts.extend_from_slice(&be32(u32::MAX));
    }
    survives("mp4: stts", "m4a", &stbl(atom(b"stts", &stts)));
    // stz2: 4 billion 16-bit sizes in a few bytes.
    let mut stz2 = vec![0, 0, 0, 0, 0, 0, 0, 16];
    stz2.extend_from_slice(&be32(u32::MAX));
    stz2.extend_from_slice(&[0; 8]);
    let started = std::time::Instant::now();
    survives("mp4: stz2", "m4a", &stbl(atom(b"stz2", &stz2)));
    assert!(started.elapsed() < Duration::from_secs(2), "stz2 took {:?}", started.elapsed());
}

#[test]
fn w64_chunk_size_that_wraps_around() {
    let mut file = super::pcm::W64_RIFF.to_vec();
    file.extend_from_slice(&200u64.to_le_bytes());
    file.extend_from_slice(b"wave");
    file.extend_from_slice(&[0xF3, 0xAC, 0xD3, 0x11, 0x8C, 0xD1, 0x00, 0xC0, 0x4F, 0x8E, 0xDB, 0x8A]);
    let mut junk = b"junk".to_vec();
    junk.extend_from_slice(&[0xF3, 0xAC, 0xD3, 0x11, 0x8C, 0xD1, 0x00, 0xC0, 0x4F, 0x8E, 0xDB, 0x8A]);
    for size in [u64::MAX, u64::MAX - 7] {
        let mut f = file.clone();
        f.extend_from_slice(&junk);
        f.extend_from_slice(&size.to_le_bytes());
        f.extend_from_slice(&[0; 64]);
        survives("w64: chunk size", "w64", &f);
    }
}

#[test]
fn matroska_simple_tags_nested_deep() {
    let mut f = vec![0x1A, 0x45, 0xDF, 0xA3, 0x80]; // EBML header, empty
    f.extend_from_slice(&[0x18, 0x53, 0x80, 0x67, 0xFF]); // Segment, unknown size
    let mut tag = vec![0x73, 0x73, 0xFF]; // Tag, unknown size
    for _ in 0..20000 {
        tag.extend_from_slice(&[0x67, 0xC8, 0xFF]); // SimpleTag, unknown size
        tag.extend_from_slice(&[0x45, 0xA3, 0x81, b'a']); // TagName "a"
    }
    f.extend_from_slice(&[0x12, 0x54, 0xC3, 0x67, 0x01]); // Tags, 8-byte size
    f.extend_from_slice(&(tag.len() as u64).to_be_bytes()[1..]);
    f.extend_from_slice(&tag);
    survives("mkv: nested SimpleTag", "mka", &f);
    // A SeekHead entry pointing past the end of the address space.
    let mut f = vec![0x1A, 0x45, 0xDF, 0xA3, 0x80, 0x18, 0x53, 0x80, 0x67, 0xFF];
    let seek = [&[0x53, 0xAB, 0x84][..], &[0x12, 0x54, 0xC3, 0x67], &[0x53, 0xAC, 0x88], &[0xFF; 8]].concat();
    let mut entry = vec![0x4D, 0xBB, 0x80 | seek.len() as u8];
    entry.extend_from_slice(&seek);
    f.extend_from_slice(&[0x11, 0x4D, 0x9B, 0x74, 0x80 | entry.len() as u8]);
    f.extend_from_slice(&entry);
    survives("mkv: seek position", "mka", &f);
}

#[test]
fn good_files_still_read() {
    // A DSDIFF with PROP (FS, CHNL, CMPR) and DIIN (artist, title).
    let mut prop = b"SND ".to_vec();
    prop.extend_from_slice(&chunk(b"FS  ", 4, &be32(2822400)));
    prop.extend_from_slice(&chunk(b"CHNL", 10, b"\0\x02SLFTSRGT"));
    prop.extend_from_slice(&chunk(b"CMPR", 20, b"DSD \x0fnot compressed\0"));
    let mut diin = chunk(b"DIAR", 4 + 6, &[&be32(6)[..], b"Artist"].concat());
    diin.extend_from_slice(&chunk(b"DITI", 4 + 5, &[&be32(5)[..], b"Title", b"\0"].concat()));
    let file = dff(&[
        chunk(b"PROP", prop.len() as u64, &prop),
        chunk(b"DIIN", diin.len() as u64, &diin),
        chunk(b"DSD ", 2822400 / 8 * 2, &vec![0x69; 2822400 / 8 * 2]),
    ]);
    let p = temp_file("dff", &file);
    let probe = super::probe(&p, false).expect("the DSDIFF reads");
    assert_eq!(probe.sample_rate, 2822400 / 8);
    assert_eq!(probe.channels, 2);
    assert!((probe.duration - 1.0).abs() < 1e-6, "{}", probe.duration);
    let dsd = super::dsd::sonkore_dsd_probe(&p).expect("Sonkore's DSD probe reads it");
    assert_eq!((dsd.rate, dsd.channels, dsd.bytes_per_channel), (2822400, 2, 2822400 / 8));
    let mut t = crate::meta::TrackMeta::default();
    assert!(crate::metadata::read_into(path(&p), &mut t));
    assert_eq!((&*t.artist, &*t.title), ("Artist", "Title"));
    let _ = std::fs::remove_file(&p);
}
