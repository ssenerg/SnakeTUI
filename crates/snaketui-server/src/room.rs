//! One multiplayer room: its game's [`Room`], its clock, and the view it
//! publishes. A [`RoomActor`] runs a room of any game; the rest of the
//! server reaches it through a [`RoomPort`], which speaks [`RoomCmd`]s.
//!
//! A room never waits on its players. It publishes each new view on a watch
//! channel, and every session reads the latest one at its own pace.

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use actix::prelude::*;
use actix_rt::ArbiterHandle;
use futures::future::BoxFuture;
use game_api::{
    Entrant, JoinError, PhaseView, Placement, PlayerId, RatingChange, RatingKey, Room, RoomKind,
    RoomView, RoundEnd,
};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tracing::{debug, warn};

use crate::cluster::Cluster;
use crate::cluster::wire::{JoinAs, RatedAt};
use crate::games::{self, AnyRoomView, Served};
use crate::lobby::{LobbyActor, RoomClosed};
use crate::metrics::TICK_LAG;
use crate::rating::Rating;
use crate::store::{AccountId, Store};

/// A room's views, newest first, of whichever game.
pub type ViewRx = watch::Receiver<Arc<dyn AnyRoomView>>;

/// Shows that a player's session is still there: the session holds the
/// `Arc`, rooms and the queue a `Weak`. A session that dies without saying
/// goodbye can't leave a ghost snake behind.
pub type Presence = Weak<()>;

/// Steering and ready keys waiting in a room's mailbox; more are dropped.
const MAILBOX: usize = 256;
/// A finished match closes this long after its results first show.
const MATCH_LINGER: Duration = Duration::from_secs(60);
/// People who may watch one room at once.
const MAX_WATCHERS: usize = 20;
/// How long a signed-in player who lost their connection mid-round keeps
/// their seat, in games that take them back. Their clock, if any, runs on.
pub const REJOIN_WITHIN: Duration = Duration::from_secs(90);
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
type Running = (Arc<dyn RoomPort>, ArbiterHandle);

/// The rooms this server runs.
#[derive(Clone, Default)]
pub struct LocalRooms {
    running: Arc<Mutex<HashMap<RoomId, Running>>>,
    /// Accounts seated in a round being played here that would take them
    /// back if they lost their connection, and the room.
    seated: Arc<Mutex<HashMap<AccountId, RoomId>>>,
}

impl LocalRooms {
    pub fn get(&self, id: RoomId) -> Option<Arc<dyn RoomPort>> {
        self.running
            .lock()
            .unwrap()
            .get(&id)
            .map(|(port, _)| Arc::clone(port))
    }

    /// The room, and the thread it runs on, for work that belongs with it.
    pub fn get_with_thread(&self, id: RoomId) -> Option<Running> {
        self.running.lock().unwrap().get(&id).cloned()
    }

    fn insert(&self, id: RoomId, port: Arc<dyn RoomPort>, thread: ArbiterHandle) {
        self.running.lock().unwrap().insert(id, (port, thread));
    }

    fn remove(&self, id: RoomId) {
        self.running.lock().unwrap().remove(&id);
    }

    /// The room here where `account` has a seat in the round being played.
    pub fn seat_of(&self, account: AccountId) -> Option<RoomId> {
        self.seated.lock().unwrap().get(&account).copied()
    }

    /// Notes that `room` has, or no longer has, a seat for `account`.
    fn seat(&self, account: AccountId, room: RoomId, seated: bool) {
        let mut all = self.seated.lock().unwrap();
        if seated {
            all.insert(account, room);
        } else if all.get(&account) == Some(&room) {
            all.remove(&account);
        }
    }

    pub fn len(&self) -> usize {
        self.running.lock().unwrap().len()
    }

    pub fn all(&self) -> Vec<Arc<dyn RoomPort>> {
        self.running
            .lock()
            .unwrap()
            .values()
            .map(|(port, _)| Arc::clone(port))
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

    /// Starts `room` as `id`, from [`Self::new_id`], on the next room
    /// thread.
    pub fn start_as<G: Served>(&self, id: RoomId, mut room: RoomActor<G>) -> RoomHandle {
        room.registered = Some((id, self.rooms.clone()));
        let i = self.next.fetch_add(1, Ordering::Relaxed) % self.arbiters.len();
        let thread = self.arbiters[i].clone();
        let addr = RoomActor::start_in_arbiter(&thread, move |_| room);
        let port: Arc<dyn RoomPort> = Arc::new(addr);
        self.rooms.insert(id, Arc::clone(&port), thread);
        RoomHandle::Local {
            at: self.at(id),
            port,
        }
    }
}

/// What a room is told, by players here and on other servers alike.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RoomCmd {
    Ready,
    /// Gives up a seat, or stops watching.
    Leave,
    /// The player's connection is gone; they didn't choose to leave. In a
    /// game that takes them back, a signed-in player keeps their seat in
    /// the round for [`REJOIN_WITHIN`]; otherwise it's [`RoomCmd::Leave`].
    Gone,
    /// The player's round trip, measured by their session.
    Ping(Duration),
    /// The host adds a bot.
    AddBot,
    /// The host takes the last bot out.
    RemoveBot,
    /// A move, as the room's game reads it.
    Play(Vec<u8>),
    /// The host changes the settings of the room's next rounds, as its
    /// game reads them.
    Settings(Vec<u8>),
}

