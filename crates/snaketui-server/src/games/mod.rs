//! The games this server serves, and what the rest of the server needs of
//! each without knowing which game it is.
//!
//! Rooms run one game's [`Room`](game_api::Room) each, but sessions, the
//! cluster and the queue pass them around as [`AnyRoomView`]s and
//! [`NewRoom`]s. Each game's [`GameServer`] in [`GAMES`] says how it's
//! matched and rated, and reads its views when they come from another
//! server.

pub mod snake;

use std::any::Any;
use std::fmt;
use std::ops::RangeInclusive;
use std::sync::Arc;
use std::time::Duration;

use game_api::{Entrant, Game, GameId, PhaseView, RatingKey, RoomView, ShellView};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::cluster::wire::WireView;
use crate::room::NewRoom;

/// A game whose settings, moves and views can cross the network, and
/// whose rooms an actor can hold (actors are `Unpin`).
pub trait Served:
    Game<
        Settings: Serialize + DeserializeOwned + Unpin,
        Input: Serialize + DeserializeOwned,
        View: Serialize + DeserializeOwned,
        Round: Unpin,
    >
{
}

impl<G> Served for G where
    G: Game<
            Settings: Serialize + DeserializeOwned + Unpin,
            Input: Serialize + DeserializeOwned,
            View: Serialize + DeserializeOwned,
            Round: Unpin,
        >
{
}

/// A room's view, of whichever game.
pub trait AnyRoomView: Any + Send + Sync + fmt::Debug {
    fn game(&self) -> GameId;
    fn shell(&self) -> &ShellView;
    /// The same view, with the room closed for `reason`.
    fn closed(&self, reason: &str) -> Arc<dyn AnyRoomView>;
    /// The view as other servers get it, which its game's
    /// [`GameServer::decode_view`] reads.
    fn to_wire(&self) -> WireView;
    fn into_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync>;
}

impl<G: Served> AnyRoomView for RoomView<G> {
    fn game(&self) -> GameId {
        G::ID
    }

    fn shell(&self) -> &ShellView {
        &self.shell
    }

    fn closed(&self, reason: &str) -> Arc<dyn AnyRoomView> {
        let mut last = self.clone();
        last.shell.phase = PhaseView::Closed {
            reason: reason.to_string(),
        };
        Arc::new(last)
    }

    fn to_wire(&self) -> WireView {
        WireView {
            game: G::ID,
            body: postcard::to_allocvec(self).expect("views always serialize"),
        }
    }

    fn into_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync> {
        self
    }
}

/// `view`, if it's of a room of `G`.
pub fn view_of<G: Served>(view: Arc<dyn AnyRoomView>) -> Option<Arc<RoomView<G>>> {
    view.into_any().downcast().ok()
}

/// A view of a room of `G`, as [`AnyRoomView::to_wire`] wrote it.
fn decode_view<G: Served>(body: &[u8]) -> Option<Arc<dyn AnyRoomView>> {
    let view: RoomView<G> = postcard::from_bytes(body).ok()?;
    Some(Arc::new(view))
}

/// A move or settings, as they cross from a session to its room.
pub fn encode<T: Serialize>(value: &T) -> Vec<u8> {
    postcard::to_allocvec(value).expect("moves and settings always serialize")
}

pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Option<T> {
    postcard::from_bytes(bytes).ok()
}

/// How a game's ratings are worked out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RatingSystem {
    /// Free-for-all games of any size: see [`crate::rating`].
    WengLin,
}

/// One way to be matched in quick play, such as Snake's free-for-all.
#[derive(Debug)]
pub struct PoolInfo {
    pub id: &'static str,
    /// The rating its matches change.
    pub category: &'static str,
    /// How many players a match takes: it starts at once when full.
    pub seats: RangeInclusive<usize>,
    /// A match with fewer than a full room starts once its longest-waiting
    /// player has waited this long.
    pub fill_wait: Duration,
}

#[derive(Debug)]
pub struct GameInfo {
    pub id: GameId,
    pub title: &'static str,
    /// SSH usernames that open this game.
    pub ssh_users: &'static [&'static str],
    pub rating: RatingSystem,
    /// Its ratings' categories, each kept apart from the others.
    pub categories: &'static [&'static str],
    pub pools: &'static [PoolInfo],
}

impl GameInfo {
    pub fn pool(&self, id: &str) -> Option<&'static PoolInfo> {
        self.pools.iter().find(|p| p.id == id)
    }
}

/// What the server needs of a game beyond its rules.
pub trait GameServer: Send + Sync + 'static {
    fn info(&self) -> &'static GameInfo;

    /// A view of one of its rooms, from [`AnyRoomView::to_wire`].
    fn decode_view(&self, body: &[u8]) -> Option<Arc<dyn AnyRoomView>>;

    /// A matched room in `pool` for `players`, longest-waiting first.
    fn new_match(&self, pool: &PoolInfo, seed: u64, players: Vec<Entrant>) -> Box<dyn NewRoom>;

    /// A match for `who` alone against bots, if the game has bots.
    fn bot_match(&self, seed: u64, who: Entrant) -> Option<Box<dyn NewRoom>>;
}

/// Every game this server serves.
pub static GAMES: &[&dyn GameServer] = &[&snake::SnakeServer];

pub fn get(id: GameId) -> Option<&'static dyn GameServer> {
    GAMES.iter().copied().find(|g| g.info().id == id)
}

/// A pool players wait in for quick play: one of a game's.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PoolKey {
    pub game: GameId,
    pub pool: String,
}

impl PoolKey {
    /// The pool's game, and how its matches form, if this server has them.
    pub fn find(&self) -> Option<(&'static dyn GameServer, &'static PoolInfo)> {
        let game = get(self.game)?;
        Some((game, game.info().pool(&self.pool)?))
    }

    /// The rating its matches change.
    pub fn rating(&self) -> Option<RatingKey> {
        let (_, pool) = self.find()?;
        Some(RatingKey {
            game: self.game,
            category: pool.category,
        })
    }
}

impl fmt::Display for PoolKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.game, self.pool)
    }
}
