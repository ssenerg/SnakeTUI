//! One multiplayer room: its round, its clock, and the view it publishes.
//!
//! A room never waits on its players. It publishes each new view on a watch
//! channel, and every session reads the latest one at its own pace.

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use actix::prelude::*;
use actix_rt::ArbiterHandle;
use snake_core::geom::Direction;
use snake_core::round::{
    Entrant, JoinError, PhaseView, PlayerId, RoomKind, RoomView, Round, RoundEvent,
};
use tokio::sync::watch;
use tracing::{debug, warn};

use crate::lobby::{LobbyActor, RoomClosed};
use crate::metrics::TICK_LAG;

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

/// The threads rooms run on, handed out in turn.
pub struct RoomHost {
    arbiters: Vec<ArbiterHandle>,
    next: AtomicUsize,
}

impl RoomHost {
    pub fn new(arbiters: Vec<ArbiterHandle>) -> Self {
        assert!(!arbiters.is_empty());
        Self {
            arbiters,
            next: AtomicUsize::new(0),
        }
    }

    /// Starts `room` on the next room thread.
    pub fn start(&self, room: RoomActor) -> Addr<RoomActor> {
        let i = self.next.fetch_add(1, Ordering::Relaxed) % self.arbiters.len();
        RoomActor::start_in_arbiter(&self.arbiters[i], move |_| room)
    }
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

pub struct RoomActor {
    round: Round,
    code: Option<String>,
    view: watch::Sender<Arc<RoomView>>,
    lobby: Option<Addr<LobbyActor>>,
    present: HashMap<PlayerId, Presence>,
    last: Instant,
    /// When the timer was meant to fire.
    due: Instant,
    over_since: Option<Instant>,
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
        if let (Some(lobby), Some(code)) = (&self.lobby, self.code.take()) {
            lobby.do_send(RoomClosed(code));
        }
    }
}

impl RoomActor {
    /// A room for `round`, with the receiver for its view. `present` lists
    /// the players already seated, as a matched room has.
    pub fn new(
        round: Round,
        code: Option<String>,
        lobby: Option<Addr<LobbyActor>>,
        present: HashMap<PlayerId, Presence>,
    ) -> (Self, ViewRx) {
        let (view, rx) = watch::channel(Arc::new(view_of(&round, &code)));
        let now = Instant::now();
        let room = Self {
            round,
            code,
            view,
            lobby,
            present,
            last: now,
            due: now,
            over_since: None,
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
        if let Some(events) = self.guarded(ctx, |round| round.leave(id)) {
            self.on_events(events);
        }
    }

    fn on_events(&mut self, events: Vec<RoundEvent>) {
        for event in events {
            let RoundEvent::Ended { outcome, .. } = event;
            debug!(code = ?self.code, ?outcome, "round over");
        }
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
            self.view
                .send_replace(Arc::new(view_of(&self.round, &self.code)));
        }
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

fn view_of(round: &Round, code: &Option<String>) -> RoomView {
    let mut view = round.view();
    view.code.clone_from(code);
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

impl Handler<Ready> for RoomActor {
    type Result = ();

    fn handle(&mut self, Ready(id): Ready, ctx: &mut Self::Context) {
        self.guarded(ctx, |round| round.toggle_ready(id));
        self.publish();
    }
}
