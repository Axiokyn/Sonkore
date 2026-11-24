//! The document portal.
//!
//! In a Flatpak the file chooser may hand out a document portal path
//! (`/run/user/1000/doc/<id>/Music`) instead of the real one. That grant can
//! lapse (a new session, the portal restarting) and the folder then looks
//! gone although its drive is there. Sonkore reads the whole system anyway
//! (`--filesystem=host:ro`), so such paths are swapped for the real ones.

//!
//! Windows has no portal: every path is already the real one.

#[cfg(target_os = "linux")]
use std::collections::HashMap;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
#[cfg(target_os = "linux")]
use std::sync::Mutex;
#[cfg(target_os = "linux")]
use std::time::Duration;

#[cfg(target_os = "linux")]
use crate::bus::Bus;
use crate::url::{clean_path, is_local_file, to_local_file};

/// A document portal path split up: `/run/user/<uid>/doc/<id>/<name>[/inside]`
/// (or `/run/flatpak/doc/…`) gives `(id, inside)`, where `<name>` is the
/// shared file itself, or the shared folder and `inside` what was reached
/// through it (empty or starting with `/`).
pub fn split_document_path(path: &str) -> Option<(&str, &str)> {
    let rest = path.strip_prefix("/run/")?;
    let rest = if let Some(r) = rest.strip_prefix("flatpak/") {
        r
    } else {
        let r = rest.strip_prefix("user/")?;
        let slash = r.find('/')?;
        let uid = &r[..slash];
        if uid.is_empty() || !uid.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        &r[slash + 1..]
    };
    let rest = rest.strip_prefix("doc/")?;
    let slash = rest.find('/')?;
    let id = &rest[..slash];
    let rest = &rest[slash + 1..];
    if id.is_empty() {
        return None;
    }
    let name_end = rest.find('/').unwrap_or(rest.len());
    if name_end == 0 {
        return None; // the name must not be empty
    }
    Some((id, &rest[name_end..]))
}

/// [`host_path`] with the portal lookup and the existence check given.
pub fn host_path_with(
    path: &Path,
    lookup: impl FnOnce(&str) -> Option<PathBuf>,
    exists: impl FnOnce(&Path) -> bool,
) -> PathBuf {
    let Some(text) = path.to_str() else {
        return path.to_path_buf();
    };
    let Some((id, inside)) = split_document_path(text) else {
        return path.to_path_buf();
    };
    // The shared file or folder must be visible from here (a CD image track,
    // "sheet.cue#3", is no file of its own).
    match lookup(id) {
        Some(host) if !host.as_os_str().is_empty() && exists(&host) => {
            let mut joined = host.into_os_string();
            joined.push(inside);
            clean_path(Path::new(&joined))
        }
        _ => path.to_path_buf(),
    }
}

/// The real path behind a document portal path, when Sonkore can read it
/// there; any other path comes back as it is. Asks the portal once per
/// document (a library holds thousands of tracks under the same one).
#[cfg(target_os = "linux")]
pub fn host_path(path: &Path) -> PathBuf {
    if !path.as_os_str().as_encoded_bytes().starts_with(b"/run/") {
        return path.to_path_buf();
    }
    host_path_with(path, document_host, |p| p.exists())
}

/// Elsewhere there is no document portal: the path is the real one.
#[cfg(not(target_os = "linux"))]
pub fn host_path(path: &Path) -> PathBuf {
    path.to_path_buf()
}

pub fn host_paths<P: AsRef<Path>>(paths: &[P]) -> Vec<PathBuf> {
    paths.iter().map(|p| host_path(p.as_ref())).collect()
}

/// A local file URL as a path, through [`host_path`]; other URLs as text.
pub fn local_path(url: &str) -> String {
    match to_local_file(url) {
        Some(path) => host_path(&path).to_string_lossy().into_owned(),
        None => url.to_string(),
    }
}

/// The real path of a document, from `org.freedesktop.portal.Documents`,
/// remembered (None too: the portal is not asked again).
#[cfg(target_os = "linux")]
fn document_host(id: &str) -> Option<PathBuf> {
    static KNOWN: Mutex<Option<HashMap<String, Option<PathBuf>>>> = Mutex::new(None);
    let mut known = KNOWN.lock().unwrap_or_else(|e| e.into_inner());
    let known = known.get_or_insert_with(HashMap::new);
    if let Some(host) = known.get(id) {
        return host.clone();
    }
    let host = ask_portal(id);
    known.insert(id.to_string(), host.clone());
    host
}

#[cfg(target_os = "linux")]
fn ask_portal(id: &str) -> Option<PathBuf> {
    let conn = Bus::Session.connect(Duration::from_secs(2)).ok()?;
    let reply = conn
        .call_method(
            Some("org.freedesktop.portal.Documents"),
            "/org/freedesktop/portal/documents",
            Some("org.freedesktop.portal.Documents"),
            "GetHostPaths",
            &(vec![id],),
        )
        .ok()?;
    let paths: HashMap<String, Vec<u8>> = reply.body().deserialize().ok()?;
    host_from_bytes(paths.get(id)?)
}

/// The portal's `ay` path, which ends in a NUL.
#[cfg(unix)]
fn host_from_bytes(bytes: &[u8]) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let bytes = bytes.strip_suffix(b"\0").unwrap_or(bytes);
    (!bytes.is_empty()).then(|| PathBuf::from(std::ffi::OsStr::from_bytes(bytes)))
}

