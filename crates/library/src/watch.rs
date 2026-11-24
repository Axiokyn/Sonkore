//! Folder watching (every scanned folder, a 2 s
//! debounce, then a rescan). Each folder is watched on its own, without
//! recursion, and reacts to what inotify reports for a folder:
//! entries created, removed, renamed or with changed attributes, and a file
//! closed after writing (retagged in place, or the end of a copy); not every
//! write on the way, nor files opened to be read (Sonkore's own scans).

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use notify::event::{AccessKind, AccessMode, EventKind, ModifyKind, RemoveKind};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};

/// At most this many folders are watched (kMaxWatchedDirs).
pub(crate) const MAX_WATCHED_DIRS: usize = 8000;
/// Quiet time after the last change before the library is scanned again.
pub(crate) const DEBOUNCE: Duration = Duration::from_millis(2000);

pub(crate) struct FolderWatcher {
    watcher: RecommendedWatcher,
    watched: Vec<String>,
    /// A folder was deleted: one made again at the same path is not
    /// watched until the folders are set again.
    lost: Arc<AtomicBool>,
}

fn relevant(kind: &EventKind) -> bool {
    match kind {
        EventKind::Create(_) | EventKind::Remove(_) => true,
        EventKind::Modify(ModifyKind::Name(_)) | EventKind::Modify(ModifyKind::Metadata(_)) => true,
        EventKind::Modify(ModifyKind::Any) | EventKind::Modify(ModifyKind::Other) => true,
        // Sonkore never writes into the watched folders: someone else did.
        EventKind::Access(AccessKind::Close(AccessMode::Write)) => true,
        EventKind::Modify(ModifyKind::Data(_)) | EventKind::Access(_) => false,
        EventKind::Any | EventKind::Other => true,
    }
}

impl FolderWatcher {
    /// A watcher that calls `changed` (on a worker thread) once things
    /// have been quiet for DEBOUNCE after a change.
    pub fn new(changed: impl Fn() + Send + 'static) -> Option<FolderWatcher> {
        let (poke, pokes) = mpsc::channel::<()>();
        thread::Builder::new()
            .name("sonkore-library-watch".into())
            .spawn(move || {
                while pokes.recv().is_ok() {
                    loop {
                        match pokes.recv_timeout(DEBOUNCE) {
                            Ok(()) => continue,
                            Err(RecvTimeoutError::Timeout) => break,
                            Err(RecvTimeoutError::Disconnected) => return,
                        }
                    }
                    changed();
                }
            })
            .ok()?;
        let poke: Sender<()> = poke;
        let lost = Arc::new(AtomicBool::new(false));
        let lost_flag = lost.clone();
        let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(event) = res
                && relevant(&event.kind)
            {
                if matches!(event.kind, EventKind::Remove(RemoveKind::Folder | RemoveKind::Any)) {
                    lost_flag.store(true, Ordering::Relaxed);
                }
                let _ = poke.send(());
            }
        })
        .ok()?;
        Some(FolderWatcher { watcher, watched: Vec::new(), lost })
    }

    /// True when exactly these folders are watched, and none was deleted
    /// since (setting them again would change nothing).
    pub fn watching(&self, dirs: &[String]) -> bool {
        !self.lost.load(Ordering::Relaxed) && self.watched.iter().eq(dirs.iter().take(MAX_WATCHED_DIRS))
    }

    /// Watches exactly these folders (the first MAX_WATCHED_DIRS).
    pub fn set(&mut self, dirs: &[String]) {
        self.lost.store(false, Ordering::Relaxed);
        for d in std::mem::take(&mut self.watched) {
            let _ = self.watcher.unwatch(Path::new(&d));
        }
        for d in dirs.iter().take(MAX_WATCHED_DIRS) {
            if self.watcher.watch(Path::new(d), RecursiveMode::NonRecursive).is_ok() {
                self.watched.push(d.clone());
            }
        }
    }

    pub fn watched(&self) -> &[String] {
        &self.watched
    }
}

#[cfg(test)]
mod tests {
    use notify::event::{CreateKind, DataChange};

    use super::*;

    #[test]
    fn a_file_written_in_place_counts() {
        // Retagged where it is, or the end of a copy (IN_CLOSE_WRITE).
        assert!(relevant(&EventKind::Access(AccessKind::Close(AccessMode::Write))));
        assert!(relevant(&EventKind::Create(CreateKind::File)));
        // Reading files (the scan itself) and every write on the way do not.
        assert!(!relevant(&EventKind::Access(AccessKind::Open(AccessMode::Any))));
        assert!(!relevant(&EventKind::Access(AccessKind::Close(AccessMode::Read))));
        assert!(!relevant(&EventKind::Modify(ModifyKind::Data(DataChange::Any))));
    }
}
