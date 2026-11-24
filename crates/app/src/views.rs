//! What the library pages show, made from the tracks: the albums, the
//! artists and genres, the library's own order and the browse filters.
//! Plain functions over `Library` / `Track`, so the
//! source of the tracks can change without touching the pages.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use crate::lib_shim::{Library, Track};

/// The key of an empty tag.
pub const UNKNOWN: &str = "\u{1}";

/// An album's key: "album artist (or artist)" and "album", lower case.
pub fn album_key(t: &Track) -> String {
    format!("{}\u{1f}{}", album_artist(t).to_lowercase(), t.album.to_lowercase())
}

/// The album's artist, else the track's.
pub fn album_artist(t: &Track) -> &str {
    if t.album_artist.is_empty() { &t.artist } else { &t.album_artist }
}

/// The library's order: by album artist, year, album, disc, track, path
/// (the order a scan leaves the library in).
#[cfg(test)]
pub fn library_cmp(a: &Track, b: &Track) -> Ordering {
    album_artist(a)
        .to_lowercase()
        .cmp(&album_artist(b).to_lowercase())
        .then(a.year.cmp(&b.year))
        .then_with(|| a.album.to_lowercase().cmp(&b.album.to_lowercase()))
        .then(a.disc_no.cmp(&b.disc_no))
        .then(a.track_no.cmp(&b.track_no))
        .then_with(|| a.path.cmp(&b.path))
}

/// The tracks' indices in the library's order: as the library keeps them
/// (sorted with `library_cmp` after a scan; the cache's order until the
/// next one).
pub fn library_order(lib: &Library) -> Vec<usize> {
    (0..lib.tracks.len()).collect()
}

/// An album of the grid.
#[derive(Clone, Debug, Default)]
pub struct Album {
    pub key: String,
    pub album: String,
    pub artist: String,
    pub year: i64,
    /// The track whose cover stands for the album (for the cover loader).
    pub cover_path: String,
    pub tracks: usize,
    pub duration: f64,
    pub format: String,
    search_text: String,
}

/// The albums: one per album key, in the library's order.
pub fn albums(lib: &Library) -> Vec<Album> {
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut list: Vec<Album> = Vec::new();
    for i in library_order(lib) {
        let t = &lib.tracks[i];
        let key = album_key(t);
        let at = *index.entry(key.clone()).or_insert_with(|| {
            let artist = album_artist(t).to_owned();
            list.push(Album {
                search_text: format!("{} {}", t.album, artist).to_lowercase(),
                key,
                album: t.album.clone(),
                artist,
                year: t.year,
                cover_path: t.path.clone(),
                tracks: 0,
                duration: 0.0,
                format: t.format_label(),
            });
            list.len() - 1
        });
        list[at].tracks += 1;
        list[at].duration += t.duration;
    }
    list
}

/// The albums a search leaves: every word of the search in "album artist".
pub fn filter_albums(albums: &[Album], search: &str) -> Vec<usize> {
    let words: Vec<String> = search.to_lowercase().split_whitespace().map(str::to_owned).collect();
    (0..albums.len()).filter(|&i| words.iter().all(|w| albums[i].search_text.contains(w.as_str()))).collect()
}

/// An artist or a genre.
#[derive(Clone, Debug, Default)]
pub struct BrowseItem {
    pub key: String,
    pub name: String,
    pub count: usize,
    pub albums: usize,
}

/// The artists, genres or albums by `field`: "genre", "artist" (the album artist, else
/// the track's) or "album"; the unknown one last.
pub fn browse(lib: &Library, field: &str) -> Vec<BrowseItem> {
    let mut entries: HashMap<String, (String, usize, HashSet<String>)> = HashMap::new();
    for t in &lib.tracks {
        let value = match field {
            "genre" => t.genre.as_str(),
            "album" => t.album.as_str(),
            _ => album_artist(t),
        };
        let key = if value.is_empty() { UNKNOWN.to_owned() } else { value.to_owned() };
        let e = entries.entry(key).or_insert_with(|| (value.to_owned(), 0, HashSet::new()));
        e.1 += 1;
        e.2.insert(album_key(t));
    }
    let mut list: Vec<BrowseItem> = entries
        .into_iter()
        .map(|(key, (name, count, albums))| BrowseItem { key, name, count, albums: albums.len() })
        .collect();
    list.sort_by(|a, b| {
        let (ua, ub) = (a.key == UNKNOWN, b.key == UNKNOWN);
        ua.cmp(&ub).then_with(|| locale_cmp(&a.name, &b.name))
    });
    list
}

