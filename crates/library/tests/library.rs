//! The library against files it makes itself (WAV and CUE written here, no
//! tools needed), JSON files earlier Sonkore versions wrote (tests/data),
//! and, when SONKORE_LIBRARY_CORPUS points at the corpus made by
//! tests/make_library_corpus.sh, real tagged files of every format.
//! No network: online covers are tested with a fake HTTP client.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sonkore_library::online_art::{ArtEvent, Http, OnlineArt, Response};
use sonkore_library::views::sort_paths;
use sonkore_library::{
    AlbumList, Event, FolderTree, Library, LibraryFiles, Playlists, TrackFilter, TrackList, UNKNOWN, metadata,
};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// A fresh folder under the target dir.
fn scratch(name: &str) -> PathBuf {
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let dir = base.join(format!("{name}-{}-{}", std::process::id(), COUNTER.fetch_add(1, Ordering::Relaxed)));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A 16-bit stereo WAV: a sine of `amp` (full scale 1), with LIST/INFO
/// tags (INAM title, IART artist, IPRD album, ITRK track).
fn wav(path: &Path, seconds: f64, amp: f64, tags: &[(&str, &str)]) {
    let rate = 44100u32;
    let frames = (seconds * rate as f64) as u32;
    let mut data = Vec::with_capacity(frames as usize * 4);
    for i in 0..frames {
        let v = (amp * (2.0 * std::f64::consts::PI * 440.0 * i as f64 / rate as f64).sin() * 32767.0).round() as i16;
        data.extend_from_slice(&v.to_le_bytes());
        data.extend_from_slice(&v.to_le_bytes());
    }
    let mut info = b"INFO".to_vec();
    for (id, value) in tags {
        let mut v = value.as_bytes().to_vec();
        v.push(0);
        info.extend_from_slice(id.as_bytes());
        info.extend_from_slice(&(v.len() as u32).to_le_bytes());
        if v.len() % 2 == 1 {
            v.push(0);
        }
        info.extend_from_slice(&v);
    }
    let mut out = b"RIFF".to_vec();
    let mut body = b"WAVE".to_vec();
    body.extend_from_slice(b"fmt ");
    body.extend_from_slice(&16u32.to_le_bytes());
    body.extend_from_slice(&1u16.to_le_bytes());
    body.extend_from_slice(&2u16.to_le_bytes());
    body.extend_from_slice(&rate.to_le_bytes());
    body.extend_from_slice(&(rate * 4).to_le_bytes());
    body.extend_from_slice(&4u16.to_le_bytes());
    body.extend_from_slice(&16u16.to_le_bytes());
    if !tags.is_empty() {
        body.extend_from_slice(b"LIST");
        body.extend_from_slice(&(info.len() as u32).to_le_bytes());
        body.extend_from_slice(&info);
    }
    body.extend_from_slice(b"data");
    body.extend_from_slice(&(data.len() as u32).to_le_bytes());
    body.extend_from_slice(&data);
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).unwrap();
    }
    fs::write(path, out).unwrap();
}

/// A library in `dir`, settings there too (no startup scan).
fn library(dir: &Path) -> Library {
    let settings = dir.join("sonkore.conf");
    if !settings.exists() {
        fs::write(&settings, "[library]\nrescanOnStartup=false\n").unwrap();
    }
    Library::open(LibraryFiles::in_dir(&dir.join("data"), &settings))
}

/// A path as the library stores it (with '/' on Windows too).
fn s(p: &Path) -> String {
    sonkore_core::paths::stored_path(&p.to_string_lossy())
}

fn wait(lib: &mut Library) -> Vec<Event> {
    lib.wait_idle(Duration::from_secs(60))
}

/// A small music folder: an album of three, a loose folder, a CD image.
fn music(dir: &Path) -> PathBuf {
    let m = dir.join("Music");
    for n in 1..=3 {
        wav(
            &m.join(format!("Band/Record/{n:02}.wav")),
            1.0,
            0.25,
            &[("INAM", &format!("Song {n}")), ("IART", "Band"), ("IPRD", "Record"), ("ITRK", &n.to_string())],
        );
    }
    wav(&m.join("Loose/track 10.wav"), 1.0, 0.25, &[]);
    wav(&m.join("Loose/track 2.wav"), 1.0, 0.25, &[("IART", "Other")]);
    wav(&m.join("Image/disc.wav"), 6.0, 0.25, &[]);
    fs::write(
        m.join("Image/disc.cue"),
        "PERFORMER \"Cue Band\"\nTITLE \"Cue Album\"\nFILE \"disc.wav\" WAVE\n  TRACK 01 AUDIO\n    TITLE \"One\"\n    INDEX 01 00:00:00\n  TRACK 02 AUDIO\n    INDEX 01 00:02:00\n",
    )
    .unwrap();
    m
}

