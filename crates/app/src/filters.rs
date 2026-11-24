//! The filters over the Tracks page (Settings › General › Filters on the
//! Tracks page, ui/trackBrowser): three panes, Genres → Artists → Albums, as
//! classic music players have them. A genre picked leaves its artists and
//! albums, an artist its albums; the list shows the tracks of all three.
//!
//! Made for big libraries: each track's genre, artist and album become
//! numbers once per library change (`Index::new`), and a pick only counts
//! them again (one pass over the numbers, no strings).

use std::collections::HashMap;

use crate::lib_shim::Track;
use crate::views::{UNKNOWN, album_artist, album_key, locale_cmp};

/// What is picked in each pane: a key ("" = all of them).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub genre: String,
    pub artist: String,
    pub album: String,
}

/// An entry of a pane: its key, the name shown, its tracks.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Item {
    pub key: String,
    pub name: String,
    pub count: usize,
    /// The empty tag ("No genre", "Unknown artist", "No album").
    pub unknown: bool,
}

/// The three panes' entries (without "All"), in the order shown.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Panes {
    pub genres: Vec<Item>,
    pub artists: Vec<Item>,
    pub albums: Vec<Item>,
}

/// The values of one field: each one's key and name, and its place in the
/// sorted order (the unknown one last).
#[derive(Default)]
struct Values {
    keys: Vec<String>,
    names: Vec<String>,
    /// Another value with the same name (two albums called "Greatest Hits"):
    /// shown with the artist.
    artists: Vec<String>,
    rank: Vec<u32>,
    by_key: HashMap<String, u32>,
}

impl Values {
    fn id(&mut self, key: &str, name: &str, artist: &str) -> u32 {
        if let Some(&id) = self.by_key.get(key) {
            return id;
        }
        let id = self.keys.len() as u32;
        self.keys.push(key.to_owned());
        self.names.push(name.to_owned());
        self.artists.push(artist.to_owned());
        self.by_key.insert(key.to_owned(), id);
        id
    }

    /// The order shown: by name (accents and case folded), the unknown last;
    /// albums of the same name by their artist.
    fn sort(&mut self) {
        let mut order: Vec<u32> = (0..self.keys.len() as u32).collect();
        order.sort_by(|&a, &b| {
            let (a, b) = (a as usize, b as usize);
            let (ua, ub) = (self.keys[a] == UNKNOWN, self.keys[b] == UNKNOWN);
            ua.cmp(&ub)
                .then_with(|| locale_cmp(&self.names[a], &self.names[b]))
                .then_with(|| locale_cmp(&self.artists[a], &self.artists[b]))
        });
        self.rank = vec![0; order.len()];
        for (place, &id) in order.iter().enumerate() {
            self.rank[id as usize] = place as u32;
        }
    }

    /// The values counted in `counts` (by id), in the order shown.
    fn items(&self, counts: &[usize]) -> Vec<Item> {
        let mut ids: Vec<usize> = (0..counts.len()).filter(|&i| counts[i] > 0).collect();
        ids.sort_by_key(|&i| self.rank[i]);
        let mut items: Vec<Item> = ids
            .iter()
            .map(|&i| Item {
                key: self.keys[i].clone(),
                name: self.names[i].clone(),
                count: counts[i],
                unknown: self.keys[i] == UNKNOWN,
            })
            .collect();
        // Two albums of the same name in the pane: each with its artist.
        for w in 0..ids.len() {
            let same = |o: usize| {
                o < ids.len()
                    && o != w
                    && !items[w].unknown
                    && self.names[ids[o]].to_lowercase() == items[w].name.to_lowercase()
            };
            if same(w + 1) || (w > 0 && same(w - 1)) {
                let artist = &self.artists[ids[w]];
                if !artist.is_empty() {
                    items[w].name = format!("{} · {}", items[w].name, artist);
                }
            }
        }
        items
    }
}

/// Every track's genre, artist and album as numbers.
#[derive(Default)]
pub struct Index {
    genres: Values,
    artists: Values,
    albums: Values,
    /// Per track: [genre, artist, album].
    tracks: Vec<[u32; 3]>,
}

