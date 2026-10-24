//! Takes new connections and hands each one to a worker thread.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use actix::Actor;
use actix_rt::ArbiterHandle;
use tokio::net::TcpListener;
use tracing::{debug, warn};

use crate::Shared;
use crate::proxy::read_header;
use crate::session::{Attach, Hangup, SessionActor};

/// A trusted proxy has this long to send its PROXY header.
const PROXY_HEADER_WAIT: std::time::Duration = std::time::Duration::from_secs(5);
use crate::ssh::ConnHandler;

/// How many connections are open, in total and per IP address.
pub struct Limits {
    total: AtomicUsize,
    per_ip: Mutex<HashMap<IpAddr, usize>>,
    max_total: usize,
    max_per_ip: usize,
}

impl Limits {
    pub fn new(max_total: usize, max_per_ip: usize) -> Arc<Self> {
        Arc::new(Self {
            total: AtomicUsize::new(0),
            per_ip: Mutex::new(HashMap::new()),
            max_total,
            max_per_ip,
        })
    }

    pub fn open(&self) -> usize {
        self.total.load(Ordering::Relaxed)
    }

    /// Lets a connection from `ip` in, if neither limit is reached.
    fn admit(self: &Arc<Self>, ip: IpAddr) -> Option<Admission> {
        let mut per_ip = self.per_ip.lock().unwrap();
        let from_ip = per_ip.entry(ip).or_default();
        if *from_ip >= self.max_per_ip || self.open() >= self.max_total {
            return None;
        }
        *from_ip += 1;
        self.total.fetch_add(1, Ordering::Relaxed);
        Some(Admission {
            limits: Arc::clone(self),
            ip,
            worker: None,
        })
    }
}

/// An admitted connection's place in the limits, given back when it ends.
struct Admission {
    limits: Arc<Limits>,
    ip: IpAddr,
    worker: Option<Arc<AtomicUsize>>,
}

impl Drop for Admission {
    fn drop(&mut self) {
        let mut per_ip = self.limits.per_ip.lock().unwrap();
        if let Some(n) = per_ip.get_mut(&self.ip) {
            *n -= 1;
            if *n == 0 {
                per_ip.remove(&self.ip);
            }
        }
        self.limits.total.fetch_sub(1, Ordering::Relaxed);
        if let Some(worker) = &self.worker {
            worker.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

/// A thread that serves sessions, and how many it has.
pub struct Worker {
    pub arbiter: ArbiterHandle,
    pub sessions: Arc<AtomicUsize>,
}

/// Accepts connections until the task is dropped.
pub async fn accept_loop(
    listener: TcpListener,
    workers: Vec<Worker>,
    limits: Arc<Limits>,
    shared: Arc<Shared>,
) {
    loop {
        let (stream, peer) = match listener.accept().await {
            // IPv4 players on an IPv6 socket show up as `::ffff:a.b.c.d`.
            Ok((stream, peer)) => (
                stream,
                SocketAddr::new(peer.ip().to_canonical(), peer.port()),
            ),
            Err(e) => {
                // Usually out of file descriptors; back off briefly.
                warn!(error = %e, "accept failed");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                continue;
            }
        };
        let proxied = shared.proxies.contains(peer.ip());
        // Through a proxy, the limits wait for the PROXY header's address.
        let admission = if proxied {
            None
        } else {
            let Some(admission) = limits.admit(peer.ip()) else {
                debug!(%peer, "too many connections; turned away");
                continue;
            };
            Some(admission)
        };
        // Moved to its worker as a plain socket, so that worker's thread
        // handles its I/O rather than this one.
        let Ok(stream) = stream.into_std() else {
            continue;
        };
        let _ = stream.set_nodelay(true);
        let worker = workers
            .iter()
            .min_by_key(|w| w.sessions.load(Ordering::Relaxed))
            .expect("at least one worker");
        worker.sessions.fetch_add(1, Ordering::Relaxed);
        let (shared, limits, sessions) = (
            Arc::clone(&shared),
            Arc::clone(&limits),
            Arc::clone(&worker.sessions),
        );
        worker.arbiter.spawn(async move {
            let admitted = match admission {
                Some(admission) => Some((stream, peer, admission)),
                None => through_proxy(stream, peer, &limits).await,
            };
            let Some((stream, peer, mut admission)) = admitted else {
                sessions.fetch_sub(1, Ordering::Relaxed);
                return;
            };
            admission.worker = Some(sessions);
            serve(stream, peer, shared, admission).await;
        });
    }
}

/// Reads the PROXY header a trusted proxy sends first, then admits the
/// player it names. The proxy's own connections (health checks) are
/// dropped.
async fn through_proxy(
    stream: std::net::TcpStream,
    proxy: SocketAddr,
    limits: &Arc<Limits>,
) -> Option<(std::net::TcpStream, SocketAddr, Admission)> {
    let mut stream = tokio::net::TcpStream::from_std(stream).ok()?;
    let header = tokio::time::timeout(PROXY_HEADER_WAIT, read_header(&mut stream)).await;
    let peer = match header {
        Ok(Ok(Some(peer))) => SocketAddr::new(peer.ip().to_canonical(), peer.port()),
        Ok(Ok(None)) => return None,
        Ok(Err(e)) => {
            debug!(%proxy, error = %e, "a bad PROXY header");
            return None;
        }
        Err(_) => {
            debug!(%proxy, "no PROXY header in time");
            return None;
        }
    };
    let Some(admission) = limits.admit(peer.ip()) else {
        debug!(%peer, %proxy, "too many connections; turned away");
        return None;
    };
    Some((stream.into_std().ok()?, peer, admission))
}

/// Runs one connection, on its worker's thread.
async fn serve(
    stream: std::net::TcpStream,
    peer: SocketAddr,
    shared: Arc<Shared>,
    _admission: Admission,
) {
    let Ok(hangup) = Hangup::new(&stream) else {
        return;
    };
    let Ok(stream) = tokio::net::TcpStream::from_std(stream) else {
        return;
    };
    let session = SessionActor::new(Arc::clone(&shared), peer).start();
    let handler = ConnHandler::new(session.clone());
    let started = tokio::time::timeout(
        shared.cfg.login_timeout(),
        russh::server::run_stream(Arc::clone(&shared.ssh), stream, handler),
    )
    .await;
    let running = match started {
        Ok(Ok(running)) => running,
        Ok(Err(e)) => return debug!(%peer, error = %e, "handshake failed"),
        Err(_) => return debug!(%peer, "no SSH greeting in time"),
    };
    session.do_send(Attach(hangup));
    if let Err(e) = running.await {
        debug!(%peer, error = %e, "connection ended with an error");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_count_per_ip_and_in_total() {
        let limits = Limits::new(3, 2);
        let a: IpAddr = "10.0.0.1".parse().unwrap();
        let b: IpAddr = "10.0.0.2".parse().unwrap();
        let first = limits.admit(a).unwrap();
        let _second = limits.admit(a).unwrap();
        assert!(limits.admit(a).is_none(), "two per IP");
        let _third = limits.admit(b).unwrap();
        assert!(limits.admit(b).is_none(), "three in total");
        drop(first);
        assert_eq!(limits.open(), 2);
        assert!(limits.admit(a).is_some(), "a slot came back");
    }
}