#[test]
fn scan_reads_folders_cue_sheets_and_keeps_order() {
    let dir = scratch("scan");
    let m = music(&dir);
    let mut lib = library(&dir);
    let ev = lib.add_folder(&s(&m));
    assert!(ev.contains(&Event::FoldersChanged) && ev.contains(&Event::ScanningChanged));
    let ev = wait(&mut lib);
    assert!(ev.contains(&Event::TracksReset));
    let paths: Vec<String> = lib.tracks().iter().map(|t| t.path.strip_prefix(&s(&m)).unwrap().to_string()).collect();
    // Library order: artist (case aside), year, album, disc, track, path.
    assert_eq!(
        paths,
        vec![
            "/Loose/track 10.wav",
            "/Band/Record/01.wav",
            "/Band/Record/02.wav",
            "/Band/Record/03.wav",
            "/Image/disc.cue#1",
            "/Image/disc.cue#2",
            "/Loose/track 2.wav"
        ]
    );
    let cue2 = lib.find(&format!("{}/Image/disc.cue#2", s(&m))).unwrap();
    assert_eq!(cue2.title, "Track 2");
    assert_eq!(&*cue2.album, "Cue Album");
    assert!((cue2.duration - 4.0).abs() < 1e-9);
    // The image file is listed through its sheet only.
    assert!(lib.find(&format!("{}/Image/disc.wav", s(&m))).is_none());
    // library.json is there, and loads back to the same.
    let json = fs::read_to_string(dir.join("data/library.json")).unwrap();
    assert_eq!(json, lib.cache_json());
    let again = library(&dir);
    assert_eq!(again.cache_json(), json);
    // Folders in natural order, named from their library folder.
    let folders: Vec<String> = lib.track_folders().iter().map(|f| format!("{}:{}", f.name, f.count)).collect();
    assert_eq!(folders, vec!["Band/Record:3", "Image:2", "Loose:2"]);
    // Recursive folder listing, in library order.
    assert_eq!(lib.folder_paths(&format!("{}/Band", s(&m))).len(), 3);
}

#[test]
fn incremental_scans_keep_unchanged_entries() {
    let dir = scratch("incremental");
    let m = music(&dir);
    let mut lib = library(&dir);
    lib.add_folder(&s(&m));
    wait(&mut lib);
    let first = lib.find(&format!("{}/Band/Record/01.wav", s(&m))).unwrap().clone();
    std::thread::sleep(Duration::from_millis(20));
    // A file changed: read again (its tags), its date added kept.
    wav(
        &m.join("Band/Record/01.wav"),
        1.0,
        0.25,
        &[("INAM", "Renamed"), ("IART", "Band"), ("IPRD", "Record"), ("ITRK", "1")],
    );
    lib.rescan();
    wait(&mut lib);
    let again = lib.find(&format!("{}/Band/Record/01.wav", s(&m))).unwrap();
    assert_eq!(again.title, "Renamed");
    assert_eq!(again.added, first.added);
    // An unchanged entry is kept as it was, even if its tags were edited in
    // library.json (no re-read).
    let mut json = lib.cache_json();
    json = json.replace("\"t\":\"Song 2\"", "\"t\":\"Cached title\"");
    fs::write(dir.join("data/library.json"), json).unwrap();
    let mut lib = library(&dir);
    lib.rescan();
    wait(&mut lib);
    assert_eq!(lib.find(&format!("{}/Band/Record/02.wav", s(&m))).unwrap().title, "Cached title");
}

#[test]
fn hidden_skipped_short_and_extra_files() {
    let dir = scratch("editing");
    let m = music(&dir);
    wav(&dir.join("Elsewhere/single.wav"), 1.0, 0.25, &[]);
    wav(&m.join("Short/jingle.wav"), 0.5, 0.25, &[]);
    let mut lib = library(&dir);
    lib.add_folder(&s(&m));
    wait(&mut lib);
    assert_eq!(lib.count(), 8);
    // Removed by hand: hidden while its folder is watched.
    let p = format!("{}/Loose/track 2.wav", s(&m));
    lib.remove_tracks(std::slice::from_ref(&p));
    assert_eq!(lib.count(), 7);
    assert_eq!(lib.hidden_count(), 1);
    lib.rescan();
    wait(&mut lib);
    assert!(!lib.contains(&p));
    // Adding the file again shows it.
    lib.add_urls(&[format!("file://{}", p.replace(' ', "%20"))]);
    wait(&mut lib);
    assert!(lib.contains(&p));
    assert_eq!(lib.hidden_count(), 0);
    // A file outside the folders is added on its own.
    let single = s(&dir.join("Elsewhere/single.wav"));
    lib.add_urls(std::slice::from_ref(&single));
    wait(&mut lib);
    assert!(lib.contains(&single));
    assert_eq!(lib.extra_files(), std::slice::from_ref(&single));
    // Short files are left out.
    lib.set_min_duration(1);
    wait(&mut lib);
    assert!(!lib.contains(&format!("{}/Short/jingle.wav", s(&m))));
    // A skipped folder leaves the library; adding it again brings it back.
    let loose = format!("{}/Loose", s(&m));
    lib.exclude_folder(&format!("{loose}/"));
    wait(&mut lib);
    assert_eq!(lib.excluded_folders(), std::slice::from_ref(&loose));
    assert!(lib.folder_paths(&loose).is_empty());
    lib.add_folder(&loose);
    wait(&mut lib);
    assert!(lib.excluded_folders().is_empty());
    assert_eq!(lib.folder_paths(&loose).len(), 2);
    // The settings went to the INI file in the stored format.
    let ini = fs::read_to_string(dir.join("sonkore.conf")).unwrap();
    assert!(ini.contains("minDuration=1"), "{ini}");
    // A parent folder takes its children's place.
    lib.add_folder(&s(&dir));
    wait(&mut lib);
    assert_eq!(lib.folders(), &[s(&dir)]);
    // Removing the folder empties the library but the file added on its own.
    lib.remove_folder(&s(&dir));
    wait(&mut lib);
    assert_eq!(lib.count(), 1);
}

