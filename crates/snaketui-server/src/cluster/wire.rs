//! What servers send each other: postcard, after a version byte. A server
//! drops messages it can't read, so during an upgrade servers of different
//! versions ignore each other instead of crashing. Bump [`VERSION`] when a
//! message changes shape.

use bytes::Bytes;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use snake_core::geom::Direction;
use snake_core::round::{Entrant, JoinError, PlayerId, RoomView};

use crate::presence::Status;
use crate::room::ServerId;
use crate::store::AccountId;

pub const VERSION: u8 = 1;

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

/// Twice a second: this server is still here.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Alive {
    pub server: ServerId,
}

/// A player on server `from` would like a seat in a room here.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct JoinRequest {
    pub who: Entrant,
    pub from: ServerId,
}

/// The room as it is now, to start from.
pub type JoinReply = Result<RoomView, JoinError>;

/// A key from a player on another server.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Input {
    pub player: PlayerId,
    pub kind: InputKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputKind {
    Steer(Direction),
    Ready,
    Leave,
}

/// What are these accounts doing? Every other server answers for its own
/// players.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct StatusQuery {
    pub from: ServerId,
    pub accounts: Vec<AccountId>,
}

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StatusReply(pub Vec<(AccountId, Status)>);

#[cfg(test)]
mod tests {
    use snake_core::round::{RoomKind, Round, RoundSettings, quick_rules};

    use super::*;

    #[test]
    fn a_running_room_crosses_the_wire_whole_and_small() {
        let settings = RoundSettings::new(quick_rules(), RoomKind::Match).unwrap();
        let players = [1, 2, 3]
            .map(|id| Entrant {
                id,
                name: format!("p{id}"),
            })
            .to_vec();
        let mut round = Round::new_match(settings, 7, players);
        round.advance(std::time::Duration::from_secs(4));
        let view = round.view();
        assert!(view.board.is_some());
        let bytes = encode(&view);
        assert!(bytes.len() < 400, "a 30x18 match is {} bytes", bytes.len());
        assert_eq!(decode::<RoomView>(&bytes), Some(view));
    }

    #[test]
    fn other_versions_and_garbage_are_dropped() {
        let mut bytes = encode(&Alive { server: 9 }).to_vec();
        assert_eq!(decode(&bytes), Some(Alive { server: 9 }));
        bytes[0] = VERSION + 1;
        assert_eq!(decode::<Alive>(&bytes), None);
        assert_eq!(decode::<Alive>(&[]), None);
        assert_eq!(decode::<JoinRequest>(&[VERSION, 0xff]), None);
    }
}
