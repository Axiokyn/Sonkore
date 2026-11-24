//! Windows: the single instance and the system's media controls, what MPRIS
//! is on Linux.
//!
//! The single instance is a per-user named pipe (`win_shared::pipe_name`):
//! the first Sonkore serves it, a second launch connects and writes lines of
//! UTF-8 (`open <path>`, `raise`, `call PlayPause`), one connection per
//! launch. It runs whether or not the media controls are on.
//!
//! The media controls are the System Media Transport Controls of a hidden
//! window: the volume flyout's player, the lock screen and the keyboard's
//! media keys (also with Sonkore in the background). The app pushes the
//! player's state with [`Mpris::update`] every tick; only what the controls
//! show changing (or a seek) reaches them. What they ask for arrives as
//! [`Command`]s on a system thread.

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::Foundation::{TimeSpan, TypedEventHandler};
use windows::Media::{
    AutoRepeatModeChangeRequestedEventArgs, MediaPlaybackAutoRepeatMode, MediaPlaybackStatus, MediaPlaybackType,
    PlaybackPositionChangeRequestedEventArgs, ShuffleEnabledChangeRequestedEventArgs, SystemMediaTransportControls,
    SystemMediaTransportControlsButton, SystemMediaTransportControlsButtonPressedEventArgs,
    SystemMediaTransportControlsTimelineProperties,
};
use windows::Win32::Foundation::{
    ERROR_IO_PENDING, ERROR_NO_DATA, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, GENERIC_WRITE, HANDLE, HWND, LRESULT,
    WAIT_OBJECT_0,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, FILE_SHARE_NONE, FlushFileBuffers, OPEN_EXISTING,
    PIPE_ACCESS_INBOUND, ReadFile, SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT, WriteFile,
};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeServerProcessId, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT, WaitNamedPipeW,
};
use windows::Win32::System::Threading::{CreateEventW, INFINITE, SetEvent, WaitForMultipleObjects};
use windows::Win32::System::WinRT::ISystemMediaTransportControlsInterop;
use windows::Win32::UI::WindowsAndMessaging::{AllowSetForegroundWindow, WM_DESTROY};
use windows::core::{HSTRING, Owned};

use crate::bus::Bus;
pub use crate::media::{Command, PlaybackStatus, PlayerState, Repeat, TrackInfo, player_command, seek_command};
use crate::url::absolute;
use crate::win_shared::{call_message, media_outdated, open_message, parse_message, pipe_name};
use crate::window::{HiddenWindow, MSG_USER};

type CommandFn = Arc<dyn Fn(Command) + Send + Sync>;

// --- The client: a second launch ---------------------------------------------------

/// Sends files to an already running instance (then raises it); true if
/// one took them.
pub fn forward_files<P: AsRef<Path>>(_bus: &Bus, paths: &[P]) -> bool {
    let paths: Vec<String> = paths.iter().map(|p| absolute(p.as_ref()).to_string_lossy().into_owned()).collect();
    send(&open_message(&paths))
}

/// Sends a player method (PlayPause, Next…) to a running instance; true if
/// one took it.
pub fn send_command(_bus: &Bus, method: &str) -> bool {
    send(&call_message(method))
}

fn own_pipe() -> HSTRING {
    let user = std::env::var("USERNAME").unwrap_or_default();
    HSTRING::from(pipe_name(&user))
}