#[test]
fn missing_folder_keeps_what_was_known() {
    let dir = scratch("missing");
    let m = music(&dir);
    // Windows refuses to rename a folder with anything open inside (a watch,
    // a reader still finishing): no watch here, and a few tries.
    fs::write(dir.join("sonkore.conf"), "[library]\nrescanOnStartup=false\nwatchFolders=false\n").unwrap();
    let mut lib = library(&dir);
    lib.add_folder(&s(&m));
    wait(&mut lib);
    let n = lib.count();
    let moved = dir.join("Unplugged");
    let mut tries = 0;
    while let Err(e) = fs::rename(&m, &moved) {
        tries += 1;
        assert!(tries < 50, "{e}");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    lib.rescan();
    wait(&mut lib);
    assert_eq!(lib.count(), n, "an unplugged drive does not empty the library");
}

#[test]
#[cfg(unix)] // symlinks on Windows need an elevated or developer-mode user
fn links_are_followed_but_not_back_up() {
    let dir = scratch("links");
    let m = music(&dir);
    wav(&dir.join("Outside/linked.wav"), 1.0, 0.25, &[]);
    std::os::unix::fs::symlink(dir.join("Outside"), m.join("Linked")).unwrap();
    std::os::unix::fs::symlink(&m, m.join("Loose/loop")).unwrap();
    std::os::unix::fs::symlink(m.join("Band"), m.join("Band again")).unwrap();
    let mut lib = library(&dir);
    lib.add_folder(&s(&m));
    wait(&mut lib);
    assert!(lib.contains(&format!("{}/Linked/linked.wav", s(&m))));
    // Music reachable both ways is listed under its real path, once.
    assert!(lib.contains(&format!("{}/Band/Record/01.wav", s(&m))));
    assert!(!lib.tracks().iter().any(|t| t.path.contains("Band again") || t.path.contains("/loop/")));
    assert_eq!(lib.count(), 8);
}

#[test]
fn watching_rescans_after_a_change() {
    let dir = scratch("watch");
    let m = music(&dir);
    let mut lib = library(&dir);
    let fired = Arc::new(AtomicUsize::new(0));
    let f = fired.clone();
    lib.set_notifier(move || {
        f.fetch_add(1, Ordering::Relaxed);
    });
    lib.add_folder(&s(&m));
    wait(&mut lib);
    assert!(fired.load(Ordering::Relaxed) > 0);
    assert!(lib.watched_dirs().len() >= 4);
    let n = lib.count();
    wav(&m.join("Loose/new.wav"), 1.0, 0.25, &[]);
    // 2 s of quiet, then a rescan.
    let mut found = false;
    for _ in 0..100 {
        std::thread::sleep(Duration::from_millis(100));
        let ev = lib.process();
        if ev.contains(&Event::ScanningChanged) {
            wait(&mut lib);
            found = true;
            break;
        }
    }
    assert!(found, "no rescan after a new file");
    assert_eq!(lib.count(), n + 1);
}

#[test]
fn favorites_file_as_stored() {
    let dir = scratch("favorites");
    let data = dir.join("data");
    fs::create_dir_all(&data).unwrap();
    let stored = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/favorites.json")).unwrap();
    fs::write(data.join("favorites.json"), &stored).unwrap();
    let mut lib = library(&dir);
    assert_eq!(lib.favorite_count(), 7);
    assert!(lib.is_favorite("/m/disc.cue#3"));
    assert_eq!(lib.favorites_json(), stored);
    let ev = lib.toggle_favorite("/m/a.flac");
    assert!(ev.contains(&Event::FavoritesChanged));
    lib.set_favorite("/m/a.flac", true);
    assert_eq!(fs::read_to_string(data.join("favorites.json")).unwrap(), stored);
}

#[test]
fn several_favorites_off_at_once() {
    let dir = scratch("favorites-batch");
    let data = dir.join("data");
    fs::create_dir_all(&data).unwrap();
    let stored = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/favorites.json")).unwrap();
    fs::write(data.join("favorites.json"), &stored).unwrap();
    let mut lib = library(&dir);
    let before = lib.favorite_count();
    let off = vec!["/m/disc.cue#3".to_string(), "/m/not-a-favorite.flac".to_string()];
    let ev = lib.set_favorites(&off, false);
    assert_eq!(ev.iter().filter(|e| **e == Event::FavoritesChanged).count(), 1);
    assert_eq!(lib.favorite_count(), before - 1);
    assert!(!fs::read_to_string(data.join("favorites.json")).unwrap().contains("disc.cue#3"));
    assert!(lib.set_favorites(&off, false).is_empty(), "nothing changed: nothing written");
}

#[test]
fn cpp_library_json_loads_and_saves_byte_for_byte() {
    let dir = scratch("cppjson");
    let data = dir.join("data");
    fs::create_dir_all(&data).unwrap();
    let stored =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/library-stored.json")).unwrap();
    fs::write(data.join("library.json"), &stored).unwrap();
    let lib = library(&dir);
    assert_eq!(lib.count(), 66);
    assert_eq!(lib.folders(), &["/media/corpus/Music".to_string()]);
    assert_eq!(lib.cache_json(), stored, "a stored library.json comes back unchanged");
    let t = lib.find("/media/corpus/Music/Formats/opus.opus").unwrap();
    assert_eq!(t.rg_track_gain, 0.0);
    assert_eq!(t.rg_album_gain, 6.0);
    assert!(lib.find("/media/corpus/Music/Cue/disc.cue#2").unwrap().has_embedded_cover);
}

#[test]
fn old_cache_versions_are_read_again() {
    let dir = scratch("versions");
    let data = dir.join("data");
    fs::create_dir_all(&data).unwrap();
    let entry = |p: &str, ch: i32, bits: i32| {
        format!(
            "{{\"a\":\"Unknown artist\",\"aa\":\"\",\"al\":\"A\",\"b\":{bits},\"br\":0,\"c\":\"FLAC\",\"ch\":{ch},\"cv\":false,\"d\":3,\"dn\":0,\"g\":\"\",\"m\":5,\"p\":\"{p}\",\"rag\":-1,\"rap\":0.5,\"rga\":true,\"rtg\":-1,\"rtp\":0.5,\"sr\":44100,\"sz\":1,\"t\":\"T\",\"tn\":0,\"y\":0}}"
        )
    };
    fs::write(
        data.join("library.json"),
        format!(
            "{{\"version\":3,\"folders\":[],\"files\":[],\"excluded\":[],\"tracks\":[{},{}]}}",
            entry("/a", 1, 16),
            entry("/b", 2, 16)
        ),
    )
    .unwrap();
    let lib = library(&dir);
    let a = lib.find("/a").unwrap();
    assert!(a.rg_track_gain.is_nan() && !a.rg_analyzed, "mono measured by version 3: measured again");
    assert_eq!(&*a.artist, "", "the English fallback of early builds is dropped");
    assert_eq!(a.added, 5, "no date added: the modification date");
    assert_eq!(lib.find("/b").unwrap().rg_track_gain, -1.0);
}

#[test]
fn playlists_file_and_m3u() {
    let dir = scratch("playlists");
    let stored = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/playlists.json")).unwrap();
    let file = dir.join("playlists.json");
    fs::write(&file, &stored).unwrap();
    let mut p = Playlists::open(&file, None);
    assert_eq!(p.names(), vec!["Mi lista ñ", "Empty"]);
    assert_eq!(p.to_json(), stored);
    let (name, _) = p.create("Empty");
    assert_eq!(name, "Empty 2");
    let (name, _) = p.create("  ");
    assert_eq!(name, "New playlist");
    let (renamed, _) = p.rename("New playlist", " Road ");
    assert_eq!(renamed, "Road");
    p.add_tracks("Road", &["/x/1.flac".into(), "/x/2.flac".into(), "/x/3.flac".into()]);
    p.move_track("Road", 0, 2);
    assert_eq!(p.paths("Road"), vec!["/x/2.flac", "/x/3.flac", "/x/1.flac"]);
    p.insert_tracks("Road", 99, &["/x/4.flac".into()]);
    p.remove_rows("Road", &[0, 0, 3]);
    assert_eq!(p.paths("Road"), vec!["/x/3.flac", "/x/1.flac"]);
    p.remove("Empty 2");
    let reopened = Playlists::open(&file, None);
    assert_eq!(reopened.names(), vec!["Mi lista ñ", "Empty", "Road"]);
    let empty = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/playlists-empty.json");
    let none = Playlists::open(dir.join("none.json"), None);
    assert_eq!(none.to_json(), fs::read_to_string(empty).unwrap());

    // M3U: relative and absolute entries (C:\… on Windows), file:// URLs
    // (file:///C:/… on Windows), comments.
    let m = music(&dir);
    let mut lib = library(&dir);
    lib.add_folder(&s(&m));
    wait(&mut lib);
    let m3u = m.join("Band/mix.m3u8");
    let url = if cfg!(windows) { format!("file:///{}", s(&m)) } else { format!("file://{}", s(&m)) };
    let native = m.join("Band").join("Record").join("02.wav").to_string_lossy().into_owned();
    fs::write(&m3u, format!("\u{feff}#EXTM3U\r\n#EXTINF:1,x\r\nRecord/01.wav\r\n{url}/Loose/track%202.wav\r\n\r\n../Band/./Record/03.wav\n{native}\n")).unwrap();
    let (name, _) = p.import_m3u(&s(&m3u)).unwrap();
    assert_eq!(name, "mix");
    assert_eq!(
        p.paths("mix"),
        vec![
            format!("{}/Band/Record/01.wav", s(&m)),
            format!("{}/Loose/track 2.wav", s(&m)),
            format!("{}/Band/Record/03.wav", s(&m)),
            format!("{}/Band/Record/02.wav", s(&m))
        ]
    );
    let out = dir.join("out.m3u");
    assert!(p.export_m3u("mix", &s(&out), &lib));
    let text = fs::read_to_string(out).unwrap();
    assert!(text.starts_with(&format!("#EXTM3U\n#EXTINF:1,Band - Song 1\n{}/Band/Record/01.wav\n", s(&m))), "{text}");
    assert!((p.duration("mix", &lib) - 4.0).abs() < 1e-9);
}

#[test]
fn views_filter_sort_group() {
    let dir = scratch("views");
    let m = music(&dir);
    let mut lib = library(&dir);
    lib.add_folder(&s(&m));
    wait(&mut lib);
    let mut f = TrackFilter::new();
    f.search = "SONG band".into();
    f.refresh(&lib);
    assert_eq!(f.count(), 3);
    f.search.clear();
    f.sort_field = "title".into();
    f.refresh(&lib);
    let titles: Vec<String> = f.rows().iter().map(|&r| lib.tracks()[r].title.clone()).collect();
    // Natural order, case aside; equal titles by path ("Image/" before "Loose/").
    assert_eq!(titles, vec!["One", "Song 1", "Song 2", "Song 3", "Track 2", "track 2", "track 10"]);
    f.sort_field = "artist".into();
    f.descending = true;
    f.refresh(&lib);
    // Blanks (no artist) last whichever way.
    let last = f.track(&lib, f.count() - 1).unwrap();
    assert_eq!(last.display_artist(), "");
    assert_eq!(f.track(&lib, 0).unwrap().display_artist(), "Other");
    f.sort_field.clear();
    f.artist = UNKNOWN.into();
    f.refresh(&lib);
    assert_eq!(f.count(), 1);
    f.artist.clear();
    f.folder = format!("{}/Loose", s(&m));
    f.refresh(&lib);
    assert_eq!(f.count(), 2);
    f.folder.clear();
    f.favorites_only = true;
    lib.set_favorite(&format!("{}/Band/Record/02.wav", s(&m)), true);
    assert!(f.stale(&lib));
    f.refresh(&lib);
    assert_eq!(f.paths(&lib), vec![format!("{}/Band/Record/02.wav", s(&m))]);

    let mut albums = AlbumList::new();
    albums.rebuild(&lib);
    let names: Vec<String> = albums.albums().map(|a| format!("{}/{}:{}", a.artist, a.album, a.tracks)).collect();
    assert_eq!(names, vec!["/Loose:1", "Band/Record:3", "Cue Band/Cue Album:2", "Other/Loose:1"]);
    albums.set_search("cue");
    assert_eq!(albums.count(), 1);

    let browse = lib.browse("artist", "", "");
    let artists: Vec<(String, usize)> = browse.iter().map(|b| (b.name.clone(), b.count)).collect();
    assert_eq!(artists, vec![("Band".into(), 3), ("Cue Band".into(), 2), ("Other".into(), 1), ("".into(), 1)]);
    assert_eq!(browse.last().unwrap().key, UNKNOWN);

    let mut tree = FolderTree::new();
    assert!(tree.rebuild(&lib));
    let root = &tree.roots()[0];
    assert_eq!(root.count, 7);
    let kids: Vec<String> = tree.children_of(&s(&m)).iter().map(|c| format!("{}:{}", c.name, c.count)).collect();
    assert_eq!(kids, vec!["Band:3", "Image:2", "Loose:2"]);
    assert_eq!(tree.index_of_path(&format!("{}/Band/Record", s(&m))), Some(vec![0, 0, 0]));
    assert!(!tree.rebuild(&lib), "same folders: counts only");

    let mut list = TrackList::new();
    let outside = s(&dir.join("nowhere.flac"));
    list.set_paths(&lib, vec![format!("{}/Band/Record/03.wav", s(&m)), outside.clone()]);
    let row = list.row(&lib, 1).unwrap();
    assert!(row.missing);
    assert_eq!(row.meta.title, "nowhere");
    list.set_search(&lib, "song");
    assert_eq!(list.count(), 1);
    assert_eq!(list.row_of_list_index(1), None);
    assert_eq!(list.row_of_path(&format!("{}/Band/Record/03.wav", s(&m))), Some(0));

    // The queue sorts the way the lists do.
    let mut queue: Vec<String> = lib.tracks().iter().map(|t| t.path.clone()).collect();
    sort_paths(&mut queue, "fileName", false, |p| lib.track(p), |p| lib.is_favorite(p));
    let names: Vec<&str> = queue.iter().map(|p| p.rsplit('/').next().unwrap()).collect();
    assert_eq!(names, vec!["01.wav", "02.wav", "03.wav", "disc.cue#1", "disc.cue#2", "track 2.wav", "track 10.wav"]);

    // Dropped folders expand in natural order.
    let paths = lib.expand_audio_paths(&[s(&m.join("Loose")), s(&m.join("Image"))]);
    let names: Vec<&str> = paths.iter().map(|p| p.rsplit('/').next().unwrap()).collect();
    assert_eq!(names, vec!["track 2.wav", "track 10.wav", "disc.cue#1", "disc.cue#2"]);
}

#[test]
fn loudness_as_libebur128_measures_it() {
    let dir = scratch("loudness");
    let m = dir.join("Music");
    // A 440 Hz sine at 1/8 of full scale on both channels: -18.75 LUFS, so
    // +0.75 dB (ReplayGain 2.0, -18 LUFS). (The corpus' files, the same sine
    // upmixed by FFmpeg at -3 dB, measure +3.75 dB, as in earlier versions.)
    wav(&m.join("A/1.wav"), 3.0, 0.125, &[("IART", "X"), ("IPRD", "Y")]);
    wav(&m.join("A/2.wav"), 3.0, 0.25, &[("IART", "X"), ("IPRD", "Y")]);
    let mut lib = library(&dir);
    lib.add_folder(&s(&m));
    wait(&mut lib);
    assert_eq!(lib.tracks_without_loudness(), 2);
    lib.analyze_loudness();
    let ev = wait(&mut lib);
    assert!(ev.iter().any(|e| matches!(e, Event::LoudnessUpdated(p) if p.len() == 2)));
    let one = lib.find(&s(&m.join("A/1.wav"))).unwrap();
    assert!((one.rg_track_gain - 0.75).abs() < 0.02, "{}", one.rg_track_gain);
    assert!((one.rg_track_peak - 0.125).abs() < 0.002);
    let two = lib.find(&s(&m.join("A/2.wav"))).unwrap();
    assert!((two.rg_track_gain - (0.75 - 6.02)).abs() < 0.03, "{}", two.rg_track_gain);
    assert!(one.rg_analyzed && (one.rg_album_gain - two.rg_album_gain).abs() < 1e-12);
    assert!(one.rg_album_gain < one.rg_track_gain && one.rg_album_gain > two.rg_track_gain);
    assert_eq!(lib.tracks_without_loudness(), 0);
    // Kept across a rescan (same files).
    lib.rescan();
    wait(&mut lib);
    assert!(lib.find(&s(&m.join("A/1.wav"))).unwrap().rg_analyzed);
}

/// MusicBrainz and the archive, as a test sees them.
struct FakeHttp {
    asked: Mutex<Vec<String>>,
}

impl Http for FakeHttp {
    fn get(&self, url: &str) -> Response {
        self.asked.lock().unwrap().push(url.to_string());
        if url.starts_with("https://musicbrainz.org/ws/2/release/") {
            let body = r#"{"releases":[{"id":"r-low","score":50,"status":"Official","release-group":{"id":"g-low"}},
                {"id":"r1","score":100,"status":"Bootleg","release-group":{"id":"g1"}},{"id":"r2","score":90,"status":"Official"}]}"#;
            return Response { status: 200, retry_after: None, body: body.as_bytes().to_vec() };
        }
        if url == "https://coverartarchive.org/release-group/g1/front-500" {
            return Response { status: 404, retry_after: None, body: Vec::new() };
        }
        if url == "https://coverartarchive.org/release/r2/front-500" {
            // Noisy enough not to compress below 256 bytes, mostly blue.
            let mut seed = 1u32;
            let img = image::RgbImage::from_fn(40, 40, |_, _| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                image::Rgb([(seed >> 24) as u8 / 8, (seed >> 16) as u8 / 8, 255])
            });
            let mut png = Vec::new();
            image::DynamicImage::ImageRgb8(img)
                .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
                .unwrap();
            return Response { status: 200, retry_after: None, body: png };
        }
        Response { status: 404, retry_after: None, body: Vec::new() }
    }
}

