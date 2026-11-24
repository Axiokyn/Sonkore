//! What a drop on the window does (the drops themselves: dropped.rs). A
//! child module of the controller.
//!
//! - On the queue (and Now playing): every track dropped, folders opened,
//!   goes to the end of the queue.
//! - On a playlist: the same, at the end of the playlist (as its "Add
//!   files…" and "Add a folder…").
//! - Anywhere else: the files play now, in place of the queue (as when
//!   Sonkore opens them from the file manager), and a folder joins the
//!   library (as "Add music folder": watched and kept in sync); one that is
//!   already in it is shown.
//!
//! While something is dragged over the window the overlay says which.

use std::path::{Path, PathBuf};

use slint::ComponentHandle;

use super::Controller;
use crate::Drops;
use crate::dropped::{self, DropEvent, Sorted};
use crate::i18n::{tr, trf};

/// Where the drop goes, by page.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Target {
    Queue,
    Playlist,
    /// Files play, folders join the library.
    Library,
}

fn target(page: &str) -> Target {
    match page {
        "queue" | "playing" => Target::Queue,
        "playlist" => Target::Playlist,
        _ => Target::Library,
    }
}

/// The overlay's words for what a drop would do.
fn hint(target: Target, playlist: &str, s: Option<&Sorted>) -> String {
    let Some(s) = s else { return tr("Drops", "Drop music files or folders") };
    if s.files.is_empty() && s.folders.is_empty() {
        return tr("Drops", "Nothing here Sonkore can play");
    }
    match target {
        Target::Queue => tr("Drops", "Drop to add to the queue"),
        Target::Playlist => trf("Drops", "Drop to add to “{0}”", &[playlist]),
        Target::Library if s.folders.is_empty() => tr("Drops", "Drop to play"),
        Target::Library if s.files.is_empty() => tr("Drops", "Drop to add to the library"),
        Target::Library => tr("Drops", "Drop to play the files and add the folders to the library"),
    }
}

impl Controller {
    /// The drops reach the controller (dropped.rs calls on this thread).
    pub(super) fn connect_drops(self: &std::rc::Rc<Self>) {
        let weak = std::rc::Rc::downgrade(self);
        dropped::set_handler(move |event| {
            if let Some(c) = weak.upgrade() {
                c.drop_event(event);
            }
        });
    }

    fn drop_event(&self, event: DropEvent) {
        let (page, arg) = self.page.borrow().clone();
        let w = self.window();
        let d = w.global::<Drops>();
        match event {
            DropEvent::Hover(paths) => {
                let sorted = (!paths.is_empty()).then(|| dropped::sort(&paths));
                d.set_hint(hint(target(&page), &arg, sorted.as_ref()).into());
                d.set_shown(true);
            }
            DropEvent::Left => d.set_shown(false),
            DropEvent::Dropped(paths) => {
                d.set_shown(false);
                self.files_dropped(&page, &arg, &paths);
            }
        }
    }

    pub(super) fn files_dropped(&self, page: &str, arg: &str, paths: &[PathBuf]) {
        let s = dropped::sort(paths);
        if s.files.is_empty() && s.folders.is_empty() {
            if !s.ignored.is_empty() {
                self.show_message(&tr("Drops", "Nothing here Sonkore can play"));
            }
            return;
        }
        match target(page) {
            Target::Queue | Target::Playlist => {
                let all: Vec<String> = s.files.iter().chain(&s.folders).cloned().collect();
                let tracks = self.library.borrow().inner().expand_audio_paths(&all);
                if target(page) == Target::Playlist {
                    self.add_to_playlist(arg, tracks);
                } else if tracks.is_empty() {
                    self.show_message(&tr("PlaylistPage", "No audio files in that folder"));
                } else {
                    let changes = self.player.borrow_mut().enqueue(tracks);
                    self.after(changes);
                    self.show_message(&tr("TrackMenu", "Added to queue"));
                }
            }
            Target::Library => {
                let files = self.library.borrow().inner().audio_paths(&s.files);
                if !files.is_empty() {
                    let changes = self.player.borrow_mut().play_paths(files, 0);
                    self.after(changes);
                }
                let mut shown = None;
                let mut added = Vec::new();
                for folder in &s.folders {
                    let known =
                        self.library.borrow().inner().folders().iter().any(|f| Path::new(folder).starts_with(f));
                    if known {
                        shown = Some(folder.clone());
                    } else {
                        self.ctx.library.borrow_mut().add_folder(folder);
                        added.push(
                            Path::new(folder).file_name().map_or(folder.clone(), |n| n.to_string_lossy().into_owned()),
                        );
                    }
                }
                if !added.is_empty() {
                    self.show_message(&trf("Drops", "Adding to the library: {0}", &[&added.join(", ")]));
                } else if let Some(folder) = shown {
                    // Already there: its tracks.
                    self.open("folders", &folder);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hint_says_what_a_drop_does() {
        let files = Sorted { files: vec!["/a.flac".into()], ..Default::default() };
        let folders = Sorted { folders: vec!["/m".into()], ..Default::default() };
        let both = Sorted { files: vec!["/a.flac".into()], folders: vec!["/m".into()], ..Default::default() };
        let junk = Sorted { ignored: vec!["/a.txt".into()], ..Default::default() };
        assert_eq!(hint(target("tracks"), "", Some(&files)), "Drop to play");
        assert_eq!(hint(target("folders"), "/m", Some(&folders)), "Drop to add to the library");
        assert_eq!(
            hint(target("albums"), "", Some(&both)),
            "Drop to play the files and add the folders to the library"
        );
        assert_eq!(hint(target("queue"), "", Some(&folders)), "Drop to add to the queue");
        assert_eq!(hint(target("playing"), "", Some(&files)), "Drop to add to the queue");
        assert_eq!(hint(target("playlist"), "Mix", Some(&both)), "Drop to add to “Mix”");
        assert_eq!(hint(target("tracks"), "", Some(&junk)), "Nothing here Sonkore can play");
        assert_eq!(hint(target("tracks"), "", None), "Drop music files or folders");
    }
}
