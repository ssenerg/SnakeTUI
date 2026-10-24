//! Private rooms: making them, and finding one by its code.
//!
//! In a cluster, codes are kept in the shared database, so a code made on
//! one server finds its room from any other.

use std::collections::HashMap;
use std::sync::Arc;

use actix::prelude::*;
use std::time::Duration;

use rand::RngExt;
use snake_core::config::Rules;
use snake_core::round::{RoomKind, Round, RoundSettings};
use tracing::warn;

use crate::cluster::Cluster;
use crate::room::{RoomActor, RoomHandle, RoomHost, RoomRef};
use crate::store::{Store, now};

/// Codes use letters and digits that can't be mistaken for each other: no
/// I, O, 0 or 1.
const CODE_CHARS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
pub const CODE_LEN: usize = 6;
/// Codes tried before giving up, in case they keep being taken.
const CODE_TRIES: usize = 10;
/// In a cluster, each server renews its codes this often. A code not
/// renewed for [`CODES_MAX_AGE`] belongs to a server that's gone, and is
/// free again.
const CODES_REFRESH: Duration = Duration::from_secs(10 * 60);
const CODES_MAX_AGE: Duration = Duration::from_secs(30 * 60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CreateError {
    /// Two snakes don't fit on a board with these rules.
    BoardTooSmall,
    /// The cluster's database couldn't keep a code just now.
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
    store: Store,
    /// The rooms here, by code.
    rooms: HashMap<String, RoomHandle>,
}

impl LobbyActor {
    pub fn new(host: Arc<RoomHost>, cluster: Option<Arc<Cluster>>, store: Store) -> Self {
        Self {
            host,
            cluster,
            store,
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

/// Codes renewed after this are still in use.
fn fresh_since() -> i64 {
    now() - CODES_MAX_AGE.as_secs() as i64
}

/// A code no room in the cluster has, now kept for `at`.
async fn claim_code(store: Store, at: RoomRef) -> Result<String, CreateError> {
    for _ in 0..CODE_TRIES {
        let code = new_code();
        match store.claim_code(&code, at, now(), fresh_since()).await {
            Ok(true) => return Ok(code),
            Ok(false) => continue,
            Err(e) => {
                warn!(error = %e, "couldn't keep a room code");
                return Err(CreateError::Unavailable);
            }
        }
    }
    Err(CreateError::Unavailable)
}

impl Actor for LobbyActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        if let Some(cluster) = &self.cluster {
            let server = cluster.server;
            ctx.run_interval(CODES_REFRESH, move |act, _| {
                let store = act.store.clone();
                actix::spawn(async move {
                    if let Err(e) = store.renew_codes(server, now()).await {
                        warn!(error = %e, "couldn't renew this server's room codes");
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
            Some(_) => Box::pin(claim_code(self.store.clone(), self.host.at(id))),
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
        let store = self.store.clone();
        Box::pin(async move {
            let at = store.find_code(&code, fresh_since()).await.ok()??;
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
        let Some(room) = self.rooms.remove(&code) else {
            return;
        };
        if self.cluster.is_some() {
            let store = self.store.clone();
            actix::spawn(async move {
                if let Err(e) = store.forget_code(&code, room.at()).await {
                    warn!(error = %e, "couldn't forget a room code");
                }
            });
        }
    }
}
