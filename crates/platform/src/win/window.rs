//! A hidden window on its own thread, for the messages the tray icon and
//! the media controls need. A top-level window, never shown, not a
//! message-only one: those miss broadcasts such as "TaskbarCreated" (the
//! icon must come back when Explorer restarts).
//!
//! One window per thread: the thread runs its message loop and owns the
//! handler, so the handler may hold what can't leave the thread.

use std::cell::RefCell;
use std::io;
use std::rc::Rc;
use std::sync::mpsc;
use std::thread::JoinHandle;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, MSG, PostMessageW, PostQuitMessage,
    RegisterClassExW, TranslateMessage, WINDOW_EX_STYLE, WM_APP, WM_DESTROY, WNDCLASSEXW, WS_OVERLAPPED,
};
use windows::core::{PCWSTR, w};

/// What the window does with a message: Some(result), or None for the
/// default handling. Also called with WM_DESTROY, the last chance to clean
/// up while the window exists. It may be re-entered (a popup menu runs its
/// own loop), hence `Fn`.
pub(crate) type Handler = Box<dyn Fn(HWND, u32, WPARAM, LPARAM) -> Option<LRESULT>>;

/// Destroys the window (and so ends its thread).
const MSG_CLOSE: u32 = WM_APP;
/// The first message free for handlers.
pub(crate) const MSG_USER: u32 = WM_APP + 1;

const CLASS: PCWSTR = w!("SonkoreHiddenWindow");

thread_local! {
    /// The handler of this thread's window (one window per thread).
    static HANDLER: RefCell<Option<Rc<Handler>>> = const { RefCell::new(None) };
}

pub(crate) struct HiddenWindow {
    /// The HWND as a number: a window handle may be used from any thread
    /// for posting, but `HWND` itself isn't Send.
    hwnd: usize,
    thread: Option<JoinHandle<()>>,
}

impl HiddenWindow {
    /// Creates the window on a new thread named `name`; `make` runs there
    /// with the window and returns its handler. Returns once the window
    /// exists.
    pub fn spawn(name: &str, make: impl FnOnce(HWND) -> Handler + Send + 'static) -> io::Result<HiddenWindow> {
        let (tx, rx) = mpsc::sync_channel(1);
        let thread = std::thread::Builder::new().name(name.into()).spawn(move || {
            let hwnd = match create() {
                Ok(hwnd) => hwnd,
                Err(e) => {
                    let _ = tx.send(Err(e));
                    return;
                }
            };
            let handler = Rc::new(make(hwnd));
            HANDLER.with(|h| *h.borrow_mut() = Some(handler));
            let _ = tx.send(Ok(hwnd.0 as usize));
            run();
            // Gone with the loop (WM_DESTROY ended it); anything still
            // dispatched gets the default handling.
            let handler = HANDLER.with(|h| h.borrow_mut().take());
            drop(handler);
        })?;
        match rx.recv() {
            Ok(Ok(hwnd)) => Ok(HiddenWindow { hwnd, thread: Some(thread) }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => {
                let _ = thread.join();
                Err(io::Error::other("the window thread ended"))
            }
        }
    }

    fn hwnd(&self) -> HWND {
        HWND(self.hwnd as *mut _)
    }

    /// Queues a message for the handler; false once the window is gone.
    pub fn post(&self, msg: u32, wparam: usize, lparam: isize) -> bool {
        // SAFETY: posting to a window of another thread is what PostMessage
        // is for; a stale handle only makes it fail.
        unsafe { PostMessageW(Some(self.hwnd()), msg, WPARAM(wparam), LPARAM(lparam)).is_ok() }
    }

    /// Destroys the window and waits for its thread.
    pub fn close(&mut self) {
        let Some(thread) = self.thread.take() else {
            return;
        };
        self.post(MSG_CLOSE, 0, 0);
        let _ = thread.join();
    }
}

impl Drop for HiddenWindow {
    fn drop(&mut self) {
        self.close();
    }
}

fn create() -> io::Result<HWND> {
    // SAFETY: plain Win32 calls with valid arguments; the class name and
    // window procedure are 'static.
    unsafe {
        let instance = GetModuleHandleW(None).map_err(io::Error::other)?;
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        // Fails harmlessly once registered (the second window); creating
        // the window tells whether the class is there.
        RegisterClassExW(&class);
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            CLASS,
            w!("Sonkore"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .map_err(io::Error::other)
    }
}

fn run() {
    let mut msg = MSG::default();
    // SAFETY: the standard loop on this thread's own queue; `msg` outlives
    // each call. GetMessage returns 0 on WM_QUIT and -1 on error: both end.
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == MSG_CLOSE {
        // SAFETY: our own window, on its own thread.
        let _ = unsafe { DestroyWindow(hwnd) };
        return LRESULT(0);
    }
    // Cloned out so the borrow ends before the call: the handler may be
    // re-entered (TrackPopupMenu dispatches while it runs).
    let handler = HANDLER.with(|h| h.borrow().clone());
    let result = handler.and_then(|h| h(hwnd, msg, wparam, lparam));
    if msg == WM_DESTROY {
        // SAFETY: ends this thread's loop, which only serves this window.
        unsafe { PostQuitMessage(0) };
    }
    // SAFETY: the arguments are the ones the system passed in.
    result.unwrap_or_else(|| unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) })
}
