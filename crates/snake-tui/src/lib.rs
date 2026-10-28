//! Snake's screens: single player, and its part of the app's rooms, as a
//! [`SnakeUi`] the app shows through [`shell_tui::GameUi`]. The local game
//! drives the same screens on their own.

pub mod app;
pub mod palette;
pub mod ui;

pub use app::{SnakeScreen, SnakeUi};
