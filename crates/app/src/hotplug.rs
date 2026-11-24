//! Sound devices plugged in or out. On Linux, a watch on /dev/snd: a USB DAC's
//! nodes appear one by one and PipeWire publishes its sink a moment later, so
//! the outputs are listed again 1.5 s after the last change, and what came or
//! went is reported. SONKORE_DEV_SND stands in for /dev/snd in tests.

use std::cell::{Cell, RefCell};

use sonkore_engine::output::DeviceInfo;

/// How long the folder has to stay quiet before the outputs are listed.
#[cfg(target_os = "linux")]
const SETTLE: std::time::Duration = std::time::Duration::from_millis(1500);

type Report = Box<dyn Fn(&[String], &[String])>;

thread_local! {
    static REPORT: RefCell<Option<Report>> = const { RefCell::new(None) };
    static PENDING: Cell<bool> = const { Cell::new(false) };
    #[cfg(target_os = "linux")]
    static SETTLE_TIMER: slint::Timer = slint::Timer::default();
    #[cfg(target_os = "linux")]
    static WATCHER: RefCell<Option<notify::RecommendedWatcher>> = const { RefCell::new(None) };
}

/// The outputs `after` has and `before` had not, and the other way round, by
/// name ("System default", which has no key, never counts).
pub fn changes(before: &[DeviceInfo], after: &[DeviceInfo]) -> (Vec<String>, Vec<String>) {
    let names = |list: &[DeviceInfo]| {
        let mut out: Vec<String> = Vec::new();
        for d in list.iter().filter(|d| !d.key.is_empty()) {
            if !out.contains(&d.name) {
                out.push(d.name.clone());
            }
        }
        out
    };
    let (was, is) = (names(before), names(after));
    let added = is.iter().filter(|n| !was.contains(n)).cloned().collect();
    let removed = was.iter().filter(|n| !is.contains(n)).cloned().collect();
    (added, removed)
}

/// Starts watching; `report(added, removed)` runs on the interface thread
/// when the outputs changed. Nothing happens where there is nothing to watch.
pub fn watch(report: impl Fn(&[String], &[String]) + 'static) {
    REPORT.with(|r| *r.borrow_mut() = Some(Box::new(report)));
    #[cfg(target_os = "linux")]
    {
        use notify::Watcher;
        let dir = std::env::var_os("SONKORE_DEV_SND").map_or_else(|| std::path::PathBuf::from("/dev/snd"), Into::into);
        if !dir.is_dir() {
            return;
        }
        let watcher = notify::recommended_watcher(|event: notify::Result<notify::Event>| {
            if event.is_ok() {
                let _ = slint::invoke_from_event_loop(changed);
            }
        });
        let Ok(mut watcher) = watcher else { return };
        if watcher.watch(&dir, notify::RecursiveMode::NonRecursive).is_ok() {
            WATCHER.with(|w| *w.borrow_mut() = Some(watcher));
        }
    }
}

/// Something moved in the folder: list the outputs once it settles.
#[cfg(target_os = "linux")]
fn changed() {
    SETTLE_TIMER.with(|t| {
        t.start(slint::TimerMode::SingleShot, SETTLE, || {
            PENDING.with(|p| p.set(true));
            crate::settings_ui::list_devices();
        })
    });
}

/// The outputs were listed again (settings_ui::list_devices): reported when
/// the listing came from a change here and there was a list to compare with.
pub fn listed(before: Option<&[DeviceInfo]>, after: &[DeviceInfo]) {
    if !PENDING.with(|p| p.replace(false)) {
        return;
    }
    let Some(before) = before else { return };
    let (added, removed) = changes(before, after);
    if added.is_empty() && removed.is_empty() {
        return;
    }
    REPORT.with(|r| {
        if let Some(report) = r.borrow().as_ref() {
            report(&added, &removed);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(key: &str, name: &str) -> DeviceInfo {
        DeviceInfo {
            key: key.into(),
            backend: String::new(),
            name: name.into(),
            detail: String::new(),
            is_default: false,
            direct: false,
            alsa_card: -1,
        }
    }

    #[test]
    fn changes_by_name_without_the_system_default() {
        let before =
            [device("", "System default"), device("alsa|hw:CARD=NVidia,DEV=3", "HDMI 0"), device("pipewire|a", "DX1")];
        let after = [
            device("", "System default"),
            device("alsa|hw:CARD=NVidia,DEV=3", "HDMI 0"),
            device("alsa|hw:CARD=E30,DEV=0", "E30"),
            device("pipewire|b", "E30"),
        ];
        assert_eq!(changes(&before, &after), (vec!["E30".to_string()], vec!["DX1".to_string()]));
        assert_eq!(changes(&after, &after), (vec![], vec![]));
    }
}
