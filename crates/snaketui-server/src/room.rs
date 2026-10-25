//! One multiplayer room: its round, its clock, and the view it publishes.
//!
//! A room never waits on its players. It publishes each new view on a watch
//! channel, and every session reads the latest one at its own pace.

use std::collections::HashMap;
use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use actix::prelude::*;
use actix_rt::ArbiterHandle;
use serde::{Deserialize, Serialize};
use snake_core::geom::Direction;
use snake_core::round::{
    Entrant, JoinError, Outcome, PhaseView, Placement, PlayerId, RatingChange, RoomKind, RoomView,
    Round, RoundEvent,
};
use tokio::sync::watch;
use tracing::{debug, warn};

use crate::cluster::Cluster;
use crate::cluster::wire::{InputKind, RatedAt};
use crate::lobby::{LobbyActor, RoomClosed};
use crate::metrics::TICK_LAG;
use crate::rating::{self, Rating};
use crate::store::{AccountId, RatedResult, Store};

pub type ViewRx = watch::Receiver<Arc<RoomView>>;

/// Shows that a player's session is still there: the session holds the
/// `Arc`, rooms and the queue a `Weak`. A session that dies without saying
/// goodbye can't leave a ghost snake behind.
pub type Presence = Weak<()>;

/// Steering and ready keys waiting in a room's mailbox; more are dropped.
const MAILBOX: usize = 256;
/// A finished match closes this long after its results first show.
const MATCH_LINGER: Duration = Duration::from_secs(60);
const BROKEN: &str = "Something went wrong in this room, so it closed. Sorry!";
const MATCH_OVER: &str = "The match is over.";
const DRAINED: &str = "This room's server is restarting. Reconnect to keep playing.";
/// While the server drains: how long a finished round's results show
/// before its players go.
pub const RESULTS_GRACE: Duration = Duration::from_secs(5);

/// A server in a cluster. A server on its own is 0.
pub type ServerId = u64;
/// A room, among those of the server running it.
pub type RoomId = u64;

/// Where a room runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RoomRef {
    pub server: ServerId,
    pub room: RoomId,
}

/// A room here, and the thread it runs on.
type Running = (Addr<RoomActor>, ArbiterHandle);

/// The rooms this server runs.
#[derive(Clone, Default)]
pub struct LocalRooms(Arc<Mutex<HashMap<RoomId, Running>>>);

impl LocalRooms {
    pub fn get(&self, id: RoomId) -> Option<Addr<RoomActor>> {
        self.0
            .lock()
            .unwrap()
            .get(&id)
            .map(|(addr, _)| addr.clone())
    }

    /// The room, and the thread it runs on, for work that belongs with it.
    pub fn get_with_thread(&self, id: RoomId) -> Option<Running> {
        self.0.lock().unwrap().get(&id).cloned()
    }

    fn insert(&self, id: RoomId, addr: Addr<RoomActor>, thread: ArbiterHandle) {
        self.0.lock().unwrap().insert(id, (addr, thread));
    }

    fn remove(&self, id: RoomId) {
        self.0.lock().unwrap().remove(&id);
    }

    pub fn len(&self) -> usize {
        self.0.lock().unwrap().len()
    }

