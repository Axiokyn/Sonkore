//! The pure part of the Windows modules (the pipe's protocol, the tray's
//! menu and tooltip, the icon's pixels, when the media controls need a new
//! state), compiled on every system so its tests run on Linux too.

#![cfg_attr(not(windows), allow(dead_code))]

use crate::media::{Command, PlaybackStatus, PlayerState, TrayCommand, TrayLabels};

// --- The single instance's pipe ---------------------------------------------------

/// The pipe a running instance listens on: one per user (another user's
/// Sonkore is another instance). A pipe name takes anything but `\`; the
/// user name is kept to a safe set anyway.
pub fn pipe_name(user: &str) -> String {
    let user: String =
        user.chars().map(|c| if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '_' }).collect();
    format!(r"\\.\pipe\com.axiokyn.sonkore-{user}")
}

/// What a second launch sends: each file to open, then raise. One command a
/// line (a Windows path cannot hold a line break).
pub fn open_message<S: AsRef<str>>(paths: &[S]) -> String {
    let mut out = String::new();
    for p in paths {
        let p = p.as_ref();
        if !p.is_empty() && !p.contains(['\n', '\r']) {
            out.push_str("open ");
            out.push_str(p);
            out.push('\n');
        }
    }
    out.push_str("raise\n");
    out
}

/// A desktop entry's quick action (`player_command`'s method names).
pub fn call_message(method: &str) -> String {
    format!("call {method}\n")
}

/// The player methods a `call` line may name.
pub fn call_command(method: &str) -> Option<Command> {
    Some(match method {
        "PlayPause" => Command::PlayPause,
        "Play" => Command::Play,
        "Pause" => Command::Pause,
        "Stop" => Command::Stop,
        "Next" => Command::Next,
        "Previous" => Command::Previous,
        _ => return None,
    })
}

/// The commands in what a client sent. Only whole lines count: a client cut
/// off mid-line must not open half a path. Unknown lines are skipped.
pub fn parse_message(bytes: &[u8]) -> Vec<Command> {
    let Some(end) = bytes.iter().rposition(|&b| b == b'\n') else {
        return Vec::new();
    };
    String::from_utf8_lossy(&bytes[..end])
        .split('\n')
        .filter_map(|line| {
            let line = line.strip_suffix('\r').unwrap_or(line);
            let (verb, arg) = line.split_once(' ').unwrap_or((line, ""));
            match verb {
                "open" if !arg.is_empty() => Some(Command::Open(arg.to_string())),
                "raise" => Some(Command::Raise),
                "call" => call_command(arg),
                _ => None,
            }
        })
        .collect()
}

// --- The tray icon -----------------------------------------------------------------

/// The menu's command ids (Linux's dbusmenu ids; 0 is TrackPopupMenu's
/// "nothing chosen").
pub mod item {
    pub const PLAY: u32 = 1;
    pub const NEXT: u32 = 2;
    pub const PREVIOUS: u32 = 3;
    pub const RAISE: u32 = 5;
    pub const QUIT: u32 = 6;
}

/// The window's title in the menu (never translated).
pub const TITLE: &str = "Sonkore";

/// A Win32 menu label for plain text: `&` marks an access key there, so a
/// literal one is doubled.
pub fn menu_text(text: &str) -> String {
    text.replace('&', "&&")
}

/// The menu, in order: (id, label), None for the separator.
pub fn menu_entries(labels: &TrayLabels, playing: bool) -> Vec<Option<(u32, String)>> {
    vec![
        Some((item::PLAY, menu_text(if playing { &labels.pause } else { &labels.play }))),
        Some((item::NEXT, menu_text(&labels.next))),
        Some((item::PREVIOUS, menu_text(&labels.previous))),
        None,
        Some((item::RAISE, menu_text(TITLE))),
        Some((item::QUIT, menu_text(&labels.quit))),
    ]
}

/// What a menu id asks for (None: dismissed).
pub fn menu_command(id: u32) -> Option<TrayCommand> {
    Some(match id {
        item::PLAY => TrayCommand::TogglePlay,
        item::NEXT => TrayCommand::Next,
        item::PREVIOUS => TrayCommand::Previous,
        item::RAISE => TrayCommand::Raise,
        item::QUIT => TrayCommand::Quit,
        _ => return None,
    })
}

