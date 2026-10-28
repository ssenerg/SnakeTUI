//! What servers send each other: postcard, after a version byte. A server
//! drops messages it can't read, so during an upgrade servers of different
//! versions ignore each other instead of crashing. Bump [`VERSION`] when a
//! message changes shape.

use std::time::Duration;

use bytes::Bytes;
use game_api::{Entrant, GameId, JoinError, PlayerId};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::games::PoolKey;
use crate::presence::Doing;
use crate::rating::Skill;
use crate::room::{RoomCmd, RoomRef, ServerId};
use crate::store::AccountId;

pub const VERSION: u8 = 12;

/// Subjects for one server: `<prefix>.server.<server>.<what>`.
pub fn server_subject(prefix: &str, server: ServerId, what: &str) -> String {
    format!("{prefix}.server.{server:016x}.{what}")
}

pub fn encode<T: Serialize>(msg: &T) -> Bytes {
    postcard::to_extend(msg, vec![VERSION])
        .expect("messages always serialize")
        .into()
}

/// `None` for another version's message, or a broken one.
pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Option<T> {
    match bytes.split_first() {
        Some((&VERSION, rest)) => postcard::from_bytes(rest).ok(),
        _ => None,
    }
}

/// Twice a second: this server is still here, where it is, and how busy.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Alive {
    pub server: ServerId,
    pub region: String,
    /// Players connected.
    pub load: u32,
    /// Stopping: it takes no new matches.
    pub draining: bool,
    /// The games it hosts matches of.
    pub games: Vec<GameId>,
}

/// Twice a second, from the cluster's queue (`snaketui-matchmaker`). A new
/// `instance` means it restarted, and servers send their players again.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct QueueAlive {
    pub instance: u64,
    pub waiting: u32,
}

/// A player waiting for a match, sent to the cluster's queue.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QueueTicket {
    pub pool: PoolKey,
    pub who: Entrant,
    pub account: Option<AccountId>,
    pub rating: Skill,
    pub region: String,
    pub server: ServerId,
    /// How long they've waited so far, which a restarted queue keeps.
    pub waited: Duration,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct QueueLeave {
    pub player: PlayerId,
}

/// One player of a match, as the queue tells the server that will host it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MatchSeat {
    pub who: Entrant,
    pub account: Option<AccountId>,
    pub rating: Skill,
    pub server: ServerId,
}

/// Asks a server to host a match, longest-waiting player first. It answers
/// with the room, once everyone's seated.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct MakeMatch {
    pub pool: PoolKey,
    pub seats: Vec<MatchSeat>,
}

pub type MadeMatch = Option<RoomRef>;

/// To a player's server: their match is ready, and they're seated in it.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct MatchedAt {
    pub player: PlayerId,
    pub room: RoomRef,
}

/// To a player's server: how their last match moved their rating in a
/// game's `category`.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct RatedAt {
    pub player: PlayerId,
    pub game: GameId,
    pub category: String,
    pub rating: Skill,
    pub won: bool,
}

/// A player on server `from` would like to be in a room here.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct JoinRequest {
    pub who: Entrant,
    pub from: ServerId,
    pub how: JoinAs,
}

/// What a player joining a room is after.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum JoinAs {
    /// A seat, signed in to `account` if any.
    Seat { account: Option<AccountId> },
    /// Only to watch, without a seat.
    Watch,
    /// The seat `account` has in the round being played, from a connection
    /// that's gone.
    Rejoin { account: AccountId },
}

/// Where is `account` seated in a round being played? Every other server
/// answers with a [`SeatReply`].
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct SeatQuery {
    pub from: ServerId,
    pub account: AccountId,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct SeatReply(pub Option<RoomRef>);

/// A room's view: its game, and the view as that game writes it
/// ([`crate::games::AnyRoomView::to_wire`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireView {
    pub game: GameId,
    pub body: Vec<u8>,
}

/// The room as it is now, to start from.
pub type JoinReply = Result<WireView, JoinError>;

/// A key from a player on another server.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Input {
    pub player: PlayerId,
    pub cmd: RoomCmd,
}

/// What are these accounts doing? Every other server answers for its own
/// players.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct StatusQuery {
    pub from: ServerId,
    pub accounts: Vec<AccountId>,
}

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StatusReply(pub Vec<(AccountId, Doing)>);

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use snake_core::round::{Room, RoomKind, RoomSettings, Snake, quick_rules};

    use super::*;
    use crate::games::{self, AnyRoomView, view_of};

    #[test]
    fn a_running_room_crosses_the_wire_whole_and_small() {
        let settings = RoomSettings::new(quick_rules(), RoomKind::Match).unwrap();
        let players = [1, 2, 3]
            .map(|id| Entrant {
                id,
                name: format!("p{id}"),
            })
            .to_vec();
        let mut room = Room::new_match(settings, 7, players);
        room.advance(Duration::from_secs(4));
        let view = room.view();
        assert!(view.game.board.is_some());
        let bytes = encode(&view.to_wire());
        assert!(bytes.len() < 400, "a 30x18 match is {} bytes", bytes.len());
        let wire: WireView = decode(&bytes).unwrap();
        let back = games::get(wire.game)
            .unwrap()
            .decode_view(&wire.body)
            .unwrap();
        assert_eq!(view_of::<Snake>(back).as_deref(), Some(&view));
    }

    #[test]
    fn a_broken_view_is_not_read() {
        let room = Room::new(
            RoomSettings::new(quick_rules(), RoomKind::Private).unwrap(),
            1,
        );
        let wire = room.view().to_wire();
        let snake = games::get(GameId::Snake).unwrap();
        assert!(
            snake
                .decode_view(&wire.body[..wire.body.len() / 2])
                .is_none()
        );
        let view: Arc<dyn AnyRoomView> = snake.decode_view(&wire.body).unwrap();
        assert_eq!(view.game(), GameId::Snake);
    }

    #[test]
    fn other_versions_and_garbage_are_dropped() {
        let alive = || Alive {
            server: 9,
            region: "eu".into(),
            load: 3,
            draining: false,
            games: vec![GameId::Snake],
        };
        let mut bytes = encode(&alive()).to_vec();
        assert_eq!(decode(&bytes), Some(alive()));
        bytes[0] = VERSION + 1;
        assert_eq!(decode::<Alive>(&bytes), None);
        assert_eq!(decode::<Alive>(&[]), None);
        assert_eq!(decode::<JoinRequest>(&[VERSION, 0xff]), None);
    }
}
