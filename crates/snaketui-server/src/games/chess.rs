//! Chess as this server plays it: private challenges with any time
//! control, unrated. Quick pairing comes later.

use std::sync::Arc;
use std::time::Duration;

use chess_core::round::{Chess, ChessSettings};
use chess_tui::ChessUi;
use game_api::{Entrant, GameId, Room, RoomKind, RoomSettings};
use shell_tui::GameUi;

use super::{AnyRoomView, GameInfo, GameServer, PoolInfo, RatingSystem, decode, decode_view};
use crate::room::NewRoom;

/// How long a finished game shows before the room's lobby, where a
/// rematch starts once both are ready.
const RESULTS_FOR: Duration = Duration::from_secs(15);

static INFO: GameInfo = GameInfo {
    id: GameId::Chess,
    title: "Chess",
    ssh_users: &["chess"],
    rating: RatingSystem::Glicko2,
    categories: &["bullet", "blitz", "rapid", "classical"],
    pools: &[],
};

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

    /// Chess has no pools yet, so nothing is matched.
    fn new_match(&self, _: &PoolInfo, seed: u64, players: Vec<Entrant>) -> Box<dyn NewRoom> {
        let room = settings(ChessSettings::default(), RoomKind::Match)
            .expect("chess seats two in any room");
        Box::new(Room::<Chess>::new_match(room, seed, players))
    }

    fn bot_match(&self, _: u64, _: Entrant) -> Option<Box<dyn NewRoom>> {
        None
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