/// The name a save dialog granted when the one written was refused: a
/// document folder holds only the name the file chooser portal granted, and
/// a default suffix added afterwards ("eq" -> "eq.txt") is not it.
pub fn granted_name(path: &str) -> Option<&str> {
    split_document_path(path)?;
    let (stem, suffix) = path.rsplit_once('.')?;
    // `<root>/<id>/<name>.<suffix>`: the suffix holds no `/` and no `.`, and
    // the name is the document itself, not something inside a folder.
    if suffix.is_empty() || suffix.contains('/') {
        return None;
    }
    let (_, inside) = split_document_path(stem)?;
    inside.is_empty().then_some(stem)
}

/// [`open_for_save`] with the opener given.
pub fn open_for_save_with<F>(url: &str, mut open: impl FnMut(&Path) -> io::Result<F>) -> io::Result<(F, PathBuf)> {
    let path = if is_local_file(url) {
        to_local_file(url).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "not a local file"))?
    } else {
        PathBuf::from(url)
    };
    let first = match open(&path) {
        Ok(file) => return Ok((file, path)),
        Err(e) => e,
    };
    match path.to_str().and_then(granted_name) {
        Some(granted) => {
            let granted = PathBuf::from(granted);
            open(&granted).map(|f| (f, granted))
        }
        None => Err(first),
    }
}

/// Opens a file chosen in a save dialog for writing (truncated), from its
/// URL (or a plain path). Not through [`host_path`]: the real path is
/// read-only from the Flatpak, the document portal's one is not. When the
/// name with a suffix is refused, the granted one without it is written;
/// returns the file and the path written.
pub fn open_for_save(url: &str) -> io::Result<(File, PathBuf)> {
    open_for_save_with(url, |p: &Path| File::create(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_portal_paths() {
        assert_eq!(split_document_path("/run/user/1000/doc/ab12/Music"), Some(("ab12", "")));
        assert_eq!(split_document_path("/run/user/1000/doc/ab12/Music/x/y.flac"), Some(("ab12", "/x/y.flac")));
        assert_eq!(split_document_path("/run/flatpak/doc/ab12/a.flac"), Some(("ab12", "")));
        assert_eq!(split_document_path("/run/user/x1/doc/ab12/Music"), None);
        assert_eq!(split_document_path("/run/user/1000/doc/ab12"), None);
        assert_eq!(split_document_path("/run/user/1000/doc/ab12/"), None);
        assert_eq!(split_document_path("/run/media/disk/doc/a/b"), None);
        assert_eq!(split_document_path("/home/u/Music"), None);
    }

    #[test]
    fn maps_to_the_host() {
        let host = |id: &str| (id == "ab12").then(|| PathBuf::from("/mnt/disk/Music"));
        let yes = |_: &Path| true;
        assert_eq!(
            host_path_with(Path::new("/run/user/1000/doc/ab12/Music/A/./b.flac"), host, yes),
            Path::new("/mnt/disk/Music/A/b.flac")
        );
        assert_eq!(host_path_with(Path::new("/run/user/1000/doc/ab12/Music"), host, yes), Path::new("/mnt/disk/Music"));
        // Unknown document, or not visible from here: unchanged.
        let p = Path::new("/run/user/1000/doc/zz/Music/a.flac");
        assert_eq!(host_path_with(p, host, yes), p);
        let p = Path::new("/run/user/1000/doc/ab12/Music/a.flac");
        assert_eq!(host_path_with(p, host, |_| false), p);
        // Not a portal path at all.
        assert_eq!(host_path(Path::new("/home/u/a.flac")), Path::new("/home/u/a.flac"));
        assert_eq!(local_path("https://x.org/a.mp3"), "https://x.org/a.mp3");
        assert_eq!(local_path("file:///home/u/a%20b.flac"), "/home/u/a b.flac");
    }

    #[test]
    #[cfg(unix)]
    fn host_bytes_drop_the_nul() {
        assert_eq!(host_from_bytes(b"/a/b\0"), Some(PathBuf::from("/a/b")));
        assert_eq!(host_from_bytes(b"\0"), None);
    }

    #[test]
    fn save_falls_back_to_the_granted_name() {
        assert_eq!(granted_name("/run/user/1000/doc/ab/eq.txt"), Some("/run/user/1000/doc/ab/eq"));
        assert_eq!(granted_name("/run/flatpak/doc/ab/list.m3u8"), Some("/run/flatpak/doc/ab/list"));
        assert_eq!(granted_name("/run/user/1000/doc/ab/eq"), None);
        assert_eq!(granted_name("/run/user/1000/doc/ab/dir/eq.txt"), None);
        assert_eq!(granted_name("/home/u/eq.txt"), None);

        let mut tried = Vec::new();
        let r = open_for_save_with("file:///run/user/1000/doc/ab/eq.txt", |p| {
            tried.push(p.to_path_buf());
            if p.ends_with("eq") { Ok(()) } else { Err(io::Error::from(io::ErrorKind::PermissionDenied)) }
        });
        assert_eq!(r.unwrap().1, Path::new("/run/user/1000/doc/ab/eq"));
        assert_eq!(tried.len(), 2);

        let r =
            open_for_save_with("/home/u/eq.txt", |_| Err::<(), _>(io::Error::from(io::ErrorKind::PermissionDenied)));
        assert_eq!(r.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
    }
}
