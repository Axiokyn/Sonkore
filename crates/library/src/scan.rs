//! The library scan (the rescan's worker): the watched folders walked as
//! earlier Sonkore versions walked them, files unchanged since the last scan
//! (same date and size) kept as they are, the others read again, on several
//! threads; then the short ones left out and the library order.

use std::collections::{HashMap, HashSet};
#[cfg(unix)]
use std::ffi::CString;
use std::fs;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::collate::utf16_cmp;
use crate::cue;
use crate::meta::{TrackMeta, lower};
use crate::metadata;

/// What a scan works from: a copy of the library's state.
pub(crate) struct ScanInput {
    pub folders: Vec<String>,
    pub extras: Vec<String>,
    pub hidden: HashSet<String>,
    pub excluded_folders: Vec<String>,
    pub min_duration: f64,
    pub known: HashMap<String, TrackMeta>,
    pub threads: usize,
}

pub(crate) struct ScanResult {
    pub tracks: Vec<TrackMeta>,
    pub directories: Vec<String>,
}

pub(crate) fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Readable: the process may read it.
#[cfg(unix)]
pub(crate) fn readable(path: &str) -> bool {
    match CString::new(path) {
        // SAFETY: a NUL-terminated path; access only reads it.
        Ok(c) => unsafe { libc::access(c.as_ptr(), libc::R_OK) == 0 },
        Err(_) => false,
    }
}

/// Readable: on Windows, what can be looked at.
#[cfg(not(unix))]
pub(crate) fn readable(path: &str) -> bool {
    fs::metadata(path).is_ok()
}

/// Hidden: a leading dot, and on Windows the hidden attribute.
fn hidden(name: &str, entry: &fs::DirEntry) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
        if entry.metadata().is_ok_and(|m| m.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0) {
            return true;
        }
    }
    #[cfg(not(windows))]
    let _ = entry;
    name.starts_with('.')
}

/// The canonical path: "" when it does not resolve. On
/// Windows in the stored form too: "C:/Music", not "\\?\C:\Music".
pub(crate) fn canonical(path: &str) -> String {
    fs::canonicalize(path).ok().and_then(|p| p.to_str().map(sonkore_core::paths::stored_path)).unwrap_or_default()
}

/// One entry of a folder listing.
pub(crate) struct Entry {
    pub path: String,
    pub name: String,
    pub is_dir: bool,
    pub is_file: bool,
    pub is_link: bool,
}

/// A folder's readable entries, without "." and "..": no hidden entries, no
/// broken links, by name (code units), last first. Names that are not UTF-8
/// are left out.
pub(crate) fn list_dir(dir: &str, reversed: bool) -> Vec<Entry> {
    let Ok(rd) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<Entry> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_string();
            if hidden(&name, &e) {
                return None;
            }
            let path = if dir.ends_with('/') { format!("{dir}{name}") } else { format!("{dir}/{name}") };
            let is_link = e.file_type().ok()?.is_symlink();
            let meta = fs::metadata(&path).ok()?; // follows links; a broken one fails
            if !readable(&path) {
                return None;
            }
            Some(Entry { path, name, is_dir: meta.is_dir(), is_file: meta.is_file(), is_link })
        })
        .collect();
    out.sort_by(|a, b| utf16_cmp(&a.name, &b.name));
    if reversed {
        out.reverse();
    }
    out
}

fn is_cue(path: &str) -> bool {
    cue::is_sheet(path)
}

/// A candidate: a known entry to keep, or a file to read.
enum Item {
    Keep(TrackMeta),
    Read { path: String, added: Option<i64> },
}

struct Walker<'a> {
    input: &'a ScanInput,
    seen: HashSet<String>,
    /// The dates of CD image tracks, by sheet (each sheet read once).
    cue_stamps: HashMap<String, Option<metadata::CueStamps>>,
    items: Vec<Item>,
    processed: &'a AtomicUsize,
    progress: &'a (dyn Fn(usize) + Sync),
}

