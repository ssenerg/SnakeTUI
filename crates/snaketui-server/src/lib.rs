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
pub mod session;
pub mod ssh;
pub mod store;

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::time::{Duration, Instant};

use actix_rt::Arbiter;
use anyhow::Result;
use russh::keys::PrivateKey;
use tokio::sync::watch;
use tracing::info;

use crate::accept::{Limits, Worker};
use crate::config::ServerConfig;
use crate::store::Store;

/// What every session can reach.
pub struct Shared {
    pub cfg: ServerConfig,
    pub ssh: Arc<russh::server::Config>,
    pub store: Store,
    /// Becomes `true` when the server is shutting down.
    pub shutdown: watch::Sender<bool>,
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
    for arbiter in arbiters {
        arbiter.stop();
        let _ = arbiter.join();
    }
    shared.store.close().await;
    Ok(())
}
