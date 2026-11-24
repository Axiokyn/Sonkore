//! Sonkore's music library: folders, tags, covers, playlists and the lists
//! the pages show, as plain data and query functions the interface wraps.
//! Every file it keeps (library.json, favorites.json, playlists.json, the
//! cover cache) is in the format earlier Sonkore versions wrote, byte for
//! byte, so all share one library.
//!
//! - `library::Library`: folders, scans, favourites, ReplayGain analysis.
//!   Lives on the interface's thread; background work calls its notifier,
//!   then `process()` applies it and returns `Event`s. Mutating calls
//!   return the events they cause.
//! - `metadata`: tags and stream facts, `cue`: CD images.
//! - `views`: `TrackFilter`, `TrackList`, `AlbumList`, `FolderTree`,
//!   `sort` / `collate` (track order, text order).
//! - `playlists::Playlists`, M3U in and out.
//! - `covers` (loading, the image provider, the cover cache),
//!   `online_art::OnlineArt` (MusicBrainz + Cover Art Archive).
//! - `loudness`: EBU R128 measurement.
//! - `json_out`: JSON written in the stored format.

pub mod collate;
pub mod covers;
pub mod cue;
pub mod json_out;
pub mod library;
pub mod loudness;
pub mod meta;
pub mod metadata;
pub mod online_art;
pub mod playlists;
mod scan;
pub mod sort;
pub mod views;
mod watch;

pub use library::{BrowseEntry, Event, Library, LibraryFiles, PathMapper, TrackFolder, UNKNOWN};
pub use meta::{FieldValue, Text, TrackMeta};
pub use playlists::{PlaylistEvent, Playlists};
pub use views::{Album, AlbumList, FolderNode, FolderTree, ListRow, TrackFilter, TrackList};