impl RoomCmd {
    /// Whether the player is done with the room, by choice or not: never
    /// dropped, and the end of their part in it.
    pub fn ends_stay(&self) -> bool {
        matches!(self, RoomCmd::Leave | RoomCmd::Gone)
    }
}

/// A room running here, of whichever game.
pub trait RoomPort: Send + Sync {
    /// Takes a seat for `who`, signed in to `account` if any; `presence`
    /// shows the player is still there.
    fn join(
        &self,
        who: Entrant,
        account: Option<AccountId>,
        presence: Presence,
    ) -> BoxFuture<'static, Joined>;
    /// `who` takes back the seat `account` has in the round being played.
    fn rejoin(
        &self,
        who: Entrant,
        account: AccountId,
        presence: Presence,
    ) -> BoxFuture<'static, Joined>;
    /// Lets `id` watch without a seat.
    fn watch(&self, id: PlayerId, presence: Presence) -> BoxFuture<'static, Joined>;
    /// Keys wait in the room's mailbox, and a full one drops them; leaving
    /// always gets there.
    fn send(&self, id: PlayerId, cmd: RoomCmd);
    /// The server is stopping.
    fn drain(&self);
}

pub type Joined = Result<ViewRx, JoinError>;

impl<G: Served> RoomPort for Addr<RoomActor<G>> {
    fn join(
        &self,
        who: Entrant,
        account: Option<AccountId>,
        presence: Presence,
    ) -> BoxFuture<'static, Joined> {
        let joined = self.send(Join {
            who,
            account,
            presence,
        });
        Box::pin(async move { joined.await.unwrap_or(Err(JoinError::Closed)) })
    }

    fn rejoin(
        &self,
        who: Entrant,
        account: AccountId,
        presence: Presence,
    ) -> BoxFuture<'static, Joined> {
        let joined = self.send(Rejoin {
            who,
            account,
            presence,
        });
        Box::pin(async move { joined.await.unwrap_or(Err(JoinError::Closed)) })
    }

    fn watch(&self, id: PlayerId, presence: Presence) -> BoxFuture<'static, Joined> {
        let joined = self.send(Watch { id, presence });
        Box::pin(async move { joined.await.unwrap_or(Err(JoinError::Closed)) })
    }

    fn send(&self, id: PlayerId, cmd: RoomCmd) {
        if cmd.ends_stay() {
            self.do_send(Cmd { id, cmd });
        } else {
            let _ = self.try_send(Cmd { id, cmd });
        }
    }

    fn drain(&self) {
        self.do_send(Drain);
    }
}

/// A room about to start, of whichever game.
pub trait NewRoom: Send {
    /// Starts it here as `id`, from [`RoomHost::new_id`].
    fn start(
        self: Box<Self>,
        host: &RoomHost,
        id: RoomId,
        setup: RoomSetup,
    ) -> (RoomHandle, ViewRx);
}

impl<G: Served> NewRoom for Room<G> {
    fn start(
        self: Box<Self>,
        host: &RoomHost,
        id: RoomId,
        setup: RoomSetup,
    ) -> (RoomHandle, ViewRx) {
        let (room, view) = RoomActor::new(*self, setup);
        (host.start_as(id, room), view)
    }
}

/// What a room starts with besides its game.
#[derive(Default)]
pub struct RoomSetup {
    /// Private rooms: the code others join with, and the lobby that keeps
    /// it.
    pub code: Option<(String, Addr<LobbyActor>)>,
    /// The players already seated, as a matched room has.
    pub present: HashMap<PlayerId, Presence>,
    /// Rated rooms: whom to rate, and where.
    pub ratings: Option<Ratings>,
}

/// A room to join and play in, wherever it runs.
#[derive(Clone)]
pub enum RoomHandle {
    Local {
        at: RoomRef,
        port: Arc<dyn RoomPort>,
    },
    /// On another server: keys go there through NATS, and its views come
    /// back.
    Remote { at: RoomRef, cluster: Arc<Cluster> },
}

impl RoomHandle {
    pub fn at(&self) -> RoomRef {
        match self {
            RoomHandle::Local { at, .. } | RoomHandle::Remote { at, .. } => *at,
        }
    }

    /// Lets `who` watch without a seat; `presence` shows they're still
    /// there.
    pub fn watch(&self, who: Entrant, presence: Presence) -> BoxFuture<'static, Joined> {
        match self {
            RoomHandle::Local { port, .. } => port.watch(who.id, presence),
            RoomHandle::Remote { at, cluster } => {
                Box::pin(Arc::clone(cluster).join_remote(*at, who, presence, JoinAs::Watch))
            }
        }
    }

    /// Takes a seat for `who`, signed in to `account` if any; `presence`
    /// shows the player is still there.
    pub fn join(
        &self,
        who: Entrant,
        account: Option<AccountId>,
        presence: Presence,
    ) -> BoxFuture<'static, Joined> {
        match self {
            RoomHandle::Local { port, .. } => port.join(who, account, presence),
            RoomHandle::Remote { at, cluster } => {
                let how = JoinAs::Seat { account };
                Box::pin(Arc::clone(cluster).join_remote(*at, who, presence, how))
            }
        }
    }

    /// `who` takes back the seat `account` has in the round being played.
    pub fn rejoin(
        &self,
        who: Entrant,
        account: AccountId,
        presence: Presence,
    ) -> BoxFuture<'static, Joined> {
        match self {
            RoomHandle::Local { port, .. } => port.rejoin(who, account, presence),
            RoomHandle::Remote { at, cluster } => {
                let how = JoinAs::Rejoin { account };
                Box::pin(Arc::clone(cluster).join_remote(*at, who, presence, how))
            }
        }
    }

    pub fn send(&self, id: PlayerId, cmd: RoomCmd) {
        match self {
            RoomHandle::Local { port, .. } => port.send(id, cmd),
            RoomHandle::Remote { at, cluster } => cluster.send_input(*at, id, cmd),
        }
    }

    pub fn leave(&self, id: PlayerId) {
        self.send(id, RoomCmd::Leave);
    }
}

