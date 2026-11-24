//! Icons picked in Rust (the sidebar's entries).

use slint::{ComponentHandle, Image};

use crate::{Icons, MainWindow};

pub fn nav_icon(window: &MainWindow, id: &str) -> Image {
    let i = window.global::<Icons>();
    match id {
        "playing" => i.get_headphones(),
        "tracks" => i.get_music(),
        "favorites" => i.get_heart(),
        "albums" => i.get_album(),
        "artists" => i.get_user(),
        "genres" => i.get_tag(),
        _ => i.get_music(),
    }
}
