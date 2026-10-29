//! Chess's rules for the terminal game, with no terminal, network or clock
//! access, so the server and the client share them.
//!
//! A [`position::Position`] wraps cozy-chess's board and speaks standard
//! notation; [`san`] writes and reads moves as players do. A [`game::Game`]
//! adds the history, the [`clock::Clock`]s, draw offers and every way a
//! game ends, and [`view::GameView`] is the snapshot that gets drawn or
//! sent. [`control`] sorts time controls into the categories [`rating`]
//! keeps Glicko-2 ratings in, [`round`] plays games in rooms, and
//! [`engine`] is the computer opponent.

pub mod clock;
pub mod control;
pub mod engine;
pub mod game;
mod material;
pub mod moves;
pub mod outcome;
pub mod piece;
pub mod position;
pub mod rating;
pub mod round;
pub mod san;
pub mod square;
pub mod view;