#[test]
fn online_covers_with_a_fake_network() {
    let dir = scratch("online");
    let m = music(&dir);
    let mut lib = library(&dir);
    lib.add_folder(&s(&m));
    wait(&mut lib);
    fs::write(dir.join("sonkore.conf"), "[library]\nrescanOnStartup=false\nonlineCovers=true\n").unwrap();
    let http = Arc::new(FakeHttp { asked: Mutex::new(Vec::new()) });
    let mut art = OnlineArt::new(dir.join("cache/covers"), dir.join("sonkore.conf"), http.clone());
    assert!(art.enabled());
    art.reindex(&lib);
    let lookup = art.lookup();
    let track = format!("{}/Band/Record/02.wav", s(&m));
    assert_eq!(lookup(&track), None);
    let mut arrived = None;
    for _ in 0..200 {
        std::thread::sleep(Duration::from_millis(20));
        for e in art.process(&lib) {
            if let ArtEvent::CoverArrived(paths) = e {
                arrived = Some(paths);
            }
        }
        if arrived.is_some() {
            break;
        }
    }
    let paths = arrived.expect("a cover");
    assert_eq!(paths.len(), 3, "every track of the album");
    let file = lookup(&track).unwrap();
    assert!(file.ends_with(".img") && Path::new(&file).exists());
    let asked = http.asked.lock().unwrap().clone();
    assert!(asked[0].contains("query=release%3A%22Record%22%20AND%20artist%3A%22Band%22"), "{}", asked[0]);
    assert_eq!(asked[1], "https://coverartarchive.org/release-group/g1/front-500");
    assert_eq!(asked[2], "https://coverartarchive.org/release/r2/front-500");
    // The cover loader uses it for a track without art of its own.
    sonkore_library::covers::set_online_lookup(Some(lookup.clone()));
    let cover = sonkore_library::covers::load_cover(&track, 32, 4).unwrap();
    assert_eq!(cover.dimensions(), (32, 32));
    assert_eq!(cover.get_pixel(16, 16)[2], 255);
    sonkore_library::covers::set_online_lookup(None);
    assert!(art.cache_bytes() > 0);
    art.clear_cache();
    assert_eq!(art.cache_bytes(), 0);
}

