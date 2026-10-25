//! Running servers: stopping without cutting rounds short, health and
//! metrics, real client addresses through a proxy, and one host key for
//! every server.

mod common;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use common::rooms::*;
use common::*;
use russh::client;
use russh::keys::{HashAlg, PublicKeyOrCertificate};
use snaketui_server::proxy::v2_header;
use tokio::io::AsyncWriteExt;

#[tokio::test]
async fn a_stopping_server_lets_rounds_finish_and_sends_everyone_else_off() {
    let mut server = TestServer::with(|c| c.drain_timeout_secs = 60);
    let (addr, admin) = (server.addr, server.admin);
    let mut ann = guest(addr, "ann", 100, 30).await;
    let mut ben = guest(addr, "ben", 100, 30).await;
    let mut cat = guest(addr, "cat", 100, 30).await;
    let code = create_room(&mut ann).await;
    join_room(&mut ben, &code).await;
    ann.send(ENTER).await;
    ben.send(ENTER).await;
    for p in [&mut ann, &mut ben] {
        p.wait_until("the countdown", Duration::from_secs(5), |p| {
            p.text().contains("GET READY")
        })
        .await;
    }

    assert_eq!(http_get(admin, "/healthz").await.0, 200);

    let stopping = tokio::task::spawn_blocking(move || {
        server.stop();
        server
    });
    // cat, in the menu, is told at once, and nobody new gets in.
    cat.wait_closed(Duration::from_secs(3)).await;
    assert!(cat.text().contains("restarting"), "{}", cat.text());
    assert!(tokio::net::TcpStream::connect(addr).await.is_err());
    // A load balancer sees it's draining, and sends players elsewhere.
    assert_eq!(
        http_get(admin, "/healthz").await,
        (503, "draining\n".into())
    );

    // ann and ben play their round to the end and see the results before
    // they go. (Waiting fails if they're cut off first.)
    for p in [&mut ann, &mut ben] {
        p.wait_until("the results", Duration::from_secs(40), |p| {
            p.text().contains("RESULTS")
        })
        .await;
    }
    for p in [&mut ann, &mut ben] {
        p.wait_closed(Duration::from_secs(10)).await;
        assert!(p.text().contains("restarting"), "{}", p.text());
    }
    let _stopped = stopping.await.unwrap();
}

#[tokio::test]
async fn metrics_count_players_and_rooms() {
    let server = TestServer::start();
    let mut ann = guest(server.addr, "ann", 100, 30).await;
    let _ben = guest(server.addr, "ben", 100, 30).await;
    create_room(&mut ann).await;
    let (status, metrics) = http_get(server.admin, "/metrics").await;
    assert_eq!(status, 200);
    for line in [
        "snaketui_players 2",
        "snaketui_rooms 1",
        "snaketui_draining 0",
        "# TYPE snaketui_tick_lag_seconds histogram",
        "snaketui_tick_lag_seconds_bucket{le=\"+Inf\"}",
    ] {
        assert!(metrics.contains(line), "no {line:?} in:\n{metrics}");
    }
    assert_eq!(http_get(server.admin, "/nothing").await.0, 404);
}

/// Players listed with their round trip.
fn timed_players(p: &Player) -> usize {
    p.text()
        .lines()
        .filter(|l| l.contains('~') && l.contains(" ms"))
        .count()
}

#[tokio::test]
async fn players_see_each_others_round_trips() {
    let server = TestServer::start();
    let mut ann = guest(server.addr, "ann", 100, 30).await;
    let mut ben = guest(server.addr, "ben", 100, 30).await;
    let code = create_room(&mut ann).await;
    join_room(&mut ben, &code).await;
    // Both terminals answer the server's status queries (while read, so
    // both at once); each lobby shows both players' round trips.
    let limit = Duration::from_secs(10);
    tokio::join!(
        ann.wait_until("both round trips", limit, |p| timed_players(p) == 2),
        ben.wait_until("both round trips", limit, |p| timed_players(p) == 2),
    );
    let (_, metrics) = http_get(server.admin, "/metrics").await;
    let count: u64 = metrics
        .lines()
        .find_map(|l| l.strip_prefix("snaketui_ping_seconds_count "))
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("no ping count in:\n{metrics}"));
    assert!(count >= 2, "{count} round trips counted");
    assert!(metrics.contains("snaketui_ping_seconds_bucket{le=\"0.01\"}"));
}

