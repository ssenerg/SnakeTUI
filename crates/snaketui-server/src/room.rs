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
use snake_core::config::Rules;
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
use crate::rating::Rating;
use crate::store::{AccountId, Store};

pub type ViewRx = watch::Receiver<Arc<RoomView>>;

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

    /// Lets `who` watch without a seat; `presence` shows they're still
    /// there.
    pub fn watch(
        &self,
        who: Entrant,
        presence: Presence,
    ) -> impl Future<Output = Result<ViewRx, JoinError>> + use<> {
        let room = self.clone();
        async move {
            match room {
                RoomHandle::Local { addr, .. } => addr
                    .send(Watch {
                        id: who.id,
                        presence,
                    })
                    .await
                    .unwrap_or(Err(JoinError::Closed)),
                RoomHandle::Remote { at, cluster } => {
                    cluster.join_remote(at, who, presence, true).await
                }
            }
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
                RoomHandle::Remote { at, cluster } => {
                    cluster.join_remote(at, who, presence, false).await
                }
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

    /// The host adds a bot, or with `add` false, takes one out.
    pub fn bot(&self, id: PlayerId, add: bool) {
        match self {
            RoomHandle::Local { addr, .. } => {
                let _ = addr.try_send(Bot { id, add });
            }
            RoomHandle::Remote { at, cluster } => {
                let kind = if add {
                    InputKind::AddBot
                } else {
                    InputKind::RemoveBot
                };
                cluster.send_input(*at, id, kind)
            }
        }
    }

    pub fn set_rules(&self, id: PlayerId, rules: Rules) {
        match self {
            RoomHandle::Local { addr, .. } => {
                let _ = addr.try_send(SetRules { id, rules });
            }
            RoomHandle::Remote { at, cluster } => {
                cluster.send_input(*at, id, InputKind::Rules(rules))
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

/// Watching without a seat. A full room of watchers says
/// [`JoinError::Full`].
#[derive(Message)]
#[rtype(result = "Result<ViewRx, JoinError>")]
pub struct Watch {
    pub id: PlayerId,
    pub presence: Presence,
}

/// Gives up a seat, or stops watching.
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

/// The host changes the rules of the room's next rounds.
#[derive(Message)]
#[rtype(result = "()")]
pub struct SetRules {
    pub id: PlayerId,
    pub rules: Rules,
}

/// The host adds a bot, or takes one out.
#[derive(Message)]
#[rtype(result = "()")]
pub struct Bot {
    pub id: PlayerId,
    pub add: bool,
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
        let (view, rx) = watch::channel(Arc::new(view_of(&round, &code, &[], &pings, 0)));
        let now = Instant::now();
        let room = Self {
            registered: None,
            round,
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
        self.on_events(events, ctx);
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
        if let Some(events) = self.guarded(ctx, |round| round.leave(id)) {
            self.on_events(events, ctx);
        }
    }

    fn on_events(&mut self, events: Vec<RoundEvent>, ctx: &mut Context<Self>) {
        for event in events {
            let RoundEvent::Ended {
                outcome,
                placements,
            } = event;
            debug!(code = ?self.code, ?outcome, "round over");
            self.rate(&outcome, &placements, ctx);
        }
    }

    /// Rates the accounts that played against each other: works out their
    /// new ratings from the saved ones, saves them, tells each player, and
    /// shows the changes with the results. It finishes even if the room
    /// closes meanwhile.
    fn rate(&mut self, outcome: &Outcome, placements: &[Placement], ctx: &mut Context<Self>) {
        self.changes.clear();
        let Some(ratings) = &self.ratings else {
            return;
        };
        let winner = match outcome {
            Outcome::Won { winner, .. } => Some(winner.id),
            Outcome::Draw { .. } => None,
        };
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
        let (store, room) = (ratings.store.clone(), ctx.address());
        actix::spawn(async move {
            let changes = match store.rate_round(&players).await {
                Ok(changes) => changes,
                Err(e) => return warn!(error = %e, "couldn't save ratings"),
            };
            let mut shown = Vec::with_capacity(changes.len());
            for (((id, notify), (_, _, won)), (before, after)) in
                notify.into_iter().zip(&players).zip(changes)
            {
                if let Some(notify) = notify {
                    notify.send(Rated {
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
            self.watchers.len(),
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

fn view_of(
    round: &Round,
    code: &Option<String>,
    changes: &[RatingChange],
    pings: &HashMap<PlayerId, Duration>,
    watchers: usize,
) -> RoomView {
    let mut view = round.view();
    view.code.clone_from(code);
    view.watchers = watchers;
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

/// A rated round's changes, saved: shown with its results.
#[derive(Message)]
#[rtype(result = "()")]
struct RatingsChanged(Vec<RatingChange>);

impl Handler<RatingsChanged> for RoomActor {
    type Result = ();

    fn handle(&mut self, RatingsChanged(changes): RatingsChanged, _: &mut Self::Context) {
        self.changes = changes;
        self.publish_now();
    }
}

impl Handler<Watch> for RoomActor {
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

impl Handler<SetRules> for RoomActor {
    type Result = ();

    fn handle(&mut self, SetRules { id, rules }: SetRules, ctx: &mut Self::Context) {
        if self.guarded(ctx, |round| round.set_rules(id, rules)) == Some(true) {
            debug!(code = ?self.code, "rules changed");
            self.publish();
        }
    }
}

impl Handler<Bot> for RoomActor {
    type Result = ();

    fn handle(&mut self, Bot { id, add }: Bot, ctx: &mut Self::Context) {
        let changed = self.guarded(ctx, |round| {
            if add {
                round.add_bot(id)
            } else {
                round.remove_bot(id)
            }
        });
        if changed == Some(true) {
            debug!(code = ?self.code, add, players = self.round.players(), "bots changed");
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

#[cfg(test)]
mod tests {
    use snake_core::round::{Placement, RoomKind, Round, RoundSettings, quick_rules};

    use super::*;

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
                length: 3,
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
        let settings = RoundSettings::new(quick_rules(), RoomKind::Private).unwrap();
        let round = Round::new(settings, 1);
        let (room, view) = RoomActor::new(round, None, None, HashMap::new(), None);
        let room = room.start();
        let seat = Arc::new(());
        let who = Entrant {
            id: 1,
            name: "~p1".into(),
        };
        let presence = Arc::downgrade(&seat);
        room.send(Join { who, presence }).await.unwrap().unwrap();

        let here: Vec<Arc<()>> = (0..=MAX_WATCHERS).map(|_| Arc::new(())).collect();
        let watch = |i: usize| Watch {
            id: 100 + i as PlayerId,
            presence: Arc::downgrade(&here[i]),
        };
        for i in 0..MAX_WATCHERS {
            room.send(watch(i)).await.unwrap().unwrap();
        }
        let full = room.send(watch(MAX_WATCHERS)).await.unwrap();
        assert!(matches!(full, Err(JoinError::Full)));
        assert_eq!(view.borrow().watchers, MAX_WATCHERS);
        let seats: Vec<PlayerId> = view.borrow().seats.iter().map(|s| s.who.id).collect();
        assert_eq!(seats, [1], "watchers have no seats");

        room.send(Leave(100)).await.unwrap();
        assert_eq!(view.borrow().watchers, MAX_WATCHERS - 1);
        assert!(room.send(watch(MAX_WATCHERS)).await.unwrap().is_ok());
    }
}