/// A player's new rating after a rated round, for their session.
#[derive(Message)]
#[rtype(result = "()")]
pub struct Rated {
    /// The rating the round changed.
    pub key: RatingKey,
    pub rating: Rating,
    pub won: bool,
}

/// A player in a rated room.
pub struct RatedMember {
    /// Guests play, but only accounts are rated, against each other.
    pub account: Option<AccountId>,
    /// What they were matched by. Ratings are worked out from the saved
    /// ones instead.
    pub rating: Rating,
    pub notify: RatedNotify,
}

/// Where a player's new rating goes.
#[derive(Clone)]
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
                    game: rated.key.game,
                    category: rated.key.category.to_string(),
                    rating: rated.rating,
                    won: rated.won,
                };
                cluster.send(cluster.server_subject(*server, "rated"), &news);
            }
            RatedNotify::Nobody => {}
        }
    }
}

/// What a rated room needs: which rating it changes, everyone's, and where
/// to save new ones.
pub struct Ratings {
    pub key: RatingKey,
    pub store: Store,
    pub members: HashMap<PlayerId, RatedMember>,
}

#[derive(Message)]
#[rtype(result = "Result<ViewRx, JoinError>")]
struct Join {
    who: Entrant,
    account: Option<AccountId>,
    presence: Presence,
}

/// A player back on a new connection takes their seat again.
#[derive(Message)]
#[rtype(result = "Result<ViewRx, JoinError>")]
struct Rejoin {
    who: Entrant,
    account: AccountId,
    presence: Presence,
}

/// Watching without a seat. A full room of watchers says
/// [`JoinError::Full`].
#[derive(Message)]
#[rtype(result = "Result<ViewRx, JoinError>")]
struct Watch {
    id: PlayerId,
    presence: Presence,
}

#[derive(Message)]
#[rtype(result = "()")]
struct Cmd {
    id: PlayerId,
    cmd: RoomCmd,
}

/// The server is stopping: finish the round in play, if any, then close.
#[derive(Clone, Copy, Message)]
#[rtype(result = "()")]
pub struct Drain;

pub struct RoomActor<G: Served> {
    /// Its id, and the list of this server's rooms it leaves on closing.
    registered: Option<(RoomId, LocalRooms)>,
    room: Room<G>,
    code: Option<String>,
    view: watch::Sender<Arc<dyn AnyRoomView>>,
    lobby: Option<Addr<LobbyActor>>,
    present: HashMap<PlayerId, Presence>,
    /// People watching without a seat.
    watchers: HashMap<PlayerId, Presence>,
    /// Each player's round trip, as their session last measured it.
    pings: HashMap<PlayerId, Duration>,
    /// The account each signed-in player is signed in to.
    accounts: HashMap<PlayerId, AccountId>,
    /// Players whose connection went mid-round, and when their seat stops
    /// waiting for them.
    away: HashMap<PlayerId, Instant>,
    /// The accounts [`LocalRooms`] lists as seated here.
    seated: Vec<AccountId>,
    /// How long a seat waits: [`REJOIN_WITHIN`], or less in tests.
    rejoin_within: Duration,
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

impl<G: Served> Actor for RoomActor<G> {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        ctx.set_mailbox_capacity(MAILBOX);
        self.last = Instant::now();
        self.schedule(ctx);
    }

    fn stopped(&mut self, _: &mut Self::Context) {
        debug!(game = %G::ID, code = ?self.code, "room closed");
        if let Some((id, rooms)) = self.registered.take() {
            for &account in &self.seated {
                rooms.seat(account, id, false);
            }
            rooms.remove(id);
        }
        if let (Some(lobby), Some(code)) = (&self.lobby, self.code.take()) {
            lobby.do_send(RoomClosed(code));
        }
    }
}