    pub fn all(&self) -> Vec<Addr<RoomActor>> {
        self.0
            .lock()
            .unwrap()
            .values()
            .map(|(addr, _)| addr.clone())
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Starts rooms on the room threads, in turn, and keeps them in
/// [`LocalRooms`] while they run.
pub struct RoomHost {
    server: ServerId,
    rooms: LocalRooms,
    arbiters: Vec<ArbiterHandle>,
    next: AtomicUsize,
}

impl RoomHost {
    pub fn new(server: ServerId, rooms: LocalRooms, arbiters: Vec<ArbiterHandle>) -> Self {
        assert!(!arbiters.is_empty());
        Self {
            server,
            rooms,
            arbiters,
            next: AtomicUsize::new(0),
        }
    }

    pub fn rooms(&self) -> &LocalRooms {
        &self.rooms
    }

    /// An id no room here has.
    pub fn new_id(&self) -> RoomId {
        loop {
            let id: RoomId = rand::random();
            if self.rooms.get(id).is_none() {
                return id;
            }
        }
    }

    /// Where the room with `id` runs.
    pub fn at(&self, id: RoomId) -> RoomRef {
        RoomRef {
            server: self.server,
            room: id,
        }
    }

    /// Starts `room` on the next room thread.
    pub fn start(&self, room: RoomActor) -> RoomHandle {
        self.start_as(self.new_id(), room)
    }

    /// Starts `room` as `id`, from [`Self::new_id`].
    pub fn start_as(&self, id: RoomId, mut room: RoomActor) -> RoomHandle {
        room.registered = Some((id, self.rooms.clone()));
        let i = self.next.fetch_add(1, Ordering::Relaxed) % self.arbiters.len();
        let thread = self.arbiters[i].clone();
        let addr = RoomActor::start_in_arbiter(&thread, move |_| room);
        self.rooms.insert(id, addr.clone(), thread);
        RoomHandle::Local {
            at: self.at(id),
            addr,
        }
    }
}

/// A room to join and play in, wherever it runs.
#[derive(Clone)]
pub enum RoomHandle {
    Local {
        at: RoomRef,
        addr: Addr<RoomActor>,
    },
    /// On another server: keys go there through NATS, and its views come
    /// back.
    Remote {
        at: RoomRef,
        cluster: Arc<Cluster>,
    },
}

impl RoomHandle {
    pub fn at(&self) -> RoomRef {
        match self {
            RoomHandle::Local { at, .. } | RoomHandle::Remote { at, .. } => *at,
        }
    }

    /// Takes a seat for `who`; `presence` shows the player is still there.
    pub fn join(
        &self,
        who: Entrant,
        presence: Presence,
    ) -> impl Future<Output = Result<ViewRx, JoinError>> + use<> {
        let room = self.clone();
        async move {
            match room {
                RoomHandle::Local { addr, .. } => addr
                    .send(Join { who, presence })
                    .await
                    .unwrap_or(Err(JoinError::Closed)),
                RoomHandle::Remote { at, cluster } => cluster.join_remote(at, who, presence).await,
            }
        }
    }

    /// Steering waits in the room's mailbox; a full one drops it.
    pub fn steer(&self, id: PlayerId, dir: Direction) {
        match self {
            RoomHandle::Local { addr, .. } => {
                let _ = addr.try_send(Steer { id, dir });
            }
            RoomHandle::Remote { at, cluster } => {
                cluster.send_input(*at, id, InputKind::Steer(dir))
            }
        }
    }

    pub fn ready(&self, id: PlayerId) {
        match self {
            RoomHandle::Local { addr, .. } => {
                let _ = addr.try_send(Ready(id));
            }
            RoomHandle::Remote { at, cluster } => cluster.send_input(*at, id, InputKind::Ready),
        }
    }

    pub fn leave(&self, id: PlayerId) {
        match self {
            RoomHandle::Local { addr, .. } => addr.do_send(Leave(id)),
            RoomHandle::Remote { at, cluster } => cluster.send_input(*at, id, InputKind::Leave),
        }
    }

    pub fn set_speed(&self, id: PlayerId, tick_ms: u64) {
        match self {
            RoomHandle::Local { addr, .. } => {
                let _ = addr.try_send(SetSpeed { id, tick_ms });
            }
            RoomHandle::Remote { at, cluster } => {
                cluster.send_input(*at, id, InputKind::Speed(tick_ms))
            }
        }
    }

    pub fn ping(&self, id: PlayerId, rtt: Duration) {
        match self {
            RoomHandle::Local { addr, .. } => {
                let _ = addr.try_send(Ping { id, rtt });
            }
            RoomHandle::Remote { at, cluster } => cluster.send_input(*at, id, InputKind::Ping(rtt)),
        }
    }
}

/// A player's new rating after a rated round, for their session.
#[derive(Message)]
#[rtype(result = "()")]
pub struct Rated {
    pub rating: Rating,
    pub won: bool,
}

/// A player in a rated room.
pub struct RatedMember {
    /// Guests play at the starting rating, and nothing is saved for them.
    pub account: Option<AccountId>,
    pub rating: Rating,
    pub notify: RatedNotify,
}

/// Where a player's new rating goes.
pub enum RatedNotify {
    /// Their session, on this server.
    Here(Recipient<Rated>),
    /// Their server, which passes it on.
    There {
        cluster: Arc<Cluster>,
        server: ServerId,
        player: PlayerId,
    },
    /// They left before the match started.
    Nobody,
}

impl RatedNotify {
    fn send(&self, rated: Rated) {
        match self {
            RatedNotify::Here(session) => session.do_send(rated),
            RatedNotify::There {
                cluster,
                server,
                player,
            } => {
                let news = RatedAt {
                    player: *player,
                    rating: rated.rating,
                    won: rated.won,
                };
                cluster.send(cluster.server_subject(*server, "rated"), &news);
            }
            RatedNotify::Nobody => {}
        }
    }
}

/// What a rated room needs: everyone's rating, and where to save new ones.
pub struct Ratings {
    pub store: Store,
    pub members: HashMap<PlayerId, RatedMember>,
}

#[derive(Message)]
#[rtype(result = "Result<ViewRx, JoinError>")]
pub struct Join {
    pub who: Entrant,
    pub presence: Presence,
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct Leave(pub PlayerId);

#[derive(Message)]
#[rtype(result = "()")]
pub struct Steer {
    pub id: PlayerId,
    pub dir: Direction,
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct Ready(pub PlayerId);

/// The host picks the next round's speed.
#[derive(Message)]
#[rtype(result = "()")]
pub struct SetSpeed {
    pub id: PlayerId,
    pub tick_ms: u64,
}

/// A player's latest round trip, for everyone in the room to see.
#[derive(Message)]
#[rtype(result = "()")]
pub struct Ping {
    pub id: PlayerId,
    pub rtt: Duration,
}

/// The server is stopping: finish the round in play, if any, then close.
#[derive(Clone, Copy, Message)]
#[rtype(result = "()")]
pub struct Drain;

pub struct RoomActor {
    /// Its id, and the list of this server's rooms it leaves on closing.
    registered: Option<(RoomId, LocalRooms)>,
    round: Round,
    code: Option<String>,
    view: watch::Sender<Arc<RoomView>>,
    lobby: Option<Addr<LobbyActor>>,
    present: HashMap<PlayerId, Presence>,
    /// Each player's round trip, as their session last measured it.
    pings: HashMap<PlayerId, Duration>,
    ratings: Option<Ratings>,
    /// The last rated round's changes, shown with its results.
    changes: Vec<RatingChange>,
    last: Instant,
    /// When the timer was meant to fire.
    due: Instant,
    over_since: Option<Instant>,
    /// The server is stopping; the room closes once its round is over.
    draining: bool,
    closing_soon: bool,
}

impl Actor for RoomActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        ctx.set_mailbox_capacity(MAILBOX);
        self.last = Instant::now();
        self.schedule(ctx);
    }

    fn stopped(&mut self, _: &mut Self::Context) {
        debug!(code = ?self.code, "room closed");
        if let Some((id, rooms)) = self.registered.take() {
            rooms.remove(id);
        }
        if let (Some(lobby), Some(code)) = (&self.lobby, self.code.take()) {
            lobby.do_send(RoomClosed(code));
        }
    }
}

impl RoomActor {
    /// A room for `round`, with the receiver for its view. `present` lists
    /// the players already seated, as a matched room has; with `ratings`,
    /// its rounds are rated.
    pub fn new(
        round: Round,
        code: Option<String>,
        lobby: Option<Addr<LobbyActor>>,
        present: HashMap<PlayerId, Presence>,
        ratings: Option<Ratings>,
    ) -> (Self, ViewRx) {
        let pings = HashMap::new();
        let (view, rx) = watch::channel(Arc::new(view_of(&round, &code, &[], &pings)));
        let now = Instant::now();
        let room = Self {
            registered: None,
            round,
            code,
            view,
            lobby,
            present,
            pings,
            ratings,
            changes: Vec::new(),
            last: now,
            due: now,
            over_since: None,
            draining: false,
            closing_soon: false,
        };
        (room, rx)
    }

    fn schedule(&mut self, ctx: &mut Context<Self>) {
        let delay = self
            .round
            .until_next()
            .clamp(Duration::from_millis(1), Duration::from_secs(1));
        self.due = Instant::now() + delay;
        ctx.run_later(delay, |act, ctx| act.on_timer(ctx));
    }

    fn on_timer(&mut self, ctx: &mut Context<Self>) {
        let now = Instant::now();
        if self.round.in_play() {
            TICK_LAG.record(now.saturating_duration_since(self.due));
        }
        let dt = now - self.last;
        self.last = now;
        self.drop_absent(ctx);
        let Some(events) = self.guarded(ctx, |round| round.advance(dt)) else {
            return;
        };
        self.on_events(events);
        self.publish();
        if self.round.is_empty() {
            return ctx.stop();
        }
        if self.draining && !self.round.in_play() && !self.closing_soon {
            self.closing_soon = true;
            ctx.run_later(RESULTS_GRACE, |act, ctx| act.close(DRAINED, ctx));
        }
        if self.round.settings.kind == RoomKind::Match
            && matches!(self.view.borrow().phase, PhaseView::Results { .. })
            && self.over_since.get_or_insert(now).elapsed() > MATCH_LINGER
        {
            return self.close(MATCH_OVER, ctx);
        }
        self.schedule(ctx);
    }

    /// Players whose session is gone leave, as if they'd pressed Esc.
    fn drop_absent(&mut self, ctx: &mut Context<Self>) {
        let gone: Vec<PlayerId> = self
            .present
            .iter()
            .filter(|(_, p)| p.strong_count() == 0)
            .map(|(id, _)| *id)
            .collect();
        for id in gone {
            self.leave(id, ctx);
        }
    }

    fn leave(&mut self, id: PlayerId, ctx: &mut Context<Self>) {
        self.present.remove(&id);
        self.pings.remove(&id);
        if let Some(events) = self.guarded(ctx, |round| round.leave(id)) {
            self.on_events(events);
        }
    }

    fn on_events(&mut self, events: Vec<RoundEvent>) {
        for event in events {
            let RoundEvent::Ended {
                outcome,
                placements,
            } = event;
            debug!(code = ?self.code, ?outcome, "round over");
            self.rate(&outcome, &placements);
        }
    }

    /// Works out everyone's new rating, saves the accounts' ones, tells each
    /// player, and shows the changes with the results.
    fn rate(&mut self, outcome: &Outcome, placements: &[Placement]) {
        let Some(ratings) = &self.ratings else {
            return;
        };
        let players: Vec<(&Placement, &RatedMember)> = placements
            .iter()
            .filter_map(|p| ratings.members.get(&p.who.id).map(|m| (p, m)))
            .collect();
        let before: Vec<(Rating, usize)> =
            players.iter().map(|(p, m)| (m.rating, p.place)).collect();
        let after = rating::after_round(&before);
        let winner = match outcome {
            Outcome::Won { winner, .. } => Some(winner.id),
            Outcome::Draw { .. } => None,
        };
        let mut saved = Vec::new();
        self.changes.clear();
        for ((placement, member), rating) in players.into_iter().zip(after) {
            let won = winner == Some(placement.who.id);
            member.notify.send(Rated { rating, won });
            if let Some(account) = member.account {
                saved.push(RatedResult {
                    account,
                    rating,
                    won,
                });
                self.changes.push(RatingChange {
                    id: placement.who.id,
                    before: member.rating.shown(),
                    after: rating.shown(),
                });
            }
        }
        let store = ratings.store.clone();
        actix::spawn(async move {
            if let Err(e) = store.save_rated_round(&saved).await {
                warn!(error = %e, "couldn't save ratings");
            }
        });
        self.publish_now();
    }

    /// Runs `f` on the round; a panic closes the room with a message for its
    /// players instead of killing the thread.
    fn guarded<R>(
        &mut self,
        ctx: &mut Context<Self>,
        f: impl FnOnce(&mut Round) -> R,
    ) -> Option<R> {
        match catch_unwind(AssertUnwindSafe(|| f(&mut self.round))) {
            Ok(r) => Some(r),
            Err(_) => {
                warn!(code = ?self.code, "a room panicked; closing it");
                self.close(BROKEN, ctx);
                None
            }
        }
    }

    fn publish(&mut self) {
        if self.round.take_changed() {
            self.publish_now();
        }
    }

    fn publish_now(&mut self) {
        self.view.send_replace(Arc::new(view_of(
            &self.round,
            &self.code,
            &self.changes,
            &self.pings,
        )));
    }

    fn close(&mut self, reason: &str, ctx: &mut Context<Self>) {
        let mut last = (**self.view.borrow()).clone();
        last.phase = PhaseView::Closed {
            reason: reason.to_string(),
        };
        self.view.send_replace(Arc::new(last));
        ctx.stop();
    }
}

fn view_of(
    round: &Round,
    code: &Option<String>,
    changes: &[RatingChange],
    pings: &HashMap<PlayerId, Duration>,
) -> RoomView {
    let mut view = round.view();
    view.code.clone_from(code);
    if !view.placements.is_empty() {
        view.ratings = changes.to_vec();
    }
    for seat in &mut view.seats {
        seat.ping = pings.get(&seat.who.id).copied();
    }
    view
}

impl Handler<Join> for RoomActor {
    type Result = Result<ViewRx, JoinError>;

    fn handle(&mut self, msg: Join, ctx: &mut Self::Context) -> Self::Result {
        let id = msg.who.id;
        let joined = self
            .guarded(ctx, |round| round.join(msg.who))
            .unwrap_or(Err(JoinError::Closed));
        joined?;
        self.present.insert(id, msg.presence);
        self.publish();
        Ok(self.view.subscribe())
    }
}

impl Handler<Leave> for RoomActor {
    type Result = ();

    fn handle(&mut self, Leave(id): Leave, ctx: &mut Self::Context) {
        self.leave(id, ctx);
        self.publish();
        if self.round.is_empty() {
            ctx.stop();
        }
    }
}

impl Handler<Steer> for RoomActor {
    type Result = ();

    fn handle(&mut self, msg: Steer, ctx: &mut Self::Context) {
        self.guarded(ctx, |round| round.steer(msg.id, msg.dir));
    }
}

impl Handler<SetSpeed> for RoomActor {
    type Result = ();

    fn handle(&mut self, SetSpeed { id, tick_ms }: SetSpeed, ctx: &mut Self::Context) {
        if self.guarded(ctx, |round| round.set_tick(id, tick_ms)) == Some(true) {
            debug!(code = ?self.code, tick_ms, "speed changed");
            self.publish();
        }
    }
}

impl Handler<Ping> for RoomActor {
    type Result = ();

    fn handle(&mut self, Ping { id, rtt }: Ping, _: &mut Self::Context) {
        if !self.present.contains_key(&id) {
            return;
        }
        self.pings.insert(id, rtt);
        // In play, the next tick's view carries it; otherwise show it now.
        if !self.round.in_play() {
            self.publish_now();
        }
    }
}

impl Handler<Drain> for RoomActor {
    type Result = ();

    fn handle(&mut self, _: Drain, ctx: &mut Self::Context) {
        self.draining = true;
        if !self.round.in_play() {
            self.close(DRAINED, ctx);
        }
    }
}

impl Handler<Ready> for RoomActor {
    type Result = ();

    fn handle(&mut self, Ready(id): Ready, ctx: &mut Self::Context) {
        self.guarded(ctx, |round| round.toggle_ready(id));
        self.publish();
    }
}
