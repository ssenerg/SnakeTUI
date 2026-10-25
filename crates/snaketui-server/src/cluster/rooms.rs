//! Rooms shared between servers.
//!
//! The room's own server keeps a seat for each player from elsewhere and
//! sends the room's views to the servers they're on, from the room's own
//! thread. Each of those servers takes the views into one local watch
//! channel per room, which its sessions read like a local room's.
//!
//! Subjects, under the cluster's prefix: `room.<server>.<room>.join` (a
//! request), `.in` (keys) and `.view`.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};

use actix_rt::ArbiterHandle;
use anyhow::Result;
use async_nats::{Message, Subscriber};
use futures::StreamExt;
use snake_core::round::{Entrant, JoinError, PhaseView, PlayerId, RoomView};
use tokio::sync::watch;
use tokio::task::AbortHandle;

use super::Cluster;
use super::wire::{Input, InputKind, JoinReply, JoinRequest, decode};
use crate::room::{Join, Leave, Ping, Ready, RoomId, RoomRef, ServerId, Steer, ViewRx};

pub const ROOM_LOST: &str = "The server running this room went away.";

/// Players from other servers in a room here.
#[derive(Default)]
pub(super) struct Export {
    guests: HashMap<PlayerId, Guest>,
    /// Which run of the room's sender is the live one; 0 before it starts.
    sender: u64,
}

struct Guest {
    server: ServerId,
    /// Holds the player's seat; dropping it lets the room drop them.
    _seat: Arc<()>,
}

/// A room on another server, with players here.
pub(super) struct Import {
    view: watch::Sender<Arc<RoomView>>,
    /// Each player's session, to notice when it's gone.
    members: HashMap<PlayerId, Weak<()>>,
    feed: AbortHandle,
}

static SENDERS: AtomicU64 = AtomicU64::new(0);

impl Cluster {
    fn room_subject(&self, at: RoomRef, what: &str) -> String {
        format!(
            "{}.room.{:016x}.{:016x}.{what}",
            self.prefix, at.server, at.room
        )
    }

    /// Answers other servers' players: their joins, and their keys.
    pub(super) async fn serve_rooms(self: &Arc<Self>) -> Result<()> {
        let mine = format!("{}.room.{:016x}", self.prefix, self.server);
        let mut joins = self.subscribe(format!("{mine}.*.join")).await?;
        let mut inputs = self.subscribe(format!("{mine}.*.in")).await?;
        let me = Arc::clone(self);
        tokio::spawn(async move {
            while let Some(msg) = joins.next().await {
                tokio::spawn(Arc::clone(&me).answer_join(msg));
            }
        });
        let me = Arc::clone(self);
        tokio::spawn(async move {
            while let Some(msg) = inputs.next().await {
                me.take_input(&msg);
            }
        });
        Ok(())
    }

    async fn answer_join(self: Arc<Self>, msg: Message) {
        let Some(reply) = msg.reply else { return };
        let answer: JoinReply = match (room_of(&msg.subject), decode(&msg.payload)) {
            (Some(room), Some(request)) => self.seat_guest(room, request).await,
            _ => Err(JoinError::Closed),
        };
        self.send(reply.to_string(), &answer);
    }

    async fn seat_guest(self: &Arc<Self>, room: RoomId, request: JoinRequest) -> JoinReply {
        let Some((addr, thread)) = self.rooms.get_with_thread(room) else {
            return Err(JoinError::Closed);
        };
        let seat = Arc::new(());
        let player = request.who.id;
        let join = Join {
            who: request.who,
            presence: Arc::downgrade(&seat),
        };
        let view = addr.send(join).await.unwrap_or(Err(JoinError::Closed))?;
        let now = (**view.borrow()).clone();
        self.exports
            .lock()
            .unwrap()
            .entry(room)
            .or_default()
            .guests
            .insert(
                player,
                Guest {
                    server: request.from,
                    _seat: seat,
                },
            );
        self.start_sending(room, &thread, view);
        Ok(now)
    }

