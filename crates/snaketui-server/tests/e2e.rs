//! End to end: a real server on a local port, played through russh's SSH
//! client, with what the player would see read back by a terminal emulator.

mod common;

use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use common::*;
use russh::ChannelMsg;
use russh::client;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio::time::{Instant, sleep, timeout};

#[tokio::test]
async fn quitting_logs_out_at_once_and_restores_the_screen() {
    let server = TestServer::start();
    let key = new_key();
    let mut p = Player::join(server.addr, Login::Key(&key), 100, 30).await;
    p.single_player().await;
    assert!(
        p.screen.screen().alternate_screen(),
        "the game has its own screen"
    );

    p.send(ENTER).await;
    p.wait_for("press an arrow key").await;
    p.send(ESC).await;
    p.wait_for("PAUSED").await;
    p.send(b"q").await;
    p.wait_for("Settings").await;

    p.send(b"q").await;
    p.wait_for("Quick play").await;

    let quit = Instant::now();
    p.send(b"q").await;
    assert_eq!(p.wait_closed(Duration::from_secs(2)).await, Some(0));
    assert!(
        quit.elapsed() < Duration::from_secs(1),
        "took {:?}",
        quit.elapsed()
    );
    assert!(
        !p.screen.screen().alternate_screen(),
        "back on the player's own screen"
    );
}

#[tokio::test]
async fn ctrl_c_logs_out_and_a_key_keeps_its_settings_and_records() {
    let server = TestServer::start();
    let key = new_key();
    let mut p = Player::join(server.addr, Login::Key(&key), 100, 40).await;
    p.single_player().await;
    p.wait_for("best for this setup: 0").await;
    p.send(b"d").await; // board width 30 -> 31
    p.wait_for("◂ 31 ▸").await;
    p.send(ENTER).await;
    p.wait_for("press an arrow key").await;
    p.send(CTRL_C).await;
    assert_eq!(p.wait_closed(Duration::from_secs(2)).await, Some(0));
    drop(p);
    // The saves run in the background; give them a moment.
    sleep(Duration::from_millis(200)).await;

    let mut again = Player::join(server.addr, Login::Key(&key), 100, 40).await;
    again.single_player().await;
    again.wait_for("◂ 31 ▸").await;
    again.wait_for("best for this setup: 3").await;

    let other = new_key();
    let mut someone_else = Player::join(server.addr, Login::Key(&other), 100, 40).await;
    someone_else.single_player().await;
    someone_else.wait_for("◂ 30 ▸").await;
    someone_else.wait_for("best for this setup: 0").await;
}

#[tokio::test]
async fn guests_play_without_a_key_and_none_lets_nobody_in() {
    let server = TestServer::start();
    let mut client = connect(server.addr).await;
    assert!(!client.authenticate_none("alice").await.unwrap().success());
    assert!(sign_in(&mut client, Login::Guest).await);
    let mut p = Player::open(client, 100, 40).await;
    p.wait_for("Quick play").await;
}

#[tokio::test]
async fn commands_subsystems_and_shells_without_a_terminal_are_refused() {
    let server = TestServer::start();
    let key = new_key();

    let mut client = connect(server.addr).await;
    assert!(sign_in(&mut client, Login::Key(&key)).await);
    let channel = client.channel_open_session().await.unwrap();
    channel.exec(true, "ls -la").await.unwrap();
    let mut p = Player::on(client, channel, 100, 10);
    assert_eq!(p.wait_closed(Duration::from_secs(2)).await, Some(1));
    assert!(p.text().contains("can't run commands"), "{}", p.text());

    let mut client = connect(server.addr).await;
    assert!(sign_in(&mut client, Login::Key(&key)).await);
    let channel = client.channel_open_session().await.unwrap();
    channel.request_shell(true).await.unwrap();
    let mut p = Player::on(client, channel, 100, 10);
    assert_eq!(p.wait_closed(Duration::from_secs(2)).await, Some(1));
    assert!(p.text().contains("needs a terminal"), "{}", p.text());

    let mut client = connect(server.addr).await;
    assert!(sign_in(&mut client, Login::Key(&key)).await);
    let mut channel = client.channel_open_session().await.unwrap();
    channel.request_subsystem(true, "sftp").await.unwrap();
    let mut refused = false;
    while let Ok(Some(msg)) = timeout(Duration::from_secs(2), channel.wait()).await {
        match msg {
            ChannelMsg::Failure => refused = true,
            ChannelMsg::Close => break,
            _ => {}
        }
    }
    assert!(refused, "sftp must be refused");
}

#[tokio::test]
async fn port_forwarding_is_refused() {
    let server = TestServer::start();
    let key = new_key();
    let mut client = connect(server.addr).await;
    assert!(sign_in(&mut client, Login::Key(&key)).await);
    let local = client
        .channel_open_direct_tcpip("example.com", 80, "127.0.0.1", 40000)
        .await;
    assert!(local.is_err(), "-L must be refused");
    let remote = client.tcpip_forward("127.0.0.1", 0).await;
    assert!(remote.is_err(), "-R must be refused");
}