/// Writes `message` to the running instance's pipe; true once it was read.
fn send(message: &str) -> bool {
    let name = own_pipe();
    for _ in 0..3 {
        // Identification only: a pipe squatted by someone else can't act as
        // this user.
        // SAFETY: a valid name; the handle is owned at once.
        let opened = unsafe {
            CreateFileW(
                &name,
                GENERIC_WRITE.0,
                FILE_SHARE_NONE,
                None,
                OPEN_EXISTING,
                SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                None,
            )
        };
        let pipe = match opened {
            // SAFETY: a handle just opened, closed once.
            Ok(h) => unsafe { Owned::new(h) },
            Err(e) if e.code() == ERROR_PIPE_BUSY.to_hresult() => {
                // Every instance taken (another launch writing): wait a bit.
                // SAFETY: a valid name.
                let _ = unsafe { WaitNamedPipeW(&name, 2000) };
                continue;
            }
            Err(_) => return false,
        };
        // The running instance raises its window: let it take the
        // foreground, which this process (just launched by the user) holds.
        let mut pid = 0;
        // SAFETY: an open pipe handle and a valid out pointer.
        if unsafe { GetNamedPipeServerProcessId(*pipe, &mut pid) }.is_ok() {
            // SAFETY: no pointers.
            let _ = unsafe { AllowSetForegroundWindow(pid) };
        }
        // SAFETY: a synchronous write of a live buffer on an open handle;
        // the flush returns once the server has read it all.
        return unsafe { write_all(*pipe, message.as_bytes()) && FlushFileBuffers(*pipe).is_ok() };
    }
    false
}

/// # Safety
/// `pipe` is an open handle without FILE_FLAG_OVERLAPPED.
unsafe fn write_all(pipe: HANDLE, mut bytes: &[u8]) -> bool {
    while !bytes.is_empty() {
        let mut written = 0u32;
        // SAFETY: as the caller promises; `written` outlives the call.
        if unsafe { WriteFile(pipe, Some(bytes), Some(&mut written), None) }.is_err() || written == 0 {
            return false;
        }
        bytes = &bytes[written as usize..];
    }
    true
}

// --- The server: the running instance ------------------------------------------------------

/// A manual-reset event shared across threads.
struct Event(HANDLE);

// SAFETY: a kernel event handle may be waited on and set from any thread.
unsafe impl Send for Event {}
// SAFETY: as above; nothing in it is mutated through `&`.
unsafe impl Sync for Event {}

impl Event {
    fn new() -> Option<Event> {
        // SAFETY: no pointers; an unnamed event.
        unsafe { CreateEventW(None, true, false, None) }.ok().map(Event)
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        // SAFETY: an event we created, closed once.
        drop(unsafe { Owned::new(self.0) });
    }
}

struct Server {
    stop: Arc<Event>,
    thread: Option<JoinHandle<()>>,
}

