//! The quick-play queue, as each server sees it.
//!
//! On its own, a server runs the [`Matcher`] once a second and starts a room
//! for every match it forms. In a cluster, it sends its waiting players to
//! the cluster's queue (`snaketui-matchmaker`, see [`crate::queue_service`])
//! instead. It then hosts the matches the queue gives it, and passes its
//! players on to matches hosted elsewhere.

use std::collections::HashMap;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use actix::prelude::*;
use snake_core::round::{Entrant, PlayerId, RoomKind, Round, RoundSettings, quick_rules};
use tokio::sync::watch;
use tracing::{debug, warn};

use crate::cluster::Cluster;
use crate::cluster::wire::{
    InputKind, MadeMatch, MakeMatch, MatchSeat, MatchedAt, QueueAlive, QueueLeave, QueueTicket,
    RatedAt, decode,
};
use crate::matcher::{Matcher, Ticket};
use crate::rating::Rating;
use crate::room::{
    Drain, Presence, Rated, RatedMember, RatedNotify, Ratings, RoomActor, RoomHandle, RoomHost,
    ViewRx,
};
use crate::store::{AccountId, Store};

const RUN_EVERY: Duration = Duration::from_secs(1);
/// In a cluster: every waiting player is sent to the queue again this
/// often, in case a message went missing.
const RESEND_EVERY: Duration = Duration::from_secs(15);
/// In a cluster: how long a player's server waits for news of their rating
/// after their match started elsewhere.
const RATED_WAIT: Duration = Duration::from_secs(15 * 60);

/// Puts a player in the queue. They get [`Matched`] once a match forms.
#[derive(Message)]
#[rtype(result = "()")]
pub struct Enqueue {
    pub who: Entrant,
    pub account: Option<AccountId>,
    pub rating: Rating,
    pub region: String,
    pub presence: Presence,
    pub reply: Recipient<Matched>,
    pub on_rated: Recipient<Rated>,
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct Dequeue(pub PlayerId);

/// A match formed: the player is already seated in this room.
#[derive(Message)]
#[rtype(result = "()")]
pub struct Matched {
    pub room: RoomHandle,
    pub view: ViewRx,
}

struct Waiter {
    who: Entrant,
    account: Option<AccountId>,
    rating: Rating,
    region: String,
    presence: Presence,
    reply: Recipient<Matched>,
    on_rated: Recipient<Rated>,
    since: Instant,
}

/// Who makes the matches.
enum Queue {
    /// This server, for its own players.
    Here { matcher: Matcher, epoch: Instant },
    /// The cluster's queue.
    Cluster {
        cluster: Arc<Cluster>,
        /// The queue's instance last heard from; a new one has restarted
        /// and needs every waiting player again.
        heard: Option<u64>,
        /// Players whose match runs elsewhere, until their rating arrives.
        rated: HashMap<PlayerId, (Recipient<Rated>, Instant)>,
    },
}

pub struct MatchmakerActor {
    queue: Queue,
    waiting: HashMap<PlayerId, Waiter>,
    host: Arc<RoomHost>,
    store: Store,
    settings: RoundSettings,
    seed: Option<u64>,
    queue_len: watch::Sender<usize>,
    /// The server is stopping: no new matches here.
    draining: bool,
}

impl MatchmakerActor {
    /// `queue_len` is kept up to date with how many are waiting. `seed`
    /// fixes every match's seed, for tests. With `cluster`, the cluster's
    /// queue makes the matches.
    pub fn new(
        host: Arc<RoomHost>,
        store: Store,
        queue_len: watch::Sender<usize>,
        seed: Option<u64>,
        cluster: Option<Arc<Cluster>>,
    ) -> Self {
        let queue = match cluster {
            Some(cluster) => Queue::Cluster {
                cluster,
                heard: None,
                rated: HashMap::new(),
            },
            None => Queue::Here {
                matcher: Matcher::default(),
                epoch: Instant::now(),
            },
        };
        Self {
            queue,
            waiting: HashMap::new(),
            host,
            store,
            settings: RoundSettings::new(quick_rules(), RoomKind::Match)
                .expect("the quick-play board fits a full room"),
            seed,
            queue_len,
            draining: false,
        }
    }

