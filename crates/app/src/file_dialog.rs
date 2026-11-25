//! File and folder dialogs: the desktop's own. On Linux through the XDG
//! desktop portal (org.freedesktop.portal.FileChooser),
//! else zenity or kdialog; on Windows the system's common item dialog. Each
//! runs on a thread of its own; the answer comes back on the interface's
//! thread.

use std::collections::HashMap;
#[cfg(target_os = "linux")]
use std::process::Command;

#[cfg(target_os = "linux")]
use zbus::MatchRule;
#[cfg(target_os = "linux")]
use zbus::blocking::{Connection, MessageIterator};
#[cfg(target_os = "linux")]
use zbus::zvariant::{OwnedValue, Value};

/// What to ask for.
#[derive(Clone, Debug, Default)]
pub struct Request {
    pub title: String,
    /// A folder rather than files.
    pub directory: bool,
    pub multiple: bool,
    /// Save under a name (the suggested name).
    pub save_name: Option<String>,
    /// (name, patterns like "*.m3u") shown as the dialog's filters.
    pub filters: Vec<(String, Vec<String>)>,
}

type Done = Box<dyn FnOnce(Vec<String>)>;

thread_local! {
    /// The answers awaited, on the interface's thread.
    static PENDING: std::cell::RefCell<(u64, HashMap<u64, Done>)> = Default::default();
}

/// Asks, then calls `done` on the interface's thread (where this is called)
/// with the chosen paths (empty when cancelled).
pub fn ask(request: Request, done: impl FnOnce(Vec<String>) + 'static) {
    let id = PENDING.with(|p| {
        let mut p = p.borrow_mut();
        p.0 += 1;
        let id = p.0;
        p.1.insert(id, Box::new(done));
        id
    });
    std::thread::spawn(move || {
        #[cfg(target_os = "linux")]
        let chosen = portal(&request).unwrap_or_else(|| fallback(&request));
        #[cfg(windows)]
        let chosen = windows_dialog(&request);
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(done) = PENDING.with(|p| p.borrow_mut().1.remove(&id)) {
                done(chosen);
            }
        });
    });
}

/// The paths the portal answered. A file to save keeps the document
/// portal's path (`/run/user/<uid>/doc/<id>/<name>`): in the Flatpak the real
/// one is read-only (`--filesystem=host:ro`), and overwriting a file that
/// already existed failed. What is opened goes to the real path (`to_host`),
/// so that an M3U's relative entries start from its true folder.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn chosen_paths(uris: &[String], save: bool, to_host: impl Fn(&str) -> String) -> Vec<String> {
    uris.iter()
        .map(|u| match sonkore_platform::url::to_local_file(u) {
            Some(path) if save => path.to_string_lossy().into_owned(),
            _ => to_host(u),
        })
        .collect()
}

/// The portal's dialog; None when there is no portal to ask.
#[cfg(target_os = "linux")]
fn portal(request: &Request) -> Option<Vec<String>> {
    let conn = Connection::session().ok()?;
    let token = format!(
        "sonkore{}",
        std::process::id() ^ (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?.subsec_nanos())
    );
    let sender = conn.unique_name()?.trim_start_matches(':').replace('.', "_");
    let handle = format!("/org/freedesktop/portal/desktop/request/{sender}/{token}");
    // Listen before asking: the answer may come before the call returns.
    let rule = MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface("org.freedesktop.portal.Request")
        .ok()?
        .member("Response")
        .ok()?
        .path(handle.as_str())
        .ok()?
        .build();
    let mut answers = MessageIterator::for_match_rule(rule, &conn, Some(4)).ok()?;

    let mut options: HashMap<&str, Value> = HashMap::new();
    options.insert("handle_token", Value::from(token.as_str()));
    options.insert("modal", Value::from(true));
    let filters: Vec<(String, Vec<(u32, String)>)> = request
        .filters
        .iter()
        .map(|(name, patterns)| (name.clone(), patterns.iter().map(|p| (0u32, p.clone())).collect()))
        .collect();
    if !filters.is_empty() {
        options.insert("filters", Value::from(filters));
    }
    let method = if let Some(name) = &request.save_name {
        options.insert("current_name", Value::from(name.as_str()));
        "SaveFile"
    } else {
        options.insert("multiple", Value::from(request.multiple));
        options.insert("directory", Value::from(request.directory));
        "OpenFile"
    };
    conn.call_method(
        Some("org.freedesktop.portal.Desktop"),
        "/org/freedesktop/portal/desktop",
        Some("org.freedesktop.portal.FileChooser"),
        method,
        &("", request.title.as_str(), options),
    )
    .ok()?;
    let message = answers.next()?.ok()?;
    let (response, results): (u32, HashMap<String, OwnedValue>) = message.body().deserialize().ok()?;
    if response != 0 {
        return Some(Vec::new()); // cancelled
    }
    let uris: Vec<String> =
        results.get("uris").and_then(|v| Vec::<String>::try_from(v.clone()).ok()).unwrap_or_default();
    Some(chosen_paths(&uris, request.save_name.is_some(), sonkore_platform::portal::local_path))
}

