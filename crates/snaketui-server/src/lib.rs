//! Serves SnakeTUI over SSH. The game process is the SSH server: players get
//! no account and no shell, only the game, and quitting it ends the
//! connection.
//!
//! Each connection is a [`session::SessionActor`] on one of several worker
//! threads (actix arbiters); its SSH I/O runs on the same thread.

pub mod accept;
pub mod admin;
pub mod auth;
pub mod backend;
pub mod cluster;
pub mod config;
pub mod identity;
pub mod input;
pub mod lobby;
pub mod matcher;
pub mod matchmaker;
pub mod metrics;
pub mod presence;
pub mod proxy;
pub mod queue_service;
pub mod rating;
pub mod room;
pub mod session;
pub mod ssh;
pub mod store;

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize};
use std::time::{Duration, Instant};

use actix::{Actor, Addr};
use actix_rt::Arbiter;
use anyhow::Result;
use russh::keys::PrivateKey;
use tokio::sync::watch;
use tracing::{info, warn};

use crate::accept::{Limits, Worker};
use crate::auth::{GuessLimits, Hasher};
use crate::cluster::Cluster;
use crate::config::ServerConfig;
use crate::lobby::LobbyActor;
use crate::matchmaker::MatchmakerActor;
use crate::metrics::TICK_LAG;
use crate::presence::PresenceActor;
use crate::proxy::Trusted;
use crate::room::{Drain, LocalRooms, RoomHandle, RoomHost, RoomRef, ServerId};
use crate::store::Store;

/// Threads that run rooms, the lobby and the matchmaker.
const ROOM_THREADS: usize = 2;

/// What every session can reach.
pub struct Shared {
    pub cfg: ServerConfig,
    /// Proxies whose connections start with a PROXY header.
    pub proxies: Trusted,
    /// This server, among those sharing rooms; 0 for a server on its own.
    pub server: ServerId,
    pub rooms: LocalRooms,
    pub cluster: Option<Arc<Cluster>>,
    pub ssh: Arc<russh::server::Config>,
    pub store: Store,
    /// How far along stopping the server is.
    pub shutdown: watch::Sender<Stopping>,
    pub lobby: Addr<LobbyActor>,
    pub matchmaker: Addr<MatchmakerActor>,
    pub presence: Addr<PresenceActor>,
    pub hasher: Hasher,
    /// Wrong passwords and new accounts, for the guessing limits.
    pub guesses: GuessLimits,
    /// How many players are waiting in the quick-play queue.
    pub queue_len: watch::Receiver<usize>,
    /// Hands out each session's player id.
    pub next_player: AtomicU64,
}

impl Shared {
    /// The room at `at`, if it's still there.
    pub fn room(&self, at: RoomRef) -> Option<RoomHandle> {
        if at.server != self.server {
            let cluster = self.cluster.as_ref().filter(|c| c.alive(at.server))?;
            return Some(RoomHandle::Remote {
                at,
                cluster: Arc::clone(cluster),
            });
        }
        let addr = self.rooms.get(at.room)?;
        Some(RoomHandle::Local { at, addr })
    }
}

/// Stopping, step by step.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stopping {
    No,
    /// No new players or matches; rounds in play finish.
    Draining,
    /// Everyone is logged out.
    Now,
}

/// How long sessions get to say goodbye when the server stops.
const GOODBYE: Duration = Duration::from_secs(3);