impl Index {
    /// The genre as tagged; the artist is the album's artist, else the
    /// track's (as the Artists page lists them: a compilation under its
    /// album artist); the album is the album of that artist.
    pub fn new(tracks: &[Track]) -> Index {
        let mut index = Index { tracks: Vec::with_capacity(tracks.len()), ..Default::default() };
        for t in tracks {
            let genre = if t.genre.is_empty() { UNKNOWN } else { t.genre.as_str() };
            let artist = album_artist(t);
            let artist_key = if artist.is_empty() { UNKNOWN } else { artist };
            let album = if t.album.is_empty() { UNKNOWN.to_owned() } else { album_key(t) };
            index.tracks.push([
                index.genres.id(genre, &t.genre, ""),
                index.artists.id(artist_key, artist, ""),
                index.albums.id(&album, &t.album, artist),
            ]);
        }
        index.genres.sort();
        index.artists.sort();
        index.albums.sort();
        index
    }

    fn picked(values: &Values, key: &str) -> Option<u32> {
        if key.is_empty() { None } else { Some(values.by_key.get(key).copied().unwrap_or(u32::MAX)) }
    }

    /// The panes for `sel`: every genre; the artists of the genre picked;
    /// the albums of both. A pick no longer listed (the library changed, a
    /// genre picked without that artist) falls back to all.
    pub fn panes(&self, sel: &mut Selection) -> Panes {
        let mut genres = vec![0; self.genres.keys.len()];
        for t in &self.tracks {
            genres[t[0] as usize] += 1;
        }
        let genre = Self::picked(&self.genres, &sel.genre).filter(|&g| genres.get(g as usize).is_some_and(|&n| n > 0));
        if genre.is_none() {
            sel.genre.clear();
        }
        let mut artists = vec![0; self.artists.keys.len()];
        for t in self.tracks.iter().filter(|t| genre.is_none_or(|g| t[0] == g)) {
            artists[t[1] as usize] += 1;
        }
        let artist =
            Self::picked(&self.artists, &sel.artist).filter(|&a| artists.get(a as usize).is_some_and(|&n| n > 0));
        if artist.is_none() {
            sel.artist.clear();
        }
        let mut albums = vec![0; self.albums.keys.len()];
        for t in self.tracks.iter().filter(|t| genre.is_none_or(|g| t[0] == g) && artist.is_none_or(|a| t[1] == a)) {
            albums[t[2] as usize] += 1;
        }
        let album = Self::picked(&self.albums, &sel.album).filter(|&a| albums.get(a as usize).is_some_and(|&n| n > 0));
        if album.is_none() {
            sel.album.clear();
        }
        Panes {
            genres: self.genres.items(&genres),
            artists: self.artists.items(&artists),
            albums: self.albums.items(&albums),
        }
    }

    /// The tracks (library rows) of all three picks.
    pub fn rows(&self, sel: &Selection) -> Vec<usize> {
        let genre = Self::picked(&self.genres, &sel.genre);
        let artist = Self::picked(&self.artists, &sel.artist);
        let album = Self::picked(&self.albums, &sel.album);
        (0..self.tracks.len())
            .filter(|&i| {
                let t = self.tracks[i];
                genre.is_none_or(|g| t[0] == g) && artist.is_none_or(|a| t[1] == a) && album.is_none_or(|a| t[2] == a)
            })
            .collect()
    }