impl Server {
    fn start(on_command: CommandFn) -> Option<Server> {
        let stop = Arc::new(Event::new()?);
        let thread = {
            let stop = Arc::clone(&stop);
            std::thread::Builder::new()
                .name("sonkore-instance".into())
                .spawn(move || serve(&own_pipe(), &stop, &on_command))
                .ok()?
        };
        Some(Server { stop, thread: Some(thread) })
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // SAFETY: our event, alive until the thread is joined.
        let _ = unsafe { SetEvent(self.stop.0) };
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// A pipe instance; the first one fails when another process (another
/// Sonkore) already serves the name.
fn create_pipe(name: &HSTRING, first: bool) -> Option<Owned<HANDLE>> {
    let mut mode = PIPE_ACCESS_INBOUND | FILE_FLAG_OVERLAPPED;
    if first {
        mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
    }
    // SAFETY: a valid name; default security (only this user, the
    // administrators and SYSTEM may write).
    let pipe = unsafe {
        CreateNamedPipeW(
            name,
            mode,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            0,
            64 * 1024,
            0,
            None,
        )
    };
    // SAFETY: owned at once, closed once.
    (!pipe.is_invalid()).then(|| unsafe { Owned::new(pipe) })
}

fn serve(name: &HSTRING, stop: &Event, on_command: &CommandFn) {
    let Some(mut pipe) = create_pipe(name, true) else {
        return;
    };
    let mut failures = 0;
    loop {
        let connected = match connect(*pipe, stop) {
            Io::Done(_) => true,
            Io::Stopped => return,
            _ => false,
        };
        // The next instance before this one closes: the name is never free
        // for another process to take.
        let Some(next) = create_pipe(name, false) else {
            return;
        };
        if connected {
            failures = 0;
            let bytes = read_all(*pipe, stop);
            drop(std::mem::replace(&mut pipe, next));
            for command in parse_message(&bytes) {
                on_command(command);
            }
        } else {
            failures += 1;
            if failures > 10 {
                return;
            }
            pipe = next;
        }
    }
}

/// How an overlapped operation ended.
enum Io {
    Done(u32),
    Failed,
    Stopped,
    TimedOut,
}

/// Waits for the overlapped operation `started` (its result) on `pipe`,
/// up to `timeout` ms or until `stop`. A pending one left behind is
/// cancelled and waited for, so `ov` (and the buffer) are free again on
/// return.
///
/// # Safety
/// `ov` is the OVERLAPPED the operation was started with, `pipe` its
/// handle.
unsafe fn finish(
    pipe: HANDLE,
    ov: &mut OVERLAPPED,
    started: windows::core::Result<()>,
    stop: &Event,
    timeout: u32,
) -> Io {
    if let Err(e) = started
        && e.code() != ERROR_IO_PENDING.to_hresult()
    {
        return Io::Failed;
    }
    let mut n = 0u32;
    // SAFETY: handles alive for the call.
    let woke = unsafe { WaitForMultipleObjects(&[ov.hEvent, stop.0], false, timeout) };
    if woke == WAIT_OBJECT_0 {
        // SAFETY: the operation finished; `ov` and `n` are live.
        return match unsafe { GetOverlappedResult(pipe, ov, &mut n, false) } {
            Ok(()) => Io::Done(n),
            Err(_) => Io::Failed,
        };
    }
    // SAFETY: cancel, then wait for the cancellation to land: only then may
    // the OVERLAPPED and the buffer go.
    unsafe {
        let _ = CancelIoEx(pipe, Some(ov));
        let _ = GetOverlappedResult(pipe, ov, &mut n, true);
    }
    if woke.0 == WAIT_OBJECT_0.0 + 1 { Io::Stopped } else { Io::TimedOut }
}

/// Waits for a client.
fn connect(pipe: HANDLE, stop: &Event) -> Io {
    let Some(event) = Event::new() else {
        return Io::Failed;
    };
    let mut ov = OVERLAPPED { hEvent: event.0, ..Default::default() };
    // SAFETY: an overlapped pipe handle; `ov` stays put until `finish` is
    // done with it.
    let started = unsafe { ConnectNamedPipe(pipe, Some(&mut ov)) };
    if let Err(e) = &started {
        // Connected before the call (or came and went, having sent nothing):
        // nothing pending.
        if e.code() == ERROR_PIPE_CONNECTED.to_hresult() || e.code() == ERROR_NO_DATA.to_hresult() {
            return Io::Done(0);
        }
    }
    // SAFETY: the operation just started on `pipe` with `ov`.
    unsafe { finish(pipe, &mut ov, started, stop, INFINITE) }
}

/// What the client writes, until it closes; a client that stalls for two
/// seconds or sends more than a megabyte is cut off.
fn read_all(pipe: HANDLE, stop: &Event) -> Vec<u8> {
    let mut out = Vec::new();
    let Some(event) = Event::new() else {
        return out;
    };
    let mut buf = vec![0u8; 4096];
    while out.len() < 1 << 20 {
        let mut ov = OVERLAPPED { hEvent: event.0, ..Default::default() };
        // SAFETY: `buf` and `ov` are not touched until `finish` returns,
        // which waits for the read to end (or be cancelled).
        let n = unsafe {
            let started = ReadFile(pipe, Some(&mut buf), None, Some(&mut ov));
            finish(pipe, &mut ov, started, stop, 2000)
        };
        match n {
            Io::Done(0) => break,
            Io::Done(n) => out.extend_from_slice(&buf[..n as usize]),
            // Failed is mostly ERROR_BROKEN_PIPE: the client wrote all and
            // closed.
            Io::Failed | Io::Stopped | Io::TimedOut => break,
        }
    }
    out
}

// --- The media controls -----------------------------------------------------------------------

/// The shared state changed: push it.
const MSG_PUSH: u32 = MSG_USER;

/// How often the timeline is refreshed while playing, should the controls
/// not run it on by themselves.
const TIMELINE_REFRESH: Duration = Duration::from_secs(5);

struct Shared {
    enabled: bool,
    state: PlayerState,
    /// When `state.position` was true.
    stamp: Instant,
    /// When the controls last got the state.
    pushed_at: Instant,
    /// A MSG_PUSH is on its way.
    posted: bool,
}

type SharedRef = Arc<Mutex<Shared>>;

fn lock(shared: &SharedRef) -> MutexGuard<'_, Shared> {
    shared.lock().unwrap_or_else(|e| e.into_inner())
}

/// The single instance and the media controls. The pipe server runs from
/// [`Mpris::new`]; [`Mpris::set_enabled`] shows or hides the player in the
/// system's controls (Settings › System).
pub struct Mpris {
    shared: SharedRef,
    window: Option<HiddenWindow>,
    _server: Option<Server>,
}

impl Mpris {
    pub fn new(_bus: Bus, on_command: impl Fn(Command) + Send + Sync + 'static) -> Mpris {
        let on_command: CommandFn = Arc::new(on_command);
        let now = Instant::now();
        let shared = Arc::new(Mutex::new(Shared {
            enabled: false,
            state: PlayerState::default(),
            stamp: now,
            pushed_at: now,
            posted: false,
        }));
        let server = Server::start(Arc::clone(&on_command));
        let window = {
            let shared = Arc::clone(&shared);
            HiddenWindow::spawn("sonkore-media", move |hwnd| {
                let controls = Controls::new(hwnd, on_command).ok();
                Box::new(move |_hwnd, msg, _wparam, _lparam| {
                    let controls = controls.as_ref()?;
                    match msg {
                        MSG_PUSH => {
                            controls.push(&shared);
                            Some(LRESULT(0))
                        }
                        WM_DESTROY => {
                            controls.detach();
                            None
                        }
                        _ => None,
                    }
                })
            })
            .ok()
        };
        Mpris { shared, window, _server: server }
    }

