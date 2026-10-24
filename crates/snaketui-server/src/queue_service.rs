//! The cluster's quick-play queue, which `snaketui-matchmaker` runs: one
//! for the whole cluster.
//!
//! Servers send it their waiting players. It runs the [`Matcher`] on all of
//! them and asks a server to host each match it forms: one in the region
//! most of the players are in, preferring the server that already has the
//! most of them, then the least busy.
//!
//! Nothing is kept on disk. When it restarts, servers hear its new
//! instance and send their players again, with how long they've waited.

use std::collections::HashMap;
use std::future::Future;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use async_nats::{Client, ConnectOptions};
use futures::StreamExt;
use snake_core::round::PlayerId;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::cluster::wire::{
    Alive, MadeMatch, MakeMatch, MatchSeat, QueueAlive, QueueLeave, QueueTicket, decode, encode,
    server_subject,
};
use crate::cluster::{HEARTBEAT, PEER_TIMEOUT, prefix};
use crate::config::ClusterConfig;
use crate::matcher::{Matcher, Ticket};
use crate::room::{RoomRef, ServerId};

/// How often the queue looks for matches.
const RUN_EVERY: Duration = Duration::from_millis(250);
/// Players matched this recently are ignored if their server sends them
/// again before it has heard of the match.
const RECENTLY: Duration = Duration::from_secs(30);
/// Servers asked to host one match, at most, before its players go back
/// in the queue.
const HOST_TRIES: usize = 3;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
/// Where the queue's clock starts, so players who waited before it started
/// can be placed before then.
const CLOCK_START: Duration = Duration::from_secs(24 * 60 * 60);

/// A server, as its heartbeats describe it.
#[derive(Clone, Debug)]
pub struct ServerSeen {
    pub region: String,
    pub load: u32,
    pub seen: Instant,
}

/// A match formed, and the servers to ask to host it, best first.
#[derive(Debug)]
pub struct Plan {
    pub seats: Vec<MatchSeat>,
    pub hosts: Vec<ServerId>,
    /// The players' tickets, and when each joined on the queue's clock, in
    /// case the match can't be hosted and they go back.
    back: Vec<(QueueTicket, Duration)>,
}

/// The queue, apart from NATS: tickets in, plans out.
pub struct Queue {
    matcher: Matcher,
    /// Each waiting player's ticket, and when they joined on the queue's
    /// clock.
    tickets: HashMap<PlayerId, (QueueTicket, Duration)>,
    servers: HashMap<ServerId, ServerSeen>,
    recently: HashMap<PlayerId, Instant>,
    started: Instant,
    /// Tests only: how much faster than real time the queue's clock runs.
    speed: f64,
}

impl Queue {
    pub fn new(speed: f64) -> Self {
        Self {
            matcher: Matcher::default(),
            tickets: HashMap::new(),
            servers: HashMap::new(),
            recently: HashMap::new(),
            started: Instant::now(),
            speed,
        }
    }

    /// The queue's clock.
    fn now(&self) -> Duration {
        CLOCK_START + self.started.elapsed().mul_f64(self.speed)
    }

    pub fn len(&self) -> usize {
        self.matcher.len()
    }

    pub fn is_empty(&self) -> bool {
        self.matcher.is_empty()
    }

    /// Queues a player, or updates them if they're already queued.
    pub fn add(&mut self, ticket: QueueTicket) {
        let player = ticket.who.id;
        if self
            .recently
            .get(&player)
            .is_some_and(|at| at.elapsed() < RECENTLY)
        {
            return;
        }
        let since = self.now().saturating_sub(ticket.waited.mul_f64(self.speed));
        self.queue(ticket, since);
    }

    fn queue(&mut self, ticket: QueueTicket, since: Duration) {
        self.matcher.add(Ticket {
            player: ticket.who.id,
            rating: f64::from(ticket.rating.shown()),
            region: ticket.region.clone(),
            since,
        });
        self.tickets.insert(ticket.who.id, (ticket, since));
    }

    pub fn remove(&mut self, player: PlayerId) {
        self.matcher.remove(player);
        self.tickets.remove(&player);
    }

    pub fn heard(&mut self, alive: Alive) {
        let seen = ServerSeen {
            region: alive.region,
            load: alive.load,
            seen: Instant::now(),
        };
        self.servers.insert(alive.server, seen);
    }

    fn alive(&self, server: ServerId) -> bool {
        self.servers
            .get(&server)
            .is_some_and(|s| s.seen.elapsed() < PEER_TIMEOUT)
    }