impl<G: Served> RoomActor<G> {
    /// A room for `room`, with the receiver for its view.
    pub fn new(room: Room<G>, setup: RoomSetup) -> (Self, ViewRx) {
        let RoomSetup {
            code,
            present,
            ratings,
        } = setup;
        let (code, lobby) = code.unzip();
        let pings = HashMap::new();
        let away = HashMap::new();
        let first: Arc<dyn AnyRoomView> = Arc::new(view_of(&room, &code, &[], &pings, &away, 0));
        // A matched room knows its players' accounts from the start.
        let accounts = ratings
            .iter()
            .flat_map(|r| &r.members)
            .filter_map(|(&id, m)| Some((id, m.account?)))
            .collect();
        let (view, rx) = watch::channel(first);
        let now = Instant::now();
        let room = Self {
            registered: None,
            room,
            code,
            view,
            lobby,
            present,
            watchers: HashMap::new(),
            pings,
            accounts,
            away,
            seated: Vec::new(),
            rejoin_within: REJOIN_WITHIN,
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
            .room
            .until_next()
            .clamp(Duration::from_millis(1), Duration::from_secs(1));
        self.due = Instant::now() + delay;
        ctx.run_later(delay, |act, ctx| act.on_timer(ctx));
    }

    fn on_timer(&mut self, ctx: &mut Context<Self>) {
        let now = Instant::now();
        if self.room.in_play() {
            TICK_LAG.record(now.saturating_duration_since(self.due));
        }
        let dt = now - self.last;
        self.last = now;
        self.drop_absent(ctx);
        self.give_up_on_away(now, ctx);
        let Some(end) = self.guarded(ctx, |room| room.advance(dt)) else {
            return;
        };
        self.on_end(end, ctx);
        self.publish();
        if self.room.is_empty() {
            return ctx.stop();
        }
        if self.draining && !self.room.in_play() && !self.closing_soon {
            self.closing_soon = true;
            ctx.run_later(RESULTS_GRACE, |act, ctx| act.close(DRAINED, ctx));
        }
        if self.room.settings.kind == RoomKind::Match
            && matches!(self.view.borrow().shell().phase, PhaseView::Results { .. })
            && self.over_since.get_or_insert(now).elapsed() > MATCH_LINGER
        {
            return self.close(MATCH_OVER, ctx);
        }
        self.schedule(ctx);
    }

    /// Players and watchers whose session is gone leave, as if they'd
    /// pressed Esc.
    fn drop_absent(&mut self, ctx: &mut Context<Self>) {
        let gone: Vec<PlayerId> = self
            .present
            .iter()
            .chain(&self.watchers)
            .filter(|(_, p)| p.strong_count() == 0)
            .map(|(id, _)| *id)
            .collect();
        for id in gone {
            self.gone(id, ctx);
        }
    }

    /// `id`'s connection is gone. A signed-in player in a round of a game
    /// that takes them back keeps their seat a while; anyone else leaves.
    fn gone(&mut self, id: PlayerId, ctx: &mut Context<Self>) {
        let waits = G::REJOIN
            && !self.watchers.contains_key(&id)
            && self.accounts.contains_key(&id)
            && self.room.plays(id);
        if !waits {
            return self.leave(id, ctx);
        }
        self.present.remove(&id);
        self.pings.remove(&id);
        if !self.away.contains_key(&id) {
            debug!(game = %G::ID, code = ?self.code, player = id, "away; the seat waits");
            self.away.insert(id, Instant::now() + self.rejoin_within);
            self.publish_now();
        }
    }

    /// Players away too long leave, and so do all those away once their
    /// round is over: the next one starts without them.
    fn give_up_on_away(&mut self, now: Instant, ctx: &mut Context<Self>) {
        let in_play = self.room.in_play();
        let gone: Vec<PlayerId> = self
            .away
            .iter()
            .filter(|&(_, &until)| until <= now || !in_play)
            .map(|(&id, _)| id)
            .collect();
        for id in gone {
            debug!(game = %G::ID, code = ?self.code, player = id, "didn't come back");
            self.leave(id, ctx);
        }
    }

    fn leave(&mut self, id: PlayerId, ctx: &mut Context<Self>) {
        if self.watchers.remove(&id).is_some() {
            return self.publish_now();
        }
        self.present.remove(&id);
        self.pings.remove(&id);
        self.away.remove(&id);
        self.accounts.remove(&id);
        if let Some(end) = self.guarded(ctx, |room| room.leave(id)) {
            self.on_end(end, ctx);
        }
    }

    fn on_end(&mut self, end: Option<RoundEnd>, ctx: &mut Context<Self>) {
        if let Some(end) = end {
            debug!(game = %G::ID, code = ?self.code, ?end, "round over");
            self.rate(end.placements(), ctx);
        }
    }

    /// Rates the accounts that played against each other: works out their
    /// new ratings from the saved ones, saves them, tells each player, and
    /// shows the changes with the results. It finishes even if the room
    /// closes meanwhile.
    fn rate(&mut self, placements: &[Placement], ctx: &mut Context<Self>) {
        self.changes.clear();
        let Some(ratings) = &self.ratings else {
            return;
        };
        let winner = game_api::winner(placements).map(|w| w.id);
        let rated = rated_players(placements, &ratings.members);
        // Rated against nobody: nothing changes.
        if rated.len() < 2 {
            return;
        }
        let players: Vec<(AccountId, usize, bool)> = rated
            .iter()
            .map(|&(id, account, place)| (account, place, winner == Some(id)))
            .collect();
        let notify: Vec<(PlayerId, Option<RatedNotify>)> = rated
            .iter()
            .map(|&(id, _, _)| (id, ratings.members.get(&id).map(|m| m.notify.clone())))
            .collect();
        let (key, store, room) = (ratings.key, ratings.store.clone(), ctx.address());
        actix::spawn(async move {
            let changes = match store.rate_round(key, &players).await {
                Ok(changes) => changes,
                Err(e) => return warn!(error = %e, "couldn't save ratings"),
            };
            let mut shown = Vec::with_capacity(changes.len());
            for (((id, notify), (_, _, won)), (before, after)) in
                notify.into_iter().zip(&players).zip(changes)
            {
                if let Some(notify) = notify {
                    notify.send(Rated {
                        key,
                        rating: after,
                        won: *won,
                    });
                }
                shown.push(RatingChange {
                    id,
                    before: before.shown(),
                    after: after.shown(),
                });
            }
            room.do_send(RatingsChanged(shown));
        });
    }

    /// Runs `f` on the room; a panic closes it with a message for its
    /// players instead of killing the thread.
    fn guarded<R>(
        &mut self,
        ctx: &mut Context<Self>,
        f: impl FnOnce(&mut Room<G>) -> R,
    ) -> Option<R> {
        match catch_unwind(AssertUnwindSafe(|| f(&mut self.room))) {
            Ok(r) => Some(r),
            Err(_) => {
                warn!(game = %G::ID, code = ?self.code, "a room panicked; closing it");
                self.close(BROKEN, ctx);
                None
            }
        }
    }

    fn publish(&mut self) {
        if self.room.take_changed() {
            self.publish_now();
        }
    }

    fn publish_now(&mut self) {
        self.list_seated();
        self.view.send_replace(Arc::new(view_of(
            &self.room,
            &self.code,
            &self.changes,
            &self.pings,
            &self.away,
            self.watchers.len(),
        )));
    }

    /// Keeps [`LocalRooms`]' list of the accounts seated in this room's
    /// round up to date, for players coming back on a new connection.
    fn list_seated(&mut self) {
        let Some((id, rooms)) = &self.registered else {
            return;
        };
        let mut now: Vec<AccountId> = Vec::new();
        if G::REJOIN {
            now = self
                .accounts
                .iter()
                .filter(|&(&player, _)| self.room.plays(player))
                .map(|(_, &account)| account)
                .collect();
            now.sort_unstable();
        }
        if now == self.seated {
            return;
        }
        for &account in self.seated.iter().filter(|a| !now.contains(a)) {
            rooms.seat(account, *id, false);
        }
        for &account in now.iter().filter(|a| !self.seated.contains(a)) {
            rooms.seat(account, *id, true);
        }
        self.seated = now;
    }

    fn close(&mut self, reason: &str, ctx: &mut Context<Self>) {
        let last = self.view.borrow().closed(reason);
        self.view.send_replace(last);
        ctx.stop();
    }

    fn command(&mut self, id: PlayerId, cmd: RoomCmd, ctx: &mut Context<Self>) {
        match cmd {
            RoomCmd::Ready => {
                self.guarded(ctx, |room| room.toggle_ready(id));
                self.publish();
            }
            RoomCmd::Leave | RoomCmd::Gone => {
                if cmd == RoomCmd::Leave {
                    self.leave(id, ctx);
                } else {
                    self.gone(id, ctx);
                }
                self.publish();
                if self.room.is_empty() {
                    ctx.stop();
                }
            }
            RoomCmd::Ping(rtt) => {
                if !self.present.contains_key(&id) {
                    return;
                }
                self.pings.insert(id, rtt);
                // In play, the next tick's view carries it; otherwise show
                // it now.
                if !self.room.in_play() {
                    self.publish_now();
                }
            }
            RoomCmd::AddBot | RoomCmd::RemoveBot => {
                let add = cmd == RoomCmd::AddBot;
                let changed = self.guarded(ctx, |room| {
                    if add {
                        room.add_bot(id)
                    } else {
                        room.remove_bot(id)
                    }
                });
                if changed == Some(true) {
                    debug!(code = ?self.code, add, players = self.room.players(), "bots changed");
                    self.publish();
                }
            }
            RoomCmd::Play(bytes) => {
                let Some(input) = games::decode::<G::Input>(&bytes) else {
                    return;
                };
                let rtt = self.pings.get(&id).copied();
                if let Some(end) = self.guarded(ctx, |room| room.play(id, input, rtt)) {
                    self.on_end(end, ctx);
                    // A move that shows, such as chess's, goes out at once;
                    // Snake's turns show on the next tick.
                    self.publish();
                }
            }
            RoomCmd::Settings(bytes) => {
                let Some(settings) = games::decode::<G::Settings>(&bytes) else {
                    return;
                };
                if self.guarded(ctx, |room| room.set_settings(id, settings)) == Some(true) {
                    debug!(code = ?self.code, "settings changed");
                    self.publish();
                }
            }
        }
    }
}

/// Who a round rates, with their account and place: accounts only, each
/// once. Guests aren't rated, and beating them doesn't count; an account
/// that played twice isn't rated at all.
fn rated_players(
    placements: &[Placement],
    members: &HashMap<PlayerId, RatedMember>,
) -> Vec<(PlayerId, AccountId, usize)> {
    let accounts: Vec<(PlayerId, AccountId, usize)> = placements
        .iter()
        .filter_map(|p| {
            let account = members.get(&p.who.id)?.account?;
            Some((p.who.id, account, p.place))
        })
        .collect();
    accounts
        .iter()
        .filter(|(_, account, _)| accounts.iter().filter(|a| a.1 == *account).count() == 1)
        .copied()
        .collect()
}

fn view_of<G: Served>(
    room: &Room<G>,
    code: &Option<String>,
    changes: &[RatingChange],
    pings: &HashMap<PlayerId, Duration>,
    away: &HashMap<PlayerId, Instant>,
    watchers: usize,
) -> RoomView<G> {
    let mut view = room.view();
    let shell = &mut view.shell;
    shell.code.clone_from(code);
    shell.watchers = watchers;
    if !shell.placements.is_empty() {
        shell.ratings = changes.to_vec();
    }
    let now = Instant::now();
    for seat in &mut shell.seats {
        seat.ping = pings.get(&seat.who.id).copied();
        seat.away = away
            .get(&seat.who.id)
            .map(|until| until.saturating_duration_since(now));
    }
    view
}

impl<G: Served> Handler<Join> for RoomActor<G> {
    type Result = Result<ViewRx, JoinError>;