    /// In the system's media controls or not; the single instance stays.
    pub fn set_enabled(&self, on: bool) {
        {
            let mut s = lock(&self.shared);
            if s.enabled == on {
                return;
            }
            s.enabled = on;
        }
        self.post();
    }

    /// What [`Mpris::set_enabled`] last asked for.
    pub fn enabled(&self) -> bool {
        lock(&self.shared).enabled
    }

    /// The player's state now (every tick): the controls hear what they
    /// show changing, a seek, and the timeline now and then while playing.
    pub fn update(&self, state: PlayerState) {
        let post = {
            let mut s = lock(&self.shared);
            let now = Instant::now();
            let elapsed = now.duration_since(s.stamp).as_secs_f64();
            let refresh =
                state.status == PlaybackStatus::Playing && now.duration_since(s.pushed_at) >= TIMELINE_REFRESH;
            let post = s.enabled && (refresh || media_outdated(&s.state, &state, elapsed));
            s.state = state;
            s.stamp = now;
            post
        };
        if post {
            self.post();
        }
    }

    fn post(&self) {
        {
            let mut s = lock(&self.shared);
            if s.posted {
                return;
            }
            s.posted = true;
            s.pushed_at = Instant::now();
        }
        if let Some(w) = &self.window
            && !w.post(MSG_PUSH, 0, 0)
        {
            lock(&self.shared).posted = false;
        }
    }
}

impl Drop for Mpris {
    fn drop(&mut self) {
        if let Some(mut w) = self.window.take() {
            w.close();
        }
    }
}

/// The window's System Media Transport Controls, on its thread.
struct Controls {
    smtc: SystemMediaTransportControls,
    tokens: [i64; 4],
    /// What they show (None: nothing pushed since they were enabled).
    shown: std::cell::RefCell<Option<PlayerState>>,
    enabled: std::cell::Cell<bool>,
}

fn seconds(span: TimeSpan) -> f64 {
    span.Duration as f64 / 1e7
}

fn span(seconds: f64) -> TimeSpan {
    TimeSpan { Duration: (seconds.max(0.0) * 1e7) as i64 }
}

impl Controls {
    fn new(hwnd: HWND, on_command: CommandFn) -> windows::core::Result<Controls> {
        let interop = windows::core::factory::<SystemMediaTransportControls, ISystemMediaTransportControlsInterop>()?;
        // SAFETY: our own live top-level window.
        let smtc: SystemMediaTransportControls = unsafe { interop.GetForWindow(hwnd) }?;
        smtc.SetIsEnabled(false)?;
        smtc.SetIsPlayEnabled(true)?;
        smtc.SetIsPauseEnabled(true)?;
        smtc.SetIsStopEnabled(true)?;
        smtc.SetIsNextEnabled(true)?;
        smtc.SetIsPreviousEnabled(true)?;
        // The handlers run on system threads: they only hand the command on.
        let send = Arc::clone(&on_command);
        let buttons = smtc.ButtonPressed(&TypedEventHandler::<
            SystemMediaTransportControls,
            SystemMediaTransportControlsButtonPressedEventArgs,
        >::new(move |_, args| {
            if let Some(args) = args.as_ref()
                && let Some(command) = button_command(args.Button()?)
            {
                send(command);
            }
            Ok(())
        }))?;
        let send = Arc::clone(&on_command);
        let position = smtc.PlaybackPositionChangeRequested(&TypedEventHandler::<
            SystemMediaTransportControls,
            PlaybackPositionChangeRequestedEventArgs,
        >::new(move |_, args| {
            if let Some(args) = args.as_ref() {
                send(Command::SeekTo(seconds(args.RequestedPlaybackPosition()?).max(0.0)));
            }
            Ok(())
        }))?;
        let send = Arc::clone(&on_command);
        let shuffle = smtc.ShuffleEnabledChangeRequested(&TypedEventHandler::<
            SystemMediaTransportControls,
            ShuffleEnabledChangeRequestedEventArgs,
        >::new(move |_, args| {
            if let Some(args) = args.as_ref() {
                send(Command::SetShuffle(args.RequestedShuffleEnabled()?));
            }
            Ok(())
        }))?;
        let send = on_command;
        let repeat = smtc.AutoRepeatModeChangeRequested(&TypedEventHandler::<
            SystemMediaTransportControls,
            AutoRepeatModeChangeRequestedEventArgs,
        >::new(move |_, args| {
            if let Some(args) = args.as_ref() {
                send(Command::SetRepeat(repeat_of(args.RequestedAutoRepeatMode()?)));
            }
            Ok(())
        }))?;
        Ok(Controls {
            smtc,
            tokens: [buttons, position, shuffle, repeat],
            shown: Default::default(),
            enabled: Default::default(),
        })
    }

