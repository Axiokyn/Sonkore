//! Windows: the notification-area icon (Shell_NotifyIcon).
//!
//! The menu, in order: Play/Pause (its text follows the player), Next,
//! Previous, a separator, Sonkore (raise the window) and Quit. A left click
//! (or Enter on the focused icon) is [`TrayCommand::Activate`].
//!
//! The icon lives on a hidden window's thread; the UI thread only writes
//! shared state. The menu is built from it when it opens, so
//! [`Tray::set_playing`] and [`Tray::set_labels`] cost a lock; a new tooltip
//! also posts one message.

use std::io;
use std::sync::{Arc, Mutex, MutexGuard, mpsc};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NIN_SELECT, NINF_KEY,
    NOTIFY_ICON_DATA_FLAGS, NOTIFYICON_VERSION_4, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, ChangeWindowMessageFilterEx, CreateIcon, CreatePopupMenu, DestroyIcon, DestroyMenu, GetSystemMetrics,
    HICON, MF_SEPARATOR, MF_STRING, MSGFLT_ALLOW, PostMessageW, RegisterWindowMessageW, SM_CXSMICON,
    SetForegroundWindow, TPM_LAYOUTRTL, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, WM_CONTEXTMENU,
    WM_DESTROY, WM_NULL,
};
use windows::core::{HSTRING, PCWSTR, w};

use crate::bus::Bus;
pub use crate::media::{TrayCommand, TrayLabels};
use crate::win_shared::{TITLE, callback_anchor, icon_bgra, icon_source, menu_command, menu_entries, tooltip_units};
use crate::window::{HiddenWindow, MSG_USER};

/// The icon's callback message.
const MSG_ICON: u32 = MSG_USER;
/// The tooltip changed.
const MSG_TIP: u32 = MSG_USER + 1;
/// Enter or Space on the focused icon.
const NIN_KEYSELECT: u32 = NIN_SELECT | NINF_KEY;
/// The icon's id within the window (one icon).
const ICON_ID: u32 = 1;

/// The app icon's PNGs (as the Linux tray's pixmaps), by size.
const PNGS: [(u32, &[u8]); 6] = [
    (16, include_bytes!("../../../../packaging/icons/png/16.png")),
    (22, include_bytes!("../../../../packaging/icons/png/22.png")),
    (24, include_bytes!("../../../../packaging/icons/png/24.png")),
    (32, include_bytes!("../../../../packaging/icons/png/32.png")),
    (48, include_bytes!("../../../../packaging/icons/png/48.png")),
    (64, include_bytes!("../../../../packaging/icons/png/64.png")),
];

struct State {
    labels: TrayLabels,
    playing: bool,
    tooltip: String,
}

type Shared = Arc<Mutex<State>>;

fn lock(shared: &Shared) -> MutexGuard<'_, State> {
    shared.lock().unwrap_or_else(|e| e.into_inner())
}

pub struct Tray {
    shared: Shared,
    window: HiddenWindow,
}

impl Tray {
    /// The notification area is always there on Windows.
    pub fn available(_bus: &Bus) -> bool {
        true
    }

    /// Puts the icon in the notification area; the commands arrive on the
    /// receiver (poll it from the UI thread). Explorer not running yet is no
    /// error: the icon is added when its taskbar appears.
    pub fn start(_bus: Bus, labels: TrayLabels) -> io::Result<(Tray, mpsc::Receiver<TrayCommand>)> {
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Mutex::new(State { labels, playing: false, tooltip: TITLE.into() }));
        let window = {
            let shared = Arc::clone(&shared);
            HiddenWindow::spawn("sonkore-tray", move |hwnd| {
                let icon = Icon::new(hwnd, shared, tx);
                icon.add();
                Box::new(move |hwnd, msg, wparam, lparam| icon.handle(hwnd, msg, wparam, lparam))
            })?
        };
        Ok((Tray { shared, window }, rx))
    }

    /// The menu's first entry reads Pause while playing, else Play.
    pub fn set_playing(&self, playing: bool) {
        lock(&self.shared).playing = playing;
    }

    /// New texts (the interface language changed).
    pub fn set_labels(&self, labels: TrayLabels) {
        lock(&self.shared).labels = labels;
    }

    /// The tooltip (the app uses the window title, "Artist — Title").
    pub fn set_tooltip(&self, text: &str) {
        {
            let mut s = lock(&self.shared);
            if s.tooltip == text {
                return;
            }
            s.tooltip = text.to_string();
        }
        self.window.post(MSG_TIP, 0, 0);
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        // The handler takes the icon off on WM_DESTROY.
        self.window.close();
    }
}

/// The icon, on the window's thread.
struct Icon {
    hwnd: HWND,
    hicon: HICON,
    /// Explorer's "TaskbarCreated": it (re)started, the icon must be added.
    taskbar_created: u32,
    shared: Shared,
    commands: mpsc::Sender<TrayCommand>,
}

