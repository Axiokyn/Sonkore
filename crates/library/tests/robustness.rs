//! Damaged files against every reader of the library (tags, stream facts,
//! covers, CUE sheets, playlists): fuzz-lite, see tests/fuzz_lite.rs. A
//! panic leaves the file out of the library (metadata::read_any catches it),
//! and a file in the library is read on every start.

#[path = "../../../tests/fuzz_lite.rs"]
mod fuzz_lite;

use std::fs;
use std::path::Path;
use std::time::Duration;

use fuzz_lite::{Case, Plan, Rng, Seed, corpus_seeds, run, run_plan};
use sonkore_library::{Playlists, TrackMeta, covers, cue, metadata};

#[global_allocator]
static GUARD: fuzz_lite::Guard = fuzz_lite::Guard;

const STACK: usize = 512 << 10;
const LIMIT: Duration = Duration::from_secs(5);

// --- Synthetic seeds -----------------------------------------------------------------

fn le16(v: u16) -> [u8; 2] {
    v.to_le_bytes()
}
fn le32(v: u32) -> [u8; 4] {
    v.to_le_bytes()
}
fn be32(v: u32) -> [u8; 4] {
    v.to_be_bytes()
}

/// A 1×1 PNG.
const PNG: [u8; 67] = [
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00,
    0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49,
    0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00,
    0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

fn syncsafe(n: u32) -> [u8; 4] {
    [(n >> 21) as u8 & 0x7F, (n >> 14) as u8 & 0x7F, (n >> 7) as u8 & 0x7F, n as u8 & 0x7F]
}

/// An ID3v2.3 (or .4) tag: title, artist, track, a ReplayGain TXXX and a picture.
fn id3(version: u8) -> Vec<u8> {
    let frame = |id: &[u8; 4], body: &[u8]| {
        let mut f = id.to_vec();
        f.extend_from_slice(&if version == 4 { syncsafe(body.len() as u32) } else { be32(body.len() as u32) });
        f.extend_from_slice(&[0, 0]);
        f.extend_from_slice(body);
        f
    };
    let mut frames = frame(b"TIT2", b"\x03Title");
    frames.extend(frame(b"TPE1", b"\x01\xFF\xFEA\0r\0t\0"));
    frames.extend(frame(b"TRCK", b"\x003/12"));
    frames.extend(frame(b"TXXX", b"\x00REPLAYGAIN_TRACK_GAIN\0-3.50 dB"));
    let mut apic = b"\x00image/png\0\x03cover\0".to_vec();
    apic.extend_from_slice(&PNG);
    frames.extend(frame(b"APIC", &apic));
    let mut t = b"ID3".to_vec();
    t.extend_from_slice(&[version, 0, 0]);
    t.extend_from_slice(&syncsafe(frames.len() as u32));
    t.extend(frames);
    t
}

/// An APEv2 tag with header and footer.
fn ape_tag() -> Vec<u8> {
    let mut items = Vec::new();
    for (k, v) in
        [("Title", "Ape title"), ("Artist", "Ape artist"), ("Track", "4/9"), ("REPLAYGAIN_ALBUM_GAIN", "-6 dB")]
    {
        items.extend_from_slice(&le32(v.len() as u32));
        items.extend_from_slice(&le32(0));
        items.extend_from_slice(k.as_bytes());
        items.push(0);
        items.extend_from_slice(v.as_bytes());
    }
    let block = |header: bool| {
        let mut b = b"APETAGEX".to_vec();
        b.extend_from_slice(&le32(2000));
        b.extend_from_slice(&le32(items.len() as u32 + 32));
        b.extend_from_slice(&le32(4));
        b.extend_from_slice(&le32(if header { 0xA000_0000 } else { 0x8000_0000 }));
        b.extend_from_slice(&[0; 8]);
        b
    };
    let mut t = block(true);
    t.extend_from_slice(&items);
    t.extend(block(false));
    t
}

fn id3v1() -> Vec<u8> {
    let mut t = b"TAG".to_vec();
    for (s, n) in [("v1 title", 30), ("v1 artist", 30), ("v1 album", 30), ("1999", 4), ("comment", 28)] {
        let mut f = s.as_bytes().to_vec();
        f.resize(n, 0);
        t.extend(f);
    }
    t.extend_from_slice(&[0, 7, 13]);
    t
}

fn mp3() -> Vec<u8> {
    let mut f = id3(3);
    for _ in 0..6 {
        // MPEG-1 layer III, 128 kb/s, 44.1 kHz, stereo: 417 bytes a frame.
        let mut frame = vec![0xFF, 0xFB, 0x90, 0x64];
        frame.resize(417, 0);
        f.extend(frame);
    }
    f.extend(ape_tag());
    f.extend(id3v1());
    f
}

fn flac() -> Vec<u8> {
    let block = |kind: u8, last: bool, body: &[u8]| {
        let mut b = vec![kind | if last { 0x80 } else { 0 }];
        b.extend_from_slice(&be32(body.len() as u32)[1..]);
        b.extend_from_slice(body);
        b
    };
    let mut info = Vec::new();
    info.extend_from_slice(&[0x10, 0x00, 0x10, 0x00]); // block sizes 4096
    info.extend_from_slice(&[0; 6]); // frame sizes unknown
    // 44100 Hz (20 bits), 2 channels (3), 16 bits (5), 88200 samples (36).
    let packed: u64 = (44100u64 << 44) | (1 << 41) | (15 << 36) | 88200;
    info.extend_from_slice(&packed.to_be_bytes());
    info.extend_from_slice(&[0; 16]);
    let mut vc = le32(6).to_vec();
    vc.extend_from_slice(b"Sonkore");
    vc.truncate(4 + 6);
    vc.extend_from_slice(&le32(4));
    for c in ["TITLE=Flac title", "ARTIST=Flac artist", "TRACKNUMBER=2", "REPLAYGAIN_TRACK_PEAK=0.9"] {
        vc.extend_from_slice(&le32(c.len() as u32));
        vc.extend_from_slice(c.as_bytes());
    }
    let mut pic = be32(3).to_vec();
    pic.extend_from_slice(&be32(9));
    pic.extend_from_slice(b"image/png");
    pic.extend_from_slice(&be32(5));
    pic.extend_from_slice(b"front");
    for v in [1u32, 1, 32, 0, PNG.len() as u32] {
        pic.extend_from_slice(&be32(v));
    }
    pic.extend_from_slice(&PNG);
    let mut f = b"fLaC".to_vec();
    f.extend(block(0, false, &info));
    f.extend(block(4, false, &vc));
    f.extend(block(6, true, &pic));
    f.extend_from_slice(&[0xFF, 0xF8, 0x69, 0x08, 0x00, 0x00, 0x00]);
    f.resize(f.len() + 200, 0);
    f
}

fn riff_chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut c = id.to_vec();
    c.extend_from_slice(&le32(body.len() as u32));
    c.extend_from_slice(body);
    if body.len() % 2 == 1 {
        c.push(0);
    }
    c
}