    /// Seats for players from other servers in a match about to start here,
    /// held like those of guests who joined: they drop out if their server
    /// goes quiet. Answers what shows each one is still there.
    pub fn reserve_guests(
        &self,
        room: RoomId,
        guests: &[(PlayerId, ServerId)],
    ) -> HashMap<PlayerId, Weak<()>> {
        let mut exports = self.exports.lock().unwrap();
        let export = exports.entry(room).or_default();
        guests
            .iter()
            .map(|&(player, server)| {
                let seat = Arc::new(());
                let presence = Arc::downgrade(&seat);
                export.guests.insert(
                    player,
                    Guest {
                        server,
                        _seat: seat,
                    },
                );
                (player, presence)
            })
            .collect()
    }

    /// Starts sending the room's views to its guests' servers, from the
    /// room's own thread, unless that's already happening.
    pub fn start_sending(self: &Arc<Self>, room: RoomId, thread: &ArbiterHandle, view: ViewRx) {
        let sender = {
            let mut exports = self.exports.lock().unwrap();
            let Some(export) = exports.get_mut(&room).filter(|e| e.sender == 0) else {
                return;
            };
            export.sender = SENDERS.fetch_add(1, Ordering::Relaxed) + 1;
            export.sender
        };
        thread.spawn(Arc::clone(self).send_views(room, sender, view));
    }

    /// Sends the room's views to the servers its guests are on, until it
    /// closes or has no guests left.
    async fn send_views(self: Arc<Self>, room: RoomId, sender: u64, mut view: ViewRx) {
        let subject = self.room_subject(
            RoomRef {
                server: self.server,
                room,
            },
            "view",
        );
        loop {
            let open = view.changed().await.is_ok();
            let live = self
                .exports
                .lock()
                .unwrap()
                .get(&room)
                .is_some_and(|e| e.sender == sender);
            if !live {
                return;
            }
            if !open {
                self.exports.lock().unwrap().remove(&room);
                return;
            }
            let latest = view.borrow_and_update().clone();
            self.send(subject.clone(), &*latest);
        }
    }

    fn take_input(&self, msg: &Message) {
        let (Some(room), Some(Input { player, kind })) =
            (room_of(&msg.subject), decode(&msg.payload))
        else {
            return;
        };
        let Some(addr) = self.rooms.get(room) else {
            return;
        };
        match kind {
            InputKind::Steer(dir) => {
                let _ = addr.try_send(Steer { id: player, dir });
            }
            InputKind::Ready => {
                let _ = addr.try_send(Ready(player));
            }
            InputKind::Ping(rtt) => {
                let _ = addr.try_send(Ping { id: player, rtt });
            }
            InputKind::Leave => {
                addr.do_send(Leave(player));
                let mut exports = self.exports.lock().unwrap();
                if let Some(export) = exports.get_mut(&room) {
                    export.guests.remove(&player);
                    if export.guests.is_empty() {
                        exports.remove(&room);
                    }
                }
            }
        }
    }

    /// Takes a seat for `who` in the room at `at`, on another server.
    pub async fn join_remote(
        self: Arc<Self>,
        at: RoomRef,
        who: Entrant,
        presence: Weak<()>,
    ) -> Result<ViewRx, JoinError> {
        if !self.alive(at.server) {
            return Err(JoinError::Closed);
        }
        let player = who.id;
        // Listen before asking, so no view goes missing in between.
        let views = if self.imports.lock().unwrap().contains_key(&at) {
            None
        } else {
            let subject = self.room_subject(at, "view");
            Some(
                self.subscribe(subject)
                    .await
                    .map_err(|_| JoinError::Closed)?,
            )
        };
        let request = JoinRequest {
            who,
            from: self.server,
        };
        let reply: Option<JoinReply> = self.request(self.room_subject(at, "join"), &request).await;
        let now = reply.unwrap_or(Err(JoinError::Closed))?;
        let mut imports = self.imports.lock().unwrap();
        let import = match imports.entry(at) {
            Entry::Occupied(e) => e.into_mut(),
            Entry::Vacant(e) => {
                let Some(views) = views else {
                    // Its views stopped while we asked: it has just closed.
                    drop(imports);
                    self.send_input(at, player, InputKind::Leave);
                    return Err(JoinError::Closed);
                };
                let (view, _) = watch::channel(Arc::new(now));
                let feed = tokio::spawn(Arc::clone(&self).take_views(at, views, view.clone()));
                e.insert(Import {
                    view,
                    members: HashMap::new(),
                    feed: feed.abort_handle(),
                })
            }
        };
        import.members.insert(player, presence);
        Ok(import.view.subscribe())
    }