    /// Brings the controls to the shared state.
    fn push(&self, shared: &SharedRef) {
        let (enabled, state, position) = {
            let mut s = lock(shared);
            s.posted = false;
            let mut position = s.state.position;
            if s.state.status == PlaybackStatus::Playing {
                position += s.stamp.elapsed().as_secs_f64();
            }
            (s.enabled, s.state.clone(), position)
        };
        if enabled != self.enabled.get() {
            self.enabled.set(enabled);
            let _ = self.smtc.SetIsEnabled(enabled);
            // Back on: everything again.
            self.shown.replace(None);
        }
        if enabled {
            let _ = self.show(&state, position);
            self.shown.replace(Some(state));
        }
    }

    fn show(&self, new: &PlayerState, position: f64) -> windows::core::Result<()> {
        let shown = self.shown.borrow();
        let old = shown.as_ref();
        let changed = |f: fn(&PlayerState) -> bool| old.is_none_or(|o| f(o) != f(new));
        if old.is_none_or(|o| {
            o.status != new.status || o.track.is_some() != new.track.is_some() || o.has_queue != new.has_queue
        }) {
            self.smtc.SetPlaybackStatus(status_of(new))?;
        }
        if changed(|s| s.has_queue) {
            self.smtc.SetIsNextEnabled(new.has_queue)?;
            self.smtc.SetIsPreviousEnabled(new.has_queue)?;
        }
        if old.is_none_or(|o| o.track != new.track) {
            let display = self.smtc.DisplayUpdater()?;
            match &new.track {
                None => display.ClearAll()?,
                Some(t) => {
                    display.SetType(MediaPlaybackType::Music)?;
                    let music = display.MusicProperties()?;
                    music.SetTitle(&HSTRING::from(t.title.as_str()))?;
                    music.SetArtist(&HSTRING::from(t.artist.as_str()))?;
                    music.SetAlbumTitle(&HSTRING::from(t.album.as_str()))?;
                    music.SetAlbumArtist(&HSTRING::from(t.album_artist.as_str()))?;
                    music.SetTrackNumber(t.track_no)?;
                }
            }
            display.Update()?;
        }
        if old.is_none_or(|o| o.repeat != new.repeat) {
            self.smtc.SetAutoRepeatMode(match new.repeat {
                Repeat::Off => MediaPlaybackAutoRepeatMode::None,
                Repeat::All => MediaPlaybackAutoRepeatMode::List,
                Repeat::One => MediaPlaybackAutoRepeatMode::Track,
            })?;
        }
        if changed(|s| s.shuffle) {
            self.smtc.SetShuffleEnabled(new.shuffle)?;
        }
        let timeline = SystemMediaTransportControlsTimelineProperties::new()?;
        let duration = new.duration.max(0.0);
        timeline.SetStartTime(span(0.0))?;
        timeline.SetEndTime(span(duration))?;
        timeline.SetMinSeekTime(span(0.0))?;
        timeline.SetMaxSeekTime(span(duration))?;
        timeline.SetPosition(span(if duration > 0.0 { position.min(duration) } else { 0.0 }))?;
        self.smtc.UpdateTimelineProperties(&timeline)
    }