    /// Forgets waiting players whose session is gone.
    fn drop_gone(&mut self) {
        let gone: Vec<PlayerId> = self
            .waiting
            .iter()
            .filter(|(_, w)| w.presence.strong_count() == 0)
            .map(|(id, _)| *id)
            .collect();
        for id in gone {
            self.waiting.remove(&id);
            self.unqueue(id);
        }
    }

    fn unqueue(&mut self, player: PlayerId) {
        match &mut self.queue {
            Queue::Here { matcher, .. } => {
                matcher.remove(player);
                self.queue_len.send_replace(matcher.len());
            }
            Queue::Cluster { cluster, .. } => {
                cluster.send(cluster.subject("queue.leave"), &QueueLeave { player });
            }
        }
    }

    /// Sends waiting players to the cluster's queue.
    fn send_tickets<'a>(cluster: &Cluster, waiters: impl Iterator<Item = &'a Waiter>) {
        let subject = cluster.subject("queue.join");
        for w in waiters {
            let ticket = QueueTicket {
                who: w.who.clone(),
                account: w.account,
                rating: w.rating,
                region: w.region.clone(),
                server: cluster.server,
                waited: w.since.elapsed(),
            };
            cluster.send(subject.clone(), &ticket);
        }
    }

    /// Matches this server's own players.
    fn run(&mut self) {
        self.drop_gone();
        let Queue::Here { matcher, epoch } = &mut self.queue else {
            return;
        };
        if self.draining {
            return;
        }
        for group in matcher.run(epoch.elapsed()) {
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
            let members = waiters
                .iter()
                .map(|w| {
                    let member = RatedMember {
                        account: w.account,
                        rating: w.rating,
                        notify: RatedNotify::Here(w.on_rated.clone()),
                    };
                    (w.who.id, member)
                })
                .collect();
            let ratings = Ratings {
                store: self.store.clone(),
                members,
            };
            let seed = self.seed.unwrap_or_else(rand::random);
            let round = Round::new_match(self.settings.clone(), seed, entrants);
            let (room, view) = RoomActor::new(round, None, None, present, Some(ratings));
            let room = self.host.start(room);
            debug!(players = waiters.len(), region = %group.region, "match formed");
            for w in waiters {
                w.reply.do_send(Matched {
                    room: room.clone(),
                    view: view.clone(),
                });
            }
        }
        self.queue_len.send_replace(matcher.len());
    }

