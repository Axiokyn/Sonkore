//! Where Sonkore keeps its files, where earlier Sonkore versions kept
//! them. On Linux: settings in `$XDG_CONFIG_HOME/sonkore`, the
//! library and playlists in `$XDG_DATA_HOME/sonkore`, covers and waveforms
//! in `$XDG_CACHE_HOME/sonkore`. On Windows: settings in
//! `%APPDATA%\sonkore` (earlier versions kept them in the registry; this one
//! keeps the same INI file it writes on Linux), data in
//! `%LOCALAPPDATA%\sonkore` and the cache in `%LOCALAPPDATA%\cache\sonkore`,
//! the generic data and cache folders there, so the library and playlists
//! are the ones earlier versions wrote.

use std::env;
use std::path::PathBuf;

/// The application's id: its folder names and the settings file's name.
pub const APP_ID: &str = "sonkore";

#[cfg(not(windows))]
fn home() -> PathBuf {
    env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

/// An XDG base directory: the variable when it holds an absolute path,
/// else `$HOME/<fallback>`.
#[cfg(not(windows))]
fn xdg(variable: &str, fallback: &str) -> PathBuf {
    match env::var_os(variable).map(PathBuf::from) {
        Some(dir) if dir.is_absolute() => dir,
        _ => home().join(fallback),
    }
}

/// A known folder on Windows: the variable, else the user's profile below.
/// SONKORE_* overrides (as XDG_* on Linux) let tests use their own profile.
#[cfg(windows)]
fn known(variable: &str, fallback: &str) -> PathBuf {
    let own = format!("SONKORE_{variable}");
    for v in [own.as_str(), variable] {
        if let Some(dir) = env::var_os(v).map(PathBuf::from).filter(|d| d.is_absolute()) {
            return dir;
        }
    }
    env::var_os("USERPROFILE").map_or_else(|| PathBuf::from("C:\\"), PathBuf::from).join(fallback)
}

/// `$XDG_CONFIG_HOME/sonkore/sonkore.conf` (an INI file in the stored format).
#[cfg(not(windows))]
pub fn settings_file() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config").join(APP_ID).join(format!("{APP_ID}.conf"))
}

/// `%APPDATA%\sonkore\sonkore.conf`, the same INI file as on Linux.
#[cfg(windows)]
pub fn settings_file() -> PathBuf {
    known("APPDATA", "AppData\\Roaming").join(APP_ID).join(format!("{APP_ID}.conf"))
}

/// `$XDG_DATA_HOME/sonkore`: library.json, favorites.json, playlists.json…
#[cfg(not(windows))]
pub fn data_dir() -> PathBuf {
    xdg("XDG_DATA_HOME", ".local/share").join(APP_ID)
}

/// `%LOCALAPPDATA%\sonkore` (the generic data folder on Windows).
#[cfg(windows)]
pub fn data_dir() -> PathBuf {
    known("LOCALAPPDATA", "AppData\\Local").join(APP_ID)
}

/// A file in the data folder.
pub fn data_file(name: &str) -> PathBuf {
    data_dir().join(name)
}

/// `$XDG_CACHE_HOME/sonkore`: covers, waveforms.
#[cfg(not(windows))]
pub fn cache_dir() -> PathBuf {
    xdg("XDG_CACHE_HOME", ".cache").join(APP_ID)
}

/// `%LOCALAPPDATA%\cache\sonkore` (the generic cache folder on Windows).
#[cfg(windows)]
pub fn cache_dir() -> PathBuf {
    known("LOCALAPPDATA", "AppData\\Local").join("cache").join(APP_ID)
}

/// A path in the stored form, which is how the library and playlists store
/// them: on Windows with '/' and without the "\\?\" prefix canonicalize
/// adds ("C:/Music/a.flac"); elsewhere as it is.
pub fn stored_path(path: &str) -> String {
    if cfg!(windows) {
        let path = path
            .strip_prefix(r"\\?\UNC\")
            .map(|p| format!(r"\\{p}"))
            .unwrap_or_else(|| path.strip_prefix(r"\\?\").unwrap_or(path).to_owned());
        path.replace('\\', "/")
    } else {
        path.to_owned()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    #[cfg(windows)]
    fn stored_paths_use_slashes() {
        assert_eq!(super::stored_path(r"\\?\C:\Music\a.flac"), "C:/Music/a.flac");
        assert_eq!(super::stored_path(r"\\?\UNC\nas\music"), "//nas/music");
        assert_eq!(super::stored_path(r"D:\x"), "D:/x");
    }

    #[test]
    #[cfg(not(windows))]
    fn stored_paths_stay_on_unix() {
        assert_eq!(super::stored_path("/home/a/b\\c.flac"), "/home/a/b\\c.flac");
    }
}
