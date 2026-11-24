//! What the desktop's media controls see of the player and what they ask
//! for: shared by MPRIS (Linux) and the Windows media controls.

use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PlaybackStatus {
    #[default]
    Stopped,
    Paused,
    Playing,
}

impl PlaybackStatus {
    /// From the player: playing, else paused when a track is loaded.
    pub fn from_player(playing: bool, loaded: bool) -> Self {
        if playing {
            PlaybackStatus::Playing
        } else if loaded {
            PlaybackStatus::Paused
        } else {
            PlaybackStatus::Stopped
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            PlaybackStatus::Stopped => "Stopped",
            PlaybackStatus::Paused => "Paused",
            PlaybackStatus::Playing => "Playing",
        }
    }
}

/// The player's repeat mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Repeat {
    #[default]
    Off,
    All,
    One,
}

/// MPRIS `LoopStatus` of a repeat mode.
pub fn loop_status(repeat: Repeat) -> &'static str {
    match repeat {
        Repeat::Off => "None",
        Repeat::All => "Playlist",
        Repeat::One => "Track",
    }
}

/// The repeat mode a `LoopStatus` asks for (anything unknown is Off).
pub fn repeat_from_loop_status(status: &str) -> Repeat {
    match status {
        "Playlist" => Repeat::All,
        "Track" => Repeat::One,
        _ => Repeat::Off,
    }
}

/// The track loaded, as the metadata shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrackInfo {
    pub path: PathBuf,
    pub title: String,
    pub album: String,
    pub artist: String,
    pub album_artist: String,
    /// 0: unknown.
    pub track_no: u32,
    pub genre: String,
    /// Seconds.
    pub duration: f64,
    /// `mpris:artUrl`: see [`cached_cover_url`] and [`file_url`].
    pub art_url: Option<String>,
}

/// What the desktop sees of the player.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerState {
    pub status: PlaybackStatus,
    pub repeat: Repeat,
    pub shuffle: bool,
    /// 0..1.
    pub volume: f64,
    /// Seconds, at the time of the update; carried forward while playing.
    pub position: f64,
    /// Seconds; 0 when nothing seekable is loaded.
    pub duration: f64,
    /// The queue holds something (CanGoNext / CanGoPrevious / CanPlay).
    pub has_queue: bool,
    pub track: Option<TrackInfo>,
}

impl Default for PlayerState {
    fn default() -> Self {
        PlayerState {
            status: PlaybackStatus::Stopped,
            repeat: Repeat::Off,
            shuffle: false,
            volume: 1.0,
            position: 0.0,
            duration: 0.0,
            has_queue: false,
            track: None,
        }
    }
}

/// What the desktop asks for. Delivered on a D-Bus thread: the callback
/// should hand it to the app's own loop and return.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// Bring the window forward.
    Raise,
    Quit,
    Play,
    /// A pause from the desktop (WirePlumber also pauses players whose
    /// sink went away).
    Pause,
    PlayPause,
    Stop,
    Next,
    Previous,
    /// Seek to an absolute position, seconds (from Seek and SetPosition).
    SeekTo(f64),
    SetRepeat(Repeat),
    SetShuffle(bool),
    /// 0..1.
    SetVolume(f64),
    /// OpenUri: a local path (document portal paths mapped to the host) or
    /// the URI as given. The app appends it to the queue and plays it.
    Open(String),
}

/// What `Seek(offset)` does from `position`: past the end it goes to the
/// next track, otherwise to the new position (not before the start). With
/// the length unknown (0) it only seeks: every seek used to skip the track.
pub fn seek_command(position: f64, duration: f64, offset_us: i64) -> Command {
    let target = position + offset_us as f64 / 1e6;
    if duration > 0.0 && target >= duration { Command::Next } else { Command::SeekTo(target.max(0.0)) }
}

/// What `SetPosition(position)` does: outside 0..=length nothing (the MPRIS
/// spec says to ignore it); with the length unknown, any position ≥ 0.
pub fn set_position_command(position: f64, duration: f64) -> Option<Command> {
    (position >= 0.0 && (duration <= 0.0 || position <= duration)).then_some(Command::SeekTo(position))
}

/// The player's methods for the desktop entry's quick actions
/// (`Sonkore --play-pause`), meant for the instance already running.
pub fn player_command<S: AsRef<str>>(args: &[S]) -> Option<&'static str> {
    args.iter().find_map(|a| match a.as_ref() {
        "--play-pause" => Some("PlayPause"),
        "--next" => Some("Next"),
        "--previous" => Some("Previous"),
        _ => None,
    })
}

/// What the tray icon's menu asks for. Delivered on another thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayCommand {
    /// Menu › Play / Pause.
    TogglePlay,
    /// Menu › Next.
    Next,
    /// Menu › Previous.
    Previous,
    /// Menu › Sonkore: bring the window forward.
    Raise,
    /// Menu › Quit.
    Quit,
    /// The icon was clicked: the app hides the
    /// window when it is visible and not minimized, else raises it.
    Activate,
}

/// The menu's texts, already translated by the app (contexts: Main for
/// Play/Pause, NowPlayingBar for Next/Previous, SettingsPage for Quit).
/// "Sonkore" is never translated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrayLabels {
    pub play: String,
    pub pause: String,
    pub next: String,
    pub previous: String,
    pub quit: String,
    /// A right-to-left interface language (dbusmenu `TextDirection`).
    pub rtl: bool,
}

impl Default for TrayLabels {
    fn default() -> Self {
        TrayLabels {
            play: "Play".into(),
            pause: "Pause".into(),
            next: "Next".into(),
            previous: "Previous".into(),
            quit: "Quit".into(),
            rtl: false,
        }
    }
}