fn wav() -> Vec<u8> {
    let mut fmt = le16(1).to_vec();
    fmt.extend_from_slice(&le16(2));
    fmt.extend_from_slice(&le32(44100));
    fmt.extend_from_slice(&le32(44100 * 4));
    fmt.extend_from_slice(&le16(4));
    fmt.extend_from_slice(&le16(16));
    let mut info = b"INFO".to_vec();
    info.extend(riff_chunk(b"INAM", b"Wav title\0"));
    info.extend(riff_chunk(b"IART", b"Wav artist\0"));
    let mut body = b"WAVE".to_vec();
    body.extend(riff_chunk(b"fmt ", &fmt));
    body.extend(riff_chunk(b"data", &[0; 400]));
    body.extend(riff_chunk(b"LIST", &info));
    body.extend(riff_chunk(b"id3 ", &id3(4)));
    riff_chunk(b"RIFF", &body)
}

fn aiff() -> Vec<u8> {
    let chunk = |id: &[u8; 4], body: &[u8]| {
        let mut c = id.to_vec();
        c.extend_from_slice(&be32(body.len() as u32));
        c.extend_from_slice(body);
        if body.len() % 2 == 1 {
            c.push(0);
        }
        c
    };
    let mut comm = 2u16.to_be_bytes().to_vec();
    comm.extend_from_slice(&be32(100));
    comm.extend_from_slice(&16u16.to_be_bytes());
    comm.extend_from_slice(&[0x40, 0x0E, 0xAC, 0x44, 0, 0, 0, 0, 0, 0]); // 44100 as 80-bit
    let mut ssnd = vec![0; 8];
    ssnd.extend_from_slice(&[0; 400]);
    let mut body = b"AIFF".to_vec();
    body.extend(chunk(b"COMM", &comm));
    body.extend(chunk(b"SSND", &ssnd));
    body.extend(chunk(b"ID3 ", &id3(3)));
    chunk(b"FORM", &body)
}