    fn handle(&mut self, msg: Join, ctx: &mut Self::Context) -> Self::Result {
        let id = msg.who.id;
        let joined = self
            .guarded(ctx, |room| room.join(msg.who))
            .unwrap_or(Err(JoinError::Closed));
        joined?;
        self.present.insert(id, msg.presence);
        if let Some(account) = msg.account {
            self.accounts.insert(id, account);
        }
        self.publish();
        Ok(self.view.subscribe())
    }
}

impl<G: Served> Handler<Rejoin> for RoomActor<G> {
    type Result = Result<ViewRx, JoinError>;

    /// The account's seat in the round goes to its new connection, whether
    /// the old one is known to be gone yet or not.
    fn handle(&mut self, msg: Rejoin, ctx: &mut Self::Context) -> Self::Result {
        let new = msg.who.id;
        let old = self
            .accounts
            .iter()
            .find(|&(&id, &account)| account == msg.account && self.room.plays(id))
            .map(|(&id, _)| id)
            .ok_or(JoinError::Closed)?;
        if self.guarded(ctx, |room| room.rejoin(old, msg.who)) != Some(true) {
            return Err(JoinError::Closed);
        }
        self.present.remove(&old);
        self.pings.remove(&old);
        self.away.remove(&old);
        self.accounts.remove(&old);
        self.accounts.insert(new, msg.account);
        self.present.insert(new, msg.presence);
        if let Some(ratings) = &mut self.ratings
            && let Some(member) = ratings.members.remove(&old)
        {
            ratings.members.insert(new, member);
        }
        debug!(game = %G::ID, code = ?self.code, player = new, "back in their seat");
        self.publish_now();
        Ok(self.view.subscribe())
    }
}

/// A rated round's changes, saved: shown with its results.
#[derive(Message)]
#[rtype(result = "()")]
struct RatingsChanged(Vec<RatingChange>);

impl<G: Served> Handler<RatingsChanged> for RoomActor<G> {
    type Result = ();

