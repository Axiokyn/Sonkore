//! For tests that drive the real window (tools/ui-live.py):
//! SONKORE_STATE_FILE=path makes the app keep there, as `key=value` lines,
//! what it is doing — rewritten (atomically) whenever it changes.

use std::cell::RefCell;

thread_local! {
    static LAST: RefCell<String> = const { RefCell::new(String::new()) };
}

pub fn enabled() -> bool {
    std::env::var_os("SONKORE_STATE_FILE").is_some()
}

/// Writes `lines` if they differ from the last ones written.
pub fn write(lines: &[(&str, String)]) {
    let Some(path) = std::env::var_os("SONKORE_STATE_FILE") else { return };
    let text: String = lines.iter().map(|(k, v)| format!("{k}={}\n", v.replace('\n', " "))).collect();
    let changed = LAST.with(|last| {
        let mut last = last.borrow_mut();
        if *last == text {
            return false;
        }
        *last = text.clone();
        true
    });
    if changed {
        let tmp = std::path::PathBuf::from(format!("{}.tmp", path.to_string_lossy()));
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}