impl Walker<'_> {
    /// A file met during the walk.
    fn consider(&mut self, path: &str) {
        if self.seen.contains(path) || self.input.hidden.contains(path) {
            return;
        }
        self.seen.insert(path.to_string());
        let known = self.input.known.get(path);
        // A CD image track is unchanged while neither its sheet nor its
        // image file is (the newer of the two dates).
        let cue_track = cue::is_track_path(path);
        let (size, modified) = metadata::file_stamp(path);
        let stamp = if cue_track { self.cue_stamp(path) } else { modified };
        match known {
            Some(k) if k.modified == stamp && (cue_track || k.file_size == size) => {
                // Unreadable ones were kept by older versions: left out.
                if k.sample_rate > 0 && k.channels > 0 {
                    self.items.push(Item::Keep(k.clone()));
                }
                let n = self.processed.fetch_add(1, Ordering::Relaxed) + 1;
                if n.is_multiple_of(64) {
                    (self.progress)(n);
                }
            }
            _ => self.items.push(Item::Read { path: path.to_string(), added: known.map(|k| k.added) }),
        }
    }

    /// A CD image track's date (-1 when its sheet does not read), each
    /// sheet read once per scan.
    fn cue_stamp(&mut self, path: &str) -> i64 {
        let Some((sheet, number)) = cue::split_track_path(path) else {
            return -1;
        };
        match self.cue_stamps.entry(sheet).or_insert_with_key(|s| metadata::cue_stamps(s)) {
            Some(stamps) => stamps.of(number),
            None => -1,
        }
    }
}

/// The library order: album artist (else artist), year, album, disc,
/// track, path; case aside, code unit order.
pub(crate) fn sort_tracks(tracks: &mut Vec<TrackMeta>) {
    struct Key {
        artist: Vec<u16>,
        year: i32,
        album: Vec<u16>,
        disc: i32,
        track: i32,
        path: Vec<u16>,
    }
    let keys: Vec<Key> = tracks
        .iter()
        .map(|t| Key {
            artist: lower(t.album_or_track_artist()).encode_utf16().collect(),
            year: t.year,
            album: lower(&t.album).encode_utf16().collect(),
            disc: t.disc_no,
            track: t.track_no,
            path: t.path.encode_utf16().collect(),
        })
        .collect();
    let mut order: Vec<usize> = (0..tracks.len()).collect();
    order.sort_by(|&a, &b| {
        let (x, y) = (&keys[a], &keys[b]);
        x.artist
            .cmp(&y.artist)
            .then(x.year.cmp(&y.year))
            .then_with(|| x.album.cmp(&y.album))
            .then(x.disc.cmp(&y.disc))
            .then(x.track.cmp(&y.track))
            .then_with(|| x.path.cmp(&y.path))
    });
    crate::sort::apply_order(tracks, &order);
}