/// The browse filters: `key` "" is any, UNKNOWN an empty tag;
/// an artist matches the track's artist or its album artist.
pub fn matches_browse(t: &Track, field: &str, key: &str) -> bool {
    if key.is_empty() {
        return true;
    }
    match field {
        "genre" => {
            if key == UNKNOWN {
                t.genre.is_empty()
            } else {
                t.genre == key
            }
        }
        "album" => {
            if key == UNKNOWN {
                t.album.is_empty()
            } else {
                t.album == key
            }
        }
        _ => {
            let artist = album_artist(t);
            if key == UNKNOWN { artist.is_empty() } else { t.artist == key || t.album_artist == key || artist == key }
        }
    }
}

/// A locale-aware compare, near enough: accents and case folded first.
pub fn locale_cmp(a: &str, b: &str) -> Ordering {
    fold(a).cmp(&fold(b)).then_with(|| a.cmp(b))
}

fn fold(s: &str) -> String {
    s.chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'á' | 'à' | 'â' | 'ä' | 'ã' | 'å' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'ö' | 'õ' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            // ñ sorts after n (Spanish), as the collator puts it.
            'ñ' => 'n',
            c => c,
        })
        .collect()
}

/// "12 min", "1 h 5 min".
pub fn duration_text(seconds: f64) -> String {
    let minutes = (seconds / 60.0 + 0.5) as i64;
    if minutes >= 60 {
        crate::i18n::trf("TracksPage", "{0} h {1} min", &[&(minutes / 60).to_string(), &(minutes % 60).to_string()])
    } else {
        crate::i18n::trf("TracksPage", "{0} min", &[&minutes.to_string()])
    }
}

/// "1 track", "12 tracks".
pub fn tracks_text(n: usize) -> String {
    if n == 1 {
        crate::i18n::tr("AppActions", "1 track")
    } else {
        crate::i18n::trf("AppActions", "{0} tracks", &[&n.to_string()])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(path: &str, artist: &str, album: &str, genre: &str, year: i64, no: i64) -> Track {
        Track {
            path: path.into(),
            artist: artist.into(),
            album: album.into(),
            genre: genre.into(),
            year,
            track_no: no,
            ..Default::default()
        }
    }

    #[test]
    fn grouping() {
        let lib = Library::with_tracks(vec![
            track("/b/2", "B", "Two", "Rock", 2001, 2),
            track("/b/1", "B", "Two", "Rock", 2001, 1),
            track("/a/1", "A", "One", "", 1999, 1),
            track("/x", "", "", "Jazz", 0, 0),
        ]);
        let a = albums(&lib);
        assert_eq!(a.len(), 3);
        assert_eq!(a[0].album, "Two"); // in the library's order
        assert_eq!(a[0].tracks, 2);
        assert_eq!(a[2].album, "");
        let mut order = library_order(&lib);
        order.sort_by(|&x, &y| library_cmp(&lib.tracks[x], &lib.tracks[y]));
        assert_eq!(lib.tracks[order[2]].path, "/b/1");
        let artists = browse(&lib, "artist");
        assert_eq!(artists.last().unwrap().key, UNKNOWN);
        assert_eq!(artists[0].name, "A");
        let genres = browse(&lib, "genre");
        assert_eq!(genres.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(), ["Jazz", "Rock", ""]);
        assert!(matches_browse(&lib.tracks[3], "artist", UNKNOWN));
        assert_eq!(filter_albums(&a, "two b"), vec![0]);
    }
}