/// The anchor point of a NOTIFYICON_VERSION_4 callback (its WPARAM: x low,
/// y high, both signed: a monitor left of the primary one is negative).
pub fn callback_anchor(wparam: usize) -> (i32, i32) {
    ((wparam & 0xFFFF) as u16 as i16 as i32, ((wparam >> 16) & 0xFFFF) as u16 as i16 as i32)
}

/// `szTip` holds 128 UTF-16 units with the terminating NUL: the text cut to
/// 127 units, never between the halves of a surrogate pair.
pub fn tooltip_units(text: &str) -> Vec<u16> {
    const MAX: usize = 127;
    let mut out = Vec::with_capacity(MAX);
    for c in text.chars() {
        let mut buf = [0u16; 2];
        let units = c.encode_utf16(&mut buf);
        if out.len() + units.len() > MAX {
            break;
        }
        out.extend_from_slice(units);
    }
    out
}

/// Of the PNG sizes on hand, the one to scale to `want` pixels: the
/// smallest not below it (scaling down keeps the detail), else the largest.
pub fn icon_source(sizes: &[u32], want: u32) -> Option<usize> {
    let fits = sizes.iter().enumerate().filter(|(_, s)| **s >= want).min_by_key(|(_, s)| **s);
    fits.or_else(|| sizes.iter().enumerate().max_by_key(|(_, s)| **s)).map(|(i, _)| i)
}

/// A PNG as CreateIcon's 32-bit colour bits: `size`×`size`, BGRA, top row
/// first, straight alpha.
pub fn icon_bgra(png: &[u8], size: u32) -> Option<Vec<u8>> {
    let image = image::load_from_memory_with_format(png, image::ImageFormat::Png).ok()?.to_rgba8();
    let image = if image.dimensions() == (size, size) {
        image
    } else {
        image::imageops::resize(&image, size, size, image::imageops::FilterType::CatmullRom)
    };
    let mut out = image.into_raw();
    for px in out.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    Some(out)
}

// --- The media controls ---------------------------------------------------------------

/// The position jumped (a seek) rather than ran on with the clock, from
/// `old` to `new` `elapsed` seconds later.
pub fn position_jumped(old: &PlayerState, new: &PlayerState, elapsed: f64) -> bool {
    let expected = if old.status == PlaybackStatus::Playing { old.position + elapsed } else { old.position };
    (new.position - expected).abs() > 1.0
}