fn dsf() -> Vec<u8> {
    let data = vec![0x69u8; 4096 * 2];
    let tag = id3(3);
    let data_size = 12 + data.len() as u64;
    let total = 28 + 52 + data_size + tag.len() as u64;
    let mut f = b"DSD ".to_vec();
    f.extend_from_slice(&28u64.to_le_bytes());
    f.extend_from_slice(&total.to_le_bytes());
    f.extend_from_slice(&(28 + 52 + data_size).to_le_bytes());
    f.extend_from_slice(b"fmt ");
    f.extend_from_slice(&52u64.to_le_bytes());
    for v in [1u32, 0, 2, 2, 2_822_400, 1] {
        f.extend_from_slice(&le32(v));
    }
    f.extend_from_slice(&(4096u64 * 8).to_le_bytes());
    f.extend_from_slice(&le32(4096));
    f.extend_from_slice(&le32(0));
    f.extend_from_slice(b"data");
    f.extend_from_slice(&data_size.to_le_bytes());
    f.extend(data);
    f.extend(tag);
    f
}

fn dff() -> Vec<u8> {
    let chunk = |id: &[u8; 4], body: &[u8]| {
        let mut c = id.to_vec();
        c.extend_from_slice(&(body.len() as u64).to_be_bytes());
        c.extend_from_slice(body);
        if body.len() % 2 == 1 {
            c.push(0);
        }
        c
    };
    let mut prop = b"SND ".to_vec();
    prop.extend(chunk(b"FS  ", &be32(2_822_400)));
    prop.extend(chunk(b"CHNL", b"\0\x02SLFTSRGT"));
    prop.extend(chunk(b"CMPR", b"DSD \x0fnot compressed\0"));
    let mut diin = chunk(b"DIAR", &[&be32(6)[..], b"Artist"].concat());
    diin.extend(chunk(b"DITI", &[&be32(5)[..], b"Title"].concat()));
    let mut body = b"DSD ".to_vec();
    body.extend(chunk(b"FVER", &[1, 5, 0, 0]));
    body.extend(chunk(b"PROP", &prop));
    body.extend(chunk(b"DIIN", &diin));
    body.extend(chunk(b"DSD ", &[0x69; 4096]));
    body.extend(chunk(b"ID3 ", &id3(4)));
    chunk(b"FRM8", &body)
}

fn synthetic() -> Vec<Seed> {
    vec![
        Seed::new("synthetic.mp3", "mp3", mp3()),
        Seed::new("synthetic.flac", "flac", flac()),
        Seed::new("synthetic.wav", "wav", wav()),
        Seed::new("synthetic.aiff", "aiff", aiff()),
        Seed::new("synthetic.dsf", "dsf", dsf()),
        Seed::new("synthetic.dff", "dff", dff()),
        Seed::new("synthetic-ape.ape", "ape", [b"MAC ".to_vec(), vec![0; 60], ape_tag()].concat()),
        Seed::new("synthetic-tags.wv", "wv", [vec![0; 32], ape_tag(), id3v1()].concat()),
    ]
}

// --- The readers ---------------------------------------------------------------------

/// Everything the library does with an audio file it finds.
fn read_all(path: &Path) {
    let p = path.to_str().unwrap();
    let mut t = TrackMeta::default();
    let _ = metadata::read_into(p, &mut t);
    let _ = metadata::read(p);
    let _ = metadata::embedded_cover(p);
}

fn audio_target(target: &'static str, exts: &[&str], mutations: usize, cover_every: usize) {
    let mut seeds = corpus_seeds(exts);
    seeds.extend(synthetic().into_iter().filter(|s| exts.contains(&s.ext.as_str())));
    let mut plan = Plan::new(seeds, mutations, 120);
    plan.other_exts =
        ["flac", "mp3", "wav", "m4a", "ogg", "opus", "aiff", "w64", "caf", "mka", "wma", "dsf", "dff", "ape", "wv"]
            .iter()
            .map(|s| s.to_string())
            .collect();
    let dir = fuzz_lite::scratch(target);
    let mut n = 0usize;
    run_plan(target, plan, STACK, LIMIT, move |case: &Case| {
        let path = dir.join(format!("case.{}", case.ext));
        fs::write(&path, &case.bytes).unwrap();
        read_all(&path);
        // Decoding the embedded picture (a few cases: the image crate is slow).
        n += 1;
        if cover_every > 0 && n % cover_every == 0 {
            let _ = covers::load_cover(path.to_str().unwrap(), 64, 8);
        }
    });
}

