//! What every part of the interface shares: the settings, the player and
//! the library, plus a way to show a short message. Each window or page
//! module gets an `Rc<Ctx>`.

use std::cell::RefCell;
use std::rc::Rc;

use sonkore_core::settings::Settings;
use sonkore_engine::Player;

use crate::lib_shim::Library;

pub struct Ctx {
    pub settings: Rc<RefCell<Settings>>,
    pub player: Rc<RefCell<Player>>,
    pub library: Rc<RefCell<Library>>,
    /// The equalizer's logic (profiles, modes, AutoEQ), over the engine's EQ.
    pub equalizer: Rc<RefCell<sonkore_eq::controller::EqController>>,
    /// Shows a toast at the bottom of the main window.
    pub notify: Box<dyn Fn(&str)>,
    /// Missing covers from the internet (Settings › Library), None when
    /// offscreen.
    pub online_art: Option<RefCell<sonkore_library::online_art::OnlineArt>>,
}
