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
use crate::cluster::wire::RatedAt;
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
pub struct LocalRooms(Arc<Mutex<HashMap<RoomId, Running>>>);

impl LocalRooms {
    pub fn get(&self, id: RoomId) -> Option<Arc<dyn RoomPort>> {
        self.0
            .lock()
            .unwrap()
            .get(&id)
            .map(|(port, _)| Arc::clone(port))
    }

    /// The room, and the thread it runs on, for work that belongs with it.
    pub fn get_with_thread(&self, id: RoomId) -> Option<Running> {
        self.0.lock().unwrap().get(&id).cloned()
    }

    fn insert(&self, id: RoomId, port: Arc<dyn RoomPort>, thread: ArbiterHandle) {
        self.0.lock().unwrap().insert(id, (port, thread));
    }

    fn remove(&self, id: RoomId) {
        self.0.lock().unwrap().remove(&id);
    }

    pub fn len(&self) -> usize {
        self.0.lock().unwrap().len()
    }

    pub fn all(&self) -> Vec<Arc<dyn RoomPort>> {
        self.0
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

/// A room running here, of whichever game.
pub trait RoomPort: Send + Sync {
    /// Takes a seat for `who`; `presence` shows the player is still there.
    fn join(&self, who: Entrant, presence: Presence) -> BoxFuture<'static, Joined>;
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
    fn join(&self, who: Entrant, presence: Presence) -> BoxFuture<'static, Joined> {
        let joined = self.send(Join { who, presence });
        Box::pin(async move { joined.await.unwrap_or(Err(JoinError::Closed)) })
    }

    fn watch(&self, id: PlayerId, presence: Presence) -> BoxFuture<'static, Joined> {
        let joined = self.send(Watch { id, presence });
        Box::pin(async move { joined.await.unwrap_or(Err(JoinError::Closed)) })
    }

    fn send(&self, id: PlayerId, cmd: RoomCmd) {
        if cmd == RoomCmd::Leave {
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
                Box::pin(Arc::clone(cluster).join_remote(*at, who, presence, true))
            }
        }
    }

    /// Takes a seat for `who`; `presence` shows the player is still there.
    pub fn join(&self, who: Entrant, presence: Presence) -> BoxFuture<'static, Joined> {
        match self {
            RoomHandle::Local { port, .. } => port.join(who, presence),
            RoomHandle::Remote { at, cluster } => {
                Box::pin(Arc::clone(cluster).join_remote(*at, who, presence, false))
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
        let first: Arc<dyn AnyRoomView> = Arc::new(view_of(&room, &code, &[], &pings, 0));
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
            self.leave(id, ctx);
        }
    }

    fn leave(&mut self, id: PlayerId, ctx: &mut Context<Self>) {
        if self.watchers.remove(&id).is_some() {
            return self.publish_now();
        }
        self.present.remove(&id);
        self.pings.remove(&id);
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
        self.view.send_replace(Arc::new(view_of(
            &self.room,
            &self.code,
            &self.changes,
            &self.pings,
            self.watchers.len(),
        )));
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
            RoomCmd::Leave => {
                self.leave(id, ctx);
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
                if let Some(end) = self.guarded(ctx, |room| room.play(id, input, rtt))
                    && end.is_some()
                {
                    self.on_end(end, ctx);
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
    watchers: usize,
) -> RoomView<G> {
    let mut view = room.view();
    let shell = &mut view.shell;
    shell.code.clone_from(code);
    shell.watchers = watchers;
    if !shell.placements.is_empty() {
        shell.ratings = changes.to_vec();
    }
    for seat in &mut shell.seats {
        seat.ping = pings.get(&seat.who.id).copied();
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
        self.publish();
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
    use snake_core::round::{Placement, RoomKind, RoomSettings, Snake, quick_rules};

    use super::*;
    use crate::games::view_of;

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
        room.join(who, Arc::downgrade(&seat)).await.unwrap();

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
        room.join(who, Arc::downgrade(&seat)).await.unwrap();
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