/// Serves players on `listener` until `stop` completes. Must run inside an
/// actix `System`.
pub async fn serve(
    cfg: ServerConfig,
    listener: std::net::TcpListener,
    admin: Option<std::net::TcpListener>,
    host_key: PrivateKey,
    stop: impl Future<Output = ()>,
) -> Result<()> {
    let store = Store::open(&cfg.database).await?;
    let limits = Limits::new(cfg.max_sessions, cfg.max_sessions_per_ip);
    let room_arbiters: Vec<Arbiter> = (0..ROOM_THREADS).map(|_| Arbiter::new()).collect();
    let rooms = LocalRooms::default();
    let (server, cluster) = match &cfg.cluster {
        Some(cluster) => {
            let server = rand::random::<ServerId>().max(1);
            let open = Arc::clone(&limits);
            let load = Box::new(move || open.open() as u32);
            let joined = Cluster::connect(cluster, server, rooms.clone(), load).await?;
            (server, Some(joined))
        }
        None => (0, None),
    };
    let host = Arc::new(RoomHost::new(
        server,
        rooms.clone(),
        room_arbiters.iter().map(Arbiter::handle).collect(),
    ));
    let lobby = LobbyActor::start_in_arbiter(&room_arbiters[0].handle(), {
        let host = Arc::clone(&host);
        let (cluster, store) = (cluster.clone(), store.clone());
        move |_| LobbyActor::new(host, cluster, store)
    });
    let presence = PresenceActor::start_in_arbiter(&room_arbiters[0].handle(), {
        let cluster = cluster.clone();
        move |_| PresenceActor::new(cluster)
    });
    let (queue_tx, queue_len) = watch::channel(0);
    let seed = cfg.room_seed;
    let matchmaker = MatchmakerActor::start_in_arbiter(&room_arbiters[0].handle(), {
        let store = store.clone();
        let cluster = cluster.clone();
        move |_| MatchmakerActor::new(host, store, queue_tx, seed, cluster)
    });
    let arbiters: Vec<Arbiter> = (0..cfg.workers()).map(|_| Arbiter::new()).collect();
    let workers = arbiters
        .iter()
        .map(|a| Worker {
            arbiter: a.handle(),
            sessions: Arc::new(AtomicUsize::new(0)),
        })
        .collect();
    let guesses = GuessLimits::new(store.clone());
    let shared = Arc::new(Shared {
        ssh: Arc::new(ssh::config(host_key)),
        proxies: Trusted::parse(&cfg.trusted_proxies)?,
        cfg,
        server,
        rooms,
        cluster: cluster.clone(),
        store,
        shutdown: watch::channel(Stopping::No).0,
        lobby,
        matchmaker,
        presence,
        hasher: Hasher::new()?,
        guesses,
        queue_len,
        // In a cluster, ids from different servers meet in rooms.
        next_player: AtomicU64::new(if cluster.is_some() {
            u64::from(rand::random::<u32>()) << 32 | 1
        } else {
            1
        }),
    });
    // Forget old wrong guesses now and then, so the limits' tables stay
    // small.
    let tidying = actix_rt::spawn({
        let guesses = shared.guesses.clone();
        async move {
            loop {
                tokio::time::sleep(Duration::from_secs(600)).await;
                if let Err(e) = guesses.tidy(store::now()).await {
                    warn!(error = %e, "couldn't tidy the guessing limits");
                }
            }
        }
    });

    listener.set_nonblocking(true)?;
    let listener = tokio::net::TcpListener::from_std(listener)?;
    info!(
        addr = %listener.local_addr()?,
        workers = arbiters.len(),
        "serving SnakeTUI over SSH"
    );
    let answering = match admin {
        Some(admin) => {
            admin.set_nonblocking(true)?;
            let admin = tokio::net::TcpListener::from_std(admin)?;
            info!(addr = %admin.local_addr()?, "health and metrics over HTTP");
            let serving = admin::serve_admin(admin, Arc::clone(&shared), Arc::clone(&limits));
            Some(actix_rt::spawn(serving))
        }
        None => None,
    };
    let accepting = actix_rt::spawn(accept::accept_loop(
        listener,
        workers,
        Arc::clone(&limits),
        Arc::clone(&shared),
    ));

    stop.await;
    accepting.abort();
    tidying.abort();
    let drain = shared.cfg.drain_timeout();
    if !drain.is_zero() {
        info!(
            players = limits.open(),
            rooms = shared.rooms.len(),
            "draining: rounds in play finish first"
        );
        shared.shutdown.send_replace(Stopping::Draining);
        if let Some(cluster) = &shared.cluster {
            cluster.set_draining();
        }
        shared.matchmaker.do_send(Drain);
        for room in shared.rooms.all() {
            room.do_send(Drain);
        }
        let deadline = Instant::now() + drain;
        while (limits.open() > 0 || !shared.rooms.is_empty()) && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    info!(players = limits.open(), "shutting down");
    shared.shutdown.send_replace(Stopping::Now);
    let deadline = Instant::now() + GOODBYE;
    while limits.open() > 0 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    if let Some(answering) = answering {
        answering.abort();
    }
    for arbiter in arbiters.into_iter().chain(room_arbiters) {
        arbiter.stop();
        let _ = arbiter.join();
    }
    if let (Some(p50), Some(p99)) = (TICK_LAG.quantile(0.5), TICK_LAG.quantile(0.99)) {
        info!(?p50, ?p99, ticks = TICK_LAG.count(), "rooms' tick lag");
    }
    shared.store.close().await;
    Ok(())
}
