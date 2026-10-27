//! Snake as this server plays it: quick play is one free-for-all pool, on
//! the quick-play board, rated by Weng-Lin.

use std::sync::Arc;
use std::time::Duration;

use game_api::{Entrant, GameId, MIN_PLAYERS};
use snake_core::round::{MAX_PLAYERS, RoomKind, RoomSettings, Snake, bot_entrant, quick_rules};

use super::{AnyRoomView, GameInfo, GameServer, PoolInfo, PoolKey, RatingSystem, decode_view};
use crate::rating::SNAKE_QUICK;
use crate::room::NewRoom;

/// Every snake for itself.
pub const QUICK_POOL: &str = "ffa";

/// Bots in a match against bots.
const BOT_MATCH_BOTS: usize = 3;

static INFO: GameInfo = GameInfo {
    id: GameId::Snake,
    title: "Snake",
    ssh_users: &["snake"],
    rating: RatingSystem::WengLin,
    categories: &[SNAKE_QUICK.category],
    pools: &[PoolInfo {
        id: QUICK_POOL,
        category: SNAKE_QUICK.category,
        seats: MIN_PLAYERS..=MAX_PLAYERS,
        fill_wait: Duration::from_secs(10),
    }],
};

/// Quick play's pool.
pub fn quick_pool() -> PoolKey {
    PoolKey {
        game: GameId::Snake,
        pool: QUICK_POOL.to_string(),
    }
}

/// A matched room on the quick-play board.
fn match_settings() -> RoomSettings {
    RoomSettings::new(quick_rules(), RoomKind::Match)
        .expect("the quick-play board fits a full room")
}

pub struct SnakeServer;

impl GameServer for SnakeServer {
    fn info(&self) -> &'static GameInfo {
        &INFO
    }

    fn decode_view(&self, body: &[u8]) -> Option<Arc<dyn AnyRoomView>> {
        decode_view::<Snake>(body)
    }

    fn new_match(&self, _: &PoolInfo, seed: u64, players: Vec<Entrant>) -> Box<dyn NewRoom> {
        Box::new(game_api::Room::<Snake>::new_match(
            match_settings(),
            seed,
            players,
        ))
    }

    fn bot_match(&self, seed: u64, who: Entrant) -> Option<Box<dyn NewRoom>> {
        let mut players = vec![who];
        players.extend((0..BOT_MATCH_BOTS).map(bot_entrant));
        Some(Box::new(game_api::Room::<Snake>::new_match(
            match_settings(),
            seed,
            players,
        )))
    }
}
