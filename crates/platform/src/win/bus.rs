//! Windows has no session bus; `Bus` is kept so the callers read the same
//! on both systems (`Mpris::new(Bus::Session, …)`).

/// The session, for the per-user single instance and media controls.
#[derive(Clone, Debug, Default)]
pub enum Bus {
    #[default]
    Session,
}