impl Icon {
    fn new(hwnd: HWND, shared: Shared, commands: mpsc::Sender<TrayCommand>) -> Icon {
        // SAFETY: a constant string; returns 0 on failure, which no message
        // is.
        let taskbar_created = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
        if taskbar_created != 0 {
            // An elevated Sonkore would not hear Explorer's broadcast (UIPI).
            // SAFETY: our own window.
            let _ = unsafe { ChangeWindowMessageFilterEx(hwnd, taskbar_created, MSGFLT_ALLOW, None) };
        }
        Icon { hwnd, hicon: make_icon(), taskbar_created, shared, commands }
    }

    fn data(&self, flags: NOTIFY_ICON_DATA_FLAGS) -> NOTIFYICONDATAW {
        let mut data = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: ICON_ID,
            uFlags: flags,
            uCallbackMessage: MSG_ICON,
            hIcon: self.hicon,
            ..Default::default()
        };
        let tip = tooltip_units(&lock(&self.shared).tooltip);
        data.szTip[..tip.len()].copy_from_slice(&tip);
        data
    }

    fn add(&self) {
        // NIF_SHOWTIP: with version 4 the standard tooltip only shows when
        // asked for.
        let mut data = self.data(NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP);
        // SAFETY: `data` is a complete NOTIFYICONDATAW for our window.
        unsafe {
            if Shell_NotifyIconW(NIM_ADD, &data).as_bool() {
                data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
                let _ = Shell_NotifyIconW(NIM_SETVERSION, &data);
            }
        }
    }

    fn handle(&self, hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
        match msg {
            MSG_ICON => {
                // Version 4: the event in LOWORD(lParam), the anchor in wParam.
                match lparam.0 as u32 & 0xFFFF {
                    NIN_SELECT | NIN_KEYSELECT => self.send(TrayCommand::Activate),
                    WM_CONTEXTMENU => {
                        let (x, y) = callback_anchor(wparam.0);
                        self.menu(hwnd, x, y);
                    }
                    _ => {}
                }
                Some(LRESULT(0))
            }
            MSG_TIP => {
                let data = self.data(NIF_TIP | NIF_SHOWTIP);
                // SAFETY: as in `add`.
                let _ = unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) };
                Some(LRESULT(0))
            }
            WM_DESTROY => {
                let data = self.data(NOTIFY_ICON_DATA_FLAGS(0));
                // SAFETY: as in `add`.
                let _ = unsafe { Shell_NotifyIconW(NIM_DELETE, &data) };
                None
            }
            m if m == self.taskbar_created && m != 0 => {
                self.add();
                Some(LRESULT(0))
            }
            _ => None,
        }
    }

    fn send(&self, command: TrayCommand) {
        let _ = self.commands.send(command);
    }

    fn menu(&self, hwnd: HWND, x: i32, y: i32) {
        let (entries, rtl) = {
            let s = lock(&self.shared);
            (menu_entries(&s.labels, s.playing), s.labels.rtl)
        };
        // SAFETY: a menu we create, fill, show and destroy here, owned by
        // our own window.
        let chosen = unsafe {
            let Ok(menu) = CreatePopupMenu() else {
                return;
            };
            for entry in &entries {
                let _ = match entry {
                    Some((id, text)) => AppendMenuW(menu, MF_STRING, *id as usize, &HSTRING::from(text.as_str())),
                    None => AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null()),
                };
            }
            // Without the foreground the menu would not close on a click
            // elsewhere; the WM_NULL after it is the documented workaround
            // for the menu's second opening (KB135788).
            let _ = SetForegroundWindow(hwnd);
            let mut flags = TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON;
            if rtl {
                flags |= TPM_LAYOUTRTL;
            }
            let chosen = TrackPopupMenu(menu, flags, x, y, None, hwnd, None).0;
            let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
            let _ = DestroyMenu(menu);
            chosen
        };
        if let Some(command) = u32::try_from(chosen).ok().and_then(menu_command) {
            self.send(command);
        }
    }
}

impl Drop for Icon {
    fn drop(&mut self) {
        if !self.hicon.is_invalid() {
            // SAFETY: an icon we made, no longer in the notification area
            // (WM_DESTROY removed it before the handler is dropped).
            let _ = unsafe { DestroyIcon(self.hicon) };
        }
    }
}

/// The app icon at the small-icon size (16 px at 100 %, 24 at 150 %…),
/// scaled from the nearest PNG at or above it. A null icon when it can't be
/// made: Windows then shows an empty slot, still clickable.
fn make_icon() -> HICON {
    // SAFETY: no arguments to get wrong.
    let size = unsafe { GetSystemMetrics(SM_CXSMICON) }.clamp(16, 256) as u32;
    let sizes = PNGS.map(|(s, _)| s);
    let Some(bgra) = icon_source(&sizes, size).and_then(|i| icon_bgra(PNGS[i].1, size)) else {
        return HICON::default();
    };
    // Fully transparent AND mask: the colour bits' alpha decides.
    let mask = vec![0u8; (size as usize).div_ceil(16) * 2 * size as usize];
    // SAFETY: `bgra` holds size×size 32-bit pixels and `mask` size rows of
    // 1-bit pixels padded to 16 bits, as CreateIcon reads them.
    unsafe { CreateIcon(None, size as i32, size as i32, 1, 32, mask.as_ptr(), bgra.as_ptr()) }.unwrap_or_default()
}