    /// Passes the room's views on to the players here, until it closes.
    async fn take_views(
        self: Arc<Self>,
        at: RoomRef,
        mut views: Subscriber,
        view: watch::Sender<Arc<RoomView>>,
    ) {
        while let Some(msg) = views.next().await {
            let Some(latest) = decode::<RoomView>(&msg.payload) else {
                continue;
            };
            let closed = matches!(latest.phase, PhaseView::Closed { .. });
            view.send_replace(Arc::new(latest));
            if closed {
                break;
            }
        }
        let mut imports = self.imports.lock().unwrap();
        if imports
            .get(&at)
            .is_some_and(|i| i.feed.id() == tokio::task::id())
        {
            imports.remove(&at);
        }
    }

    /// A key, or leaving, for a room on another server.
    pub fn send_input(&self, at: RoomRef, player: PlayerId, kind: InputKind) {
        if kind == InputKind::Leave {
            let mut imports = self.imports.lock().unwrap();
            if let Some(import) = imports.get_mut(&at) {
                import.members.remove(&player);
                if import.members.is_empty()
                    && let Some(gone) = imports.remove(&at)
                {
                    gone.feed.abort();
                }
            }
        }
        self.send(self.room_subject(at, "in"), &Input { player, kind });
    }

    /// Lets go of players whose server went quiet, closes rooms whose
    /// server did, and tells rooms elsewhere about players here who are
    /// gone.
    pub(super) fn sweep_rooms(&self, peers: &HashSet<ServerId>) {
        {
            let mut exports = self.exports.lock().unwrap();
            for export in exports.values_mut() {
                export.guests.retain(|_, g| peers.contains(&g.server));
            }
            exports.retain(|_, e| !e.guests.is_empty());
        }
        let mut left = Vec::new();
        self.imports.lock().unwrap().retain(|at, import| {
            if !peers.contains(&at.server) {
                let mut last = (**import.view.borrow()).clone();
                last.phase = PhaseView::Closed {
                    reason: ROOM_LOST.to_string(),
                };
                import.view.send_replace(Arc::new(last));
                import.feed.abort();
                return false;
            }
            import.members.retain(|player, session| {
                let here = session.strong_count() > 0;
                if !here {
                    left.push((*at, *player));
                }
                here
            });
            if import.members.is_empty() {
                import.feed.abort();
            }
            !import.members.is_empty()
        });
        for (at, player) in left {
            let leave = Input {
                player,
                kind: InputKind::Leave,
            };
            self.send(self.room_subject(at, "in"), &leave);
        }
    }
}

/// The room in `<prefix>.room.<server>.<room>.<what>`.
fn room_of(subject: &str) -> Option<RoomId> {
    let mut tokens = subject.rsplit('.');
    tokens.next()?;
    RoomId::from_str_radix(tokens.next()?, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rooms_are_read_from_subjects() {
        let subject = format!("snaketui.play.room.{:016x}.{:016x}.in", 7, 0xbeef);
        assert_eq!(room_of(&subject), Some(0xbeef));
        assert_eq!(room_of("snaketui.play.room.x.zz.in"), None);
    }
}
