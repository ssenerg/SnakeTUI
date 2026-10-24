//! End to end: a real server on a local port, played through russh's SSH
//! client, with what the player would see read back by a terminal emulator.

use std::io;
use std::net::SocketAddr;
use std::num::NonZeroU16;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use russh::client::{self, KeyboardInteractiveAuthResponse, Msg};
use russh::keys::{Algorithm, PrivateKey, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use russh::{Channel, ChannelMsg};
use snaketui_server::config::ServerConfig;
use snaketui_server::{serve, ssh};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio::time::{Instant, sleep, timeout};

/// A server on `127.0.0.1:<free port>` with its own host key and database.
struct TestServer {
    addr: SocketAddr,
    dir: PathBuf,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl TestServer {
    fn start() -> Self {
        Self::with(|_| {})
    }

    fn with(tweak: impl FnOnce(&mut ServerConfig)) -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "snaketui-e2e-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self::in_dir(dir, tweak)
    }

    fn in_dir(dir: PathBuf, tweak: impl FnOnce(&mut ServerConfig)) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let mut cfg = ServerConfig {
            listen: addr,
            host_key: dir.join("host_key"),
            database: format!("sqlite://{}", dir.join("db.sqlite").display()),
            workers: 2,
            ..ServerConfig::default()
        };
        tweak(&mut cfg);
        let host_key = ssh::load_or_create_host_key(&cfg.host_key).unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let thread = std::thread::spawn(move || {
            let until_stopped = async {
                let _ = stopped.await;
            };
            actix_rt::System::new()
                .block_on(serve(cfg, listener, host_key, until_stopped))
                .unwrap();
        });
        Self {
            addr,
            dir,
            stop: Some(stop),
            thread: Some(thread),
        }
    }

    /// Stops the server and waits until it has finished.
    fn stop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }

    /// Stops the server, then serves again with the same host key and
    /// database. The old one gives up its directory, so dropping it removes
    /// nothing.
    fn restart(mut self) -> Self {
        self.stop();
        Self::in_dir(std::mem::take(&mut self.dir), |_| {})
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn new_key() -> PrivateKey {
    PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap()
}

struct TrustAnyHost;

impl client::Handler for TrustAnyHost {
    type Error = russh::Error;

    async fn check_server_key(&mut self, _: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

type Client = client::Handle<TrustAnyHost>;

#[derive(Clone, Copy)]
enum Login<'a> {
    Key(&'a PrivateKey),
    Guest,
}

async fn connect(addr: SocketAddr) -> Client {
    client::connect(Arc::new(client::Config::default()), addr, TrustAnyHost)
        .await
        .unwrap()
}

async fn sign_in(client: &mut Client, login: Login<'_>) -> bool {
    match login {
        Login::Key(key) => {
            let key = PrivateKeyWithHashAlg::new(Arc::new(key.clone()), None);
            client
                .authenticate_publickey("alice", key)
                .await
                .unwrap()
                .success()
        }
        Login::Guest => matches!(
            client
                .authenticate_keyboard_interactive_start("alice", None)
                .await
                .unwrap(),
            KeyboardInteractiveAuthResponse::Success
        ),
    }
}

/// One player's channel, and their screen as a terminal would show it.
struct Player {
    _client: Client,
    channel: Channel<Msg>,
    screen: vt100::Parser,
    exit_status: Option<u32>,
    closed: bool,
    received: usize,
}

impl Player {
    /// Signs in, asks for a `cols`×`rows` terminal and opens the game.
    async fn join(addr: SocketAddr, login: Login<'_>, cols: u32, rows: u32) -> Self {
        let mut client = connect(addr).await;
        assert!(sign_in(&mut client, login).await);
        Self::open(client, cols, rows).await
    }

    async fn open(client: Client, cols: u32, rows: u32) -> Self {
        let channel = client.channel_open_session().await.unwrap();
        channel
            .request_pty(true, "xterm-256color", cols, rows, 0, 0, &[])
            .await
            .unwrap();
        channel.request_shell(true).await.unwrap();
        Self::on(client, channel, cols, rows)
    }

    fn on(client: Client, channel: Channel<Msg>, cols: u32, rows: u32) -> Self {
        let size = |n: u32| NonZeroU16::new(n as u16).unwrap();
        Self {
            _client: client,
            channel,
            screen: vt100::Parser::new(size(rows), size(cols), 0),
            exit_status: None,
            closed: false,
            received: 0,
        }
    }

    async fn send(&self, bytes: &[u8]) {
        self.channel.data(bytes).await.unwrap();
    }

    fn text(&self) -> String {
        self.screen.screen().contents()
    }

    /// Reads from the server until `done`, failing after `limit`.
    async fn wait_until(&mut self, what: &str, limit: Duration, done: impl Fn(&Self) -> bool) {
        let deadline = Instant::now() + limit;
        while !done(self) {
            assert!(
                !self.closed,
                "closed while waiting for {what}:\n{}",
                self.text()
            );
            let Ok(msg) = timeout(deadline - Instant::now(), self.channel.wait()).await else {
                panic!(
                    "timed out waiting for {what} after {} bytes; the screen shows:\n{}",
                    self.received,
                    self.text()
                );
            };
            match msg {
                Some(ChannelMsg::Data { data }) => {
                    self.received += data.len();
                    self.screen.process(&data)
                }
                Some(ChannelMsg::ExitStatus { exit_status }) => {
                    self.exit_status = Some(exit_status)
                }
                Some(ChannelMsg::Close) | None => self.closed = true,
                Some(_) => {}
            }
        }
    }

    async fn wait_for(&mut self, text: &str) {
        self.wait_until(text, Duration::from_secs(5), |p| p.text().contains(text))
            .await;
    }

    /// Waits for the server to close the channel; returns the exit status.
    async fn wait_closed(&mut self, limit: Duration) -> Option<u32> {
        self.wait_until("the connection to close", limit, |p| p.closed)
            .await;
        self.exit_status
    }
}

const ENTER: &[u8] = b"\r";
const ESC: &[u8] = b"\x1b";
const CTRL_C: &[u8] = b"\x03";

#[tokio::test]
async fn quitting_logs_out_at_once_and_restores_the_screen() {
    let server = TestServer::start();
    let key = new_key();
    let mut p = Player::join(server.addr, Login::Key(&key), 100, 30).await;
    p.wait_for("Settings").await;
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
    again.wait_for("◂ 31 ▸").await;
    again.wait_for("best for this setup: 3").await;

    let other = new_key();
    let mut someone_else = Player::join(server.addr, Login::Key(&other), 100, 40).await;
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
    p.wait_for("Settings").await;
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
    p.wait_for("Settings").await;
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
    first.wait_for("Settings").await;
    second.wait_for("Settings").await;
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
    fourth.wait_for("Settings").await;
}

#[tokio::test]
async fn stopping_the_server_tells_players_logs_them_out_and_keeps_their_records() {
    let server = TestServer::start();
    let key = new_key();
    let mut p = Player::join(server.addr, Login::Key(&key), 100, 40).await;
    p.wait_for("best for this setup: 0").await;
    p.send(ENTER).await;
    p.wait_for("press an arrow key").await;
    let restarting = tokio::task::spawn_blocking(move || server.restart());
    assert_eq!(p.wait_closed(Duration::from_secs(5)).await, Some(0));
    assert!(p.text().contains("restarting"), "{}", p.text());
    let server = restarting.await.unwrap();

    // The game running at the restart counts toward the record.
    let mut again = Player::join(server.addr, Login::Key(&key), 100, 40).await;
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
    p.wait_for("Settings").await;
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