/// The corpus of tests/make_library_corpus.sh, when there is one.
fn corpus() -> Option<PathBuf> {
    std::env::var_os("SONKORE_LIBRARY_CORPUS").map(PathBuf::from)
}

#[test]
fn corpus_tags_of_every_format() {
    let Some(c) = corpus() else { return };
    let f = |name: &str| s(&c.join("Music/Formats").join(name));
    let t = metadata::read(&f("cbr-v23.mp3")).unwrap();
    assert_eq!(
        (t.title.as_str(), &*t.artist, &*t.genre, t.track_no, t.disc_no, t.year),
        ("MP3 título", "Ärtist", "Rock", 5, 2, 2004)
    );
    assert_eq!((t.rg_track_gain, t.rg_track_peak, t.bitrate_kbps), (-3.21, 0.5, 193));
    let t = metadata::read(&f("aac.m4a")).unwrap();
    assert_eq!(
        (&*t.codec, t.bitrate_kbps, t.track_no, t.disc_no, t.year, t.has_embedded_cover),
        ("AAC", 160, 3, 1, 2020, true)
    );
    assert_eq!((t.rg_track_gain, t.rg_album_gain), (-2.0, -2.5));
    let t = metadata::read(&f("opus.opus")).unwrap();
    assert_eq!((t.rg_track_gain, t.rg_album_gain, t.duration), (0.0, 6.0, 3.0));
    let t = metadata::read(&f("dsd64.dsf")).unwrap();
    assert_eq!(
        (&*t.codec, t.sample_rate, t.bitrate_kbps, t.title.as_str(), t.duration),
        ("DSD64", 352800, 5645, "DSD title", 2.0)
    );
    let t = metadata::read(&f("alac24.m4a")).unwrap();
    assert_eq!((&*t.codec, t.bits, t.sample_rate), ("ALAC", 24, 96000));
    let t = metadata::read(&f("wavpack.wv")).unwrap();
    assert_eq!((&*t.album_artist, t.track_no, t.rg_track_gain), ("WV AA", 4, 0.42));
    let t = metadata::read(&f("v24.mp3")).unwrap();
    assert_eq!(
        (t.title.as_str(), t.year, t.track_no, &*t.genre, t.rg_album_gain, t.has_embedded_cover),
        ("Four", 2011, 11, "", 1.5, true)
    );
    assert!(metadata::read(&f("fake.wav")).is_none());
    assert!(metadata::read(&f("empty.flac")).is_none());
    // Covers: the front picture of a FLAC with two, and folder images.
    let front = metadata::embedded_cover(&s(&c.join("Music/Artist A/2019 - Album One/02 - Song 2.flac")));
    let img = image::load_from_memory(&front).unwrap();
    assert_eq!((img.width(), img.height()), (600, 400));
    assert!(metadata::folder_cover(&s(&c.join("Music/Artist A/2019 - Album One"))).unwrap().ends_with("Folder.JPG"));
    assert!(sonkore_library::covers::load_cover(&f("vorbis.ogg"), 100, 0).is_some());
}