#[tokio::test]
async fn huge_terminals_are_drawn_at_the_largest_size_served() {
    let server = TestServer::start();
    let key = new_key();
    let mut p = Player::join(server.addr, Login::Key(&key), 1000, 300).await;
    p.wait_for("Quick play").await;
    // The 15-column logo is centred in 240 columns, not 1000.
    let logo_row = p.screen.screen().rows(0, 1000).next().unwrap();
    let indent = logo_row.len() - logo_row.trim_start().len();
    assert!(
        (112..=113).contains(&indent),
        "logo starts at column {indent}"
    );
    assert!(
        p.screen
            .screen()
            .rows(0, 1000)
            .all(|row| row.chars().count() <= 240)
    );
}

#[tokio::test]
async fn connections_that_never_open_the_game_are_dropped() {
    let server = TestServer::with(|c| c.login_timeout_secs = 1);

    // Connects but never even says hello.
    let mut silent = TcpStream::connect(server.addr).await.unwrap();
    let mut buf = [0u8; 256];
    let closed = timeout(Duration::from_secs(4), async {
        use tokio::io::AsyncReadExt;
        loop {
            match silent.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
    })
    .await;
    assert!(closed.is_ok(), "a silent connection must be closed");

    // Signs in, like `ssh -N`, and then does nothing.
    let mut idle = connect(server.addr).await;
    assert!(sign_in(&mut idle, Login::Key(&new_key())).await);
    sleep(Duration::from_millis(2500)).await;
    assert!(idle.channel_open_session().await.is_err());
}

#[tokio::test]
async fn too_many_connections_from_one_address_are_turned_away() {
    let server = TestServer::with(|c| c.max_sessions_per_ip = 2);
    let key = new_key();
    let mut first = Player::join(server.addr, Login::Key(&key), 100, 40).await;
    let mut second = Player::join(server.addr, Login::Key(&key), 100, 40).await;
    first.wait_for("Quick play").await;
    second.wait_for("Quick play").await;
    let third = client::connect(
        Arc::new(client::Config::default()),
        server.addr,
        TrustAnyHost,
    );
    assert!(
        timeout(Duration::from_secs(3), third)
            .await
            .unwrap()
            .is_err()
    );
    drop(first);
    sleep(Duration::from_millis(300)).await;
    let mut fourth = Player::join(server.addr, Login::Key(&key), 100, 40).await;
    fourth.wait_for("Quick play").await;
}

#[tokio::test]
async fn stopping_the_server_tells_players_logs_them_out_and_keeps_their_records() {
    let server = TestServer::start();
    let key = new_key();
    let mut p = Player::join(server.addr, Login::Key(&key), 100, 40).await;
    p.single_player().await;
    p.wait_for("best for this setup: 0").await;
    p.send(ENTER).await;
    p.wait_for("press an arrow key").await;
    let restarting = tokio::task::spawn_blocking(move || server.restart());
    assert_eq!(p.wait_closed(Duration::from_secs(5)).await, Some(0));
    assert!(p.text().contains("restarting"), "{}", p.text());
    let server = restarting.await.unwrap();

    // The game running at the restart counts toward the record.
    let mut again = Player::join(server.addr, Login::Key(&key), 100, 40).await;
    again.single_player().await;
    again.wait_for("best for this setup: 3").await;
}

/// A socket whose reading can be paused, like a client that stops taking
/// data while still sending.
struct Pausable {
    inner: TcpStream,
    paused: Arc<AtomicBool>,
    waker: Arc<Mutex<Option<Waker>>>,
}

impl AsyncRead for Pausable {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.paused.load(Ordering::SeqCst) {
            *self.waker.lock().unwrap() = Some(cx.waker().clone());
            return Poll::Pending;
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for Pausable {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[tokio::test]
async fn a_client_that_stops_reading_is_disconnected() {
    let server = TestServer::with(|c| c.stall_timeout_secs = 1);
    let paused = Arc::new(AtomicBool::new(false));
    let waker = Arc::new(Mutex::new(None::<Waker>));
    let stream = Pausable {
        inner: TcpStream::connect(server.addr).await.unwrap(),
        paused: Arc::clone(&paused),
        waker: Arc::clone(&waker),
    };
    // A small SSH window: the server runs out of it soon after we stop
    // reading, rather than whenever the OS socket buffers fill.
    let config = client::Config {
        window_size: 32 * 1024,
        ..Default::default()
    };
    let mut client = client::connect_stream(Arc::new(config), stream, TrustAnyHost)
        .await
        .unwrap();
    assert!(sign_in(&mut client, Login::Key(&new_key())).await);
    let mut p = Player::open(client, 240, 80).await;
    p.single_player().await;
    p.send(ENTER).await;
    p.wait_for("press an arrow key").await;

    // Stop reading, and keep making the server redraw the whole screen. Paced,
    // so our own outgoing requests never back up: a client stuck writing
    // would never read the hang-up either.
    paused.store(true, Ordering::SeqCst);
    let mut cols = 240;
    for _ in 0..300 {
        cols = if cols == 240 { 239 } else { 240 };
        let _ = timeout(
            Duration::from_millis(100),
            p.channel.window_change(cols, 80, 0, 0),
        )
        .await;
        sleep(Duration::from_millis(10)).await;
    }

    // Read again: whatever was in flight arrives, then the disconnect.
    paused.store(false, Ordering::SeqCst);
    if let Some(w) = waker.lock().unwrap().take() {
        w.wake();
    }
    p.wait_closed(Duration::from_secs(5)).await;
}
