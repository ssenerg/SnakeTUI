//! Chess as this server plays it: quick pairing at each preset, rated by
//! Glicko-2 in the preset's category, and private challenges with any time
//! control, unrated.

use std::sync::Arc;
use std::time::Duration;

use chess_core::control::{PRESET_NAMES, PRESETS, TimeControl};
use chess_core::round::{Chess, ChessSettings, LEVELS, Side};
use chess_tui::ChessUi;
use game_api::{Entrant, Game, GameId, Room, RoomKind, RoomSettings};
use shell_tui::GameUi;

use super::{AnyRoomView, GameInfo, GameServer, PoolInfo, RatingSystem, decode, decode_view};
use crate::room::NewRoom;

/// How long a finished game shows before the room's lobby, where a
/// rematch starts once both are ready.
const RESULTS_FOR: Duration = Duration::from_secs(15);

/// One preset's quick pairing: two players, matched as soon as there are
/// two, rated in its category.
const fn pool(id: &'static str, category: &'static str) -> PoolInfo {
    PoolInfo {
        id,
        category,
        seats: 2..=2,
        fill_wait: Duration::ZERO,
    }
}

static INFO: GameInfo = GameInfo {
    id: GameId::Chess,
    title: "Chess",
    ssh_users: &["chess"],
    rating: RatingSystem::Glicko2,
    categories: &["bullet", "blitz", "rapid", "classical"],
    // Each of chess_core's PRESETS, by its name.
    pools: &[
        pool(PRESET_NAMES[0], "bullet"),
        pool(PRESET_NAMES[1], "bullet"),
        pool(PRESET_NAMES[2], "blitz"),
        pool(PRESET_NAMES[3], "blitz"),
        pool(PRESET_NAMES[4], "blitz"),
        pool(PRESET_NAMES[5], "blitz"),
        pool(PRESET_NAMES[6], "rapid"),
        pool(PRESET_NAMES[7], "rapid"),
        pool(PRESET_NAMES[8], "rapid"),
        pool(PRESET_NAMES[9], "classical"),
        pool(PRESET_NAMES[10], "classical"),
    ],
};

/// The preset a pool plays.
fn preset(pool: &PoolInfo) -> Option<TimeControl> {
    let i = PRESET_NAMES.iter().position(|&name| name == pool.id)?;
    Some(PRESETS[i])
}

/// A room playing by `settings`: a game starts once both players are
/// ready, never by itself.
fn settings(settings: ChessSettings, kind: RoomKind) -> Option<RoomSettings<Chess>> {
    let mut room = RoomSettings::new(settings, kind)?;
    room.start_after = None;
    room.results_for = RESULTS_FOR;
    Some(room)
}

pub struct ChessServer;

impl GameServer for ChessServer {
    fn info(&self) -> &'static GameInfo {
        &INFO
    }

    fn decode_view(&self, body: &[u8]) -> Option<Arc<dyn AnyRoomView>> {
        decode_view::<Chess>(body)
    }

    /// A game at the pool's preset, with sides drawn.
    fn new_match(&self, pool: &PoolInfo, seed: u64, players: Vec<Entrant>) -> Box<dyn NewRoom> {
        let chess = ChessSettings {
            control: preset(pool).unwrap_or(ChessSettings::default().control),
            side: Side::Random,
            ..ChessSettings::default()
        };
        let room = settings(chess, RoomKind::Match).expect("chess seats two in any room");
        Box::new(Room::<Chess>::new_match(room, seed, players))
    }

    /// The pool's preset against the computer, at the level chosen, with
    /// sides drawn.
    fn bot_match(
        &self,
        pool: &PoolInfo,
        seed: u64,
        who: Entrant,
        choice: &[u8],
    ) -> Option<Box<dyn NewRoom>> {
        let level = decode::<u8>(choice).filter(|level| LEVELS.contains(level));
        let chess = ChessSettings {
            control: preset(pool)?,
            side: Side::Random,
            level: level.unwrap_or(ChessSettings::default().level),
        };
        let room = settings(chess, RoomKind::Match)?;
        let players = vec![who, Chess::bot(0)];
        Some(Box::new(Room::<Chess>::new_match(room, seed, players)))
    }

    fn private_room(&self, bytes: &[u8], seed: u64) -> Result<Box<dyn NewRoom>, &'static str> {
        let room = decode::<ChessSettings>(bytes)
            .filter(|s| s.control.is_playable())
            .and_then(|s| settings(s, RoomKind::Private))
            .ok_or("That time control can't be played. Choose another.")?;
        Ok(Box::new(Room::<Chess>::new(room, seed)))
    }

    fn ui(&self) -> Box<dyn GameUi> {
        Box::new(ChessUi::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_preset_has_a_pool_rated_in_its_category() {
        assert_eq!(INFO.pools.len(), PRESETS.len());
        for (pool, preset) in INFO.pools.iter().zip(PRESETS) {
            assert_eq!(super::preset(pool), Some(preset));
            assert_eq!(pool.category, preset.category().name(), "{}", pool.id);
            assert!(INFO.categories.contains(&pool.category));
        }
    }
}