#[test]
fn corpus_scan_speed() {
    let Some(c) = corpus() else { return };
    let perf = c.join("Perf");
    if !perf.exists() {
        return;
    }
    let dir = scratch("perf");
    let mut lib = library(&dir);
    let t = std::time::Instant::now();
    lib.add_folder(&s(&perf));
    wait(&mut lib);
    let first = t.elapsed();
    let t = std::time::Instant::now();
    lib.rescan();
    wait(&mut lib);
    let again = t.elapsed();
    eprintln!(
        "scan speed: {} files, first {:.3} s, unchanged {:.3} s",
        lib.count(),
        first.as_secs_f64(),
        again.as_secs_f64()
    );
}

/// Quitting right after skipping or removing a folder, before its scan is
/// through: the next start must not bring its tracks back from the cache.
#[test]
fn removed_folders_stay_gone_after_quitting_mid_scan() {
    let dir = scratch("quit-mid-scan");
    let m = music(&dir);
    let other = dir.join("Other");
    wav(&other.join("Live/one.wav"), 1.0, 0.25, &[]);
    let loose = format!("{}/Loose", s(&m));
    {
        let mut lib = library(&dir);
        lib.add_folder(&s(&m));
        wait(&mut lib);
        lib.add_folder(&s(&other));
        wait(&mut lib);
        assert_eq!(lib.folder_paths(&loose).len(), 2);
        assert_eq!(lib.folder_paths(&s(&other)).len(), 1);
        // Skipped and removed, and the app quits at once.
        lib.exclude_folder(&loose);
        lib.remove_folder(&s(&other));
    }
    let lib = library(&dir);
    assert!(lib.folder_paths(&loose).is_empty(), "the skipped folder came back");
    assert!(lib.folder_paths(&s(&other)).is_empty(), "the removed folder came back");
    assert_eq!(lib.folders(), &[s(&m)]);
    assert!(lib.count() > 0);
}

