//! SnakeTUI's screens and app state machine. Drawing goes through any ratatui
//! backend, and the app reads [`Key`]s and returns [`app::Effect`]s instead of
//! touching files, so the local game and the SSH server share all of it.

pub mod app;
pub mod key;
pub mod online;
pub mod palette;
pub mod ui;

pub use app::{App, Effect, Screen};
pub use key::Key;
