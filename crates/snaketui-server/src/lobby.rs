//! Private rooms: making them, and finding one by its code.

use std::collections::HashMap;
use std::sync::Arc;

use actix::prelude::*;
use rand::RngExt;
use snake_core::config::Rules;
use snake_core::round::{RoomKind, Round, RoundSettings};

use crate::room::{RoomActor, RoomHost};

/// Codes use letters and digits that can't be mistaken for each other: no
/// I, O, 0 or 1.
const CODE_CHARS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
pub const CODE_LEN: usize = 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CreateError {
    /// Two snakes don't fit on a board with these rules.
    BoardTooSmall,
}

/// Makes a private room with `rules`, made multiplayer-safe. The creator
/// then joins it like anyone else.
#[derive(Message)]
#[rtype(result = "Result<(String, Addr<RoomActor>), CreateError>")]
pub struct CreateRoom {
    pub rules: Rules,
    /// Fixes the room's seed, for tests.
    pub seed: Option<u64>,
}

/// The room with this code, if it's still open.
#[derive(Message)]
#[rtype(result = "Option<Addr<RoomActor>>")]
pub struct FindRoom(pub String);

/// Sent by a room as it closes.
#[derive(Message)]
#[rtype(result = "()")]
pub struct RoomClosed(pub String);

pub struct LobbyActor {
    host: Arc<RoomHost>,
    rooms: HashMap<String, Addr<RoomActor>>,
}

impl LobbyActor {
    pub fn new(host: Arc<RoomHost>) -> Self {
        Self {
            host,
            rooms: HashMap::new(),
        }
    }

    fn new_code(&self) -> String {
        let mut rng = rand::rng();
        loop {
            let code: String = (0..CODE_LEN)
                .map(|_| CODE_CHARS[rng.random_range(0..CODE_CHARS.len())] as char)
                .collect();
            if !self.rooms.contains_key(&code) {
                return code;
            }
        }
    }
}

impl Actor for LobbyActor {
    type Context = Context<Self>;
}

impl Handler<CreateRoom> for LobbyActor {
    type Result = Result<(String, Addr<RoomActor>), CreateError>;

    fn handle(&mut self, msg: CreateRoom, ctx: &mut Self::Context) -> Self::Result {
        let settings =
            RoundSettings::new(msg.rules, RoomKind::Private).ok_or(CreateError::BoardTooSmall)?;
        let code = self.new_code();
        let round = Round::new(settings, msg.seed.unwrap_or_else(rand::random));
        let (room, _) = RoomActor::new(
            round,
            Some(code.clone()),
            Some(ctx.address()),
            HashMap::new(),
        );
        let addr = self.host.start(room);
        self.rooms.insert(code.clone(), addr.clone());
        Ok((code, addr))
    }
}

impl Handler<FindRoom> for LobbyActor {
    type Result = Option<Addr<RoomActor>>;

    fn handle(&mut self, FindRoom(code): FindRoom, _: &mut Self::Context) -> Self::Result {
        self.rooms.get(&code).filter(|a| a.connected()).cloned()
    }
}

impl Handler<RoomClosed> for LobbyActor {
    type Result = ();

    fn handle(&mut self, RoomClosed(code): RoomClosed, _: &mut Self::Context) {
        self.rooms.remove(&code);
    }
}