/// zenity, else kdialog.
#[cfg(target_os = "linux")]
fn fallback(request: &Request) -> Vec<String> {
    let patterns: Vec<String> = request.filters.iter().map(|(_, p)| p.join(" ")).collect();
    let mut zenity = Command::new("zenity");
    zenity.args(["--file-selection", "--title", &request.title]);
    if request.directory {
        zenity.arg("--directory");
    }
    if request.multiple {
        zenity.args(["--multiple", "--separator", "\n"]);
    }
    if let Some(name) = &request.save_name {
        zenity.args(["--save", "--confirm-overwrite", "--filename", name]);
    }
    for p in &patterns {
        zenity.arg(format!("--file-filter={p}"));
    }
    let out = zenity.output().ok().filter(|o| o.status.code().is_some_and(|c| c == 0 || c == 1)).or_else(|| {
        let mut kdialog = Command::new("kdialog");
        kdialog.args(["--title", &request.title]);
        let filter = patterns.join(" ");
        if request.directory {
            kdialog.args(["--getexistingdirectory", "."]);
        } else if let Some(name) = &request.save_name {
            kdialog.args(["--getsavefilename", name, &filter]);
        } else {
            if request.multiple {
                kdialog.args(["--multiple", "--separate-output"]);
            }
            kdialog.args(["--getopenfilename", ".", &filter]);
        }
        kdialog.output().ok()
    });
    out.filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// The filter of a dialog on Windows: "*.m3u8;*.m3u" (the patterns as one).
#[cfg_attr(not(windows), allow(dead_code))]
fn windows_spec(patterns: &[String]) -> String {
    patterns.join(";")
}

/// The extension a saved file gets when its name has none: the first
/// pattern's ("*.m3u8" -> "m3u8").
#[cfg_attr(not(windows), allow(dead_code))]
fn default_extension(request: &Request) -> Option<String> {
    let first = request.filters.first()?.1.first()?;
    let ext = first.strip_prefix("*.")?;
    (!ext.is_empty() && !ext.contains(['*', '?'])).then(|| ext.to_owned())
}

/// Windows' common item dialog (IFileOpenDialog / IFileSaveDialog), on this
/// thread in its own COM apartment. Paths with forward slashes ("C:/Music").
#[cfg(windows)]
fn windows_dialog(request: &Request) -> Vec<String> {
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoCreateInstance, CoInitializeEx,
        CoTaskMemFree, CoUninitialize,
    };
    use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
    use windows::Win32::UI::Shell::{
        FOS_ALLOWMULTISELECT, FOS_FORCEFILESYSTEM, FOS_OVERWRITEPROMPT, FOS_PICKFOLDERS, FileOpenDialog,
        FileSaveDialog, IFileDialog, IFileOpenDialog, IFileSaveDialog, IShellItem, SIGDN_FILESYSPATH,
    };
    use windows::core::{HSTRING, Interface, PCWSTR};

    fn path_of(item: &IShellItem) -> windows::core::Result<String> {
        // SAFETY: GetDisplayName hands over a CoTaskMem string, freed here.
        unsafe {
            let name = item.GetDisplayName(SIGDN_FILESYSPATH)?;
            let text = name.to_string();
            CoTaskMemFree(Some(name.0 as *const _));
            Ok(sonkore_core::paths::stored_path(&text.map_err(|_| windows::core::Error::empty())?))
        }
    }

    // SAFETY: COM calls on this thread's own apartment; every string handed
    // to the dialog outlives the call that reads it.
    unsafe {
        let initialised = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE).is_ok();
        let chosen = (|| -> windows::core::Result<Vec<String>> {
            let save = request.save_name.is_some();
            let dialog: IFileDialog = if save {
                CoCreateInstance::<_, IFileSaveDialog>(&FileSaveDialog, None, CLSCTX_INPROC_SERVER)?.cast()?
            } else {
                CoCreateInstance::<_, IFileOpenDialog>(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?.cast()?
            };
            let mut options = dialog.GetOptions()? | FOS_FORCEFILESYSTEM;
            if request.directory {
                options |= FOS_PICKFOLDERS;
            }
            if request.multiple && !save {
                options |= FOS_ALLOWMULTISELECT;
            }
            if save {
                options |= FOS_OVERWRITEPROMPT;
            }
            dialog.SetOptions(options)?;
            if !request.title.is_empty() {
                dialog.SetTitle(&HSTRING::from(request.title.as_str()))?;
            }
            let names: Vec<HSTRING> = request.filters.iter().map(|(n, _)| HSTRING::from(n.as_str())).collect();
            let specs: Vec<HSTRING> = request.filters.iter().map(|(_, p)| HSTRING::from(windows_spec(p))).collect();
            let filters: Vec<COMDLG_FILTERSPEC> = names
                .iter()
                .zip(&specs)
                .map(|(n, s)| COMDLG_FILTERSPEC { pszName: PCWSTR(n.as_ptr()), pszSpec: PCWSTR(s.as_ptr()) })
                .collect();
            if !filters.is_empty() && !request.directory {
                dialog.SetFileTypes(&filters)?;
            }
            if let Some(ext) = default_extension(request) {
                dialog.SetDefaultExtension(&HSTRING::from(ext))?;
            }
            if let Some(name) = &request.save_name {
                dialog.SetFileName(&HSTRING::from(name.as_str()))?;
            }
            if dialog.Show(None).is_err() {
                return Ok(Vec::new()); // cancelled (or no dialog to show)
            }
            if request.multiple && !save {
                let open: IFileOpenDialog = dialog.cast()?;
                let items = open.GetResults()?;
                let mut paths = Vec::new();
                for i in 0..items.GetCount()? {
                    paths.push(path_of(&items.GetItemAt(i)?)?);
                }
                Ok(paths)
            } else {
                Ok(vec![path_of(&dialog.GetResult()?)?])
            }
        })();
        if initialised {
            CoUninitialize();
        }
        chosen.unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_filters_and_extension() {
        let r = Request { filters: vec![("M3U".into(), vec!["*.m3u8".into(), "*.m3u".into()])], ..Default::default() };
        assert_eq!(windows_spec(&r.filters[0].1), "*.m3u8;*.m3u");
        assert_eq!(default_extension(&r).as_deref(), Some("m3u8"));
        assert_eq!(default_extension(&Request::default()), None);
    }

    #[test]
    fn a_file_to_save_keeps_the_portals_writable_path() {
        // The portal knows the real file: /home/u/list.m3u8, read-only in the Flatpak.
        let to_host = |u: &str| u.replace("file:///run/user/1000/doc/ab12", "/home/u");
        let uris = vec!["file:///run/user/1000/doc/ab12/my%20list.m3u8".to_string()];
        assert_eq!(chosen_paths(&uris, true, to_host), vec!["/run/user/1000/doc/ab12/my list.m3u8"]);
        assert_eq!(chosen_paths(&uris, false, to_host), vec!["/home/u/my%20list.m3u8"]);
        let plain = vec!["file:///home/u/a.m3u8".to_string()];
        assert_eq!(chosen_paths(&plain, true, to_host), vec!["/home/u/a.m3u8"]);
    }
}
