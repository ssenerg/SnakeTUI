//! Serves SnakeTUI over SSH. The game process is the SSH server: players get
//! no account and no shell, only the game, and quitting it ends the
//! connection.
//!
//! Each connection is a [`session::SessionActor`] on one of several worker
//! threads (actix arbiters); its SSH I/O runs on the same thread.

pub mod accept;
pub mod backend;
pub mod config;
pub mod identity;
pub mod input;
pub mod lobby;
pub mod matcher;
pub mod matchmaker;
pub mod metrics;
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
use tracing::info;

use crate::accept::{Limits, Worker};
use crate::config::ServerConfig;
use crate::lobby::LobbyActor;
use crate::matchmaker::MatchmakerActor;
use crate::metrics::TICK_LAG;
use crate::room::RoomHost;
use crate::store::Store;

/// Threads that run rooms, the lobby and the matchmaker.
const ROOM_THREADS: usize = 2;

/// What every session can reach.
pub struct Shared {
    pub cfg: ServerConfig,
    pub ssh: Arc<russh::server::Config>,
    pub store: Store,
    /// Becomes `true` when the server is shutting down.
    pub shutdown: watch::Sender<bool>,
    pub lobby: Addr<LobbyActor>,
    pub matchmaker: Addr<MatchmakerActor>,
    /// How many players are waiting in the quick-play queue.
    pub queue_len: watch::Receiver<usize>,
    /// Hands out each session's player id.
    pub next_player: AtomicU64,
}

/// How long sessions get to say goodbye when the server stops.
const GOODBYE: Duration = Duration::from_secs(3);

/// Serves players on `listener` until `stop` completes. Must run inside an
/// actix `System`.
pub async fn serve(
    cfg: ServerConfig,
    listener: std::net::TcpListener,
    host_key: PrivateKey,
    stop: impl Future<Output = ()>,
) -> Result<()> {
    let store = Store::open(&cfg.database).await?;
    let limits = Limits::new(cfg.max_sessions, cfg.max_sessions_per_ip);
    let room_arbiters: Vec<Arbiter> = (0..ROOM_THREADS).map(|_| Arbiter::new()).collect();
    let host = Arc::new(RoomHost::new(
        room_arbiters.iter().map(Arbiter::handle).collect(),
    ));
    let lobby = LobbyActor::start_in_arbiter(&room_arbiters[0].handle(), {
        let host = Arc::clone(&host);
        move |_| LobbyActor::new(host)
    });
    let (queue_tx, queue_len) = watch::channel(0);
    let seed = cfg.room_seed;
    let matchmaker = MatchmakerActor::start_in_arbiter(&room_arbiters[0].handle(), move |_| {
        MatchmakerActor::new(host, queue_tx, seed)
    });
    let arbiters: Vec<Arbiter> = (0..cfg.workers()).map(|_| Arbiter::new()).collect();
    let workers = arbiters
        .iter()
        .map(|a| Worker {
            arbiter: a.handle(),
            sessions: Arc::new(AtomicUsize::new(0)),
        })
        .collect();
    let shared = Arc::new(Shared {
        ssh: Arc::new(ssh::config(host_key)),
        cfg,
        store,
        shutdown: watch::channel(false).0,
        lobby,
        matchmaker,
        queue_len,
        next_player: AtomicU64::new(1),
    });

    listener.set_nonblocking(true)?;
    let listener = tokio::net::TcpListener::from_std(listener)?;
    info!(
        addr = %listener.local_addr()?,
        workers = arbiters.len(),
        "serving SnakeTUI over SSH"
    );
    let accepting = actix_rt::spawn(accept::accept_loop(
        listener,
        workers,
        Arc::clone(&limits),
        Arc::clone(&shared),
    ));

    stop.await;
    info!(players = limits.open(), "shutting down");
    accepting.abort();
    shared.shutdown.send_replace(true);
    let deadline = Instant::now() + GOODBYE;
    while limits.open() > 0 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
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