    /// Starts a match the cluster's queue picked this server for. Players
    /// here are told at once; the others' servers are sent word.
    fn host_match(&mut self, seats: Vec<MatchSeat>) -> MadeMatch {
        let Queue::Cluster { cluster, .. } = &self.queue else {
            return None;
        };
        if self.draining {
            return None;
        }
        let cluster = Arc::clone(cluster);
        let me = cluster.server;
        let id = self.host.new_id();
        let elsewhere: Vec<(PlayerId, _)> = seats
            .iter()
            .filter(|s| s.server != me)
            .map(|s| (s.who.id, s.server))
            .collect();
        let mut present = cluster.reserve_guests(id, &elsewhere);
        let mut here = Vec::new();
        let mut members = HashMap::new();
        for seat in &seats {
            let player = seat.who.id;
            let notify = if seat.server != me {
                RatedNotify::There {
                    cluster: Arc::clone(&cluster),
                    server: seat.server,
                    player,
                }
            } else if let Some(w) = self.waiting.remove(&player) {
                present.insert(player, w.presence.clone());
                let notify = RatedNotify::Here(w.on_rated.clone());
                here.push(w);
                notify
            } else {
                // Left the queue meanwhile: seated anyway, and dropped on
                // the room's first tick.
                present.insert(player, Weak::new());
                RatedNotify::Nobody
            };
            let member = RatedMember {
                account: seat.account,
                rating: seat.rating,
                notify,
            };
            members.insert(player, member);
        }
        let ratings = Ratings {
            store: self.store.clone(),
            members,
        };
        let seed = self.seed.unwrap_or_else(rand::random);
        let entrants = seats.iter().map(|s| s.who.clone()).collect();
        let round = Round::new_match(self.settings.clone(), seed, entrants);
        let (room, view) = RoomActor::new(round, None, None, present, Some(ratings));
        let room = self.host.start_as(id, room);
        if let Some((_, thread)) = self.host.rooms().get_with_thread(id) {
            cluster.start_sending(id, &thread, view.clone());
        }
        debug!(
            players = seats.len(),
            from_elsewhere = elsewhere.len(),
            "hosting a match"
        );
        for w in here {
            w.reply.do_send(Matched {
                room: room.clone(),
                view: view.clone(),
            });
        }
        for (player, server) in elsewhere {
            let news = MatchedAt {
                player,
                room: room.at(),
            };
            cluster.send(cluster.server_subject(server, "matched"), &news);
        }
        Some(room.at())
    }

    /// A player here was matched into a room on another server: they take
    /// their seat there. If that fails, they're back in the queue.
    fn matched_elsewhere(
        &mut self,
        MatchedAt { player, room }: MatchedAt,
        ctx: &mut Context<Self>,
    ) {
        let Queue::Cluster { cluster, rated, .. } = &mut self.queue else {
            return;
        };
        let cluster = Arc::clone(cluster);
        let Some(w) = self
            .waiting
            .remove(&player)
            .filter(|w| w.presence.strong_count() > 0)
        else {
            cluster.send_input(room, player, InputKind::Leave);
            return;
        };
        rated.insert(player, (w.on_rated.clone(), Instant::now()));
        let join = Arc::clone(&cluster).join_remote(room, w.who.clone(), w.presence.clone());
        ctx.spawn(
            join.into_actor(self)
                .map(move |joined, act, _| match joined {
                    Ok(view) => w.reply.do_send(Matched {
                        room: RoomHandle::Remote { at: room, cluster },
                        view,
                    }),
                    Err(_) => {
                        debug!(
                            player,
                            "couldn't take a seat in a match elsewhere; queueing again"
                        );
                        MatchmakerActor::send_tickets(&cluster, std::iter::once(&w));
                        act.waiting.insert(player, w);
                    }
                }),
        );
    }

    fn rated_elsewhere(&mut self, news: RatedAt) {
        if let Queue::Cluster { rated, .. } = &mut self.queue
            && let Some((session, _)) = rated.remove(&news.player)
        {
            session.do_send(Rated {
                rating: news.rating,
                won: news.won,
            });
        }
    }

    fn heard_queue(&mut self, alive: QueueAlive) {
        self.queue_len.send_replace(alive.waiting as usize);
        if let Queue::Cluster { cluster, heard, .. } = &mut self.queue
            && *heard != Some(alive.instance)
        {
            if heard.is_some() {
                debug!("the cluster's queue restarted; sending it everyone waiting");
            }
            *heard = Some(alive.instance);
            Self::send_tickets(cluster, self.waiting.values());
        }
    }