#[test]
fn a_scan_already_running_does_not_bring_back_what_was_removed() {
    let dir = scratch("stale-scan");
    let m = music(&dir);
    let other = dir.join("Other");
    wav(&other.join("Live/one.wav"), 1.0, 0.25, &[]);
    let extra = s(&dir.join("Elsewhere/single.wav"));
    wav(Path::new(&extra), 1.0, 0.25, &[]);
    let loose = format!("{}/Loose", s(&m));
    let song = format!("{}/Band/Record/01.wav", s(&m));
    let mut lib = library(&dir);
    lib.add_folder(&s(&m));
    wait(&mut lib);
    lib.add_folder(&s(&other));
    wait(&mut lib);
    lib.add_urls(std::slice::from_ref(&extra));
    wait(&mut lib);
    assert!(lib.contains(&song) && lib.contains(&extra));
    assert_eq!(lib.folder_paths(&loose).len(), 2);
    assert_eq!(lib.folder_paths(&s(&other)).len(), 1);
    // A scan runs through (its result not applied yet: the notifier only
    // tells the owner), and meanwhile things are removed.
    let (tx, rx) = std::sync::mpsc::channel();
    lib.set_notifier(move || {
        let _ = tx.send(());
    });
    lib.rescan();
    rx.recv_timeout(Duration::from_secs(60)).expect("the scan finishes");
    lib.remove_tracks(&[song.clone(), extra.clone()]);
    lib.exclude_folder(&loose);
    lib.remove_folder(&s(&other));
    let check = |lib: &Library, when: &str| {
        assert!(!lib.contains(&song), "{when}: the removed track came back");
        assert!(!lib.contains(&extra), "{when}: the removed file came back");
        assert!(lib.folder_paths(&loose).is_empty(), "{when}: the skipped folder came back");
        assert!(lib.folder_paths(&s(&other)).is_empty(), "{when}: the removed folder came back");
        assert!(lib.count() > 0, "{when}");
    };
    // The scan's result, made from the state before.
    lib.process();
    check(&lib, "the scan from before");
    wait(&mut lib);
    check(&lib, "the scan after");
    drop(lib);
    check(&library(&dir), "reopened");
}

