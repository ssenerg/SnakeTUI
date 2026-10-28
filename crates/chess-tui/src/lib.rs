//! Chess's screens in the shared app: its settings, a private room's
//! challenge, and the board a game is played on, by typing moves or
//! picking squares.

mod app;
mod entry;
mod prefs;
mod ui;

pub use app::{ChessScreen, ChessUi, MenuItem, chess_view};
pub use prefs::{BoardLook, MoveInput, PieceStyle, Prefs};
