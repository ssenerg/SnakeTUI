//! The quick-play queue: runs the [`Matcher`] once a second and starts a
//! room for every match it forms.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use actix::prelude::*;
use snake_core::round::{Entrant, PlayerId, RoomKind, Round, RoundSettings, quick_rules};
use tokio::sync::watch;
use tracing::debug;

use crate::matcher::{Matcher, Ticket};
use crate::room::{Presence, RoomActor, RoomHost, ViewRx};

const RUN_EVERY: Duration = Duration::from_secs(1);

/// Puts a player in the queue. They get [`Matched`] once a match forms.
#[derive(Message)]
#[rtype(result = "()")]
pub struct Enqueue {
    pub who: Entrant,
    pub rating: f64,
    pub region: String,
    pub presence: Presence,
    pub reply: Recipient<Matched>,
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct Dequeue(pub PlayerId);

/// A match formed: the player is already seated in this room.
#[derive(Message)]
#[rtype(result = "()")]
pub struct Matched {
    pub room: Addr<RoomActor>,
    pub view: ViewRx,
}

struct Waiter {
    who: Entrant,
    presence: Presence,
    reply: Recipient<Matched>,
}

pub struct MatchmakerActor {
    matcher: Matcher,
    waiting: HashMap<PlayerId, Waiter>,
    epoch: Instant,
    host: Arc<RoomHost>,
    settings: RoundSettings,
    seed: Option<u64>,
    queue_len: watch::Sender<usize>,
}

impl MatchmakerActor {
    /// `queue_len` is kept up to date with how many are waiting. `seed`
    /// fixes every match's seed, for tests.
    pub fn new(host: Arc<RoomHost>, queue_len: watch::Sender<usize>, seed: Option<u64>) -> Self {
        Self {
            matcher: Matcher::default(),
            waiting: HashMap::new(),
            epoch: Instant::now(),
            host,
            settings: RoundSettings::new(quick_rules(), RoomKind::Match)
                .expect("the quick-play board fits a full room"),
            seed,
            queue_len,
        }
    }

    fn run(&mut self) {
        let gone: Vec<PlayerId> = self
            .waiting
            .iter()
            .filter(|(_, w)| w.presence.strong_count() == 0)
            .map(|(id, _)| *id)
            .collect();
        for id in gone {
            self.waiting.remove(&id);
            self.matcher.remove(id);
        }
        for group in self.matcher.run(self.epoch.elapsed()) {
            let waiters: Vec<Waiter> = group
                .players
                .iter()
                .filter_map(|id| self.waiting.remove(id))
                .collect();
            let entrants = waiters.iter().map(|w| w.who.clone()).collect();
            let present = waiters
                .iter()
                .map(|w| (w.who.id, w.presence.clone()))
                .collect();
            let seed = self.seed.unwrap_or_else(rand::random);
            let round = Round::new_match(self.settings.clone(), seed, entrants);
            let (room, view) = RoomActor::new(round, None, None, present);
            let room = self.host.start(room);
            debug!(players = waiters.len(), region = %group.region, "match formed");
            for w in waiters {
                w.reply.do_send(Matched {
                    room: room.clone(),
                    view: view.clone(),
                });
            }
        }
        self.queue_len.send_replace(self.matcher.len());
    }
}

impl Actor for MatchmakerActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        ctx.run_interval(RUN_EVERY, |act, _| act.run());
    }
}

impl Handler<Enqueue> for MatchmakerActor {
    type Result = ();

    fn handle(&mut self, msg: Enqueue, _: &mut Self::Context) {
        self.matcher.add(Ticket {
            player: msg.who.id,
            rating: msg.rating,
            region: msg.region,
            since: self.epoch.elapsed(),
        });
        self.waiting.insert(
            msg.who.id,
            Waiter {
                who: msg.who,
                presence: msg.presence,
                reply: msg.reply,
            },
        );
        self.queue_len.send_replace(self.matcher.len());
    }
}

impl Handler<Dequeue> for MatchmakerActor {
    type Result = ();

    fn handle(&mut self, Dequeue(id): Dequeue, _: &mut Self::Context) {
        self.waiting.remove(&id);
        self.matcher.remove(id);
        self.queue_len.send_replace(self.matcher.len());
    }
}