/// Runs a scan. `progress(n)` is called now and then with the files gone
/// through; None when cancelled.
pub(crate) fn run(input: ScanInput, cancel: &AtomicBool, progress: &(dyn Fn(usize) + Sync)) -> Option<ScanResult> {
    let processed = AtomicUsize::new(0);
    let mut w = Walker {
        input: &input,
        seen: HashSet::new(),
        cue_stamps: HashMap::new(),
        items: Vec::new(),
        processed: &processed,
        progress,
    };
    let mut directories = Vec::new();

    let mut visited: HashSet<String> = HashSet::new(); // canonical paths: each real folder once
    for folder in &input.folders {
        if !fs::metadata(folder).map(|m| m.is_dir()).unwrap_or(false) {
            // An unplugged drive or an unmounted share: what was known stays,
            // except the folders skipped since.
            let prefix = format!("{folder}/");
            let skipped = |p: &str| input.excluded_folders.iter().any(|f| p.starts_with(&format!("{f}/")));
            let mut kept: Vec<&TrackMeta> = input
                .known
                .iter()
                .filter(|(k, _)| {
                    k.starts_with(&prefix) && !input.hidden.contains(*k) && !w.seen.contains(*k) && !skipped(k)
                })
                .map(|(_, t)| t)
                .collect();
            kept.sort_by(|a, b| a.path.cmp(&b.path));
            for t in kept {
                w.seen.insert(t.path.clone());
                w.items.push(Item::Keep(t.clone()));
            }
            continue;
        }
        // Depth first, following linked folders except links back up to
        // the root or above it, and folders already read. Real folders go
        // first, so music reachable both ways is listed under its real path.
        let root_canonical = canonical(folder);
        let mut pending = vec![folder.clone()];
        let mut linked: Vec<String> = Vec::new();
        while !pending.is_empty() || !linked.is_empty() {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            let dir = pending.pop().or_else(|| linked.pop()).unwrap();
            let canon = canonical(&dir);
            if canon.is_empty() || visited.contains(&canon) {
                continue;
            }
            if &dir != folder && (root_canonical == canon || root_canonical.starts_with(&format!("{canon}/"))) {
                continue;
            }
            if input.excluded_folders.contains(&dir) || input.excluded_folders.contains(&canon) {
                continue;
            }
            visited.insert(canon);
            directories.push(dir.clone());
            let entries = list_dir(&dir, true);
            // CD images: the sheet's tracks stand for the image file.
            let mut images: Vec<String> = Vec::new();
            for e in &entries {
                if e.is_file && is_cue(&e.path) {
                    for track in metadata::cue_tracks(&e.path, Some(&mut images)) {
                        w.consider(&track);
                    }
                }
            }
            for e in &entries {
                if e.is_dir {
                    if e.is_link { linked.push(e.path.clone()) } else { pending.push(e.path.clone()) }
                } else if metadata::is_audio_file(&e.path) && !images.contains(&e.path) {
                    w.consider(&e.path);
                }
            }
        }
    }
    for path in &input.extras {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        if fs::metadata(path).is_ok() || cue::is_track_path(path) {
            w.consider(path);
        }
    }

    let items = w.items;
    // The files to read, on several threads.
    let to_read: Vec<usize> =
        items.iter().enumerate().filter(|(_, i)| matches!(i, Item::Read { .. })).map(|(n, _)| n).collect();
    let next = AtomicUsize::new(0);
    let read: Vec<std::sync::Mutex<Option<TrackMeta>>> = to_read.iter().map(|_| std::sync::Mutex::new(None)).collect();
    let threads = input.threads.clamp(1, 32).min(to_read.len().max(1));
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                loop {
                    if cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    let k = next.fetch_add(1, Ordering::Relaxed);
                    if k >= to_read.len() {
                        return;
                    }
                    let Item::Read { path, added } = &items[to_read[k]] else {
                        continue;
                    };
                    let mut t = TrackMeta::default();
                    // Files that are not audio after all (empty, damaged, a text
                    // file named .wav) would only fail when played: left out.
                    if metadata::read_into(path, &mut t) && t.sample_rate > 0 && t.channels > 0 {
                        t.added = added.unwrap_or_else(now_ms);
                        *read[k].lock().unwrap() = Some(t);
                    }
                    let n = processed.fetch_add(1, Ordering::Relaxed) + 1;
                    if n.is_multiple_of(64) {
                        progress(n);
                    }
                }
            });
        }
    });
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    let mut read = read.into_iter().map(|m| m.into_inner().unwrap());
    let mut tracks = Vec::with_capacity(items.len());
    for item in items {
        match item {
            Item::Keep(t) => tracks.push(t),
            Item::Read { .. } => {
                if let Some(Some(t)) = read.next() {
                    tracks.push(t);
                }
            }
        }
    }
    // Short files (jingles, effects): out, unless their length is unknown.
    if input.min_duration > 0.0 {
        tracks.retain(|t| !(t.duration > 0.0 && t.duration < input.min_duration));
    }
    sort_tracks(&mut tracks);
    Some(ScanResult { tracks, directories })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cue_sheet_is_read_once_for_the_stamps_of_its_tracks() {
        let dir = std::env::temp_dir().join(format!("sonkore-scan-cue-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("disc.wav"), b"RIFF").unwrap();
        let mut sheet = String::from("FILE \"disc.wav\" WAVE\n");
        for n in 1..=6 {
            sheet += &format!("  TRACK {n:02} AUDIO\n    INDEX 01 0{n}:00:00\n");
        }
        fs::write(dir.join("disc.cue"), sheet).unwrap();
        let folder = sonkore_core::paths::stored_path(&dir.to_string_lossy());
        let input = ScanInput {
            folders: vec![folder],
            extras: Vec::new(),
            hidden: HashSet::new(),
            excluded_folders: Vec::new(),
            min_duration: 0.0,
            known: HashMap::new(),
            threads: 1,
        };
        // The walk runs on this thread (the files are read on others).
        let before = cue::PARSES.with(|n| n.get());
        run(input, &AtomicBool::new(false), &|_| {}).unwrap();
        let parses = cue::PARSES.with(|n| n.get()) - before;
        let _ = fs::remove_dir_all(&dir);
        // Once for its tracks, once for their dates.
        assert_eq!(parses, 2);
    }
}
