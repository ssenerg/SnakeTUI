//! SnakeTUI's game rules, with no terminal or file access, so the local game
//! and the server share them.
//!
//! A [`world::World`] holds the board and moves any number of snakes at once;
//! [`solo::SoloGame`] adds what's specific to single player, and
//! [`view::MatchView`] is the snapshot that gets drawn.

pub mod board;
pub mod config;
pub mod geom;
pub mod records;
pub mod settings;
pub mod snake;
pub mod solo;
pub mod spawn;
pub mod view;
pub mod world;
