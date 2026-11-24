//! `file://` URLs to and from local paths. On Windows in the stored form
//! there too: `file:///C:/Music/a.flac`, and a
//! share `\\nas\music` is `file://nas/music`.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

/// A path's bytes as these functions read them: as they are on Unix; on
/// Windows its UTF-8 with '/' (the stored form, see
/// `sonkore_core::paths::stored_path`).
fn path_bytes(path: &Path) -> Cow<'_, [u8]> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Cow::Borrowed(path.as_os_str().as_bytes())
    }
    #[cfg(not(unix))]
    {
        Cow::Owned(sonkore_core::paths::stored_path(&path.to_string_lossy()).into_bytes())
    }
}

/// The inverse of [`path_bytes`] (on Windows, bytes that are not UTF-8
/// become U+FFFD).
fn path_from(bytes: Vec<u8>) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        PathBuf::from(std::ffi::OsString::from_vec(bytes))
    }
    #[cfg(not(unix))]
    {
        PathBuf::from(String::from_utf8_lossy(&bytes).into_owned())
    }
}

/// Where the root of a path ends ('/'-separated, as [`path_bytes`] gives
/// it): "/" on Unix; on Windows also "C:/", "C:" and "//".
fn root_len(bytes: &[u8]) -> usize {
    if cfg!(windows) {
        if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
            return if bytes.get(2) == Some(&b'/') { 3 } else { 2 };
        }
        if bytes.starts_with(b"//") {
            return 2;
        }
    }
    usize::from(bytes.first() == Some(&b'/'))
}

/// The `file://` URL of a local path, percent-encoded (RFC 3986): what
/// MPRIS clients and the file manager expect in `xesam:url`, `mpris:artUrl`
/// and `OpenUri`.
pub fn file_url(path: &Path) -> String {
    let bytes = path_bytes(path);
    let mut url = String::with_capacity(8 + bytes.len());
    url.push_str("file://");
    let mut bytes: &[u8] = &bytes;
    if cfg!(windows) {
        if let Some(share) = bytes.strip_prefix(b"//") {
            bytes = share; // the server is the URL's host
        } else if !bytes.starts_with(b"/") {
            url.push('/'); // "C:/a" -> "file:///C:/a"
        }
    }
    for &b in bytes {
        let keep = b.is_ascii_alphanumeric() || b"-._~/!$&'()*+,;=:@".contains(&b);
        if keep {
            url.push(b as char);
        } else {
            url.push_str(&format!("%{b:02X}"));
        }
    }
    url
}

/// True for a `file:` URL (the scheme compared without case).
pub fn is_local_file(url: &str) -> bool {
    url.get(..5).is_some_and(|s| s.eq_ignore_ascii_case("file:"))
}

/// The local path of a `file:` URL (`file:///a%20b`, `file://localhost/a`,
/// `file:/a`); None for any other URL or a file on another host.
pub fn to_local_file(url: &str) -> Option<PathBuf> {
    if !is_local_file(url) {
        return None;
    }
    let rest = &url[5..];
    // Query and fragment are not part of the path.
    let rest = rest.split(['?', '#']).next().unwrap_or("");
    let mut share = None;
    let path = if let Some(after) = rest.strip_prefix("//") {
        let slash = after.find('/').unwrap_or(after.len());
        let host = &after[..slash];
        if !host.is_empty() && !host.eq_ignore_ascii_case("localhost") {
            if !cfg!(windows) {
                return None;
            }
            share = Some(host); // Windows reaches it as \\host\…
        }
        &after[slash..]
    } else {
        rest
    };
    if !path.starts_with('/') {
        return None;
    }
    let mut bytes = percent_decode(path.as_bytes());
    if cfg!(windows) {
        if let Some(host) = share {
            bytes.splice(0..0, format!("//{host}").into_bytes());
        } else if bytes.len() >= 3 && bytes[2] == b':' && bytes[1].is_ascii_alphabetic() {
            bytes.remove(0); // "/C:/a" -> "C:/a"
        }
    }
    Some(path_from(bytes))
}