    /// Listens for the cluster's queue, and for matches it gives this
    /// server.
    fn join_cluster_queue(&mut self, ctx: &mut Context<Self>) {
        let Queue::Cluster { cluster, .. } = &self.queue else {
            return;
        };
        let cluster = Arc::clone(cluster);
        let subjects = [
            cluster.subject("queue.alive"),
            cluster.server_subject(cluster.server, "make_match"),
            cluster.server_subject(cluster.server, "matched"),
            cluster.server_subject(cluster.server, "rated"),
        ];
        let subscribed = async move {
            let mut subs = Vec::new();
            for subject in subjects {
                subs.push(cluster.subscribe(subject).await?);
            }
            anyhow::Ok(subs)
        };
        ctx.wait(
            subscribed
                .into_actor(self)
                .map(|subscribed, _, ctx| match subscribed {
                    Ok(subs) => {
                        for sub in subs {
                            ctx.add_stream(sub);
                        }
                    }
                    Err(e) => warn!(error = %e, "couldn't listen to the cluster's queue"),
                }),
        );
        ctx.run_interval(RESEND_EVERY, |act, _| {
            if let Queue::Cluster { cluster, .. } = &act.queue {
                Self::send_tickets(cluster, act.waiting.values());
            }
        });
        ctx.run_interval(RUN_EVERY, |act, _| {
            act.drop_gone();
            if let Queue::Cluster { rated, .. } = &mut act.queue {
                rated.retain(|_, (_, since)| since.elapsed() < RATED_WAIT);
            }
        });
    }
}

impl Actor for MatchmakerActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        match self.queue {
            Queue::Here { .. } => {
                ctx.run_interval(RUN_EVERY, |act, _| act.run());
            }
            Queue::Cluster { .. } => self.join_cluster_queue(ctx),
        }
    }
}

/// What the cluster's queue says, and matches for this server.
impl StreamHandler<async_nats::Message> for MatchmakerActor {
    fn handle(&mut self, msg: async_nats::Message, ctx: &mut Self::Context) {
        let subject = msg.subject.as_str();
        if subject.ends_with(".queue.alive") {
            if let Some(alive) = decode(&msg.payload) {
                self.heard_queue(alive);
            }
        } else if subject.ends_with(".make_match") {
            let made = decode::<MakeMatch>(&msg.payload).and_then(|m| self.host_match(m.seats));
            if let (Some(reply), Queue::Cluster { cluster, .. }) = (msg.reply, &self.queue) {
                cluster.send(reply.to_string(), &made);
            }
        } else if subject.ends_with(".matched") {
            if let Some(matched) = decode(&msg.payload) {
                self.matched_elsewhere(matched, ctx);
            }
        } else if subject.ends_with(".rated")
            && let Some(news) = decode(&msg.payload)
        {
            self.rated_elsewhere(news);
        }
    }

    // A subscription ends when NATS closes; the actor carries on.
    fn finished(&mut self, _: &mut Self::Context) {}
}

impl Handler<Enqueue> for MatchmakerActor {
    type Result = ();

    fn handle(&mut self, msg: Enqueue, _: &mut Self::Context) {
        let waiter = Waiter {
            who: msg.who,
            account: msg.account,
            rating: msg.rating,
            region: msg.region,
            presence: msg.presence,
            reply: msg.reply,
            on_rated: msg.on_rated,
            since: Instant::now(),
        };
        match &mut self.queue {
            Queue::Here { matcher, epoch } => {
                matcher.add(Ticket {
                    player: waiter.who.id,
                    rating: f64::from(waiter.rating.shown()),
                    region: waiter.region.clone(),
                    since: epoch.elapsed(),
                });
                self.queue_len.send_replace(matcher.len());
            }
            Queue::Cluster { cluster, .. } => {
                Self::send_tickets(cluster, std::iter::once(&waiter));
            }
        }
        self.waiting.insert(waiter.who.id, waiter);
    }
}

impl Handler<Drain> for MatchmakerActor {
    type Result = ();

    fn handle(&mut self, _: Drain, _: &mut Self::Context) {
        self.draining = true;
    }
}

impl Handler<Dequeue> for MatchmakerActor {
    type Result = ();

    fn handle(&mut self, Dequeue(id): Dequeue, _: &mut Self::Context) {
        self.waiting.remove(&id);
        self.unqueue(id);
    }
}