#[test]
fn an_unreadable_library_file_is_kept_aside() {
    let dir = scratch("bad-json");
    let m = music(&dir);
    let json = dir.join("data/library.json");
    fs::create_dir_all(json.parent().unwrap()).unwrap();
    fs::write(&json, b"{\"version\": 4, \"tracks\": [{\"path\": ").unwrap();
    let mut lib = library(&dir);
    assert_eq!(lib.count(), 0);
    lib.add_folder(&s(&m));
    wait(&mut lib);
    let bad = dir.join("data/library.json.bad");
    assert_eq!(fs::read(&bad).ok().as_deref(), Some(&b"{\"version\": 4, \"tracks\": [{\"path\": "[..]));
    assert!(lib.count() > 0);
    drop(lib);
    assert!(library(&dir).count() > 0, "the new library.json reads");
}

#[test]
fn online_cover_for_tags_with_spaces_around() {
    let dir = scratch("online-spaces");
    let m = dir.join("Music");
    wav(&m.join("Image/disc.wav"), 6.0, 0.25, &[]);
    fs::write(
        m.join("Image/disc.cue"),
        "PERFORMER \" Cue Band \"\nTITLE \"Cue Album  \"\nFILE \"disc.wav\" WAVE\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n  TRACK 02 AUDIO\n    INDEX 01 00:02:00\n",
    )
    .unwrap();
    let mut lib = library(&dir);
    lib.add_folder(&s(&m));
    wait(&mut lib);
    let track = format!("{}/Image/disc.cue#1", s(&m));
    assert_eq!(&*lib.find(&track).unwrap().album, "Cue Album  ");
    fs::write(dir.join("sonkore.conf"), "[library]\nrescanOnStartup=false\nonlineCovers=true\n").unwrap();
    let http = Arc::new(FakeHttp { asked: Mutex::new(Vec::new()) });
    let mut art = OnlineArt::new(dir.join("cache/covers"), dir.join("sonkore.conf"), http.clone());
    art.reindex(&lib);
    let lookup = art.lookup();
    assert_eq!(lookup(&track), None);
    let mut arrived = None;
    for _ in 0..200 {
        std::thread::sleep(Duration::from_millis(20));
        for e in art.process(&lib) {
            if let ArtEvent::CoverArrived(paths) = e {
                arrived = Some(paths);
            }
        }
        if arrived.is_some() || art.pending() == 0 && art.fetched() > 0 {
            break;
        }
    }
    assert_eq!(arrived.expect("the cover shows").len(), 2, "both tracks of the image");
    assert!(lookup(&track).is_some());
    // Found again in the cache when the app starts.
    let art = OnlineArt::new(dir.join("cache/covers"), dir.join("sonkore.conf"), http);
    art.reindex(&lib);
    assert!(art.lookup()(&track).is_some(), "the cached cover is found");
}

#[test]
fn a_scan_that_finds_nothing_new_changes_nothing() {
    let dir = scratch("same-scan");
    let m = music(&dir);
    let mut lib = library(&dir);
    lib.add_folder(&s(&m));
    wait(&mut lib);
    let json = dir.join("data/library.json");
    // Marked: a file written again loses the mark.
    let mut marked = fs::read(&json).unwrap();
    marked.push(b' ');
    fs::write(&json, &marked).unwrap();
    let revision = lib.revision();
    let dirs = lib.watched_dirs();
    lib.rescan();
    let ev = wait(&mut lib);
    assert!(!ev.contains(&Event::TracksReset), "{ev:?}");
    assert!(ev.contains(&Event::ScanningChanged), "{ev:?}");
    assert_eq!(lib.revision(), revision);
    assert_eq!(fs::read(&json).unwrap(), marked, "library.json written again");
    assert_eq!(lib.watched_dirs(), dirs);
    // Something new: the tracks and the file are new too.
    wav(&m.join("Loose/new.wav"), 1.0, 0.25, &[]);
    lib.rescan();
    let ev = wait(&mut lib);
    assert!(ev.contains(&Event::TracksReset), "{ev:?}");
    assert_ne!(fs::read(&json).unwrap(), marked);
    // A folder with nothing in it is kept all the same.
    let empty = dir.join("Empty");
    fs::create_dir_all(&empty).unwrap();
    lib.add_folder(&s(&empty));
    wait(&mut lib);
    drop(lib);
    assert_eq!(library(&dir).folders(), &[s(&m), s(&empty)]);
}
