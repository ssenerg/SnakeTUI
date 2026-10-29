//! The app around every game: the home menu, accounts, friends, the
//! leaderboard, the game picker, and rooms' lobby and results. Each game
//! adds its own screens and draws its rounds through a [`GameUi`].
//!
//! Drawing goes through any ratatui backend, and the app reads [`Key`]s
//! and returns [`Effect`]s instead of touching the network or a database,
//! so the server decides what they do.

pub mod app;
pub mod game;
pub mod key;
pub mod online;
pub mod ui;

#[cfg(test)]
mod tests;

pub use app::{App, Effect, Screen};
pub use game::{
    Ctx, GameFx, GameUi, Next, Picture, Pictures, Placement, QuickPlay, QuickStart, Reply, Saved,
};
pub use key::{Key, Mouse, MouseKind};