    /// Off the controls, the handlers gone (the window is closing).
    fn detach(&self) {
        let [buttons, position, shuffle, repeat] = self.tokens;
        let _ = self.smtc.RemoveButtonPressed(buttons);
        let _ = self.smtc.RemovePlaybackPositionChangeRequested(position);
        let _ = self.smtc.RemoveShuffleEnabledChangeRequested(shuffle);
        let _ = self.smtc.RemoveAutoRepeatModeChangeRequested(repeat);
        let _ = self.smtc.SetIsEnabled(false);
    }
}

/// Closed with nothing loaded and nothing to play (the flyout drops the
/// player); with a queue, Stopped, so the play key still starts it.
fn status_of(state: &PlayerState) -> MediaPlaybackStatus {
    match state.status {
        PlaybackStatus::Playing => MediaPlaybackStatus::Playing,
        PlaybackStatus::Paused => MediaPlaybackStatus::Paused,
        PlaybackStatus::Stopped if state.track.is_none() && !state.has_queue => MediaPlaybackStatus::Closed,
        PlaybackStatus::Stopped => MediaPlaybackStatus::Stopped,
    }
}

fn button_command(button: SystemMediaTransportControlsButton) -> Option<Command> {
    Some(match button {
        SystemMediaTransportControlsButton::Play => Command::Play,
        SystemMediaTransportControlsButton::Pause => Command::Pause,
        SystemMediaTransportControlsButton::Stop => Command::Stop,
        SystemMediaTransportControlsButton::Next => Command::Next,
        SystemMediaTransportControlsButton::Previous => Command::Previous,
        _ => return None,
    })
}

fn repeat_of(mode: MediaPlaybackAutoRepeatMode) -> Repeat {
    match mode {
        MediaPlaybackAutoRepeatMode::Track => Repeat::One,
        MediaPlaybackAutoRepeatMode::List => Repeat::All,
        _ => Repeat::Off,
    }
}
