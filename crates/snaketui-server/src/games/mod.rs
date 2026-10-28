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

pub use game_api::codec::{decode, encode};
use game_api::{AnyView, Entrant, Game, GameId, PhaseView, RatingKey, RoomView};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use shell_tui::GameUi;

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

/// A room's view, of whichever game, as the server passes it around.
pub trait AnyRoomView: AnyView {
    /// The same view, with the room closed for `reason`.
    fn closed(&self, reason: &str) -> Arc<dyn AnyRoomView>;
    /// The view as other servers get it, which its game's
    /// [`GameServer::decode_view`] reads.
    fn to_wire(&self) -> WireView;
}

impl<G: Served> AnyRoomView for RoomView<G> {
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
            body: encode(self),
        }
    }
}

/// `view`, if it's of a room of `G`.
pub fn view_of<G: Served>(view: Arc<dyn AnyRoomView>) -> Option<Arc<RoomView<G>>> {
    let view: Arc<dyn Any + Send + Sync> = view;
    view.downcast().ok()
}

/// A view of a room of `G`, as [`AnyRoomView::to_wire`] wrote it.
fn decode_view<G: Served>(body: &[u8]) -> Option<Arc<dyn AnyRoomView>> {
    let view: RoomView<G> = decode(body)?;
    Some(Arc::new(view))
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

    /// A private room with `settings`, as the game's screens wrote them;
    /// or what to tell the player if it can't have those.
    fn private_room(&self, settings: &[u8], seed: u64) -> Result<Box<dyn NewRoom>, &'static str>;

    /// The game's screens, for a new player.
    fn ui(&self) -> Box<dyn GameUi>;
}

/// Every game this server serves.
pub static GAMES: &[&dyn GameServer] = &[&snake::SnakeServer];

pub fn get(id: GameId) -> Option<&'static dyn GameServer> {
    GAMES.iter().copied().find(|g| g.info().id == id)
}

/// The rating a database names `game` and `category`, if this server
/// serves that game and the game has that rating.
pub fn rating_key(game: &str, category: &str) -> Option<RatingKey> {
    let game = get(GameId::from_name(game)?)?;
    let info = game.info();
    let category = info.categories.iter().find(|c| **c == category)?;
    Some(RatingKey {
        game: info.id,
        category,
    })
}

/// The game a player asked for by connecting as `ssh <user>@…`, such as
/// `snake`, ignoring case.
pub fn for_ssh_user(user: &str) -> Option<GameId> {
    GAMES
        .iter()
        .map(|g| g.info())
        .find(|info| info.ssh_users.iter().any(|u| u.eq_ignore_ascii_case(user)))
        .map(|info| info.id)
}

/// Every game's screens, for a new player.
pub fn uis() -> Vec<Box<dyn GameUi>> {
    GAMES.iter().map(|g| g.ui()).collect()
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