#[tokio::test]
async fn answering_round_trips_isnt_being_active() {
    let server = TestServer::with(|c| c.idle_timeout_secs = 1);
    let mut ann = guest(server.addr, "ann", 100, 30).await;
    // The idle check runs every 15 s; meanwhile ann's terminal answers
    // every status query, and that alone mustn't keep her in.
    ann.wait_closed(Duration::from_secs(25)).await;
    assert!(
        ann.text().contains("Logged out after being idle"),
        "{}",
        ann.text()
    );
}

/// A guest whose connection comes through a proxy saying they're at
/// `client`; `None` if the server turns them away.
async fn through_proxy(server: SocketAddr, client: &str) -> Option<Player> {
    let mut stream = tokio::net::TcpStream::connect(server).await.unwrap();
    let header = v2_header(client.parse().unwrap(), server);
    stream.write_all(&header).await.unwrap();
    let config = Arc::new(client::Config::default());
    let mut ssh = client::connect_stream(config, stream, TrustAnyHost)
        .await
        .ok()?;
    if !sign_in(&mut ssh, Login::Guest).await {
        return None;
    }
    let mut p = Player::open(ssh, 100, 30).await;
    p.wait_for("Quick play").await;
    Some(p)
}

#[tokio::test]
async fn behind_a_proxy_limits_count_each_players_own_address() {
    let server = TestServer::with(|c| {
        c.trusted_proxies = vec!["127.0.0.0/8".into()];
        c.max_sessions_per_ip = 1;
    });
    let _first = through_proxy(server.addr, "203.0.113.7:50000")
        .await
        .expect("the first player from 203.0.113.7");
    assert!(
        through_proxy(server.addr, "203.0.113.7:50001")
            .await
            .is_none(),
        "a second from the same address is one too many"
    );
    let _other = through_proxy(server.addr, "198.51.100.9:50002")
        .await
        .expect("a player from another address");

    // The proxy's own address must say who its player is.
    let config = Arc::new(client::Config::default());
    let plain = client::connect(config, server.addr, TrustAnyHost).await;
    assert!(plain.is_err(), "a connection from the proxy with no header");
}

/// Remembers the key the server showed.
struct NoteKey(Arc<std::sync::Mutex<Option<String>>>);

impl client::Handler for NoteKey {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        if let PublicKeyOrCertificate::PublicKey { key, .. } = key {
            *self.0.lock().unwrap() = Some(key.fingerprint(HashAlg::Sha256).to_string());
        }
        Ok(true)
    }
}

async fn host_key_of(server: SocketAddr) -> String {
    let seen = Arc::new(std::sync::Mutex::new(None));
    let config = Arc::new(client::Config::default());
    let _ssh = client::connect(config, server, NoteKey(Arc::clone(&seen)))
        .await
        .unwrap();
    seen.lock().unwrap().clone().expect("a server key")
}

#[tokio::test]
async fn servers_given_one_host_key_look_the_same_to_players() {
    let shared = std::env::temp_dir().join(format!("snaketui-key-{}", std::process::id()));
    std::fs::create_dir_all(&shared).unwrap();
    let key = shared.join("host_ed25519_key");
    snaketui_server::ssh::load_or_create_host_key(&key).unwrap();
    let a = TestServer::with(|c| c.host_key = key.clone());
    let b = TestServer::with(|c| c.host_key = key.clone());
    let elsewhere = TestServer::start();
    assert_eq!(host_key_of(a.addr).await, host_key_of(b.addr).await);
    assert_ne!(host_key_of(a.addr).await, host_key_of(elsewhere.addr).await);
    let _ = std::fs::remove_dir_all(&shared);
}