    fn handle(&mut self, RatingsChanged(changes): RatingsChanged, _: &mut Self::Context) {
        self.changes = changes;
        self.publish_now();
    }
}

impl<G: Served> Handler<Watch> for RoomActor<G> {
    type Result = Result<ViewRx, JoinError>;

    fn handle(&mut self, Watch { id, presence }: Watch, _: &mut Self::Context) -> Self::Result {
        if self.watchers.len() >= MAX_WATCHERS && !self.watchers.contains_key(&id) {
            return Err(JoinError::Full);
        }
        self.watchers.insert(id, presence);
        debug!(code = ?self.code, watchers = self.watchers.len(), "watching");
        self.publish_now();
        Ok(self.view.subscribe())
    }
}

impl<G: Served> Handler<Cmd> for RoomActor<G> {
    type Result = ();

    fn handle(&mut self, Cmd { id, cmd }: Cmd, ctx: &mut Self::Context) {
        self.command(id, cmd, ctx);
    }
}

impl<G: Served> Handler<Drain> for RoomActor<G> {
    type Result = ();

    fn handle(&mut self, _: Drain, ctx: &mut Self::Context) {
        self.draining = true;
        if !self.room.in_play() {
            self.close(DRAINED, ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use actix_rt::Arbiter;
    use chess_core::moves::Move;
    use chess_core::round::{Chess, ChessInput, ChessSettings, Side};
    use game_api::{PhaseView, Room, RoomKind, SeatView};
    use snake_core::round::{Placement, RoomSettings, Snake, quick_rules};

    use super::*;
    use crate::games::{self, view_of};

    /// A private chess room started on this thread as room 7, where player
    /// 1 hosts as white, as it's listed among [`LocalRooms`].
    fn chess_room(rejoin_within: Duration) -> (RoomHandle, ViewRx, LocalRooms) {
        let mut settings = game_api::RoomSettings::<Chess>::new(
            ChessSettings {
                side: Side::White,
                ..ChessSettings::default()
            },
            RoomKind::Private,
        )
        .unwrap();
        settings.countdown = Duration::ZERO;
        let (mut room, view) =
            RoomActor::new(Room::<Chess>::new(settings, 1), RoomSetup::default());
        room.rejoin_within = rejoin_within;
        let rooms = LocalRooms::default();
        let host = RoomHost::new(0, rooms.clone(), vec![Arbiter::current()]);
        (host.start_as(7, room), view, rooms)
    }

    fn who(id: PlayerId) -> Entrant {
        Entrant {
            id,
            name: format!("p{}", id % 10),
        }
    }

    fn chess(view: &ViewRx) -> Arc<RoomView<Chess>> {
        view_of::<Chess>(Arc::clone(&*view.borrow())).unwrap()
    }

    /// Waits for a view of the room that `done` likes.
    async fn until(
        view: &mut ViewRx,
        done: impl Fn(&RoomView<Chess>) -> bool,
    ) -> Arc<RoomView<Chess>> {
        let wait = async {
            loop {
                let now = chess(view);
                if done(&now) {
                    return now;
                }
                view.changed().await.unwrap();
            }
        };
        tokio::time::timeout(Duration::from_secs(5), wait)
            .await
            .expect("the room got there")
    }

    fn seat(view: &RoomView<Chess>, id: PlayerId) -> Option<&SeatView> {
        view.shell.seats.iter().find(|s| s.who.id == id)
    }

    fn play(room: &RoomHandle, id: PlayerId, uci: &str) {
        let mv = ChessInput::Move(Move::parse_uci(uci).unwrap());
        room.send(id, RoomCmd::Play(games::encode(&mv)));
    }

    /// Players 1 and 2, signed in to accounts 10 and 20 unless `guest`,
    /// sit down and start the game.
    async fn started(room: &RoomHandle, view: &mut ViewRx, guest: bool) -> [Arc<()>; 2] {
        let here = [Arc::new(()), Arc::new(())];
        for (id, presence) in [1, 2].into_iter().zip(&here) {
            let account = (!guest).then_some(id as AccountId * 10);
            room.join(who(id), account, Arc::downgrade(presence))
                .await
                .unwrap();
            room.send(id, RoomCmd::Ready);
        }
        until(view, |v| matches!(v.shell.phase, PhaseView::Running { .. })).await;
        here
    }

    #[actix_rt::test]
    async fn a_dropped_player_keeps_their_seat_and_takes_it_back() {
        let (room, mut view, rooms) = chess_room(REJOIN_WITHIN);
        let mut here = started(&room, &mut view, false).await;
        assert_eq!(rooms.seat_of(10), Some(7), "listed as seated");
        play(&room, 1, "e2e4");
        play(&room, 2, "e7e5");
        until(&mut view, |v| {
            v.game.as_ref().is_some_and(|g| g.moves.len() == 2)
        })
        .await;

        // Player 1's connection goes; the seat waits.
        here[0] = Arc::new(());
        room.send(1, RoomCmd::Gone);
        let away = until(&mut view, |v| seat(v, 1).is_some_and(|s| s.away.is_some())).await;
        assert!(seat(&away, 1).unwrap().away.unwrap() > Duration::from_secs(80));
        assert!(matches!(away.shell.phase, PhaseView::Running { .. }));

        // Back as player 11, signed in to account 10 again, they play on.
        let back = Arc::new(());
        let rejoined = room.rejoin(who(11), 10, Arc::downgrade(&back)).await;
        assert!(rejoined.is_ok());
        let now = until(&mut view, |v| seat(v, 11).is_some()).await;
        let seat = seat(&now, 11).unwrap();
        assert_eq!((seat.slot, seat.away, seat.left), (Some(0), None, false));
        assert!(!now.shell.seats.iter().any(|s| s.who.id == 1));
        play(&room, 11, "g1f3");
        until(&mut view, |v| {
            v.game.as_ref().is_some_and(|g| g.moves.len() == 3)
        })
        .await;
        let nobody = room.rejoin(who(12), 30, Arc::downgrade(&back)).await;
        assert!(
            matches!(nobody, Err(JoinError::Closed)),
            "account 30 has no seat"
        );

        // Resigning ends the game, and the room no longer lists them.
        room.send(2, RoomCmd::Play(games::encode(&ChessInput::Resign)));
        let over = until(&mut view, |v| {
            matches!(v.shell.phase, PhaseView::Results { .. })
        })
        .await;
        assert_eq!(over.shell.winner(), Some(&who(11)), "named as they are now");
        assert_eq!(rooms.seat_of(10), None);
    }

    #[actix_rt::test]
    async fn a_seat_waits_only_so_long_and_never_for_guests() {
        let (room, mut view, _) = chess_room(Duration::from_millis(300));
        let _here = started(&room, &mut view, false).await;
        play(&room, 1, "e2e4");
        play(&room, 2, "e7e5");
        room.send(2, RoomCmd::Gone);
        let over = until(&mut view, |v| {
            matches!(v.shell.phase, PhaseView::Results { .. })
        })
        .await;
        let outcome = over.game.as_ref().unwrap().outcome.unwrap();
        assert_eq!(outcome.to_string(), "White won by abandonment");

        let (room, mut view, rooms) = chess_room(REJOIN_WITHIN);
        let _here = started(&room, &mut view, true).await;
        assert_eq!(rooms.seat_of(10), None, "guests aren't listed");
        room.send(2, RoomCmd::Gone);
        until(&mut view, |v| {
            matches!(v.shell.phase, PhaseView::Results { .. })
        })
        .await;
    }

    #[actix_rt::test]
    async fn a_new_connection_takes_over_from_an_old_one_still_seated() {
        let (room, mut view, _) = chess_room(REJOIN_WITHIN);
        let _here = started(&room, &mut view, false).await;
        let back = Arc::new(());
        assert!(
            room.rejoin(who(11), 10, Arc::downgrade(&back))
                .await
                .is_ok()
        );
        until(&mut view, |v| seat(v, 11).is_some()).await;
        // The old connection's goodbye is nobody's now.
        room.send(1, RoomCmd::Gone);
        play(&room, 11, "e2e4");
        let now = until(&mut view, |v| {
            v.game.as_ref().is_some_and(|g| g.moves.len() == 1)
        })
        .await;
        assert!(matches!(now.shell.phase, PhaseView::Running { .. }));
    }

    #[test]
    fn only_accounts_are_rated_each_once() {
        let member = |account| RatedMember {
            account,
            rating: Rating::default(),
            notify: RatedNotify::Nobody,
        };
        let members = HashMap::from([
            (1, member(Some(10))),
            (2, member(None)),
            (3, member(Some(30))),
            (4, member(Some(30))),
            (5, member(Some(50))),
        ]);
        let placements: Vec<Placement> = (1..=5)
            .map(|id| Placement {
                who: Entrant {
                    id,
                    name: format!("p{id}"),
                },
                place: id as usize,
            })
            .collect();
        assert_eq!(
            rated_players(&placements, &members),
            [(1, 10, 1), (5, 50, 5)],
            "no guest, and not the account that played twice"
        );
    }

    #[actix_rt::test]
    async fn watchers_have_no_seats_and_only_so_many_fit() {
        let settings = RoomSettings::new(quick_rules(), RoomKind::Private).unwrap();
        let room = Room::<Snake>::new(settings, 1);
        let (room, view) = RoomActor::new(room, RoomSetup::default());
        let room: Arc<dyn RoomPort> = Arc::new(room.start());
        let seat = Arc::new(());
        let who = Entrant {
            id: 1,
            name: "~p1".into(),
        };
        room.join(who, None, Arc::downgrade(&seat)).await.unwrap();

        let here: Vec<Arc<()>> = (0..=MAX_WATCHERS).map(|_| Arc::new(())).collect();
        let watch = |i: usize| room.watch(100 + i as PlayerId, Arc::downgrade(&here[i]));
        for i in 0..MAX_WATCHERS {
            watch(i).await.unwrap();
        }
        let full = watch(MAX_WATCHERS).await;
        assert!(matches!(full, Err(JoinError::Full)));
        assert_eq!(view.borrow().shell().watchers, MAX_WATCHERS);
        let seats: Vec<PlayerId> = view
            .borrow()
            .shell()
            .seats
            .iter()
            .map(|s| s.who.id)
            .collect();
        assert_eq!(seats, [1], "watchers have no seats");

        let mut view = view;
        view.mark_unchanged();
        room.send(100, RoomCmd::Leave);
        view.changed().await.unwrap();
        assert_eq!(view.borrow().shell().watchers, MAX_WATCHERS - 1);
        assert!(watch(MAX_WATCHERS).await.is_ok());
        let snake = view_of::<Snake>(Arc::clone(&*view.borrow())).expect("a Snake room");
        assert_eq!(snake.settings.width, quick_rules().width);
    }

    #[actix_rt::test]
    async fn a_chess_move_shows_at_once() {
        use chess_core::moves::Move;
        use chess_core::round::{Chess, ChessInput, ChessSettings, Side};

        let mut settings = game_api::RoomSettings::<Chess>::new(
            ChessSettings {
                side: Side::White,
                ..ChessSettings::default()
            },
            RoomKind::Private,
        )
        .unwrap();
        settings.countdown = Duration::ZERO;
        let room = Room::<Chess>::new(settings, 1);
        let (room, mut view) = RoomActor::new(room, RoomSetup::default());
        let room: Arc<dyn RoomPort> = Arc::new(room.start());
        let seats = [Arc::new(()), Arc::new(())];
        for (id, seat) in [1, 2].into_iter().zip(&seats) {
            let who = Entrant {
                id,
                name: format!("p{id}"),
            };
            room.join(who, None, Arc::downgrade(seat)).await.unwrap();
            room.send(id, RoomCmd::Ready);
        }
        let game = |view: &ViewRx| view_of::<Chess>(Arc::clone(&*view.borrow())).unwrap();
        while !matches!(game(&view).shell.phase, PhaseView::Running { .. }) {
            view.changed().await.unwrap();
        }
        // Just after a second's tick of the first-move timer, the next is
        // most of a second away.
        view.changed().await.unwrap();
        let e4 = Move::parse_uci("e2e4").unwrap();
        room.send(1, RoomCmd::Play(games::encode(&ChessInput::Move(e4))));
        let shown = tokio::time::timeout(Duration::from_millis(300), async {
            loop {
                view.changed().await.unwrap();
                if game(&view)
                    .game
                    .as_ref()
                    .is_some_and(|g| g.moves.len() == 1)
                {
                    return;
                }
            }
        })
        .await;
        assert!(shown.is_ok(), "the move waited for the room's next tick");
    }

    #[actix_rt::test]
    async fn moves_and_settings_arrive_as_bytes() {
        let settings = RoomSettings::new(quick_rules(), RoomKind::Private).unwrap();
        let (room, mut view) =
            RoomActor::new(Room::<Snake>::new(settings, 1), RoomSetup::default());
        let room: Arc<dyn RoomPort> = Arc::new(room.start());
        let seat = Arc::new(());
        let who = Entrant {
            id: 1,
            name: "~p1".into(),
        };
        room.join(who, None, Arc::downgrade(&seat)).await.unwrap();
        let faster = snake_core::config::Rules {
            tick_ms: 100,
            ..quick_rules()
        };
        view.mark_unchanged();
        room.send(1, RoomCmd::Settings(vec![0xff, 0xff]));
        room.send(1, RoomCmd::Settings(games::encode(&faster)));
        view.changed().await.unwrap();
        let snake = view_of::<Snake>(Arc::clone(&*view.borrow())).unwrap();
        assert_eq!(
            snake.settings.tick_ms, 100,
            "garbage is dropped, and the rest taken"
        );
    }
}