    /// Forms every match it can, and plans where each should run.
    pub fn run(&mut self) -> Vec<Plan> {
        // Players whose server went quiet are gone. A queue just started
        // waits until it has heard every server once.
        if self.started.elapsed() > PEER_TIMEOUT {
            let gone: Vec<PlayerId> = self
                .tickets
                .values()
                .filter(|(t, _)| !self.alive(t.server))
                .map(|(t, _)| t.who.id)
                .collect();
            for player in gone {
                self.remove(player);
            }
        }
        self.recently.retain(|_, at| at.elapsed() < RECENTLY);
        let now = self.now();
        let mut plans = Vec::new();
        for group in self.matcher.run(now) {
            let back: Vec<(QueueTicket, Duration)> = group
                .players
                .iter()
                .filter_map(|p| self.tickets.remove(p))
                .collect();
            let seats: Vec<MatchSeat> = back
                .iter()
                .map(|(t, _)| MatchSeat {
                    who: t.who.clone(),
                    account: t.account,
                    rating: t.rating,
                    server: t.server,
                })
                .collect();
            for seat in &seats {
                self.recently.insert(seat.who.id, Instant::now());
            }
            let live: HashMap<ServerId, &ServerSeen> = self
                .servers
                .iter()
                .filter(|(_, s)| s.seen.elapsed() < PEER_TIMEOUT)
                .map(|(id, s)| (*id, s))
                .collect();
            let hosts = choose_hosts(&group.region, &seats, &live);
            plans.push(Plan { seats, hosts, back });
        }
        plans
    }

    /// Nobody could host `plan`: its players wait again, keeping their
    /// place.
    pub fn unplanned(&mut self, plan: Plan) {
        for (ticket, since) in plan.back {
            self.recently.remove(&ticket.who.id);
            self.queue(ticket, since);
        }
    }
}

/// The servers that could host a match of `seats`, best first: those in
/// `region` (or, if none is alive there, all), the ones with more of the
/// players first, then the least busy.
pub fn choose_hosts(
    region: &str,
    seats: &[MatchSeat],
    servers: &HashMap<ServerId, &ServerSeen>,
) -> Vec<ServerId> {
    let here = |id: ServerId| seats.iter().filter(|s| s.server == id).count();
    let mut hosts: Vec<ServerId> = servers
        .iter()
        .filter(|(_, s)| s.region == region)
        .map(|(id, _)| *id)
        .collect();
    if hosts.is_empty() {
        hosts = servers.keys().copied().collect();
    }
    hosts.sort_by_key(|&id| (std::cmp::Reverse(here(id)), servers[&id].load, id));
    hosts
}

/// Runs the cluster's queue until `stop` completes.
pub async fn run(cfg: &ClusterConfig, speed: f64, stop: impl Future<Output = ()>) -> Result<()> {
    let client = ConnectOptions::new()
        .name("snaketui-matchmaker")
        .request_timeout(Some(REQUEST_TIMEOUT))
        .connect(&cfg.nats)
        .await
        .context("connecting to NATS")?;
    let prefix = prefix(&cfg.name);
    let mut joins = client.subscribe(format!("{prefix}.queue.join")).await?;
    let mut leaves = client.subscribe(format!("{prefix}.queue.leave")).await?;
    let mut alive = client.subscribe(format!("{prefix}.alive")).await?;
    let instance: u64 = rand::random();
    let mut queue = Queue::new(speed);
    let (unplanned, mut unplanned_rx) = mpsc::unbounded_channel();
    let mut beat = tokio::time::interval(HEARTBEAT);
    let mut tick = tokio::time::interval(RUN_EVERY);
    tokio::pin!(stop);
    info!(cluster = %cfg.name, "matchmaking for the cluster");
    loop {
        tokio::select! {
            _ = &mut stop => break,
            Some(msg) = joins.next() => {
                if let Some(ticket) = decode(&msg.payload) {
                    queue.add(ticket);
                }
            }
            Some(msg) = leaves.next() => {
                if let Some(QueueLeave { player }) = decode(&msg.payload) {
                    queue.remove(player);
                }
            }
            Some(msg) = alive.next() => {
                if let Some(heard) = decode(&msg.payload) {
                    queue.heard(heard);
                }
            }
            Some(plan) = unplanned_rx.recv() => queue.unplanned(plan),
            _ = tick.tick() => {
                for plan in queue.run() {
                    tokio::spawn(host(client.clone(), prefix.clone(), plan, unplanned.clone()));
                }
            }
            _ = beat.tick() => {
                let news = QueueAlive {
                    instance,
                    waiting: queue.len() as u32,
                };
                if let Err(e) = client.publish(format!("{prefix}.queue.alive"), encode(&news)).await {
                    warn!(error = %e, "couldn't send to NATS");
                }
            }
        }
    }
    info!("stopped matchmaking");
    Ok(())
}