#[test]
fn flac_survives_damage() {
    audio_target("flac", &["flac", "oga"], 2500, 10);
}

#[test]
fn mp3_and_id3_survive_damage() {
    audio_target("mp3", &["mp3", "mp2", "aac", "ac3", "dts"], 2500, 10);
}

#[test]
fn wav_aiff_w64_survive_damage() {
    audio_target("pcm", &["wav", "wave", "aiff", "aif", "aifc", "w64"], 2500, 10);
}

#[test]
fn mp4_survives_damage() {
    audio_target("mp4", &["m4a", "m4b", "mp4"], 2500, 10);
}

#[test]
fn ogg_survives_damage() {
    audio_target("ogg", &["ogg", "opus"], 2000, 10);
}

#[test]
fn matroska_asf_caf_survive_damage() {
    audio_target("containers", &["mka", "webm", "wma", "caf"], 2500, 10);
}

#[test]
fn dsd_survives_damage() {
    audio_target("dsd", &["dsf", "dff"], 2500, 10);
}

#[test]
fn ape_wavpack_tta_tags_survive_damage() {
    audio_target("apev2", &["ape", "wv", "tta", "mpc", "tak"], 2500, 10);
}

#[test]
fn random_bytes_as_every_format() {
    let dir = fuzz_lite::scratch("random");
    let all: Vec<&'static str> = metadata::AUDIO_EXTENSIONS.to_vec();
    let make = move |n: usize| {
        let mut rng = Rng::for_case("random", n);
        let long = rng.chance(20);
        let len = rng.below(if long { 4096 } else { 64 });
        let mut bytes = rng.bytes(len);
        // Sometimes the right magic first, so the reader goes further.
        let magic: &[&[u8]] = &[
            b"fLaC",
            b"ID3\x03\0\0",
            b"RIFF",
            b"FORM",
            b"OggS",
            b"DSD ",
            b"FRM8",
            b"MAC ",
            b"wvpk",
            b"TTA1",
            b"caff",
            b"\x1A\x45\xDF\xA3",
            b"\0\0\0\x20ftypM4A ",
        ];
        if rng.chance(60) {
            let m = rng.pick(magic).to_vec();
            bytes.splice(0..0, m);
        }
        Case { label: format!("random {len} bytes"), ext: rng.pick(&all).to_string(), bytes }
    };
    run("random", fuzz_lite::scaled(4000), STACK, LIMIT, make, move |case: &Case| {
        let path = dir.join(format!("case.{}", case.ext));
        fs::write(&path, &case.bytes).unwrap();
        read_all(&path);
    });
}

#[test]
fn cover_images_survive_damage() {
    let mut seeds = corpus_seeds(&["jpg", "jpeg", "png"]);
    seeds.push(Seed::new("synthetic.png", "png", PNG.to_vec()));
    let plan = Plan::new(seeds, 400, 20);
    let dir = fuzz_lite::scratch("covers");
    run_plan("covers", plan, STACK, LIMIT, move |case: &Case| {
        let path = dir.join(format!("case.{}", case.ext));
        fs::write(&path, &case.bytes).unwrap();
        let _ = covers::decodes(&path);
        let _ = covers::load_cover(path.to_str().unwrap(), 96, 12);
    });
}

// --- CUE sheets ------------------------------------------------------------------------

const SHEET: &str = r#"REM GENRE "Classical"
REM DATE 1985
REM REPLAYGAIN_ALBUM_GAIN -5.10 dB
REM REPLAYGAIN_ALBUM_PEAK 0.95
PERFORMER "Cue Performer"
TITLE "Cue Album"
FILE "image.wav" WAVE
  TRACK 01 AUDIO
    TITLE "Opening"
    PERFORMER "Soloist"
    REM REPLAYGAIN_TRACK_GAIN -3.50 dB
    REM REPLAYGAIN_TRACK_PEAK 0.80
    INDEX 01 00:00:00
  TRACK 02 AUDIO
    TITLE "Middle"
    INDEX 00 00:02:50
    INDEX 01 00:03:00