    pub fn len(&self) -> usize {
        self.tracks.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(path: &str, artist: &str, album_artist: &str, album: &str, genre: &str) -> Track {
        Track {
            path: path.into(),
            artist: artist.into(),
            album_artist: album_artist.into(),
            album: album.into(),
            genre: genre.into(),
            ..Default::default()
        }
    }

    fn library() -> Vec<Track> {
        vec![
            track("/1", "Miles", "", "Kind of Blue", "Jazz"),
            track("/2", "Miles", "", "Kind of Blue", "Jazz"),
            track("/3", "Miles", "", "Bitches Brew", "Fusion"),
            track("/4", "Bill", "", "Waltz", "Jazz"),
            track("/5", "Guest", "Various", "Greatest Hits", "Rock"),
            track("/6", "Band", "", "Greatest Hits", "Rock"),
            track("/7", "", "", "", ""),
            track("/8", "Ángel", "", "Ópera", "jazz"),
        ]
    }

    fn names(items: &[Item]) -> Vec<&str> {
        items.iter().map(|i| i.name.as_str()).collect()
    }

    #[test]
    fn everything_with_nothing_picked() {
        let index = Index::new(&library());
        let mut sel = Selection::default();
        let p = index.panes(&mut sel);
        // Sorted by name with accents folded, the empty tag last.
        assert_eq!(names(&p.genres), ["Fusion", "Jazz", "jazz", "Rock", ""]);
        assert_eq!(p.genres.last().unwrap().key, UNKNOWN);
        assert!(p.genres.last().unwrap().unknown);
        assert_eq!(p.genres[1].count, 3);
        // A compilation under its album artist.
        assert_eq!(names(&p.artists), ["Ángel", "Band", "Bill", "Miles", "Various", ""]);
        // Two albums of the same name: each with its artist.
        assert_eq!(
            names(&p.albums),
            ["Bitches Brew", "Greatest Hits · Band", "Greatest Hits · Various", "Kind of Blue", "Ópera", "Waltz", ""]
        );
        assert_eq!(index.rows(&sel).len(), 8);
    }

    #[test]
    fn a_genre_leaves_its_artists_and_albums() {
        let index = Index::new(&library());
        let mut sel = Selection { genre: "Jazz".into(), ..Default::default() };
        let p = index.panes(&mut sel);
        assert_eq!(p.genres.len(), 5, "the genres stay");
        assert_eq!(names(&p.artists), ["Bill", "Miles"]);
        assert_eq!(p.artists[1].count, 2);
        assert_eq!(names(&p.albums), ["Kind of Blue", "Waltz"]);
        assert_eq!(index.rows(&sel), vec![0, 1, 3]);
        // An artist within it: its albums of that genre only.
        sel.artist = "Miles".into();
        let p = index.panes(&mut sel);
        assert_eq!(names(&p.albums), ["Kind of Blue"]);
        assert_eq!(index.rows(&sel), vec![0, 1]);
        // And an album.
        sel.album = p.albums[0].key.clone();
        index.panes(&mut sel);
        assert_eq!(index.rows(&sel), vec![0, 1]);
    }

    #[test]
    fn an_artist_alone_and_the_album_with_its_name() {
        let index = Index::new(&library());
        let mut sel = Selection { artist: "Miles".into(), ..Default::default() };
        let p = index.panes(&mut sel);
        assert_eq!(names(&p.albums), ["Bitches Brew", "Kind of Blue"]);
        assert_eq!(index.rows(&sel), vec![0, 1, 2]);
        // Greatest Hits by Band only: not the compilation of the same name.
        let mut sel = Selection::default();
        let p = index.panes(&mut sel);
        sel.album = p.albums[1].key.clone();
        assert_eq!(index.rows(&sel), vec![5]);
    }

    #[test]
    fn a_pick_that_is_gone_falls_back_to_all() {
        let index = Index::new(&library());
        // Bill has no Rock: the artist (and the album) go back to all.
        let mut sel = Selection { genre: "Rock".into(), artist: "Bill".into(), album: "bill\u{1f}waltz".into() };
        let p = index.panes(&mut sel);
        assert_eq!(sel, Selection { genre: "Rock".into(), ..Default::default() });
        assert_eq!(names(&p.artists), ["Band", "Various"]);
        assert_eq!(index.rows(&sel), vec![4, 5]);
        // A genre that left the library.
        let mut sel = Selection { genre: "Polka".into(), ..Default::default() };
        index.panes(&mut sel);
        assert_eq!(sel.genre, "");
    }

    #[test]
    fn the_empty_tags() {
        let index = Index::new(&library());
        let mut sel = Selection { genre: UNKNOWN.into(), ..Default::default() };
        let p = index.panes(&mut sel);
        assert_eq!(p.artists.len(), 1);
        assert!(p.artists[0].unknown);
        assert!(p.albums[0].unknown);
        assert_eq!(index.rows(&sel), vec![6]);
    }

    #[test]
    fn fifty_thousand_tracks_quickly() {
        let tracks: Vec<Track> = (0..50_000)
            .map(|i| {
                track(
                    &format!("/{i}"),
                    &format!("Artist {}", i % 2000),
                    "",
                    &format!("Album {}", i % 5000),
                    &format!("Genre {}", i % 40),
                )
            })
            .collect();
        let start = std::time::Instant::now();
        let index = Index::new(&tracks);
        let built = start.elapsed();
        let start = std::time::Instant::now();
        let mut sel = Selection { genre: "Genre 7".into(), ..Default::default() };
        let p = index.panes(&mut sel);
        let rows = index.rows(&sel);
        let picked = start.elapsed();
        assert_eq!(rows.len(), 1250);
        assert_eq!(p.artists.len(), 50);
        // Debug builds are slow; these are generous (release: ~15 ms, ~1 ms).
        assert!(built.as_millis() < 1500, "index: {built:?}");
        assert!(picked.as_millis() < 150, "a pick: {picked:?}");
    }
}