/// Asks the planned servers in turn to host the match; if none does, its
/// players go back in the queue.
async fn host(client: Client, prefix: String, plan: Plan, unplanned: mpsc::UnboundedSender<Plan>) {
    let request = encode(&MakeMatch {
        seats: plan.seats.clone(),
    });
    for &server in plan.hosts.iter().take(HOST_TRIES) {
        let subject = server_subject(&prefix, server, "make_match");
        let made: Option<RoomRef> = match client.request(subject, request.clone()).await {
            Ok(reply) => decode::<MadeMatch>(&reply.payload).flatten(),
            Err(e) => {
                debug!(error = %e, "a server didn't take a match");
                None
            }
        };
        if let Some(room) = made {
            debug!(
                players = plan.seats.len(),
                server = %format!("{:016x}", room.server),
                "match formed"
            );
            return;
        }
    }
    warn!(
        players = plan.seats.len(),
        "no server took a match; queueing its players again"
    );
    let _ = unplanned.send(plan);
}

#[cfg(test)]
mod tests {
    use snake_core::round::Entrant;

    use super::*;
    use crate::rating::Rating;

    fn ticket(id: PlayerId, server: ServerId, region: &str, waited: u64) -> QueueTicket {
        QueueTicket {
            who: Entrant {
                id,
                name: format!("p{id}"),
            },
            account: None,
            rating: Rating::default(),
            region: region.into(),
            server,
            waited: Duration::from_secs(waited),
        }
    }

    fn alive(server: ServerId, region: &str, load: u32) -> Alive {
        Alive {
            server,
            region: region.into(),
            load,
        }
    }

    #[test]
    fn matches_go_where_most_players_are_then_to_the_least_busy() {
        let seen = |region: &str, load| ServerSeen {
            region: region.into(),
            load,
            seen: Instant::now(),
        };
        let servers = [(1, seen("eu", 50)), (2, seen("eu", 10)), (3, seen("us", 0))];
        let servers: HashMap<ServerId, &ServerSeen> =
            servers.iter().map(|(i, s)| (*i, s)).collect();
        let seat = |id, server| MatchSeat {
            who: Entrant {
                id,
                name: String::new(),
            },
            account: None,
            rating: Rating::default(),
            server,
        };
        // Two of three players are on server 1: it hosts, busy or not.
        let seats = [seat(1, 1), seat(2, 1), seat(3, 3)];
        assert_eq!(choose_hosts("eu", &seats, &servers), [1, 2]);
        // Nobody on an eu server: the less busy one.
        let seats = [seat(1, 3), seat(2, 3)];
        assert_eq!(choose_hosts("eu", &seats, &servers), [2, 1]);
        // No server alive in the region: anywhere, by the same rules.
        assert_eq!(choose_hosts("asia", &seats, &servers), [3, 2, 1]);
    }

    #[test]
    fn players_keep_their_wait_and_are_not_matched_twice() {
        let mut queue = Queue::new(1.0);
        for server in [1, 2] {
            queue.heard(alive(server, "eu", 0));
        }
        // Both have waited long enough already, before a restart.
        queue.add(ticket(10, 1, "eu", 11));
        queue.add(ticket(11, 2, "eu", 11));
        let plans = queue.run();
        assert_eq!(plans.len(), 1, "matched at once, not 10 s from now");
        assert_eq!(plans[0].seats.len(), 2);
        // A server that hasn't heard yet sends them again.
        queue.add(ticket(10, 1, "eu", 12));
        assert!(queue.is_empty());

        // Nobody could host it: they wait again, still at the front.
        let plan = queue.run().pop();
        assert!(plan.is_none());
        queue.unplanned(plans.into_iter().next().unwrap());
        assert_eq!(queue.len(), 2);
        assert_eq!(queue.run().len(), 1);
    }

    #[test]
    fn a_player_from_a_server_that_went_quiet_is_dropped() {
        let mut queue = Queue::new(1.0);
        queue.started -= PEER_TIMEOUT * 2;
        queue.heard(alive(1, "eu", 0));
        queue.add(ticket(10, 1, "eu", 0));
        queue.add(ticket(11, 9, "eu", 0));
        queue.run();
        assert_eq!(queue.len(), 1, "server 9 never said it was alive");
    }

    #[test]
    fn the_clock_can_run_fast_for_tests() {
        let mut queue = Queue::new(1000.0);
        for server in [1, 2] {
            queue.heard(alive(server, "eu", 0));
        }
        queue.add(ticket(10, 1, "eu", 0));
        queue.add(ticket(11, 2, "eu", 0));
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(queue.run().len(), 1, "20 ms is 20 s on this clock");
    }
}