FILE "other.flac" WAVE
  TRACK 03 AUDIO
    INDEX 01 00:06:37
  TRACK 04 AUDIO
    INDEX 01 99:59:74
"#;

/// Text mutations on top of the byte ones: numbers and times that break
/// arithmetic, keywords duplicated.
fn mutate_text(rng: &mut Rng, text: &[u8]) -> Vec<u8> {
    if rng.chance(50) {
        return fuzz_lite::mutate(rng, text);
    }
    let s = String::from_utf8_lossy(text).into_owned();
    let mut words: Vec<String> = s.split(' ').map(str::to_owned).collect();
    let crazy = [
        "0",
        "-1",
        "99999999999999999999",
        "4294967295",
        "18446744073709551615",
        "-9223372036854775808",
        "1e308",
        "NaN",
        "inf",
        "99:99:99",
        "4294967295:59:74",
        "00:00:75",
        "::",
        "1:2",
        "-1:-1:-1",
        "\"",
        "\"\"",
        "TRACK",
        "INDEX",
        "FILE",
        "REM",
        "\u{FEFF}",
        "\u{0}",
        "999999999:00:00",
    ];
    for _ in 0..1 + rng.below(4) {
        let i = rng.below(words.len());
        match rng.below(3) {
            0 => words[i] = rng.pick(&crazy).to_string(),
            1 => {
                let w = words[i].clone();
                words.insert(i, w);
            }
            _ => {
                let lines: Vec<&str> = s.lines().collect();
                let l = rng.pick(&lines).to_string();
                words.insert(i, format!("\n{l}\n"));
            }
        }
    }
    words.join(" ").into_bytes()
}

#[test]
fn cue_sheets_survive_damage() {
    let dir = fuzz_lite::scratch("cue");
    // Small images the sheets point at, so the track readers go all the way.
    fs::write(dir.join("image.wav"), wav()).unwrap();
    fs::write(dir.join("other.flac"), flac()).unwrap();
    let mut seeds: Vec<Vec<u8>> = vec![SHEET.as_bytes().to_vec()];
    seeds.extend(corpus_seeds(&["cue"]).into_iter().map(|s| s.bytes));
    let make = move |n: usize| {
        let mut rng = Rng::for_case("cue", n);
        let seed = &seeds[n % seeds.len()];
        Case {
            label: format!("sheet {} mutated (case {n})", n % seeds.len()),
            ext: "cue".into(),
            bytes: mutate_text(&mut rng, seed),
        }
    };
    run("cue", fuzz_lite::scaled(3000), STACK, LIMIT, make, move |case: &Case| {
        let sheet = dir.join("case.cue");
        fs::write(&sheet, &case.bytes).unwrap();
        let p = sheet.to_str().unwrap();
        let _ = cue::CueSheet::parse(&sheet);
        let mut images = Vec::new();
        for track in metadata::cue_tracks(p, Some(&mut images)) {
            read_all(Path::new(&track));
        }
        for number in [-1, 0, 1, 2, 3, 4, i32::MAX] {
            let track = cue::track_path(p, number);
            let mut t = TrackMeta::default();
            let _ = metadata::read_into(&track, &mut t);
        }
    });
}

// --- Playlists -------------------------------------------------------------------------

#[test]
fn playlists_and_m3u_survive_damage() {
    let dir = fuzz_lite::scratch("playlists");
    let json = br#"[{"name":"One","tracks":["/music/a.flac","/music/b.mp3"]},{"name":2,"tracks":[]},{"name":true}]"#;
    let m3u = "#EXTM3U\n#EXTINF:123,Artist - Title\n/music/a.flac\nrelative/b.mp3\nfile:///music/c%20d.flac\n\u{feff}x\n../up/../e.wav\n";
    let make = move |n: usize| {
        let mut rng = Rng::for_case("playlists", n);
        if n % 2 == 0 {
            Case { label: format!("playlists.json (case {n})"), ext: "json".into(), bytes: mutate_text(&mut rng, json) }
        } else {
            Case { label: format!("m3u (case {n})"), ext: "m3u8".into(), bytes: mutate_text(&mut rng, m3u.as_bytes()) }
        }
    };
    run("playlists", fuzz_lite::scaled(600), STACK, LIMIT, make, move |case: &Case| {
        let file = dir.join(format!("case.{}", case.ext));
        fs::write(&file, &case.bytes).unwrap();
        if case.ext == "json" {
            let p = Playlists::open(&file, None);
            let _ = p.names();
            let _ = p.to_json();
        } else {
            let _ = fs::remove_file(dir.join("none.json"));
            let mut p = Playlists::open(dir.join("none.json"), None);
            let _ = p.import_m3u(file.to_str().unwrap());
            let _ = p.import_m3u(&format!("file://{}", file.display()));
        }
    });
}

