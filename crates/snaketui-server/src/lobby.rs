//! Private rooms: making them, and finding one by its code.
//!
//! In a cluster, codes are kept in NATS, so a code made on one server finds
//! its room from any other.

use std::collections::HashMap;
use std::sync::Arc;

use actix::prelude::*;
use async_nats::jetstream::kv::CreateErrorKind;
use rand::RngExt;
use snake_core::config::Rules;
use snake_core::round::{RoomKind, Round, RoundSettings};
use tracing::warn;

use crate::cluster::wire::{decode, encode};
use crate::cluster::{CODES_REFRESH, Cluster};
use crate::room::{RoomActor, RoomHandle, RoomHost, RoomRef};

/// Codes use letters and digits that can't be mistaken for each other: no
/// I, O, 0 or 1.
const CODE_CHARS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
pub const CODE_LEN: usize = 6;
/// Codes tried before giving up, in case NATS keeps saying they're taken.
const CODE_TRIES: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CreateError {
    /// Two snakes don't fit on a board with these rules.
    BoardTooSmall,
    /// The cluster couldn't keep a code just now.
    Unavailable,
}

/// Makes a private room with `rules`, made multiplayer-safe. The creator
/// then joins it like anyone else.
#[derive(Message)]
#[rtype(result = "Result<(String, RoomHandle), CreateError>")]
pub struct CreateRoom {
    pub rules: Rules,
    /// Fixes the room's seed, for tests.
    pub seed: Option<u64>,
}

/// The room with this code, if it's still open.
#[derive(Message)]
#[rtype(result = "Option<RoomHandle>")]
pub struct FindRoom(pub String);

/// Sent by a room as it closes.
#[derive(Message)]
#[rtype(result = "()")]
pub struct RoomClosed(pub String);

pub struct LobbyActor {
    host: Arc<RoomHost>,
    cluster: Option<Arc<Cluster>>,
    /// The rooms here, by code.
    rooms: HashMap<String, RoomHandle>,
}

impl LobbyActor {
    pub fn new(host: Arc<RoomHost>, cluster: Option<Arc<Cluster>>) -> Self {
        Self {
            host,
            cluster,
            rooms: HashMap::new(),
        }
    }
}

fn new_code() -> String {
    let mut rng = rand::rng();
    (0..CODE_LEN)
        .map(|_| CODE_CHARS[rng.random_range(0..CODE_CHARS.len())] as char)
        .collect()
}

/// A code no room in the cluster has, now kept for `at`.
async fn claim_code(cluster: Arc<Cluster>, at: RoomRef) -> Result<String, CreateError> {
    for _ in 0..CODE_TRIES {
        let code = new_code();
        match cluster.codes.create(&code, encode(&at)).await {
            Ok(_) => return Ok(code),
            Err(e) if e.kind() == CreateErrorKind::AlreadyExists => continue,
            Err(e) => {
                warn!(error = %e, "couldn't keep a room code in NATS");
                return Err(CreateError::Unavailable);
            }
        }
    }
    Err(CreateError::Unavailable)
}

impl Actor for LobbyActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        if self.cluster.is_some() {
            // Codes of a server that died expire; live ones are written
            // again before they would.
            ctx.run_interval(CODES_REFRESH, |act, _| {
                let Some(cluster) = act.cluster.clone() else {
                    return;
                };
                let codes: Vec<(String, RoomRef)> = act
                    .rooms
                    .iter()
                    .map(|(code, room)| (code.clone(), room.at()))
                    .collect();
                actix::spawn(async move {
                    for (code, at) in codes {
                        if let Err(e) = cluster.codes.put(&code, encode(&at)).await {
                            warn!(error = %e, "couldn't renew a room code in NATS");
                        }
                    }
                });
            });
        }
    }
}

impl Handler<CreateRoom> for LobbyActor {
    type Result = ResponseActFuture<Self, Result<(String, RoomHandle), CreateError>>;

    fn handle(&mut self, msg: CreateRoom, _: &mut Self::Context) -> Self::Result {
        let Some(settings) = RoundSettings::new(msg.rules, RoomKind::Private) else {
            return Box::pin(fut::ready(Err(CreateError::BoardTooSmall)));
        };
        let id = self.host.new_id();
        let code: std::pin::Pin<Box<dyn Future<Output = _>>> = match &self.cluster {
            Some(cluster) => Box::pin(claim_code(Arc::clone(cluster), self.host.at(id))),
            None => {
                let code = std::iter::repeat_with(new_code)
                    .find(|code| !self.rooms.contains_key(code))
                    .unwrap();
                Box::pin(std::future::ready(Ok(code)))
            }
        };
        let seed = msg.seed.unwrap_or_else(rand::random);
        Box::pin(code.into_actor(self).map(move |code, act, ctx| {
            let code = code?;
            let round = Round::new(settings, seed);
            let (room, _) = RoomActor::new(
                round,
                Some(code.clone()),
                Some(ctx.address()),
                HashMap::new(),
                None,
            );
            let room = act.host.start_as(id, room);
            act.rooms.insert(code.clone(), room.clone());
            Ok((code, room))
        }))
    }
}

impl Handler<FindRoom> for LobbyActor {
    type Result = ResponseFuture<Option<RoomHandle>>;

    fn handle(&mut self, FindRoom(code): FindRoom, _: &mut Self::Context) -> Self::Result {
        if let Some(room) = self.rooms.get(&code) {
            return Box::pin(std::future::ready(Some(room.clone())));
        }
        let Some(cluster) = self.cluster.clone() else {
            return Box::pin(std::future::ready(None));
        };
        Box::pin(async move {
            let saved = cluster.codes.get(code).await.ok()??;
            let at: RoomRef = decode(&saved)?;
            // A room here would have been found above; one on a server
            // that has gone is gone too.
            (at.server != cluster.server && cluster.alive(at.server))
                .then_some(RoomHandle::Remote { at, cluster })
        })
    }
}

impl Handler<RoomClosed> for LobbyActor {
    type Result = ();

    fn handle(&mut self, RoomClosed(code): RoomClosed, _: &mut Self::Context) {
        self.rooms.remove(&code);
        if let Some(cluster) = self.cluster.clone() {
            actix::spawn(async move {
                if let Err(e) = cluster.codes.delete(&code).await {
                    warn!(error = %e, "couldn't forget a room code in NATS");
                }
            });
        }
    }
}