/// Whether the media controls show something `new` changes: the status,
/// the track, the buttons, shuffle and repeat, or the timeline beyond the
/// running clock. Called every tick: the position alone moving is nothing.
pub fn media_outdated(old: &PlayerState, new: &PlayerState, elapsed: f64) -> bool {
    old.status != new.status
        || old.track != new.track
        || old.repeat != new.repeat
        || old.shuffle != new.shuffle
        || old.duration != new.duration
        || old.has_queue != new.has_queue
        || position_jumped(old, new, elapsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::{Repeat, TrackInfo};

    #[test]
    fn pipe_name_is_per_user_and_safe() {
        assert_eq!(pipe_name("ana"), r"\\.\pipe\com.axiokyn.sonkore-ana");
        assert_eq!(pipe_name(r"DOM\José G"), r"\\.\pipe\com.axiokyn.sonkore-DOM_José_G");
    }

    #[test]
    fn messages_round_trip() {
        let msg = open_message(&[r"C:\Music\a b.flac", r"D:\x&y.mp3"]);
        assert_eq!(msg, "open C:\\Music\\a b.flac\nopen D:\\x&y.mp3\nraise\n");
        assert_eq!(
            parse_message(msg.as_bytes()),
            vec![Command::Open(r"C:\Music\a b.flac".into()), Command::Open(r"D:\x&y.mp3".into()), Command::Raise]
        );
        for (method, command) in
            [("PlayPause", Command::PlayPause), ("Next", Command::Next), ("Previous", Command::Previous)]
        {
            assert_eq!(parse_message(call_message(method).as_bytes()), vec![command]);
        }
    }

    #[test]
    fn parse_skips_unknown_and_unfinished_lines() {
        assert_eq!(parse_message(b"call Bogus\nhello\nopen \nraise\r\nopen C:\\half"), vec![Command::Raise]);
        assert!(parse_message(b"").is_empty());
        assert!(parse_message(b"raise").is_empty());
        // Line breaks never reach a path.
        assert_eq!(open_message(&["a\nraise"]), "raise\n");
    }

    #[test]
    fn menu_follows_the_player() {
        let labels = TrayLabels {
            play: "Reproducir".into(),
            pause: "Pausa".into(),
            quit: "Salir & cerrar".into(),
            ..Default::default()
        };
        let menu = menu_entries(&labels, false);
        assert_eq!(menu.len(), 6);
        assert_eq!(menu[0], Some((item::PLAY, "Reproducir".into())));
        assert_eq!(menu[3], None);
        assert_eq!(menu[4], Some((item::RAISE, "Sonkore".into())));
        assert_eq!(menu[5], Some((item::QUIT, "Salir && cerrar".into())));
        assert_eq!(menu_entries(&labels, true)[0], Some((item::PLAY, "Pausa".into())));
        let commands: Vec<_> = menu.iter().flatten().map(|(id, _)| menu_command(*id)).collect();
        assert_eq!(
            commands,
            [TrayCommand::TogglePlay, TrayCommand::Next, TrayCommand::Previous, TrayCommand::Raise, TrayCommand::Quit]
                .map(Some)
        );
        assert_eq!(menu_command(0), None);
    }

    #[test]
    fn anchor_is_signed() {
        assert_eq!(callback_anchor(0x0020_0010), (16, 32));
        assert_eq!(callback_anchor(0x0005_FFF6), (-10, 5));
    }

    #[test]
    fn tooltip_fits_without_splitting_pairs() {
        assert_eq!(tooltip_units("Sonkore"), "Sonkore".encode_utf16().collect::<Vec<_>>());
        let long = "a".repeat(200);
        assert_eq!(tooltip_units(&long).len(), 127);
        // 126 units, then a pair that would end at 128: left out whole.
        let text = format!("{}🎵", "a".repeat(126));
        let units = tooltip_units(&text);
        assert_eq!(units.len(), 126);
        assert!(String::from_utf16(&units).is_ok());
        let text = format!("{}🎵", "a".repeat(125));
        assert_eq!(tooltip_units(&text).len(), 127);
    }

    #[test]
    fn icon_source_prefers_scaling_down() {
        let sizes = [16, 22, 24, 32, 48, 64];
        assert_eq!(icon_source(&sizes, 16), Some(0));
        assert_eq!(icon_source(&sizes, 20), Some(1));
        assert_eq!(icon_source(&sizes, 40), Some(4));
        assert_eq!(icon_source(&sizes, 96), Some(5));
        assert_eq!(icon_source(&[], 16), None);
    }

    #[test]
    fn icon_pixels_are_bgra_at_the_size_asked() {
        let png = include_bytes!("../../../../packaging/icons/png/16.png");
        let bgra = icon_bgra(png, 16).unwrap();
        assert_eq!(bgra.len(), 16 * 16 * 4);
        let rgba = image::load_from_memory(png).unwrap().to_rgba8().into_raw();
        assert_eq!(&bgra[..4], &[rgba[2], rgba[1], rgba[0], rgba[3]]);
        assert_eq!(icon_bgra(png, 20).unwrap().len(), 20 * 20 * 4);
        assert!(icon_bgra(b"not a png", 16).is_none());
    }

    #[test]
    fn media_controls_hear_changes_not_the_clock() {
        let track = TrackInfo { title: "A".into(), duration: 200.0, ..Default::default() };
        let old = PlayerState {
            status: PlaybackStatus::Playing,
            position: 10.0,
            duration: 200.0,
            track: Some(track.clone()),
            ..Default::default()
        };
        let ran = PlayerState { position: 10.5, ..old.clone() };
        assert!(!media_outdated(&old, &ran, 0.5));
        let seeked = PlayerState { position: 80.0, ..old.clone() };
        assert!(media_outdated(&old, &seeked, 0.5));
        let paused = PlayerState { status: PlaybackStatus::Paused, ..ran.clone() };
        assert!(media_outdated(&old, &paused, 0.5));
        // Paused, the position stays: a second later it has not jumped.
        assert!(!media_outdated(&paused, &paused, 1.0));
        let next = PlayerState { track: Some(TrackInfo { title: "B".into(), ..track }), ..ran.clone() };
        assert!(media_outdated(&old, &next, 0.5));
        assert!(media_outdated(&old, &PlayerState { repeat: Repeat::One, ..ran.clone() }, 0.5));
        assert!(media_outdated(&old, &PlayerState { shuffle: true, ..ran }, 0.5));
    }
}