fn percent_decode(input: &[u8]) -> Vec<u8> {
    let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] == b'%'
            && i + 2 < input.len()
            && let (Some(h), Some(l)) = (hex(input[i + 1]), hex(input[i + 2]))
        {
            out.push(h << 4 | l);
            i += 3;
            continue;
        }
        out.push(input[i]);
        i += 1;
    }
    out
}

/// A clean path: no `.`, `..` resolved against the
/// path itself, no doubled or trailing `/`.
pub fn clean_path(path: &Path) -> PathBuf {
    let bytes = path_bytes(path);
    let (root, bytes) = bytes.split_at(root_len(&bytes));
    let absolute = !root.is_empty();
    let mut parts: Vec<&[u8]> = Vec::new();
    for part in bytes.split(|&b| b == b'/') {
        match part {
            b"" | b"." => {}
            b".." => {
                if parts.last().is_some_and(|p| *p != b"..") {
                    parts.pop();
                } else if !absolute {
                    parts.push(part);
                }
            }
            _ => parts.push(part),
        }
    }
    let mut out = Vec::with_capacity(root.len() + bytes.len());
    out.extend_from_slice(root);
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            out.push(b'/');
        }
        out.extend_from_slice(part);
    }
    if out.is_empty() {
        out.push(b'.');
    }
    path_from(out)
}

/// The absolute form of `path` (against the working directory), cleaned.
pub fn absolute(path: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map(|d| d.join(path)).unwrap_or_else(|_| path.to_path_buf())
    };
    clean_path(&joined)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(unix)]
    fn file_url_encodes_what_a_url_cannot_hold() {
        assert_eq!(file_url(Path::new("/music/a b/ñ#1.flac")), "file:///music/a%20b/%C3%B1%231.flac");
        assert_eq!(file_url(Path::new("/x/sheet.cue")), "file:///x/sheet.cue");
    }

    #[test]
    #[cfg(unix)]
    fn local_file_round_trips() {
        let p = Path::new("/music/Été 100%/a+b (1).flac");
        assert_eq!(to_local_file(&file_url(p)).as_deref(), Some(p));
        assert_eq!(to_local_file("file://localhost/a/b").as_deref(), Some(Path::new("/a/b")));
        assert_eq!(to_local_file("FILE:/a%2").as_deref(), Some(Path::new("/a%2")));
        assert_eq!(to_local_file("file://server/a"), None);
        assert_eq!(to_local_file("https://example.org/a"), None);
        assert!(!is_local_file("http://x"));
    }

    #[test]
    #[cfg(windows)]
    fn windows_urls_as_stored() {
        assert_eq!(file_url(Path::new(r"C:\Music\a b.flac")), "file:///C:/Music/a%20b.flac");
        assert_eq!(file_url(Path::new(r"\\nas\music\a.flac")), "file://nas/music/a.flac");
        assert_eq!(to_local_file("file:///C:/Music/a%20b.flac").as_deref(), Some(Path::new("C:/Music/a b.flac")));
        assert_eq!(to_local_file("file://nas/music/a.flac").as_deref(), Some(Path::new("//nas/music/a.flac")));
        assert_eq!(clean_path(Path::new(r"C:\a\..\b\.\c\")), Path::new("C:/b/c"));
        assert_eq!(clean_path(Path::new("C:/..")), Path::new("C:/"));
    }

    #[test]
    #[cfg(unix)]
    fn clean_path_like_qdir() {
        assert_eq!(clean_path(Path::new("/a//b/./c/../d/")), Path::new("/a/b/d"));
        assert_eq!(clean_path(Path::new("/../a")), Path::new("/a"));
        assert_eq!(clean_path(Path::new("a/../../b")), Path::new("../b"));
        assert_eq!(clean_path(Path::new("/")), Path::new("/"));
    }
}