// --- Bugs the fuzzing found (each the smallest file that shows it) -----------------

fn read_file(target: &str, ext: &str, bytes: &[u8]) {
    let path = fuzz_lite::scratch(target).join(format!("bug.{ext}"));
    fs::write(&path, bytes).unwrap();
    read_all(&path);
}

#[test]
fn mkv_child_header_past_its_parent() {
    let mut f = vec![0x1A, 0x45, 0xDF, 0xA3, 0x80]; // EBML header, empty
    f.extend_from_slice(&[0x18, 0x53, 0x80, 0x67, 0xFF]); // Segment, unknown size
    // TrackEntry: TrackType audio, then Audio (2 bytes) holding a
    // SamplingFrequency id and the first byte of a 2-byte size: the size's
    // second byte is the next element's.
    let entry = [0x83, 0x81, 0x02, 0xE1, 0x82, 0xB5, 0x40, 0x04, 0x00, 0x00, 0x00];
    let mut tracks = vec![0xAE, 0x80 | entry.len() as u8];
    tracks.extend_from_slice(&entry);
    f.extend_from_slice(&[0x16, 0x54, 0xAE, 0x6B, 0x80 | tracks.len() as u8]);
    f.extend_from_slice(&tracks);
    read_file("bug-mkv", "mka", &f);
}

/// An Ogg page (the probe does not check the CRC).
fn ogg_page(flags: u8, granule: i64, packet: &[u8]) -> Vec<u8> {
    let mut p = b"OggS\0".to_vec();
    p.push(flags);
    p.extend_from_slice(&granule.to_le_bytes());
    p.extend_from_slice(&le32(1)); // serial
    p.extend_from_slice(&le32(0)); // sequence
    p.extend_from_slice(&le32(0)); // CRC
    let mut lacing = vec![255u8; packet.len() / 255];
    lacing.push((packet.len() % 255) as u8);
    p.push(lacing.len() as u8);
    p.extend_from_slice(&lacing);
    p.extend_from_slice(packet);
    p
}

#[test]
fn ogg_speex_frames_per_packet_overflow() {
    let mut head = b"Speex   ".to_vec();
    head.resize(80, 0);
    head[36..40].copy_from_slice(&le32(44100)); // rate
    head[48..52].copy_from_slice(&le32(2)); // channels
    head[56..60].copy_from_slice(&le32(u32::MAX)); // frame size
    head[64..68].copy_from_slice(&le32(u32::MAX)); // frames per packet
    let mut f = ogg_page(2, 0, &head);
    f.extend(ogg_page(0, 0, b"\x07\0\0\0encoder\0\0\0\0"));
    f.extend(ogg_page(0, 1000, &[0; 40]));
    read_file("bug-ogg", "ogg", &f);
}

#[test]
fn caf_bit_rate_overflow() {
    let mut f = b"caff".to_vec();
    f.extend_from_slice(&1u16.to_be_bytes());
    f.extend_from_slice(&0u16.to_be_bytes());
    f.extend_from_slice(b"desc");
    f.extend_from_slice(&32i64.to_be_bytes());
    f.extend_from_slice(&1e12f64.to_be_bytes()); // the rate: u32::MAX once converted
    f.extend_from_slice(b"lpcm");
    f.extend_from_slice(&be32(0)); // flags
    f.extend_from_slice(&be32(u32::MAX)); // bytes per packet
    f.extend_from_slice(&be32(u32::MAX)); // frames per packet
    f.extend_from_slice(&be32(2));
    f.extend_from_slice(&be32(16));
    f.extend_from_slice(b"data");
    f.extend_from_slice(&(i64::MAX).to_be_bytes());
    f.extend_from_slice(&[0; 64]);
    read_file("bug-caf", "caf", &f);
}
